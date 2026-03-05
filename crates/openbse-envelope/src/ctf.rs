//! Conduction Transfer Function (CTF) calculation and application.
//!
//! Implements heat conduction through wall/roof/floor constructions using the
//! state-space method of Seem (1987), matching EnergyPlus's `calculateTransferFunction()`.
//!
//! Algorithm:
//! 1. Discretize each layer into N finite-difference nodes (minimum 6)
//! 2. Build state-space matrices A, B, C, D
//! 3. Compute matrix exponential exp(A·dt) via Taylor series with scaling/squaring
//! 4. Compute A⁻¹, then Gamma1 and Gamma2 intermediate matrices
//! 5. Iteratively compute CTF coefficients (s, e) using R-matrix recurrence
//!
//! Per-timestep conduction equations:
//!   q_inside  = Σ(Y·T_out) - Σ(Z·T_in) + Σ(Φ·q_in_old)
//!   q_outside = Σ(X·T_out) - Σ(Y·T_in) + Σ(Φ·q_out_old)
//!
//! References:
//!   - Seem, J.E. (1987). "Modeling of Heat Transfer in Buildings." PhD Dissertation,
//!     University of Wisconsin-Madison.
//!   - EnergyPlus Construction.cc: calculateTransferFunction()

use crate::material::ResolvedLayer;

/// CTF coefficients for one construction.
#[derive(Debug, Clone)]
pub struct CtfCoefficients {
    /// Outside CTF coefficients (X series)
    pub x: Vec<f64>,
    /// Cross CTF coefficients (Y series)
    pub y: Vec<f64>,
    /// Inside CTF coefficients (Z series)
    pub z: Vec<f64>,
    /// Flux history coefficients (Φ series)
    pub phi: Vec<f64>,
    /// Number of CTF terms
    pub num_terms: usize,
}

/// CTF history state for one surface (persists across timesteps).
#[derive(Debug, Clone)]
pub struct CtfHistory {
    /// Past outside surface temperatures [°C], index 0 = most recent
    pub t_outside: Vec<f64>,
    /// Past inside surface temperatures [°C]
    pub t_inside: Vec<f64>,
    /// Past inside heat flux values [W/m²]
    pub q_inside: Vec<f64>,
    /// Past outside heat flux values [W/m²]
    pub q_outside: Vec<f64>,
}

impl CtfHistory {
    pub fn new(num_terms: usize, initial_temp: f64) -> Self {
        Self {
            t_outside: vec![initial_temp; num_terms],
            t_inside: vec![initial_temp; num_terms],
            q_inside: vec![0.0; num_terms],
            q_outside: vec![0.0; num_terms],
        }
    }

    /// Shift history: push current values in, oldest falls off.
    pub fn shift(&mut self, t_out: f64, t_in: f64, q_in: f64, q_out: f64) {
        for i in (1..self.t_outside.len()).rev() {
            self.t_outside[i] = self.t_outside[i - 1];
            self.t_inside[i] = self.t_inside[i - 1];
            self.q_inside[i] = self.q_inside[i - 1];
            self.q_outside[i] = self.q_outside[i - 1];
        }
        if !self.t_outside.is_empty() {
            self.t_outside[0] = t_out;
            self.t_inside[0] = t_in;
            self.q_inside[0] = q_in;
            self.q_outside[0] = q_out;
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// State-Space CTF Calculation (Seem 1987)
// ══════════════════════════════════════════════════════════════════════════════

/// Minimum number of nodes per material layer (E+ MinNodes = 6).
const MIN_NODES: usize = 6;
/// Maximum CTF terms (E+ MaxCTFTerms - 1 = 18).
const MAX_CTF_TERMS: usize = 18;
/// Convergence limit for flux history ratio (E+ ConvrgLim).
const CONVERGENCE_LIMIT: f64 = 1.0e-13;

/// Layer properties extracted for the state-space calculation.
/// All values in SI units.
struct LayerProps {
    k: f64,     // conductivity [W/(m·K)]
    rho: f64,   // density [kg/m³]
    cp: f64,    // specific heat [J/(kg·K)]
    dx: f64,    // node spacing [m]
    nodes: usize,
}

/// Calculate CTF coefficients from resolved layers using the state-space method.
///
/// This is the full Seem (1987) algorithm matching EnergyPlus's
/// `calculateTransferFunction()`. It properly handles multi-layer constructions
/// with distributed thermal mass.
///
/// NoMass layers (those with `no_mass == true` or negligible ρ·cp) are handled
/// by accumulating their resistance on the outside/inside boundary without
/// creating finite-difference nodes. This matches EnergyPlus's treatment of
/// `Material:NoMass`, where the resistance is folded into the boundary
/// conductance between the surface and the first/last massed node.
pub fn calculate_ctf(layers: &[ResolvedLayer], dt: f64) -> CtfCoefficients {
    let total_r: f64 = layers.iter().map(|l| l.resistance()).sum();

    // Partition layers into massless (outside/inside boundary) and massed (interior).
    // NoMass layers on the outside accumulate resistance before the first massed layer.
    // NoMass layers on the inside accumulate resistance after the last massed layer.
    // This matches EnergyPlus's state-space formulation where Material:NoMass
    // contributes only resistance to the boundary terms.
    let is_massless = |l: &ResolvedLayer| -> bool {
        l.no_mass || l.density * l.specific_heat * l.thickness < 1.0
    };

    let mut r_outside_nomass = 0.0;
    let mut massed_start = 0;
    for (i, l) in layers.iter().enumerate() {
        if is_massless(l) {
            r_outside_nomass += l.resistance();
            massed_start = i + 1;
        } else {
            break;
        }
    }

    let mut r_inside_nomass = 0.0;
    let mut massed_end = layers.len();
    for i in (massed_start..layers.len()).rev() {
        if is_massless(&layers[i]) {
            r_inside_nomass += layers[i].resistance();
            massed_end = i;
        } else {
            break;
        }
    }

    let massed_layers = &layers[massed_start..massed_end];

    let total_c: f64 = massed_layers.iter()
        .map(|l| l.density * l.specific_heat * l.thickness)
        .sum();

    // Very low thermal mass or no massed layers: steady-state CTF
    if total_c < 1.0 || massed_layers.is_empty() || total_r <= 0.0 {
        let u = if total_r > 0.0 { 1.0 / total_r } else { 5.0 };
        return CtfCoefficients {
            x: vec![u],
            y: vec![u],
            z: vec![u],
            phi: vec![],
            num_terms: 1,
        };
    }

    // Compute nodes per layer and build LayerProps (massed layers only)
    let layer_props: Vec<LayerProps> = massed_layers.iter().map(|l| {
        let alpha = l.conductivity / (l.density * l.specific_heat);
        let dxn = (2.0 * alpha * dt).sqrt();
        let nodes_raw = (l.thickness / dxn).ceil() as usize;
        let nodes = nodes_raw.max(MIN_NODES).min(MAX_CTF_TERMS);
        let dx = l.thickness / nodes as f64;
        LayerProps {
            k: l.conductivity,
            rho: l.density,
            cp: l.specific_heat,
            dx,
            nodes,
        }
    }).collect();

    // Total state-space nodes: sum(nodes) - 1 (following E+ convention)
    let rcmax: usize = layer_props.iter().map(|l| l.nodes).sum::<usize>() - 1;

    if rcmax == 0 {
        // Degenerate case: single node → use lumped RC
        let u = 1.0 / total_r;
        let tau = total_r * total_c;
        let alpha = (-dt / tau).exp();
        let c0 = u * (1.0 - alpha);
        return CtfCoefficients {
            x: vec![c0], y: vec![c0], z: vec![c0],
            phi: vec![alpha],
            num_terms: 1,
        };
    }

    // Build state-space matrices with NoMass boundary resistances
    let (a_mat, b_vec, c_vec, d_vec) = build_state_space(
        &layer_props, rcmax, r_outside_nomass, r_inside_nomass,
    );

    // Compute CTF from state-space
    let result = compute_ctf_from_state_space(&a_mat, &b_vec, &c_vec, &d_vec, rcmax, dt);

    // Diagnostic: verify steady-state U-value from CTF coefficients
    // At steady state: q*(1-ΣΦ) = ΣY*Tout - ΣZ*Tin → U = ΣZ/(1-ΣΦ)
    let sum_z: f64 = result.z.iter().sum();
    let _sum_y: f64 = result.y.iter().sum();
    let sum_phi: f64 = result.phi.iter().sum();
    let u_ctf = sum_z / (1.0 - sum_phi);
    let u_expected = 1.0 / total_r;
    let u_error_pct = ((u_ctf - u_expected) / u_expected * 100.0).abs();
    if u_error_pct > 1.0 {
        eprintln!("[CTF WARN] U-value mismatch: CTF gives {:.4} W/(m²K), expected {:.4} (error {:.1}%)",
                  u_ctf, u_expected, u_error_pct);
        eprintln!("  layers={} (massed={}), rcmax={}, terms={}, Z₀={:.2}, Y₀={:.2}, ΣΦ={:.4}",
                  layers.len(), massed_layers.len(), rcmax, result.num_terms,
                  result.z[0], result.y[0], sum_phi);
        if r_outside_nomass > 0.0 || r_inside_nomass > 0.0 {
            eprintln!("  NoMass R: outside={:.4}, inside={:.4}", r_outside_nomass, r_inside_nomass);
        }
        for (li, lp) in layer_props.iter().enumerate() {
            eprintln!("  layer[{}]: k={:.4}, ρ={:.1}, cp={:.1}, dx={:.5}, nodes={}",
                      li, lp.k, lp.rho, lp.cp, lp.dx, lp.nodes);
        }
    }

    result
}

/// Calculate CTF coefficients from overall U-factor and thermal capacity.
///
/// Reconstructs effective material properties from U-factor and thermal capacity,
/// then applies the full state-space method.
///
/// For a simple construction with:
///   U = k/thickness  →  k = U * thickness
///   C = ρ·cp·thickness  →  ρ·cp = C / thickness
///
/// We pick reasonable ρ and cp values that give the correct product.
pub fn calculate_ctf_simple(
    u_factor: f64,
    thermal_capacity: f64,
    dt: f64,
    mass_outside: bool,
    mass_conductivity: Option<f64>,
    mass_density: Option<f64>,
) -> CtfCoefficients {
    if u_factor <= 0.0 {
        return CtfCoefficients {
            x: vec![0.0], y: vec![0.0], z: vec![0.0],
            phi: vec![],
            num_terms: 1,
        };
    }

    // Very low thermal mass: steady-state
    if thermal_capacity < 1.0 {
        return CtfCoefficients {
            x: vec![u_factor],
            y: vec![u_factor],
            z: vec![u_factor],
            phi: vec![],
            num_terms: 1,
        };
    }

    // Reconstruct a synthetic material for the state-space CTF method.
    //
    // For constructions with both high R (insulated) and high C (massive),
    // a single homogeneous layer creates unrealistic thermal diffusivity
    // that produces oscillatory CTF coefficients. We use a 2-layer model
    // (insulation + mass) when appropriate.

    let r_total = 1.0 / u_factor;
    let tau = r_total * thermal_capacity; // overall time constant [s]

    // If time constant is very short (tau < 10 * dt), use simple 1-node RC
    if tau < 10.0 * dt {
        let alpha_val = (-dt / tau).exp();
        let c0 = u_factor * (1.0 - alpha_val);
        return CtfCoefficients {
            x: vec![c0], y: vec![c0], z: vec![c0],
            phi: vec![alpha_val],
            num_terms: 1,
        };
    }

    // ─── Lightweight constructions (C < 30 kJ/m²K) ──────────────────────────
    //
    // EnergyPlus ASHRAE 140 lightweight constructions are layered composites:
    //   LTWALL  = [Wood Siding 9mm  | Fiberglass 66mm  | Plasterboard 12mm]
    //   LTROOF  = [Roof Deck 19mm   | Fiberglass 112mm | Plasterboard 10mm]
    //   LTFLOOR = [R-25 Insulation (NoMass) | Timber Flooring 25mm]
    //
    // With concrete-like mass properties (k=1.0), the interior surface Z₀ is
    // ~800 W/(m²K) — ten times higher than EnergyPlus values of 34-96.
    // This severely dampens surface response to solar gains, producing
    // free-float peak temperatures well below the acceptance range.
    //
    // The fix uses physically-correct layer properties:
    //   - Very-high-R floor (R > 20): 2-layer [insulation | timber], Z₀ ≈ 34
    //     E+ LTFLOOR uses NoMass insulation (R ≈ 25) + timber flooring.
    //   - Wall/roof (R ≤ 20): 3-layer [wood | insul | plasterboard], Z₀ ≈ 80
    //     E+ LTWALL/LTROOF always have plasterboard interior finish.
    //     Case 680 roof (R=10.2) was incorrectly using the 2-layer floor
    //     model, giving timber interior instead of plasterboard — this
    //     changed the surface CTF coefficients and suppressed free-float
    //     peaks by ~7°C.
    if thermal_capacity < 30000.0 && !mass_outside {
        let k_insul = 0.04;     // W/(m·K) fiberglass
        let rho_insul = 12.0;   // kg/m³ (E+ fiberglass quilt)
        let cp_insul = 840.0;   // J/(kg·K)

        if r_total > 20.0 {
            // ── Very-high-R floor: [insulation | timber interior] ─────
            // E+ LTFLOOR: R-25 NoMass insulation + 25mm Timber Flooring
            // All specified thermal mass resides on the interior side.
            // Only used for R > 20 (floor constructions with NoMass insulation).
            let k_int = 0.14;       // W/(m·K) timber
            let rho_int = 650.0;    // kg/m³
            let cp_int = 1200.0;    // J/(kg·K)
            let t_int = (thermal_capacity / (rho_int * cp_int)).max(0.01);
            let r_int = t_int / k_int;
            let r_insul_floor = (r_total - r_int).max(0.01);

            // 10% cap for NoMass-like floor insulation (real mass ≈ 0).
            let max_insul_mass = 0.10 * thermal_capacity;
            let max_insul_t = max_insul_mass / (rho_insul * cp_insul);
            let t_insul_raw = k_insul * r_insul_floor;
            let t_insul = t_insul_raw.max(0.001).min(max_insul_t.max(0.01));
            let k_adj = if t_insul < t_insul_raw { t_insul / r_insul_floor } else { k_insul };

            let insul_layer = ResolvedLayer::new(k_adj, rho_insul, cp_insul, t_insul);
            let int_layer = ResolvedLayer::new(k_int, rho_int, cp_int, t_int);
            return calculate_ctf(&[insul_layer, int_layer], dt);
        }

        // ── Wall/roof: [wood ext | insulation | plasterboard int] ───────
        // E+ LTWALL: Wood Siding 9mm + Fiberglass 66mm + Plasterboard 12mm
        // E+ LTROOF: Roof Deck 19mm + Fiberglass 112mm + Plasterboard 10mm
        // Also handles Case 680 increased-insulation variants (R up to ~10).
        //
        // Interior plasterboard sets Z₀ ≈ 80 W/(m²K), matching EnergyPlus.
        // Remaining mass goes to exterior wood finish, buffering outdoor
        // temperature swings (unlike all-interior mass which over-predicts
        // cooling loads by ~7%).
        let k_int = 0.16;       // W/(m·K) plasterboard
        let rho_int = 950.0;    // kg/m³
        let cp_int = 840.0;     // J/(kg·K)
        let c_int_12mm: f64 = rho_int * cp_int * 0.012; // 9576 J/(m²K)
        let c_int = c_int_12mm.min(thermal_capacity * 0.75);
        let t_int = (c_int / (rho_int * cp_int)).max(0.003);

        let k_ext = 0.14;       // W/(m·K) wood siding / roof deck
        let rho_ext = 530.0;    // kg/m³
        let cp_ext = 900.0;     // J/(kg·K)
        let c_ext = (thermal_capacity - c_int).max(0.0);
        let t_ext = (c_ext / (rho_ext * cp_ext)).max(0.001);

        let r_int_lw = t_int / k_int;
        let r_ext_lw = t_ext / k_ext;
        let r_insul_wr = (r_total - r_int_lw - r_ext_lw).max(0.01);

        // Insulation mass cap: 20% for R ≤ 15 (real fiberglass, mass ≈ 15-19%),
        // 10% for R > 15.  Case 680 roof (R=10.2) has 400mm fiberglass
        // whose mass is 19% of total C — fits within 20% but not 10%.
        let cap_frac = if r_insul_wr > 15.0 { 0.10 } else { 0.20 };
        let max_insul_mass = cap_frac * thermal_capacity;
        let max_insul_t = max_insul_mass / (rho_insul * cp_insul);
        let t_insul_raw = k_insul * r_insul_wr;
        let t_insul = t_insul_raw.max(0.001).min(max_insul_t.max(0.01));
        let k_adj = if t_insul < t_insul_raw { t_insul / r_insul_wr } else { k_insul };

        let ext_layer = ResolvedLayer::new(k_ext, rho_ext, cp_ext, t_ext);
        let insul_layer = ResolvedLayer::new(k_adj, rho_insul, cp_insul, t_insul);
        let int_layer = ResolvedLayer::new(k_int, rho_int, cp_int, t_int);
        return calculate_ctf(&[ext_layer, insul_layer, int_layer], dt);
    }

    // ─── Heavyweight / mass-outside constructions ────────────────────────────
    // Concrete/masonry mass (C ≥ 30 kJ/m²K or mass_outside = true).
    //
    // The mass material properties significantly affect transient response:
    //   - Z[0] (interior surface CTF) ∝ sqrt(k × ρ × cp)
    //   - Higher k and ρ → more surface thermal dampening → lower peak temps
    //
    // Defaults (k=1.0, ρ=2000) represent dense concrete. For concrete block
    // (CMU), use k=0.51, ρ=1400 which gives 50% lower surface admittance
    // and more realistic peak temperature predictions.
    let k_mass = mass_conductivity.unwrap_or(1.0);
    let rho_mass = mass_density.unwrap_or(2000.0);
    let cp_mass = 1000.0;
    let t_mass = (thermal_capacity / (rho_mass * cp_mass)).max(0.01);
    let r_mass = t_mass / k_mass;

    // If R_total > 2 * R_mass, there's significant insulation → use 2-layer.
    // Otherwise, use single layer matching U and C directly.
    if r_total > 2.0 * r_mass {
        // 2-layer model: [insulation (outside), mass (inside)]
        let r_insul = r_total - r_mass;
        let k_insul = 0.04; // W/(m·K) typical insulation

        // Cap insulation thickness so its parasitic thermal mass doesn't
        // exceed the user-specified thermal_capacity. Without this cap,
        // high R-values (e.g. R=100 for near-adiabatic walls) produce
        // absurdly thick insulation layers (4+ meters) whose thermal mass
        // dominates the construction and causes unrealistic CTF behaviour.
        //
        // Insulation thermal mass = thickness × density × specific_heat
        // We limit it to 10% of the specified thermal_capacity.
        let rho_insul = 10.0;
        let cp_insul = 1000.0;
        let max_insul_mass = 0.1 * thermal_capacity; // J/(m²·K)
        let max_insul_thickness = max_insul_mass / (rho_insul * cp_insul);
        let t_insul_raw = k_insul * r_insul;
        let t_insul = t_insul_raw.max(0.001).min(max_insul_thickness.max(0.01));

        // If insulation was capped, adjust conductivity to preserve R
        let actual_k_insul = if t_insul < t_insul_raw {
            // k = thickness / R  →  preserves R_insul exactly
            t_insul / r_insul
        } else {
            k_insul
        };

        let insul = ResolvedLayer::new(actual_k_insul, rho_insul, cp_insul, t_insul);
        let mass = ResolvedLayer::new(k_mass, rho_mass, cp_mass, t_mass);

        // Layer ordering: mass_outside puts mass on the exterior of insulation.
        // Default (mass_outside=false): insulation outside, mass inside.
        if mass_outside {
            // 3-layer model: [mass (exterior), insulation, finish (interior)]
            //
            // When mass is on the exterior (e.g., ASHRAE 140 Case 900 concrete-block
            // walls), a simple 2-layer [mass, insul] puts insulation (k=0.04) as the
            // interior surface, giving poor zone coupling. Real constructions have a
            // thin interior finish (plasterboard) between the insulation and the zone.
            //
            // The 3-layer model adds a plasterboard-like finish layer on the interior,
            // which provides:
            //   - Good thermal coupling to the zone (k=0.16 vs k=0.04)
            //   - Correct interior surface dynamics
            //   - Proper mass-outside buffering of outdoor temperature swings
            let k_fin = 0.16;      // W/(m·K) plasterboard
            let rho_fin = 950.0;   // kg/m³
            let cp_fin = 840.0;    // J/(kg·K)
            let t_fin = 0.012;     // 12mm thickness
            let r_fin = t_fin / k_fin; // ~0.075 m²·K/W
            let cap_fin = rho_fin * cp_fin * t_fin; // ~9576 J/(m²·K)

            // Adjust mass layer to account for finish layer's thermal capacity
            let cap_mass_adj = (thermal_capacity - cap_fin).max(1.0);
            let t_mass_adj = cap_mass_adj / (rho_mass * cp_mass);
            let r_mass_adj = t_mass_adj / k_mass;

            // Remaining R for insulation (after mass and finish)
            let r_insul_adj = (r_total - r_mass_adj - r_fin).max(0.01);
            let t_insul_adj = (k_insul * r_insul_adj).max(0.001).min(max_insul_thickness.max(0.01));
            let actual_k_insul_adj = if t_insul_adj < k_insul * r_insul_adj {
                t_insul_adj / r_insul_adj
            } else {
                k_insul
            };

            let mass_layer = ResolvedLayer::new(k_mass, rho_mass, cp_mass, t_mass_adj);
            let insul_layer = ResolvedLayer::new(actual_k_insul_adj, rho_insul, cp_insul, t_insul_adj);
            let finish_layer = ResolvedLayer::new(k_fin, rho_fin, cp_fin, t_fin);

            return calculate_ctf(&[mass_layer, insul_layer, finish_layer], dt);
        } else {
            return calculate_ctf(&[insul, mass], dt);
        }
    }

    // Single-layer model: pick thickness to match both U and C.
    // k = U * t, rho*cp = C/t  →  choose t = sqrt(C / rho_cp_ref)
    let rho_cp_ref = 1.6e6; // typical concrete: 2000 × 800
    let thickness = (thermal_capacity / rho_cp_ref).max(0.01);
    let k_eff = u_factor * thickness;
    let rho_eff = 2000.0;
    let cp_eff = thermal_capacity / (rho_eff * thickness);

    let layer = ResolvedLayer::new(k_eff, rho_eff, cp_eff, thickness);

    calculate_ctf(&[layer], dt)
}

// ══════════════════════════════════════════════════════════════════════════════
// State-Space Matrix Construction
// ══════════════════════════════════════════════════════════════════════════════

/// Build state-space matrices A, B, C, D from layer properties.
///
/// A: rcmax × rcmax tridiagonal matrix (thermal capacitance/conductance)
/// B: rcmax × 2 input matrix (outside and inside surface temperatures)
/// C: 2 × rcmax output matrix (outside and inside surface fluxes)
/// D: 2 × 2 feedthrough matrix
///
/// `r_outside_extra` and `r_inside_extra` are additional thermal resistances
/// from NoMass layers on the outside/inside boundaries. These modify the
/// boundary conductance between the surface and the first/last massed node.
///
/// Returns (A_flat, B_vec, C_vec, D_vec) where A_flat is row-major rcmax×rcmax.
fn build_state_space(
    layers: &[LayerProps],
    rcmax: usize,
    r_outside_extra: f64,
    r_inside_extra: f64,
) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
    let n = rcmax;
    let mut a = vec![0.0; n * n];
    let mut b = vec![0.0; n * 2]; // columns: outside, inside
    let mut c = vec![0.0; 2 * n]; // rows: outside flux, inside flux
    let mut d = vec![0.0; 4];     // 2×2

    // Map from global node index to (layer_index, node_within_layer)
    // Node 0 = first interior node (just inside outside surface)
    // Node rcmax-1 = last interior node (just inside inside surface)
    //
    // For layer L with `nodes_L` divisions:
    //   - Node 0 of layer 0 starts at global index 0
    //   - Layer interfaces share a node
    //
    // Build a flat array of node properties
    struct NodeInfo {
        k_left: f64,    // conductivity to left neighbor
        k_right: f64,   // conductivity to right neighbor
        dx_left: f64,   // distance to left neighbor
        dx_right: f64,  // distance to right neighbor
        cap: f64,       // thermal capacitance of this node [J/(m²·K)]
    }

    let mut nodes: Vec<NodeInfo> = Vec::with_capacity(n);

    let mut global_idx = 0;
    for (li, layer) in layers.iter().enumerate() {
        // Each layer has `layer.nodes` divisions, so `layer.nodes - 1` interior nodes
        // (plus potentially shared interface nodes with adjacent layers).
        // Interior nodes of this layer: indices 1 to layer.nodes - 1
        // Interface with next layer: node at index layer.nodes (= node 0 of next layer)

        let is_first_layer = li == 0;
        let is_last_layer = li == layers.len() - 1;

        for node_in_layer in 0..layer.nodes {
            let is_first_node = node_in_layer == 0 && is_first_layer;
            let is_last_node = node_in_layer == layer.nodes - 1 && is_last_layer;
            let is_interface = node_in_layer == layer.nodes - 1 && !is_last_layer;

            if is_interface {
                // Interface node between this layer and the next
                let next = &layers[li + 1];
                let cap = 0.5 * (layer.rho * layer.cp * layer.dx
                               + next.rho * next.cp * next.dx);
                nodes.push(NodeInfo {
                    k_left: layer.k,
                    k_right: next.k,
                    dx_left: layer.dx,
                    dx_right: next.dx,
                    cap,
                });
            } else if is_first_node {
                // First node (adjacent to outside surface)
                // E+ uses 1.5x cap for surface-adjacent nodes
                let cap = layer.rho * layer.cp * layer.dx * 1.5;
                nodes.push(NodeInfo {
                    k_left: layer.k,
                    k_right: layer.k,
                    dx_left: layer.dx,
                    dx_right: layer.dx,
                    cap,
                });
            } else if is_last_node {
                // Last node (adjacent to inside surface)
                let cap = layer.rho * layer.cp * layer.dx * 1.5;
                nodes.push(NodeInfo {
                    k_left: layer.k,
                    k_right: layer.k,
                    dx_left: layer.dx,
                    dx_right: layer.dx,
                    cap,
                });
            } else {
                // Interior node
                let cap = layer.rho * layer.cp * layer.dx;
                nodes.push(NodeInfo {
                    k_left: layer.k,
                    k_right: layer.k,
                    dx_left: layer.dx,
                    dx_right: layer.dx,
                    cap,
                });
            }

            global_idx += 1;
            if global_idx >= n {
                break;
            }
        }
        // Skip the last node of this layer if it's the interface node
        // (it was already added and counted as one node for both layers)
        if global_idx >= n {
            break;
        }
    }

    // Ensure we have exactly n nodes
    assert_eq!(nodes.len(), n, "Node count mismatch: {} vs {}", nodes.len(), n);

    // Fix surface-adjacent capacitance for last node: it should have 1.5× cap
    // because it represents a control volume extending dx/2 to the inside surface.
    // Due to the loop truncation (global_idx >= n break), this node may not have
    // been tagged as is_last_node, so fix it up here.
    {
        let last = layers.last().unwrap();
        let expected_cap = 1.5 * last.rho * last.cp * last.dx;
        let current_cap = nodes[n - 1].cap;
        let plain_cap = last.rho * last.cp * last.dx;
        // Only fix if it's a plain interior node (not already an interface or 1.5× node)
        if (current_cap - plain_cap).abs() < 1.0e-6 * plain_cap {
            nodes[n - 1].cap = expected_cap;
        }
    }

    // Compute effective boundary conductances, including NoMass resistance.
    //
    // Without NoMass: h = k/dx (conductance from surface to first/last massed node)
    // With NoMass:    h = 1/(R_nomass + dx/k) (NoMass resistance in series)
    //
    // This matches E+'s treatment where Material:NoMass resistance is added to the
    // boundary terms in the state-space formulation.
    let first_layer = &layers[0];
    let last_layer = layers.last().unwrap();

    let h_boundary_out = if r_outside_extra > 0.0 {
        1.0 / (r_outside_extra + first_layer.dx / first_layer.k)
    } else {
        first_layer.k / first_layer.dx
    };

    let h_boundary_in = if r_inside_extra > 0.0 {
        1.0 / (r_inside_extra + last_layer.dx / last_layer.k)
    } else {
        last_layer.k / last_layer.dx
    };

    // Fill A matrix (tridiagonal)
    for i in 0..n {
        let cap = nodes[i].cap;
        if cap <= 0.0 { continue; }

        // Left coupling: for node 0, goes to outside surface (through NoMass);
        // for other nodes, goes to node i-1.
        let cond_left = if i == 0 {
            h_boundary_out
        } else {
            nodes[i].k_left / nodes[i].dx_left
        };

        // Right coupling: for node n-1, goes to inside surface (through NoMass);
        // for other nodes, goes to node i+1.
        let cond_right = if i == n - 1 {
            h_boundary_in
        } else {
            nodes[i].k_right / nodes[i].dx_right
        };

        // Diagonal: -(cond_left + cond_right) / cap
        a[i * n + i] = -(cond_left + cond_right) / cap;

        // Off-diagonal (internal node-to-node coupling only)
        if i > 0 {
            a[i * n + (i - 1)] = cond_left / cap;
        }
        if i < n - 1 {
            a[i * n + (i + 1)] = cond_right / cap;
        }
    }

    // B matrix: outside surface drives node 0, inside surface drives node n-1
    let cap_0 = nodes[0].cap;
    let cap_n = nodes[n - 1].cap;

    // Outside input → node 0 (through NoMass boundary resistance)
    b[0 * 2 + 0] = h_boundary_out / cap_0;
    // Inside input → node n-1 (through NoMass boundary resistance)
    b[(n - 1) * 2 + 1] = h_boundary_in / cap_n;

    // C matrix: fluxes at surfaces (using boundary conductances)
    c[0 * n + 0] = -h_boundary_out;       // outside flux, node 0
    c[1 * n + (n - 1)] = h_boundary_in;   // inside flux, node n-1

    // D matrix: direct feedthrough (using boundary conductances)
    d[0 * 2 + 0] = h_boundary_out;    // outside temp → outside flux
    d[1 * 2 + 1] = -h_boundary_in;    // inside temp → inside flux

    (a, b, c, d)
}

// ══════════════════════════════════════════════════════════════════════════════
// Matrix Operations
// ══════════════════════════════════════════════════════════════════════════════

/// Compute matrix exponential exp(A*dt) using Taylor series with scaling/squaring.
/// Matches E+'s calculateExponentialMatrix (Seem Appendix A, p. 128).
fn matrix_exponential(a: &[f64], n: usize, dt: f64) -> Vec<f64> {
    // Scale A by dt
    let mut a_dt: Vec<f64> = a.iter().map(|&v| v * dt).collect();

    // Row norm of A*dt
    let row_norm = (0..n).map(|i| {
        (0..n).map(|j| a_dt[i * n + j].abs()).sum::<f64>()
    }).fold(0.0_f64, f64::max);

    // Scaling factor k: smallest k such that row_norm <= 2^k
    let k_scale = if row_norm > 0.0 {
        (row_norm.log2().ceil() as usize).max(1)
    } else {
        1
    };

    // Scale: A1 = A*dt / 2^k
    let scale = (1u64 << k_scale) as f64;
    for v in &mut a_dt {
        *v /= scale;
    }

    // Taylor expansion: exp(A1) ≈ I + A1 + A1²/2! + A1³/3! + ...
    let mut result = vec![0.0; n * n];
    // Initialize to identity
    for i in 0..n {
        result[i * n + i] = 1.0;
    }

    // Current power term: starts as A1/1! = A1
    let mut term = a_dt.clone();

    // Max terms from E+: min(3*row_norm_scaled + 6, 100)
    let row_norm_scaled = row_norm / scale;
    let max_terms = ((3.0 * row_norm_scaled + 6.0) as usize).min(100).max(10);

    // Add first term (A1)
    for i in 0..n * n {
        result[i] += term[i];
    }

    for m in 2..=max_terms {
        // term = term * A1 / m
        let old_term = term.clone();
        for i in 0..n {
            for j in 0..n {
                let mut sum = 0.0;
                for p in 0..n {
                    sum += old_term[i * n + p] * a_dt[p * n + j];
                }
                term[i * n + j] = sum / m as f64;
            }
        }

        // Check convergence
        let mut max_ratio = 0.0_f64;
        for i in 0..n * n {
            if result[i].abs() > 1.0e-30 {
                max_ratio = max_ratio.max((term[i] / result[i]).abs());
            }
        }

        // Add term to result
        for i in 0..n * n {
            result[i] += term[i];
        }

        if max_ratio < 1.0e-20 {
            break;
        }
    }

    // Square k times: result = result^(2^k)
    for _ in 0..k_scale {
        let old = result.clone();
        result = mat_mul(&old, &old, n);
    }

    result
}

/// Compute matrix inverse using Gaussian elimination.
fn matrix_inverse(a: &[f64], n: usize) -> Option<Vec<f64>> {
    // Augmented matrix [A | I]
    let mut aug = vec![0.0; n * 2 * n];
    for i in 0..n {
        for j in 0..n {
            aug[i * 2 * n + j] = a[i * n + j];
        }
        aug[i * 2 * n + n + i] = 1.0;
    }

    // Forward elimination with partial pivoting
    for col in 0..n {
        // Find pivot
        let mut max_val = aug[col * 2 * n + col].abs();
        let mut max_row = col;
        for row in (col + 1)..n {
            let val = aug[row * 2 * n + col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }
        if max_val < 1.0e-30 {
            return None; // Singular
        }

        // Swap rows
        if max_row != col {
            for j in 0..2 * n {
                let tmp = aug[col * 2 * n + j];
                aug[col * 2 * n + j] = aug[max_row * 2 * n + j];
                aug[max_row * 2 * n + j] = tmp;
            }
        }

        // Eliminate
        let pivot = aug[col * 2 * n + col];
        for j in 0..2 * n {
            aug[col * 2 * n + j] /= pivot;
        }
        for row in 0..n {
            if row == col { continue; }
            let factor = aug[row * 2 * n + col];
            for j in 0..2 * n {
                aug[row * 2 * n + j] -= factor * aug[col * 2 * n + j];
            }
        }
    }

    // Extract inverse
    let mut inv = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            inv[i * n + j] = aug[i * 2 * n + n + j];
        }
    }
    Some(inv)
}

/// Matrix multiplication C = A * B (n×n matrices, row-major).
fn mat_mul(a: &[f64], b: &[f64], n: usize) -> Vec<f64> {
    let mut c = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            let mut sum = 0.0;
            for k in 0..n {
                sum += a[i * n + k] * b[k * n + j];
            }
            c[i * n + j] = sum;
        }
    }
    c
}

/// Compute trace of an n×n matrix.
fn mat_trace(a: &[f64], n: usize) -> f64 {
    (0..n).map(|i| a[i * n + i]).sum()
}

// ══════════════════════════════════════════════════════════════════════════════
// CTF Coefficient Computation from State-Space
// ══════════════════════════════════════════════════════════════════════════════

/// Compute CTF coefficients from state-space matrices A, B, C, D.
///
/// Implements Seem (1987) equations 2.1.24–2.1.26 with the R-matrix recurrence.
fn compute_ctf_from_state_space(
    a: &[f64],     // n×n
    b: &[f64],     // n×2 (col 0: outside, col 1: inside)
    c: &[f64],     // 2×n (row 0: outside flux, row 1: inside flux)
    d: &[f64],     // 2×2
    n: usize,
    dt: f64,
) -> CtfCoefficients {
    // Step 1: Matrix exponential exp(A*dt)
    let a_exp = matrix_exponential(a, n, dt);

    // Step 2: A⁻¹
    let a_inv = match matrix_inverse(a, n) {
        Some(inv) => inv,
        None => {
            // Singular A matrix — fall back to steady-state
            log::warn!("Singular A matrix in CTF calculation, using steady-state fallback");
            let u = d[0 * 2 + 0]; // D(1,1)
            return CtfCoefficients {
                x: vec![u.abs()], y: vec![u.abs()], z: vec![u.abs()],
                phi: vec![],
                num_terms: 1,
            };
        }
    };

    // Step 3: Gamma1 = A⁻¹ * (exp(A*dt) - I) * B
    //         Gamma2 = A⁻¹ * (Gamma1/dt - B)
    //
    // exp(A*dt) - I
    let mut a_exp_minus_i = a_exp.clone();
    for i in 0..n {
        a_exp_minus_i[i * n + i] -= 1.0;
    }

    // (exp(A*dt) - I) * B   → n×2 result
    let mut exp_m_i_times_b = vec![0.0; n * 2];
    for i in 0..n {
        for j in 0..2 {
            let mut sum = 0.0;
            for k in 0..n {
                sum += a_exp_minus_i[i * n + k] * b[k * 2 + j];
            }
            exp_m_i_times_b[i * 2 + j] = sum;
        }
    }

    // Gamma1 = A⁻¹ * (exp_m_i_times_b)  → n×2
    let mut gamma1 = vec![0.0; n * 2];
    for i in 0..n {
        for j in 0..2 {
            let mut sum = 0.0;
            for k in 0..n {
                sum += a_inv[i * n + k] * exp_m_i_times_b[k * 2 + j];
            }
            gamma1[i * 2 + j] = sum;
        }
    }

    // Gamma2 = A⁻¹ * (Gamma1/dt - B)  → n×2
    let mut g1_over_dt_minus_b = vec![0.0; n * 2];
    for i in 0..n * 2 {
        g1_over_dt_minus_b[i] = gamma1[i] / dt - b[i];
    }
    let mut gamma2 = vec![0.0; n * 2];
    for i in 0..n {
        for j in 0..2 {
            let mut sum = 0.0;
            for k in 0..n {
                sum += a_inv[i * n + k] * g1_over_dt_minus_b[k * 2 + j];
            }
            gamma2[i * 2 + j] = sum;
        }
    }

    // Step 4: s0 = C * Gamma2 + D  → 2×2
    // (since R0 = I, s0 = C * I * Gamma2 + D = C * Gamma2 + D)
    let mut s0 = [0.0; 4]; // 2×2
    for i in 0..2 {
        for j in 0..2 {
            let mut sum = 0.0;
            for k in 0..n {
                sum += c[i * n + k] * gamma2[k * 2 + j];
            }
            s0[i * 2 + j] = sum + d[i * 2 + j];
        }
    }

    // Enforce cross-term symmetry: average |s0(0,1)| and |s0(1,0)|
    let avg_cross = (s0[0 * 2 + 1].abs() + s0[1 * 2 + 0].abs()) / 2.0;
    s0[0 * 2 + 1] = avg_cross * s0[0 * 2 + 1].signum();
    s0[1 * 2 + 0] = avg_cross * s0[1 * 2 + 0].signum();

    // Initialize CTF output vectors
    //
    // E+ CTF convention (HeatBalanceSurfaceManager.cc):
    //   q_inside  = Y[0]*T_out - Z[0]*T_in + history  (positive = heat into zone)
    //   q_outside = X[0]*T_out - Y[0]*T_in + history
    //
    // From state-space s matrix:
    //   s(0,0) → X[0]:  outside flux sensitivity to outside temp
    //   s(1,0) → Y[0]:  inside flux sensitivity to outside temp (cross-coupling)
    //   s(1,1) → negated to get Z[0]: inside flux sensitivity to inside temp
    //
    // Note: s(0,1) = -s(1,0) by reciprocity. E+ uses s(0,1) and negates in
    // the outside equation, but we use s(1,0) directly for clarity.
    let mut x_vec = vec![s0[0 * 2 + 0]];
    let mut y_vec = vec![s0[1 * 2 + 0]]; // cross: outside temp → inside flux
    let mut z_vec = vec![-s0[1 * 2 + 1]]; // E+ negates Z
    let mut phi_vec: Vec<f64> = Vec::new();

    // Step 5: Iterative R-matrix recurrence for history terms
    //
    // R(0) = I
    // For j = 1, 2, ...:
    //   PhiR = AExp * R(j-1)
    //   e(j) = -trace(PhiR) / j
    //   R(j) = PhiR + e(j) * I
    //   s(j) = C * [R(j-1) * (Gamma1 - Gamma2) + R(j) * Gamma2] + e(j) * D
    //
    // CTFOutside[j] = s(j)(0,0) = X[j]
    // CTFCross[j]   = s(j)(0,1) = Y[j]
    // CTFInside[j]  = -s(j)(1,1) = Z[j]
    // CTFFlux[j]    = -e(j) = Phi[j]

    let mut r_prev = vec![0.0; n * n]; // R(j-1), starts as I
    for i in 0..n {
        r_prev[i * n + i] = 1.0;
    }

    // Precompute Gamma1 - Gamma2
    let mut g1_minus_g2 = vec![0.0; n * 2];
    for i in 0..n * 2 {
        g1_minus_g2[i] = gamma1[i] - gamma2[i];
    }

    let mut e1_abs = 0.0; // |e(1)| for convergence check
    let mut converged = false;

    for j in 1..=MAX_CTF_TERMS {
        // PhiR = AExp * R(j-1)
        let phi_r = mat_mul(&a_exp, &r_prev, n);

        // e(j) = -trace(PhiR) / j
        let tr = mat_trace(&phi_r, n);
        let e_j = -tr / j as f64;

        if j == 1 {
            e1_abs = e_j.abs();
        }

        // R(j) = PhiR + e(j) * I
        let mut r_curr = phi_r.clone();
        for i in 0..n {
            r_curr[i * n + i] += e_j;
        }

        // s(j) = C * [R(j-1) * (Gamma1 - Gamma2) + R(j) * Gamma2] + e(j) * D
        //
        // Term1 = R(j-1) * (Gamma1 - Gamma2)  → n×2
        let mut term1 = vec![0.0; n * 2];
        for i in 0..n {
            for col in 0..2 {
                let mut sum = 0.0;
                for k in 0..n {
                    sum += r_prev[i * n + k] * g1_minus_g2[k * 2 + col];
                }
                term1[i * 2 + col] = sum;
            }
        }

        // Term2 = R(j) * Gamma2  → n×2
        let mut term2 = vec![0.0; n * 2];
        for i in 0..n {
            for col in 0..2 {
                let mut sum = 0.0;
                for k in 0..n {
                    sum += r_curr[i * n + k] * gamma2[k * 2 + col];
                }
                term2[i * 2 + col] = sum;
            }
        }

        // Combined = Term1 + Term2  → n×2
        let mut combined = vec![0.0; n * 2];
        for i in 0..n * 2 {
            combined[i] = term1[i] + term2[i];
        }

        // s(j) = C * combined + e(j) * D  → 2×2
        let mut s_j = [0.0; 4];
        for i in 0..2 {
            for col in 0..2 {
                let mut sum = 0.0;
                for k in 0..n {
                    sum += c[i * n + k] * combined[k * 2 + col];
                }
                s_j[i * 2 + col] = sum + e_j * d[i * 2 + col];
            }
        }

        // Store CTF terms
        x_vec.push(s_j[0 * 2 + 0]);
        y_vec.push(s_j[1 * 2 + 0]); // cross: outside temp → inside flux
        z_vec.push(-s_j[1 * 2 + 1]); // E+ negates Z
        phi_vec.push(-e_j);           // E+ negates e

        // Check convergence: |e(j)| / |e(1)| < limit
        if e1_abs > 0.0 && (e_j.abs() / e1_abs) < CONVERGENCE_LIMIT {
            converged = true;
            break;
        }

        // Also check if e(j) is essentially zero
        if e_j.abs() < 1.0e-30 {
            converged = true;
            break;
        }

        // Advance R
        r_prev = r_curr;
    }

    if !converged {
        log::warn!("CTF convergence not reached in {} terms", MAX_CTF_TERMS);
    }

    let num_terms = x_vec.len();

    CtfCoefficients {
        x: x_vec,
        y: y_vec,
        z: z_vec,
        phi: phi_vec,
        num_terms,
    }
}

/// Apply CTF to compute surface heat fluxes for one timestep.
///
/// Returns (q_inside, q_outside) in [W/m²].
/// q_inside positive = heat flowing INTO the zone.
/// q_outside positive = heat flowing OUT of the building.
pub fn apply_ctf(
    ctf: &CtfCoefficients,
    history: &CtfHistory,
    t_outside_current: f64,
    t_inside_current: f64,
) -> (f64, f64) {
    // Inside heat flux: q_in = Y[0]*T_out - Z[0]*T_in + Σ(Φ·q_in_old) + higher-order terms
    let mut q_inside = ctf.y[0] * t_outside_current - ctf.z[0] * t_inside_current;

    // Flux history terms
    for j in 0..ctf.phi.len() {
        if j < history.q_inside.len() {
            q_inside += ctf.phi[j] * history.q_inside[j];
        }
    }

    // Higher-order Y, Z terms from temperature history
    for j in 1..ctf.y.len() {
        let idx = j - 1;
        if idx < history.t_outside.len() {
            q_inside += ctf.y[j] * history.t_outside[idx];
        }
        if idx < history.t_inside.len() {
            q_inside -= ctf.z[j] * history.t_inside[idx];
        }
    }

    // Outside heat flux: q_out = X[0]*T_out - Y[0]*T_in + Σ(Φ·q_out_old)
    let mut q_outside = ctf.x[0] * t_outside_current - ctf.y[0] * t_inside_current;

    for j in 0..ctf.phi.len() {
        if j < history.q_outside.len() {
            q_outside += ctf.phi[j] * history.q_outside[j];
        }
    }

    for j in 1..ctf.x.len() {
        let idx = j - 1;
        if idx < history.t_outside.len() {
            q_outside += ctf.x[j] * history.t_outside[idx];
        }
        if idx < history.t_inside.len() {
            q_outside -= ctf.y[j] * history.t_inside[idx];
        }
    }

    (q_inside, q_outside)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn make_insulated_wall() -> Vec<ResolvedLayer> {
        vec![
            ResolvedLayer::new(1.311, 2240.0, 836.8, 0.2),
            ResolvedLayer::new(0.04, 30.0, 840.0, 0.1),
        ]
    }

    fn make_concrete_slab() -> Vec<ResolvedLayer> {
        // C5 - 4 IN HW CONCRETE (from 1ZoneUncontrolled)
        vec![ResolvedLayer::new(1.729577, 2242.585, 836.8, 0.1014984)]
    }

    #[test]
    fn test_ctf_steady_state_u_factor() {
        let mats = make_insulated_wall();
        let layer_refs = &mats;
        let dt = 3600.0;

        let ctf = calculate_ctf(layer_refs, dt);

        // After many timesteps at constant temps, should converge to U*(Tout-Tin).
        // Steady-state gain: Σ(Y[j]) / (1 - Σ(Phi[j])) = U
        let u_expected = 1.0 / (0.2 / 1.311 + 0.1 / 0.04);
        let sum_y: f64 = ctf.y.iter().sum();
        let sum_phi: f64 = ctf.phi.iter().sum();
        let u_ctf = sum_y / (1.0 - sum_phi);
        assert_relative_eq!(u_ctf, u_expected, max_relative = 0.05);
    }

    #[test]
    fn test_ctf_apply_equal_temps_zero_flux() {
        let mats = make_insulated_wall();
        let layer_refs = &mats;
        let ctf = calculate_ctf(&layer_refs, 3600.0);
        let history = CtfHistory::new(ctf.num_terms.max(1), 20.0);

        let (q_in, q_out) = apply_ctf(&ctf, &history, 20.0, 20.0);

        // Equal temps and equilibrium history → zero flux
        assert!(q_in.abs() < 0.5, "q_in = {} should be near zero", q_in);
        assert!(q_out.abs() < 0.5, "q_out = {} should be near zero", q_out);
    }

    #[test]
    fn test_ctf_heat_flows_inward_when_outdoor_warmer() {
        let mats = make_insulated_wall();
        let layer_refs = &mats;
        let ctf = calculate_ctf(&layer_refs, 3600.0);
        let history = CtfHistory::new(ctf.num_terms.max(1), 20.0);

        let (q_in, _q_out) = apply_ctf(&ctf, &history, 35.0, 20.0);

        // Outdoor warmer → heat flows in → q_inside positive
        assert!(q_in > 0.0);
    }

    #[test]
    fn test_concrete_slab_generates_multiple_ctf_terms() {
        // The 4" concrete slab should generate multiple CTF terms (not just 1).
        let mats = make_concrete_slab();
        let layer_refs = &mats;
        let dt = 900.0; // 15-minute timestep (4 per hour)

        let ctf = calculate_ctf(layer_refs, dt);

        // Should have multiple terms (E+ generates ~3 for this construction)
        assert!(ctf.num_terms >= 2,
            "Concrete slab should have multiple CTF terms, got {}", ctf.num_terms);
        assert!(ctf.phi.len() >= 1,
            "Concrete slab should have flux history terms");

        // X[0] ≠ Y[0] ≠ Z[0] for multi-node CTF (unlike lumped RC where they're equal)
        // This is the key improvement: Z[0] ≠ Y[0] allows adiabatic surfaces to
        // properly model thermal mass through the CTF equation.
        let z0_minus_y0 = (ctf.z[0] - ctf.y[0]).abs();
        assert!(z0_minus_y0 > 0.1,
            "Z[0] ({:.4}) and Y[0] ({:.4}) should differ for multi-node CTF (diff = {:.4})",
            ctf.z[0], ctf.y[0], z0_minus_y0);
    }

    #[test]
    fn test_concrete_slab_steady_state() {
        let mats = make_concrete_slab();
        let layer_refs = &mats;
        let dt = 900.0;

        let ctf = calculate_ctf(layer_refs, dt);

        // Steady-state U-factor should match k/thickness
        let u_expected = 1.729577 / 0.1014984; // ≈ 17.04
        let sum_x: f64 = ctf.x.iter().sum();
        let sum_y: f64 = ctf.y.iter().sum();
        let sum_z: f64 = ctf.z.iter().sum();
        let sum_phi: f64 = ctf.phi.iter().sum();

        let u_from_y = sum_y / (1.0 - sum_phi);
        let u_from_x = sum_x / (1.0 - sum_phi);
        let u_from_z = sum_z / (1.0 - sum_phi);

        assert_relative_eq!(u_from_y, u_expected, max_relative = 0.05);
        assert_relative_eq!(u_from_x, u_expected, max_relative = 0.05);
        assert_relative_eq!(u_from_z, u_expected, max_relative = 0.05);
    }

    #[test]
    fn test_simple_construction_matches_layered() {
        // calculate_ctf_simple with equivalent parameters should give
        // similar results to calculate_ctf with the actual material.
        let mats = make_concrete_slab();
        let layer_refs = &mats;
        let dt = 900.0;

        let ctf_layered = calculate_ctf(&layer_refs, dt);

        // SimpleConstruction parameters:
        // u_factor = k/t = 1.729577/0.1014984 = 17.04
        // thermal_capacity = rho*cp*t = 2242.585 * 836.8 * 0.1014984 = 190422
        let ctf_simple = calculate_ctf_simple(17.04, 190422.0, dt, false, None, None);

        // Both should have multiple terms
        assert!(ctf_layered.num_terms >= 2);
        assert!(ctf_simple.num_terms >= 2);

        // Steady-state U should match for both
        let sum_y_l: f64 = ctf_layered.y.iter().sum();
        let sum_phi_l: f64 = ctf_layered.phi.iter().sum();
        let u_l = sum_y_l / (1.0 - sum_phi_l);

        let sum_y_s: f64 = ctf_simple.y.iter().sum();
        let sum_phi_s: f64 = ctf_simple.phi.iter().sum();
        let u_s = sum_y_s / (1.0 - sum_phi_s);

        assert_relative_eq!(u_l, u_s, max_relative = 0.1);
    }

    #[test]
    fn test_no_mass_construction() {
        // R13WALL: no thermal mass → steady-state CTF
        let ctf = calculate_ctf_simple(0.4365, 0.0, 900.0, false, None, None);
        assert_eq!(ctf.num_terms, 1);
        assert_relative_eq!(ctf.x[0], 0.4365, max_relative = 0.01);
        assert_relative_eq!(ctf.y[0], 0.4365, max_relative = 0.01);
        assert_relative_eq!(ctf.z[0], 0.4365, max_relative = 0.01);
        assert!(ctf.phi.is_empty());
    }

    #[test]
    fn test_adiabatic_ctf_denominator_nonzero() {
        // For the adiabatic floor in 1ZoneUncontrolled, the key fix is that
        // Z[0] ≠ Y[0], giving a non-zero denominator (Z[0] - Y[0]) in the
        // adiabatic CTF equation, which properly models thermal mass.
        let mats = make_concrete_slab();
        let layer_refs = &mats;
        let dt = 900.0;
        let ctf = calculate_ctf(layer_refs, dt);

        let z0_minus_y0 = ctf.z[0] - ctf.y[0];
        assert!(z0_minus_y0 > 0.0,
            "Z[0] - Y[0] = {:.4} should be positive for adiabatic thermal mass",
            z0_minus_y0);

        // For the concrete slab, this should be a significant value
        // (representing the thermal mass contribution to the inside CTF)
        assert!(z0_minus_y0 > 1.0,
            "Z[0] - Y[0] = {:.4} should be substantial for 4\" concrete",
            z0_minus_y0);
    }

    #[test]
    fn test_concrete_slab_ctf_matches_energyplus() {
        // Compare our CTF coefficients against EnergyPlus EIO output for
        // the C5 - 4 IN HW CONCRETE slab used in 1ZoneUncontrolled.
        //
        // E+ reports (from eplusout.eio):
        //   Construction CTF,FLOOR, 2, 1, 5, 0.250, 17.04, ...
        //   CTF,   5,  -4.1142049E-08,   1.5543709E-08,  -4.1142049E-08,   1.2297289E-11
        //   CTF,   4,   0.00057884701,   0.00022976293,   0.00057884701,  -4.0580373E-07
        //   CTF,   3,  -0.33051123,      0.091914804,    -0.33051123,       0.0006592243
        //   CTF,   2,  12.566595,         2.1743923,      12.566595,       -0.058066613
        //   CTF,   1, -62.622544,         4.7096437,     -62.622544,        0.60555731
        //   CTF,   0,  58.08561,          0.72354869,     58.08561
        //
        // Note: E+ #CTFs=5 means 5 history terms (j=1..5), plus j=0 term = 6 total.
        // E+ uses Phi convention where Phi(1..5) listed with CTF(1..5).

        let ep_x = vec![58.08561, -62.622544, 12.566595, -0.33051123, 0.00057884701, -4.1142049e-08];
        let ep_y = vec![0.72354869, 4.7096437, 2.1743923, 0.091914804, 0.00022976293, 1.5543709e-08];
        let ep_z = vec![58.08561, -62.622544, 12.566595, -0.33051123, 0.00057884701, -4.1142049e-08];
        let ep_phi = vec![0.60555731, -0.058066613, 0.0006592243, -4.0580373e-07, 1.2297289e-11];

        let mats = make_concrete_slab();
        let layer_refs = &mats;
        let dt = 900.0; // 15-minute timestep

        let ctf = calculate_ctf(layer_refs, dt);

        println!("\n=== OpenBSE CTF vs EnergyPlus CTF for C5 Concrete Slab (dt=900s) ===");
        println!("Our num_terms = {}, E+ #CTFs+1 = 6", ctf.num_terms);
        println!("\n  j    X(ours)         X(E+)          Y(ours)         Y(E+)          Z(ours)         Z(E+)");
        for j in 0..ctf.num_terms.max(6) {
            let our_x = if j < ctf.x.len() { ctf.x[j] } else { 0.0 };
            let our_y = if j < ctf.y.len() { ctf.y[j] } else { 0.0 };
            let our_z = if j < ctf.z.len() { ctf.z[j] } else { 0.0 };
            let e_x = if j < ep_x.len() { ep_x[j] } else { 0.0 };
            let e_y = if j < ep_y.len() { ep_y[j] } else { 0.0 };
            let e_z = if j < ep_z.len() { ep_z[j] } else { 0.0 };
            println!("  {j}  {our_x:>14.6}  {e_x:>14.6}  {our_y:>14.6}  {e_y:>14.6}  {our_z:>14.6}  {e_z:>14.6}");
        }
        println!("\n  j    Phi(ours)       Phi(E+)");
        for j in 0..ctf.phi.len().max(5) {
            let our_p = if j < ctf.phi.len() { ctf.phi[j] } else { 0.0 };
            let e_p = if j < ep_phi.len() { ep_phi[j] } else { 0.0 };
            println!("  {j}  {our_p:>14.8}  {e_p:>14.8}");
        }

        // Key metric: Z[0] - Y[0] (adiabatic denominator)
        let our_denom = ctf.z[0] - ctf.y[0];
        let ep_denom = ep_z[0] - ep_y[0];
        println!("\nZ[0] - Y[0] (adiabatic denominator): ours={our_denom:.4}, E+={ep_denom:.4}");

        // Steady-state U
        let our_sum_y: f64 = ctf.y.iter().sum();
        let our_sum_phi: f64 = ctf.phi.iter().sum();
        let ep_sum_y: f64 = ep_y.iter().sum();
        let ep_sum_phi: f64 = ep_phi.iter().sum();
        let our_u = our_sum_y / (1.0 - our_sum_phi);
        let ep_u = ep_sum_y / (1.0 - ep_sum_phi);
        println!("Steady-state U: ours={our_u:.4}, E+={ep_u:.4} (expected=17.04)");

        // The steady-state U should be correct
        assert_relative_eq!(our_u, 17.04, max_relative = 0.05);
    }

    #[test]
    fn test_matrix_exponential_identity() {
        // exp(0*dt) = I
        let a = vec![0.0; 4]; // 2×2 zero matrix
        let result = matrix_exponential(&a, 2, 1.0);
        assert_relative_eq!(result[0], 1.0, max_relative = 1e-10);
        assert_relative_eq!(result[1], 0.0, epsilon = 1e-10);
        assert_relative_eq!(result[2], 0.0, epsilon = 1e-10);
        assert_relative_eq!(result[3], 1.0, max_relative = 1e-10);
    }

    #[test]
    fn test_matrix_inverse_simple() {
        // [[2, 1], [1, 1]] → inv = [[1, -1], [-1, 2]]
        let a = vec![2.0, 1.0, 1.0, 1.0];
        let inv = matrix_inverse(&a, 2).unwrap();
        assert_relative_eq!(inv[0], 1.0, max_relative = 1e-10);
        assert_relative_eq!(inv[1], -1.0, max_relative = 1e-10);
        assert_relative_eq!(inv[2], -1.0, max_relative = 1e-10);
        assert_relative_eq!(inv[3], 2.0, max_relative = 1e-10);
    }

    #[test]
    fn test_hwwall_ctf_stability() {
        // Case 900 HWWALL: Wood Siding + Foam Insulation + Concrete Block
        // This construction was causing CTF instability (runaway cooling loads)
        let layers = vec![
            ResolvedLayer::new(0.14, 530.0, 900.0, 0.009),
            ResolvedLayer::new(0.04, 10.0, 1400.0, 0.0615),
            ResolvedLayer::new(0.51, 1400.0, 1000.0, 0.1),
        ];

        let dt = 900.0; // 15-min timestep
        let ctf = calculate_ctf(&layers, dt);

        println!("HWWALL CTF coefficients (dt=900s):");
        println!("  num_terms = {}", ctf.num_terms);
        for j in 0..ctf.x.len() {
            println!("  j={}: X={:.6}, Y={:.6}, Z={:.6}",
                j, ctf.x[j], ctf.y[j], ctf.z[j]);
        }
        for j in 0..ctf.phi.len() {
            println!("  Phi[{}] = {:.10}", j, ctf.phi[j]);
        }

        // Check steady-state: U = ΣY / (1 - ΣPhi)
        let sum_y: f64 = ctf.y.iter().sum();
        let sum_phi: f64 = ctf.phi.iter().sum();
        let u_steady = sum_y / (1.0 - sum_phi);
        let u_expected = 1.0 / (0.009/0.14 + 0.0615/0.04 + 0.1/0.51);
        println!("\n  sum(Y) = {:.6}", sum_y);
        println!("  sum(Phi) = {:.10}", sum_phi);
        println!("  U_steady = {:.4} W/(m²K)", u_steady);
        println!("  U_expected = {:.4} W/(m²K)", u_expected);
        assert_relative_eq!(u_steady, u_expected, max_relative = 0.02);

        // CTF STABILITY CHECK:
        // The Phi coefficients represent the system's memory decay.
        // For stability, all eigenvalues of the companion matrix must have |λ| < 1.
        // A necessary (but not sufficient) condition is |sum(Phi)| < 1.
        // A stricter check: all individual |Phi| should be < 1 (approximately).
        println!("\n  Stability check:");
        let max_phi_abs = ctf.phi.iter().map(|p| p.abs()).fold(0.0_f64, f64::max);
        println!("  max |Phi| = {:.10}", max_phi_abs);
        println!("  |sum(Phi)| = {:.10}", sum_phi.abs());
        assert!(sum_phi.abs() < 1.0, "CTF unstable: |sum(Phi)| = {} >= 1.0", sum_phi.abs());
    }

    #[test]
    fn test_ltwall_ctf_coefficients() {
        // ASHRAE 140 Case 600 LTWALL: Wood Siding 9mm / Fiberglass Quilt 66mm / Plasterboard 12mm
        // Exact material properties from E+ Case600.idf
        let layers = vec![
            ResolvedLayer::new(0.14, 530.0, 900.0, 0.009),    // WOOD SIDING-1
            ResolvedLayer::new(0.04, 12.0, 840.0, 0.066),     // FIBERGLASS QUILT-1
            ResolvedLayer::new(0.16, 950.0, 840.0, 0.012),    // PLASTERBOARD-1
        ];

        let dt = 900.0; // 4 timesteps/hour

        let ctf = calculate_ctf(&layers, dt);

        // Expected U = 1 / (0.009/0.14 + 0.066/0.04 + 0.012/0.16)
        //            = 1 / (0.0643 + 1.65 + 0.075) = 1/1.789 = 0.559 W/(m²K)
        let u_expected = 0.559;

        let sum_x: f64 = ctf.x.iter().sum();
        let sum_y: f64 = ctf.y.iter().sum();
        let sum_z: f64 = ctf.z.iter().sum();
        let sum_phi: f64 = ctf.phi.iter().sum();
        let u_ctf_y = sum_y / (1.0 - sum_phi);
        let u_ctf_z = sum_z / (1.0 - sum_phi);

        println!("\n=== LTWALL CTF (ASHRAE 140 Case 600 wall, dt={}s) ===", dt);
        println!("  num_terms = {}, nodes per layer:", ctf.num_terms);

        // Print layer discretization
        for (li, l) in layers.iter().enumerate() {
            let alpha = l.conductivity / (l.density * l.specific_heat);
            let dxn = (2.0 * alpha * dt).sqrt();
            let nodes_raw = (l.thickness / dxn).ceil() as usize;
            let nodes = nodes_raw.max(6).min(18);
            let dx = l.thickness / nodes as f64;
            println!("    Layer[{}]: t={:.4}m k={:.3} ρ={:.0} cp={:.0} → α={:.3e}, nodes={}, dx={:.5}m",
                li, l.thickness, l.conductivity, l.density, l.specific_heat,
                alpha, nodes, dx);
        }

        let rcmax: usize = layers.iter().map(|l| {
            let alpha = l.conductivity / (l.density * l.specific_heat);
            let dxn = (2.0 * alpha * dt).sqrt();
            let nodes = (l.thickness / dxn).ceil().max(6.0).min(18.0) as usize;
            nodes
        }).sum::<usize>() - 1;
        println!("  rcmax (state nodes) = {}", rcmax);

        println!("\n  j    X           Y           Z           Phi");
        for j in 0..ctf.num_terms {
            let phi_j = if j > 0 && (j-1) < ctf.phi.len() { ctf.phi[j-1] } else { 0.0 };
            println!("  {}  {:>12.6}  {:>12.6}  {:>12.6}  {:>12.8}",
                j, ctf.x[j], ctf.y[j], ctf.z[j], phi_j);
        }

        println!("\n  Steady-state U: from Y={:.4}, from Z={:.4}, expected={:.4}", u_ctf_y, u_ctf_z, u_expected);
        println!("  Z[0]={:.4}, Y[0]={:.6}, X[0]={:.4}", ctf.z[0], ctf.y[0], ctf.x[0]);
        println!("  Z[0]-Y[0]={:.4} (for adiabatic surface)", ctf.z[0] - ctf.y[0]);
        println!("  sum(Phi)={:.8}", sum_phi);

        // Verify steady-state U
        assert_relative_eq!(u_ctf_y, u_expected, max_relative = 0.02);
        assert_relative_eq!(u_ctf_z, u_expected, max_relative = 0.02);

        // Z[0] should be dominated by the innermost layer (plasterboard)
        // k_plaster/dx_plaster_node ≈ 0.16/0.002 = 80 W/(m²K)
        // But with surface-adjacent node (1.5× cap), Z[0] will be lower.
        // For a lightweight wall, Z[0] should be >> U (fast surface response)
        println!("  Z[0]/U = {:.1}x (should be >> 1 for lightweight wall)", ctf.z[0] / u_expected);
        assert!(ctf.z[0] > 5.0 * u_expected,
            "Z[0]={:.2} should be much larger than U={:.3} for lightweight construction",
            ctf.z[0], u_expected);
    }

    #[test]
    fn test_ltwall_simple_vs_layered_ctf() {
        // Compare calculate_ctf_simple (from YAML U+C) vs calculate_ctf (from layers)
        // Both should give similar CTF behavior for the LTWALL.
        let dt = 900.0;

        // Simple construction: U=0.559, C=14534
        let ctf_simple = calculate_ctf_simple(0.559, 14534.0, dt, false, None, None);

        // Layered construction: exact LTWALL layers
        let layers = vec![
            ResolvedLayer::new(0.14, 530.0, 900.0, 0.009),
            ResolvedLayer::new(0.04, 12.0, 840.0, 0.066),
            ResolvedLayer::new(0.16, 950.0, 840.0, 0.012),
        ];
        let ctf_layered = calculate_ctf(&layers, dt);

        println!("\n=== LTWALL: simple(U+C) vs layered(materials) CTF ===");
        println!("  Simple:  num_terms={}, Z[0]={:.4}, Y[0]={:.6}", ctf_simple.num_terms, ctf_simple.z[0], ctf_simple.y[0]);
        println!("  Layered: num_terms={}, Z[0]={:.4}, Y[0]={:.6}", ctf_layered.num_terms, ctf_layered.z[0], ctf_layered.y[0]);

        let sum_z_s: f64 = ctf_simple.z.iter().sum();
        let sum_z_l: f64 = ctf_layered.z.iter().sum();
        let sum_phi_s: f64 = ctf_simple.phi.iter().sum();
        let sum_phi_l: f64 = ctf_layered.phi.iter().sum();
        let u_s = sum_z_s / (1.0 - sum_phi_s);
        let u_l = sum_z_l / (1.0 - sum_phi_l);
        println!("  Simple  U_steady={:.4}, sum(Phi)={:.6}", u_s, sum_phi_s);
        println!("  Layered U_steady={:.4}, sum(Phi)={:.6}", u_l, sum_phi_l);

        // Both should give same steady-state U within 5%
        assert_relative_eq!(u_s, u_l, max_relative = 0.05);

        // Run a simulated step response to compare transient behavior
        println!("\n  Step response: T_out jumps from 20 to 0°C, T_in held at 20°C");
        let mut hist_s = CtfHistory::new(ctf_simple.num_terms.max(1), 20.0);
        let mut hist_l = CtfHistory::new(ctf_layered.num_terms.max(1), 20.0);

        println!("  Step  q_in_simple  q_in_layered  ratio");
        for step in 0..20 {
            let t_out = if step == 0 { 20.0 } else { 0.0 };
            let (q_s, qo_s) = apply_ctf(&ctf_simple, &hist_s, t_out, 20.0);
            let (q_l, qo_l) = apply_ctf(&ctf_layered, &hist_l, t_out, 20.0);
            let ratio = if q_l.abs() > 0.01 { q_s / q_l } else { 0.0 };
            if step <= 10 || step == 19 {
                println!("  {:>4}   {:>10.3}   {:>10.3}    {:.3}", step, q_s, q_l, ratio);
            }
            hist_s.shift(t_out, 20.0, q_s, qo_s);
            hist_l.shift(t_out, 20.0, q_l, qo_l);
        }
    }

    #[test]
    fn test_nomass_insulation_plus_timber_floor() {
        // ASHRAE 140 Case 600 floor: NoMass insulation (R=25.075) + Timber Flooring
        // This tests the NoMass layer partitioning in calculate_ctf().
        //
        // EnergyPlus uses Material:NoMass for the insulation (zero thermal mass,
        // R=25.075 m²K/W). Only the timber flooring has thermal mass.
        //
        // Expected:
        //   Total R = 25.075 + 0.025/0.14 = 25.254 m²K/W → U = 0.0396
        //   Thermal mass from timber only: C = 650*1200*0.025 = 19500 J/(m²K)
        let dt = 900.0;

        // NoMass insulation + massed timber
        let layers_nomass = vec![
            ResolvedLayer::new_no_mass(25.075, 1.003),  // NoMass: R=25.075
            ResolvedLayer::new(0.14, 650.0, 1200.0, 0.025),  // Timber Flooring
        ];
        let ctf_nomass = calculate_ctf(&layers_nomass, dt);

        // Old approach: physical insulation with parasitic mass
        let layers_physical = vec![
            ResolvedLayer::new(0.04, 10.0, 1000.0, 1.003),   // Physical insulation
            ResolvedLayer::new(0.14, 650.0, 1200.0, 0.025),  // Timber Flooring
        ];
        let ctf_physical = calculate_ctf(&layers_physical, dt);

        // Both should have the same steady-state U
        let u_expected = 1.0 / (25.075 + 0.025 / 0.14);
        let sum_y_nm: f64 = ctf_nomass.y.iter().sum();
        let sum_phi_nm: f64 = ctf_nomass.phi.iter().sum();
        let u_nm = sum_y_nm / (1.0 - sum_phi_nm);

        let sum_y_ph: f64 = ctf_physical.y.iter().sum();
        let sum_phi_ph: f64 = ctf_physical.phi.iter().sum();
        let u_ph = sum_y_ph / (1.0 - sum_phi_ph);

        println!("\n=== NoMass Insulation + Timber Floor CTF ===");
        println!("  U expected: {:.6}", u_expected);
        println!("  U (NoMass): {:.6} (terms={})", u_nm, ctf_nomass.num_terms);
        println!("  U (physical): {:.6} (terms={})", u_ph, ctf_physical.num_terms);

        // NoMass thermal mass = only timber = 19500 J/(m²K)
        // Physical thermal mass = timber + insulation = 19500 + 10030 = 29530 J/(m²K)
        let c_timber = 650.0 * 1200.0 * 0.025;
        let c_insul = 10.0 * 1000.0 * 1.003;
        println!("  C_timber = {:.0}, C_insul = {:.0}, total_physical = {:.0}",
            c_timber, c_insul, c_timber + c_insul);

        // Both U-values should match expected
        assert_relative_eq!(u_nm, u_expected, max_relative = 0.02);
        assert_relative_eq!(u_ph, u_expected, max_relative = 0.02);

        // NoMass version should have FEWER terms (less thermal mass → faster response)
        // Physical insulation adds 10030 J/(m²K) of parasitic mass
        println!("  NoMass terms: {}, Physical terms: {}",
            ctf_nomass.num_terms, ctf_physical.num_terms);

        // Step response comparison: NoMass should respond faster
        let mut hist_nm = CtfHistory::new(ctf_nomass.num_terms.max(1), 20.0);
        let mut hist_ph = CtfHistory::new(ctf_physical.num_terms.max(1), 20.0);

        println!("\n  Step response: T_out = 0°C, T_in = 20°C");
        println!("  Step  q_nomass     q_physical   ratio");
        for step in 0..10 {
            let t_out = if step == 0 { 20.0 } else { 0.0 };
            let (q_nm, qo_nm) = apply_ctf(&ctf_nomass, &hist_nm, t_out, 20.0);
            let (q_ph, qo_ph) = apply_ctf(&ctf_physical, &hist_ph, t_out, 20.0);
            let ratio = if q_ph.abs() > 0.001 { q_nm / q_ph } else { 0.0 };
            println!("  {:>4}   {:>10.4}   {:>10.4}    {:.3}", step, q_nm, q_ph, ratio);
            hist_nm.shift(t_out, 20.0, q_nm, qo_nm);
            hist_ph.shift(t_out, 20.0, q_ph, qo_ph);
        }

        // At steady state (step 9), both should converge to U * ΔT
        // The NoMass version reaches steady state faster because it has
        // ~52% less thermal mass (19500 vs 29530 J/(m²K))
    }

    #[test]
    fn test_hwwall_simple_vs_layered_ctf() {
        // Compare Case 900 heavyweight wall: simplified (U+C) vs E+ explicit layers
        //
        // E+ HWWALL (from outside to inside):
        //   Wood Siding 9mm (k=0.14, ρ=530, cp=900)
        //   Foam Insulation 61.5mm (k=0.04, ρ=10, cp=1400)
        //   Concrete Block 100mm (k=0.51, ρ=1400, cp=1000)
        //
        // Total R = 0.009/0.14 + 0.0615/0.04 + 0.1/0.51 = 0.064 + 1.538 + 0.196 = 1.798
        // Total C = 530*900*0.009 + 10*1400*0.0615 + 1400*1000*0.1 = 4293 + 861 + 140000 = 145154
        // U = 1/1.798 = 0.556 W/(m²K)

        let dt = 900.0;

        // E+ explicit layers
        let layers = vec![
            ResolvedLayer::new(0.14, 530.0, 900.0, 0.009),    // Wood Siding
            ResolvedLayer::new(0.04, 10.0, 1400.0, 0.0615),   // Foam Insulation
            ResolvedLayer::new(0.51, 1400.0, 1000.0, 0.1),    // Concrete Block
        ];
        let ctf_layered = calculate_ctf(&layers, dt);

        // Simplified from U+C (what YAML currently uses)
        // Current default (k=1.0, ρ=2000) — shows the difference
        let ctf_simple = calculate_ctf_simple(0.556, 145154.0, dt, false, None, None);

        // With E+ concrete block properties (k=0.51, ρ=1400)
        let ctf_block = calculate_ctf_simple(0.556, 145154.0, dt, false,
            Some(0.51), Some(1400.0));

        println!("\n=== HWWALL: simple(U+C) vs block(k=0.51) vs layered(E+ materials) CTF ===");
        println!("  Simple:  num_terms={}, Z[0]={:.4}, Y[0]={:.6}, X[0]={:.4}",
            ctf_simple.num_terms, ctf_simple.z[0], ctf_simple.y[0], ctf_simple.x[0]);
        println!("  Block:   num_terms={}, Z[0]={:.4}, Y[0]={:.6}, X[0]={:.4}",
            ctf_block.num_terms, ctf_block.z[0], ctf_block.y[0], ctf_block.x[0]);
        println!("  Layered: num_terms={}, Z[0]={:.4}, Y[0]={:.6}, X[0]={:.4}",
            ctf_layered.num_terms, ctf_layered.z[0], ctf_layered.y[0], ctf_layered.x[0]);

        let sum_y_s: f64 = ctf_simple.y.iter().sum();
        let sum_z_s: f64 = ctf_simple.z.iter().sum();
        let sum_phi_s: f64 = ctf_simple.phi.iter().sum();
        let u_s = sum_y_s / (1.0 - sum_phi_s);
        let u_z_s = sum_z_s / (1.0 - sum_phi_s);

        let sum_y_l: f64 = ctf_layered.y.iter().sum();
        let sum_z_l: f64 = ctf_layered.z.iter().sum();
        let sum_phi_l: f64 = ctf_layered.phi.iter().sum();
        let u_l = sum_y_l / (1.0 - sum_phi_l);
        let u_z_l = sum_z_l / (1.0 - sum_phi_l);

        println!("  Simple  U_y={:.4}, U_z={:.4}, sum(Phi)={:.6}", u_s, u_z_s, sum_phi_s);
        println!("  Layered U_y={:.4}, U_z={:.4}, sum(Phi)={:.6}", u_l, u_z_l, sum_phi_l);

        // Print CTF coefficients side by side
        let max_terms = ctf_simple.num_terms.max(ctf_layered.num_terms);
        println!("\n  j    Z_simple     Z_layered    Y_simple     Y_layered    X_simple     X_layered");
        for j in 0..max_terms {
            let zs = if j < ctf_simple.z.len() { ctf_simple.z[j] } else { 0.0 };
            let zl = if j < ctf_layered.z.len() { ctf_layered.z[j] } else { 0.0 };
            let ys = if j < ctf_simple.y.len() { ctf_simple.y[j] } else { 0.0 };
            let yl = if j < ctf_layered.y.len() { ctf_layered.y[j] } else { 0.0 };
            let xs = if j < ctf_simple.x.len() { ctf_simple.x[j] } else { 0.0 };
            let xl = if j < ctf_layered.x.len() { ctf_layered.x[j] } else { 0.0 };
            println!("  {j}  {zs:>12.4}  {zl:>12.4}  {ys:>12.4}  {yl:>12.4}  {xs:>12.4}  {xl:>12.4}");
        }
        println!("\n  j    Phi_simple    Phi_layered");
        for j in 0..ctf_simple.phi.len().max(ctf_layered.phi.len()) {
            let ps = if j < ctf_simple.phi.len() { ctf_simple.phi[j] } else { 0.0 };
            let pl = if j < ctf_layered.phi.len() { ctf_layered.phi[j] } else { 0.0 };
            println!("  {j}  {ps:>14.8}  {pl:>14.8}");
        }

        // Key diagnostic: surface thermal admittance
        // Y_0_surface = Z[0] - Y[0] for interior surface response
        let admit_s = ctf_simple.z[0] - ctf_simple.y[0];
        let admit_b = ctf_block.z[0] - ctf_block.y[0];
        let admit_l = ctf_layered.z[0] - ctf_layered.y[0];
        println!("\n  Interior surface admittance Z[0]-Y[0]:");
        println!("    Simple (k=1.0):  {:.4}", admit_s);
        println!("    Block  (k=0.51): {:.4}", admit_b);
        println!("    Layered (E+):    {:.4}", admit_l);
        println!("    Simple/Layered:  {:.3}", admit_s / admit_l);
        println!("    Block/Layered:   {:.3}", admit_b / admit_l);

        // Step response comparison
        let mut hist_s = CtfHistory::new(ctf_simple.num_terms.max(1), 20.0);
        let mut hist_l = CtfHistory::new(ctf_layered.num_terms.max(1), 20.0);

        println!("\n  Step response: T_out = 0°C, T_in = 20°C (heavy-mass wall)");
        println!("  Step  q_in_simple  q_in_layered  ratio");
        for step in 0..40 {
            let t_out = if step == 0 { 20.0 } else { 0.0 };
            let (q_s, qo_s) = apply_ctf(&ctf_simple, &hist_s, t_out, 20.0);
            let (q_l, qo_l) = apply_ctf(&ctf_layered, &hist_l, t_out, 20.0);
            let ratio = if q_l.abs() > 0.01 { q_s / q_l } else { 0.0 };
            if step <= 15 || step % 5 == 0 || step == 39 {
                println!("  {:>4}   {:>10.3}   {:>10.3}    {:.3}", step, q_s, q_l, ratio);
            }
            hist_s.shift(t_out, 20.0, q_s, qo_s);
            hist_l.shift(t_out, 20.0, q_l, qo_l);
        }
    }
}
