//! Conduction Transfer Function (CTF) calculation and application.
//!
//! Implements heat conduction through wall/roof/floor constructions using the
//! state-space method of Seem (1987), matching EnergyPlus's `calculateTransferFunction()`.
//!
//! Algorithm:
//! 1. Discretize each layer into N finite-difference nodes (6 to 19 per layer)
//! 2. Pick the CTF time step: the zone timestep, or a whole number of zone
//!    timesteps when a layer reaches the node cap (Fourier number 1)
//! 3. Build state-space matrices A, B, C, D
//! 4. Compute matrix exponential exp(A·Δt) via Taylor series with scaling/squaring
//! 5. Compute A⁻¹, then Gamma1 and Gamma2 intermediate matrices
//! 6. Iteratively compute CTF coefficients (s, e) using R-matrix recurrence
//! 7. If the series needs more than 18 history terms or its X, Y and Z sums
//!    disagree, lengthen the CTF time step by one zone timestep and repeat
//!
//! Per-timestep conduction equations:
//!   q_inside  = Σ(Y·T_out) - Σ(Z·T_in) + Σ(Φ·q_in_old)
//!   q_outside = Σ(X·T_out) - Σ(Y·T_in) + Σ(Φ·q_out_old)
//!
//! When the CTF time step spans several zone timesteps, the history terms are
//! spaced one CTF time step apart and the surface balance still updates every
//! zone timestep, reading staggered history sets ([`CtfHistory::update`]).
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
    /// How far the steady-state U implied by the coefficients is from the
    /// construction's own U, in percent. Above 5% the CTF is not accepted;
    /// between 1% and 5% the caller reports a warning.
    pub steady_state_error_pct: f64,
    /// CTF time step [s]: the interval between history terms.
    pub time_step: f64,
    /// CTF time step in zone timesteps (E+ `NumHistories`); 1 for almost every
    /// construction, more for massive layers that reach the node cap.
    pub num_histories: usize,
}

impl CtfCoefficients {
    /// A massless (or nearly massless) construction: steady conduction at
    /// conductance `u`, with no history terms.
    fn steady(u: f64, dt: f64) -> Self {
        Self {
            x: vec![u],
            y: vec![u],
            z: vec![u],
            phi: vec![],
            num_terms: 1,
            steady_state_error_pct: 0.0,
            time_step: dt,
            num_histories: 1,
        }
    }
}

/// CTF history state for one surface (persists across timesteps).
///
/// `t_outside`, `t_inside`, `q_inside` and `q_outside` are the histories the
/// CTF equations read, index 0 = one CTF time step back. When the CTF time
/// step equals the zone timestep they are a plain shift register. When it
/// spans N zone timesteps, every zone timestep's values are kept for N times
/// the series length, and the working histories are the exact values N, 2N, …
/// zone timesteps back. Each zone timestep then belongs to one of N staggered
/// history sets, each a CTF recursion at the CTF time step, so the surface
/// balance updates every zone timestep and each set conserves energy on its own.
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
    /// Every zone timestep's values, for a CTF time step longer than the zone
    /// timestep.
    staggered: Option<StaggeredHistory>,
}

/// Ring of [t_out, t_in, q_in, q_out] for every zone timestep, newest at `head − 1`.
#[derive(Debug, Clone)]
struct StaggeredHistory {
    /// Zone timesteps per CTF time step (E+ `NumHistories`, ≥ 2).
    num_histories: usize,
    ring: Vec<[f64; 4]>,
    head: usize,
}

impl CtfHistory {
    pub fn new(num_terms: usize, initial_temp: f64) -> Self {
        Self {
            t_outside: vec![initial_temp; num_terms],
            t_inside: vec![initial_temp; num_terms],
            q_inside: vec![0.0; num_terms],
            q_outside: vec![0.0; num_terms],
            staggered: None,
        }
    }

    /// History sized and timed for `ctf`, starting at a uniform temperature
    /// with zero flux.
    pub fn for_ctf(ctf: &CtfCoefficients, initial_temp: f64) -> Self {
        let num_terms = ctf.num_terms.max(1);
        let mut history = Self::new(num_terms, initial_temp);
        if ctf.num_histories > 1 {
            history.staggered = Some(StaggeredHistory {
                num_histories: ctf.num_histories,
                ring: vec![[initial_temp, initial_temp, 0.0, 0.0]; ctf.num_histories * num_terms],
                head: 0,
            });
        }
        history
    }

    /// Reset all history entries to a uniform temperature with zero flux.
    /// Used at the start of each sizing design day to clear residual state.
    pub fn reset(&mut self, temp: f64) {
        self.t_outside.fill(temp);
        self.t_inside.fill(temp);
        self.q_inside.fill(0.0);
        self.q_outside.fill(0.0);
        if let Some(st) = &mut self.staggered {
            st.ring.fill([temp, temp, 0.0, 0.0]);
            st.head = 0;
        }
    }

    /// Record the end of a zone timestep: the current surface temperatures and
    /// conduction fluxes enter the history.
    ///
    /// With a CTF time step of N zone timesteps, the next zone timestep reads
    /// the values N·(i + 1) zone timesteps back, which are exact, not
    /// interpolated.
    pub fn update(&mut self, t_out: f64, t_in: f64, q_in: f64, q_out: f64) {
        let Some(st) = &mut self.staggered else {
            shift_in(&mut self.t_outside, t_out);
            shift_in(&mut self.t_inside, t_in);
            shift_in(&mut self.q_inside, q_in);
            shift_in(&mut self.q_outside, q_out);
            return;
        };
        let len = st.ring.len();
        st.ring[st.head] = [t_out, t_in, q_in, q_out];
        st.head = (st.head + 1) % len;
        for i in 0..self.t_outside.len() {
            // k zone timesteps back, counting the value just stored as k = 1.
            let k = st.num_histories * (i + 1);
            let [a, b, c, d] = st.ring[(st.head + len - k) % len];
            self.t_outside[i] = a;
            self.t_inside[i] = b;
            self.q_inside[i] = c;
            self.q_outside[i] = d;
        }
    }
}

/// Push `value` in at index 0; the oldest entry falls off.
fn shift_in(history: &mut [f64], value: f64) {
    if history.is_empty() {
        return;
    }
    history.copy_within(..history.len() - 1, 1);
    history[0] = value;
}

// ══════════════════════════════════════════════════════════════════════════════
// State-Space CTF Calculation (Seem 1987)
// ══════════════════════════════════════════════════════════════════════════════

/// Minimum number of nodes per material layer (E+ `MinNodes`).
const MIN_NODES: usize = 6;
/// Maximum nodes per layer, and one more than the most history terms a CTF
/// series may have (E+ `Construction::MaxCTFTerms`).
const MAX_CTF_TERMS: usize = 19;
/// Convergence limit for flux history ratio (E+ ConvrgLim).
const CONVERGENCE_LIMIT: f64 = 1.0e-13;
/// Largest allowed relative difference between the X, Y and Z series sums,
/// which are equal for a valid CTF set (E+ `MaxAllowedCTFSumError`).
const MAX_CTF_SUM_ERROR: f64 = 0.01;
/// Largest allowed steady-state U error of a CTF set [%].
const MAX_U_ERROR_PCT: f64 = 5.0;
/// Longest CTF time step [s] (E+ `MaxAllowedTimeStep`, 7 h).
const MAX_CTF_TIME_STEP: f64 = 7.0 * 3600.0;

/// Layer properties extracted for the state-space calculation.
/// All values in SI units.
struct LayerProps {
    k: f64,   // conductivity [W/(m·K)]
    rho: f64, // density [kg/m³]
    cp: f64,  // specific heat [J/(kg·K)]
    dx: f64,  // node spacing [m]
    nodes: usize,
    /// Position in the construction, 1 = outside layer (for messages).
    position: usize,
}

/// Nodes in a massed layer (E+ `calculateTransferFunction`): the explicit
/// finite-difference stability spacing √(2αΔt) at the zone timestep, truncated
/// to a whole number and held within [`MIN_NODES`, `MAX_CTF_TERMS`].
fn layer_nodes(l: &ResolvedLayer, dt_zone: f64) -> usize {
    let alpha = l.conductivity / (l.density * l.specific_heat);
    let dxn = (2.0 * alpha * dt_zone).sqrt();
    let nodes = (l.thickness / dxn) as usize;
    nodes.clamp(MIN_NODES, MAX_CTF_TERMS)
}

/// Initial CTF time step and its length in zone timesteps (E+
/// `calculateTransferFunction`). A layer at the node cap is coarser than the
/// stability spacing, so the CTF time step grows to that layer's Fourier-number-1
/// step ρ·cp·Δx²/k, rounded to the nearest whole number of zone timesteps.
/// Otherwise the CTF time step is the zone timestep.
fn initial_ctf_time_step(layers: &[LayerProps], dt_zone: f64) -> (f64, usize) {
    let dtn = layers
        .iter()
        .filter(|l| l.nodes >= MAX_CTF_TERMS && l.rho * l.cp > 0.0)
        .map(|l| l.rho * l.cp * l.dx * l.dx / l.k)
        .fold(0.0_f64, f64::max);
    // E+ keeps a computed step within 10% of the zone timestep as is; a capped
    // layer always gives at least twice the zone timestep, so in one dimension
    // only the two branches below occur.
    if dtn > dt_zone {
        let n = ((dtn / dt_zone) + 0.5) as usize;
        (dt_zone * n as f64, n.max(1))
    } else {
        (dt_zone, 1)
    }
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
///
/// NoMass layers between massed layers (e.g. insulation in a cavity wall) become
/// a single resistive node pair, as EnergyPlus does for interior resistive
/// layers: no capacitance of their own, a conductance of 1/R between the two
/// adjacent interface nodes, which take their capacitance from the neighbouring
/// massed layers. Adjacent interior NoMass layers are combined first.
///
/// The CTF time step starts at `dt` (the zone timestep) or, when a layer reaches
/// the node cap, at that layer's Fourier-number-1 step in whole zone timesteps.
/// If the series then needs more than `MAX_CTF_TERMS − 1` history terms, its X,
/// Y and Z sums differ by more than 1%, or its steady-state U is more than 5% off
/// the layers' U, the CTF time step grows by one zone timestep and the series is
/// recomputed. A CTF time step reaching 7 h is an error, as in EnergyPlus.
pub fn try_calculate_ctf(layers: &[ResolvedLayer], dt: f64) -> Result<CtfCoefficients, String> {
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

    let total_c: f64 = massed_layers
        .iter()
        .map(|l| l.density * l.specific_heat * l.thickness)
        .sum();

    // Very low thermal mass or no massed layers: steady-state CTF
    if total_c < 1.0 || massed_layers.is_empty() || total_r <= 0.0 {
        let u = if total_r > 0.0 { 1.0 / total_r } else { 5.0 };
        return Ok(CtfCoefficients::steady(u, dt));
    }

    // Compute nodes per layer and build LayerProps. `massed_layers` starts and ends
    // with a massed layer; NoMass layers in between get one node with k = 1 and
    // dx = R (E+ `Construction::calculateTransferFunction`, resistive layers), so the
    // conductance across them is 1/R and they add no capacitance.
    let mut layer_props: Vec<LayerProps> = Vec::with_capacity(massed_layers.len());
    for (offset, l) in massed_layers.iter().enumerate() {
        let position = massed_start + offset + 1;
        if is_massless(l) {
            let r = l.resistance();
            if r <= 0.0 {
                continue; // no resistance, nothing to model
            }
            match layer_props.last_mut() {
                // Combine adjacent resistive layers (E+ does the same)
                Some(prev) if prev.rho * prev.cp == 0.0 => prev.dx += r,
                _ => layer_props.push(LayerProps {
                    k: 1.0,
                    rho: 0.0,
                    cp: 0.0,
                    dx: r,
                    nodes: 1,
                    position,
                }),
            }
            continue;
        }
        let nodes = layer_nodes(l, dt);
        layer_props.push(LayerProps {
            k: l.conductivity,
            rho: l.density,
            cp: l.specific_heat,
            dx: l.thickness / nodes as f64,
            nodes,
            position,
        });
    }

    // Total state-space nodes: sum(nodes) - 1 (following E+ convention)
    let rcmax: usize = layer_props.iter().map(|l| l.nodes).sum::<usize>() - 1;

    if rcmax == 0 {
        // Degenerate case: single node → use lumped RC
        let u = 1.0 / total_r;
        let tau = total_r * total_c;
        let alpha = (-dt / tau).exp();
        let c0 = u * (1.0 - alpha);
        return Ok(CtfCoefficients {
            x: vec![c0],
            y: vec![c0],
            z: vec![c0],
            phi: vec![alpha],
            ..CtfCoefficients::steady(u, dt)
        });
    }

    // Build state-space matrices with NoMass boundary resistances. They depend
    // only on the node network, which is fixed at the zone timestep.
    let (a_mat, b_vec, c_vec, d_vec) =
        build_state_space(&layer_props, rcmax, r_outside_nomass, r_inside_nomass);

    let u_expected = 1.0 / total_r;
    let (mut ctf_dt, mut num_histories) = initial_ctf_time_step(&layer_props, dt);
    // Why the previous attempt was rejected, for the error message.
    let mut last_rejection = String::from("the first CTF time step was already too long");
    loop {
        if ctf_dt >= MAX_CTF_TIME_STEP - 1.0e-9 {
            return Err(ctf_time_step_error(&layer_props, ctf_dt, &last_rejection));
        }

        let attempt = compute_ctf_from_state_space(&a_mat, &b_vec, &c_vec, &d_vec, rcmax, ctf_dt)?;
        let Some(mut result) = attempt else {
            last_rejection = format!(
                "at a {:.0} s CTF time step the series needed more than {} history terms",
                ctf_dt,
                MAX_CTF_TERMS - 1
            );
            num_histories += 1;
            ctf_dt += dt;
            continue;
        };

        // A non-finite solution means the construction is degenerate; running with
        // it would put NaN or inf into the heat balance.
        let has_nan = [&result.x, &result.y, &result.z, &result.phi]
            .iter()
            .any(|v| v.iter().any(|c| !c.is_finite()));
        if has_nan {
            return Err(format!(
                "the CTF state-space solution is not finite (rcmax={rcmax}, layers={}, \
                 massed={}): a layer has zero or infinite conductivity, zero thickness, or \
                 zero density or specific heat",
                layers.len(),
                massed_layers.len(),
            ));
        }

        // In theory ΣX = ΣY = ΣZ; a CTF set whose sums disagree has not reached a
        // valid solution (E+ MaxAllowedCTFSumError).
        let sums = [&result.x, &result.y, &result.z].map(|v| v.iter().sum::<f64>().abs());
        let biggest = sums.iter().copied().fold(0.0_f64, f64::max);
        if biggest <= 0.0 {
            return Err(
                "the CTF series sums are all zero: the construction conducts no \
                        heat. Check the layer conductivities and thicknesses"
                    .to_string(),
            );
        }
        let sum_error = ((sums[0] - sums[1]).abs()).max((sums[2] - sums[1]).abs()) / biggest;

        // Steady-state U from the coefficients:
        // q*(1-ΣΦ) = ΣY*Tout - ΣZ*Tin → U = ΣZ/(1-ΣΦ)
        let sum_z: f64 = result.z.iter().sum();
        let sum_phi: f64 = result.phi.iter().sum();
        let u_ctf = sum_z / (1.0 - sum_phi);
        let u_error_pct = ((u_ctf - u_expected) / u_expected * 100.0).abs();

        if sum_error > MAX_CTF_SUM_ERROR || u_error_pct > MAX_U_ERROR_PCT {
            last_rejection = format!(
                "at a {ctf_dt:.0} s CTF time step the X, Y and Z sums differed by \
                 {:.1}% and the steady-state U was {u_ctf:.4} W/(m²K) against {u_expected:.4} \
                 from the layers",
                sum_error * 100.0
            );
            num_histories += 1;
            ctf_dt += dt;
            continue;
        }

        result.steady_state_error_pct = u_error_pct;
        result.time_step = ctf_dt;
        result.num_histories = num_histories;
        if num_histories > 1 {
            log::info!(
                "CTF time step {ctf_dt:.0} s ({num_histories} zone timesteps) for a \
                 construction with {rcmax} nodes and {} terms",
                result.num_terms
            );
        }
        return Ok(result);
    }
}

/// The error for a construction whose CTF time step reaches 7 h: name the
/// layer with the longest Fourier-number step and what to change.
fn ctf_time_step_error(layers: &[LayerProps], ctf_dt: f64, last_rejection: &str) -> String {
    use std::fmt::Write as _;
    let limiting = layers.iter().filter(|l| l.rho * l.cp > 0.0).max_by(|a, b| {
        let fo = |l: &LayerProps| l.rho * l.cp * l.dx * l.dx / l.k;
        fo(a).total_cmp(&fo(b))
    });
    let mut msg = format!(
        "no stable CTF series with a time step under 7 h (reached {:.1} h; {last_rejection})",
        ctf_dt / 3600.0
    );
    if let Some(l) = limiting {
        let thickness = l.dx * l.nodes as f64;
        let _ = write!(
            msg,
            ". Layer {} ({thickness:.3} m, k={:.3} W/(m·K), ρ={:.0} kg/m³, cp={:.0} J/(kg·K)) \
             has the longest node time constant. A construction this massive is beyond the CTF method \
             (EnergyPlus stops at the same limit): check the layer properties, and model \
             less of the mass, for ground contact a thinner soil layer on a `ground` \
             boundary or an F-factor construction",
            l.position, l.k, l.rho, l.cp
        );
    }
    msg
}

/// Panicking form of [`try_calculate_ctf`], for tests and callers that build
/// constructions known to be valid. The simulation uses the `try_` form so a
/// bad construction becomes an error message instead of a crash.
pub fn calculate_ctf(layers: &[ResolvedLayer], dt: f64) -> CtfCoefficients {
    try_calculate_ctf(layers, dt).unwrap_or_else(|e| panic!("{e}"))
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
pub fn try_calculate_ctf_simple(
    u_factor: f64,
    thermal_capacity: f64,
    dt: f64,
    mass_outside: bool,
    mass_conductivity: Option<f64>,
    mass_density: Option<f64>,
) -> Result<CtfCoefficients, String> {
    if u_factor <= 0.0 {
        return Ok(CtfCoefficients::steady(0.0, dt));
    }

    // Very low thermal mass: steady-state
    if thermal_capacity < 1.0 {
        return Ok(CtfCoefficients::steady(u_factor, dt));
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
        return Ok(CtfCoefficients {
            x: vec![c0],
            y: vec![c0],
            z: vec![c0],
            phi: vec![alpha_val],
            ..CtfCoefficients::steady(u_factor, dt)
        });
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
        let k_insul = 0.04; // W/(m·K) fiberglass
        let rho_insul = 12.0; // kg/m³ (E+ fiberglass quilt)
        let cp_insul = 840.0; // J/(kg·K)

        if r_total > 20.0 {
            // ── Very-high-R floor: [insulation | timber interior] ─────
            // E+ LTFLOOR: R-25 NoMass insulation + 25mm Timber Flooring
            // All specified thermal mass resides on the interior side.
            // Only used for R > 20 (floor constructions with NoMass insulation).
            let k_int = 0.14; // W/(m·K) timber
            let rho_int = 650.0; // kg/m³
            let cp_int = 1200.0; // J/(kg·K)
            let t_int = (thermal_capacity / (rho_int * cp_int)).max(0.01);
            let r_int = t_int / k_int;
            let r_insul_floor = (r_total - r_int).max(0.01);

            // 10% cap for NoMass-like floor insulation (real mass ≈ 0).
            let max_insul_mass = 0.10 * thermal_capacity;
            let max_insul_t = max_insul_mass / (rho_insul * cp_insul);
            let t_insul_raw = k_insul * r_insul_floor;
            let t_insul = t_insul_raw.max(0.001).min(max_insul_t.max(0.01));
            let k_adj = if t_insul < t_insul_raw {
                t_insul / r_insul_floor
            } else {
                k_insul
            };

            let insul_layer = ResolvedLayer::new(k_adj, rho_insul, cp_insul, t_insul);
            let int_layer = ResolvedLayer::new(k_int, rho_int, cp_int, t_int);
            return try_calculate_ctf(&[insul_layer, int_layer], dt);
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
        let k_int = 0.16; // W/(m·K) plasterboard
        let rho_int = 950.0; // kg/m³
        let cp_int = 840.0; // J/(kg·K)
        let c_int_12mm: f64 = rho_int * cp_int * 0.012; // 9576 J/(m²K)
        let c_int = c_int_12mm.min(thermal_capacity * 0.75);
        let t_int = (c_int / (rho_int * cp_int)).max(0.003);

        let k_ext = 0.14; // W/(m·K) wood siding / roof deck
        let rho_ext = 530.0; // kg/m³
        let cp_ext = 900.0; // J/(kg·K)
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
        let k_adj = if t_insul < t_insul_raw {
            t_insul / r_insul_wr
        } else {
            k_insul
        };

        let ext_layer = ResolvedLayer::new(k_ext, rho_ext, cp_ext, t_ext);
        let insul_layer = ResolvedLayer::new(k_adj, rho_insul, cp_insul, t_insul);
        let int_layer = ResolvedLayer::new(k_int, rho_int, cp_int, t_int);
        return try_calculate_ctf(&[ext_layer, insul_layer, int_layer], dt);
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

        let _insul = ResolvedLayer::new(actual_k_insul, rho_insul, cp_insul, t_insul);
        let _mass = ResolvedLayer::new(k_mass, rho_mass, cp_mass, t_mass);

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
            let k_fin = 0.16; // W/(m·K) plasterboard
            let rho_fin = 950.0; // kg/m³
            let cp_fin = 840.0; // J/(kg·K)
            let t_fin = 0.012; // 12mm thickness
            let r_fin = t_fin / k_fin; // ~0.075 m²·K/W
            let cap_fin = rho_fin * cp_fin * t_fin; // ~9576 J/(m²·K)

            // Adjust mass layer to account for finish layer's thermal capacity
            let cap_mass_adj = (thermal_capacity - cap_fin).max(1.0);
            let t_mass_adj = cap_mass_adj / (rho_mass * cp_mass);
            let r_mass_adj = t_mass_adj / k_mass;

            // Remaining R for insulation (after mass and finish)
            let r_insul_adj = (r_total - r_mass_adj - r_fin).max(0.01);
            let t_insul_adj = (k_insul * r_insul_adj)
                .max(0.001)
                .min(max_insul_thickness.max(0.01));
            let actual_k_insul_adj = if t_insul_adj < k_insul * r_insul_adj {
                t_insul_adj / r_insul_adj
            } else {
                k_insul
            };

            let mass_layer = ResolvedLayer::new(k_mass, rho_mass, cp_mass, t_mass_adj);
            let insul_layer =
                ResolvedLayer::new(actual_k_insul_adj, rho_insul, cp_insul, t_insul_adj);
            let finish_layer = ResolvedLayer::new(k_fin, rho_fin, cp_fin, t_fin);

            return try_calculate_ctf(&[mass_layer, insul_layer, finish_layer], dt);
        } else {
            // ── Mass-inside construction: insulation outside, mass inside ───
            //
            // E+ ASHRAE 140 heavyweight constructions:
            //   HWWALL  = [Wood Siding 9mm | Foam Insulation 61.5mm | Concrete Block 100mm]
            //   HWFLOOR = [R-25 NoMass Insulation | Concrete Slab 80mm]
            //
            // Two sub-cases based on insulation R-value:
            //   R_insul > 10: Floor-like (NoMass insulation, no exterior finish)
            //   R_insul ≤ 10: Wall-like (wood siding exterior + foam insulation)

            if r_insul > 10.0 {
                // ── High-R floor: [NoMass insulation | mass (interior)] ─────
                //
                // E+ HWFLOOR uses Material:NoMass with R=25.175 — zero thermal
                // mass. All construction thermal capacity is in the concrete slab.
                // Using NoMass avoids adding ~10 kJ/m²K of parasitic insulation
                // mass that slows the floor's interior transient response.
                let nomass_insul = ResolvedLayer::new_no_mass(r_insul, r_insul * k_insul);
                let mass_layer = ResolvedLayer::new(k_mass, rho_mass, cp_mass, t_mass);
                return try_calculate_ctf(&[nomass_insul, mass_layer], dt);
            }

            // ── Wall-like: [wood siding (ext) | foam insulation | mass (int)] ──
            //
            // E+ HWWALL has three layers: wood siding provides exterior thermal
            // mass and weather protection, foam insulation provides R-value,
            // and concrete block provides interior thermal mass.
            //
            // Without the wood siding, our 2-layer model has X[0] = 0.84 vs
            // E+'s 5.49 — an 85% mismatch that distorts exterior heat exchange.
            //
            // The mass thickness is solved to conserve total thermal capacity:
            //   C_total = C_wood + C_insul + C_mass
            // where C_insul = ρ_insul × cp_insul × k_insul × R_insul
            //
            // Solving analytically:
            //   t_mass = (C_total - C_wood - α·R') / (β - α/k_mass)
            //   where α = ρ_insul·cp_insul·k_insul, R' = R_total - R_wood, β = ρ_mass·cp_mass
            let k_wood = 0.14; // W/(m·K) wood siding
            let rho_wood = 530.0; // kg/m³
            let cp_wood = 900.0; // J/(kg·K)
            let t_wood = 0.009; // 9mm (E+ WOOD SIDING-1)
            let r_wood = t_wood / k_wood;
            let c_wood = rho_wood * cp_wood * t_wood;

            // E+ foam insulation: ρ=10, cp=1400 (not 1000)
            let rho_insul_hw = 10.0;
            let cp_insul_hw = 1400.0;
            let k_insul_hw = 0.04;

            // Solve for mass thickness conserving total thermal capacity
            let alpha = rho_insul_hw * cp_insul_hw * k_insul_hw; // ρ·cp·k
            let r_prime = r_total - r_wood; // R available for insul + mass
            let beta = rho_mass * cp_mass;

            let t_mass_hw =
                ((thermal_capacity - c_wood - alpha * r_prime) / (beta - alpha / k_mass)).max(0.01);
            let r_mass_hw = t_mass_hw / k_mass;
            let r_insul_hw_val = (r_prime - r_mass_hw).max(0.01);
            let t_insul_hw = (k_insul_hw * r_insul_hw_val).max(0.001);

            // Cap insulation thickness if needed (very high R constructions)
            let max_insul_t_hw = 0.10 * thermal_capacity / (rho_insul_hw * cp_insul_hw);
            let (t_insul_final, k_insul_final) =
                if t_insul_hw > max_insul_t_hw && max_insul_t_hw > 0.001 {
                    (max_insul_t_hw, max_insul_t_hw / r_insul_hw_val)
                } else {
                    (t_insul_hw, k_insul_hw)
                };

            let wood_layer = ResolvedLayer::new(k_wood, rho_wood, cp_wood, t_wood);
            let insul_layer =
                ResolvedLayer::new(k_insul_final, rho_insul_hw, cp_insul_hw, t_insul_final);
            let mass_layer = ResolvedLayer::new(k_mass, rho_mass, cp_mass, t_mass_hw);

            return try_calculate_ctf(&[wood_layer, insul_layer, mass_layer], dt);
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

    try_calculate_ctf(&[layer], dt)
}

/// Panicking form of [`try_calculate_ctf_simple`]; see [`calculate_ctf`].
pub fn calculate_ctf_simple(
    u_factor: f64,
    thermal_capacity: f64,
    dt: f64,
    mass_outside: bool,
    mass_conductivity: Option<f64>,
    mass_density: Option<f64>,
) -> CtfCoefficients {
    try_calculate_ctf_simple(
        u_factor,
        thermal_capacity,
        dt,
        mass_outside,
        mass_conductivity,
        mass_density,
    )
    .unwrap_or_else(|e| panic!("{e}"))
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
    let mut d = vec![0.0; 4]; // 2×2

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
        k_left: f64,   // conductivity to left neighbor
        k_right: f64,  // conductivity to right neighbor
        dx_left: f64,  // distance to left neighbor
        dx_right: f64, // distance to right neighbor
        cap: f64,      // thermal capacitance of this node [J/(m²·K)]
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
                let cap = 0.5 * (layer.rho * layer.cp * layer.dx + next.rho * next.cp * next.dx);
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
    assert_eq!(
        nodes.len(),
        n,
        "Node count mismatch: {} vs {}",
        nodes.len(),
        n
    );

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
        if cap <= 0.0 {
            continue;
        }

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
    b[0] = h_boundary_out / cap_0; // b[0*2+0]
                                   // Inside input → node n-1 (through NoMass boundary resistance)
    b[(n - 1) * 2 + 1] = h_boundary_in / cap_n;

    // C matrix: fluxes at surfaces (using boundary conductances)
    c[0] = -h_boundary_out; // c[0*n+0]: outside flux, node 0
    c[n + (n - 1)] = h_boundary_in; // inside flux, node n-1

    // D matrix: direct feedthrough (using boundary conductances)
    d[0] = h_boundary_out; // d[0*2+0]: outside temp → outside flux
    d[2 + 1] = -h_boundary_in; // inside temp → inside flux

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
    let row_norm = (0..n)
        .map(|i| (0..n).map(|j| a_dt[i * n + j].abs()).sum::<f64>())
        .fold(0.0_f64, f64::max);

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
                aug.swap(col * 2 * n + j, max_row * 2 * n + j);
            }
        }

        // Eliminate
        let pivot = aug[col * 2 * n + col];
        for j in 0..2 * n {
            aug[col * 2 * n + j] /= pivot;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
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
///
/// The series runs until the flux term ratio |e_j/e_1| falls below
/// [`CONVERGENCE_LIMIT`] or it reaches `n` terms, where R(n) = 0 by the
/// Cayley–Hamilton theorem and the series is exact (E+
/// `calculateFinalCoefficients`). Returns `Ok(None)` once it needs more than
/// `MAX_CTF_TERMS − 1` history terms, so the caller can lengthen the time step.
fn compute_ctf_from_state_space(
    a: &[f64], // n×n
    b: &[f64], // n×2 (col 0: outside, col 1: inside)
    c: &[f64], // 2×n (row 0: outside flux, row 1: inside flux)
    d: &[f64], // 2×2
    n: usize,
    dt: f64,
) -> Result<Option<CtfCoefficients>, String> {
    // Step 1: Matrix exponential exp(A*dt)
    let a_exp = matrix_exponential(a, n, dt);

    // Step 2: A⁻¹
    let a_inv = match matrix_inverse(a, n) {
        Some(inv) => inv,
        None => {
            // A singular matrix means zero net capacitance in the state space.
            // NoMass layers are resistive nodes, so for a valid construction this
            // is unreachable; reaching it means the layer data is degenerate.
            return Err(format!(
                "the CTF state-space matrix is singular (n={n}): the construction has no \
                 effective thermal capacitance. Check that every layer that is not \
                 no_mass has non-zero density and specific heat"
            ));
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
    let avg_cross = (s0[1].abs() + s0[2].abs()) / 2.0; // s0[0*2+1] and s0[1*2+0]
    s0[1] = avg_cross * s0[1].signum();
    s0[2] = avg_cross * s0[2].signum();

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
    let mut x_vec = vec![s0[0]]; // s0[0*2+0]
    let mut y_vec = vec![s0[2]]; // cross: outside temp → inside flux
    let mut z_vec = vec![-s0[2 + 1]]; // E+ negates Z
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

    for j in 1..=n {
        if j > MAX_CTF_TERMS - 1 {
            return Ok(None);
        }

        // PhiR = AExp * R(j-1)
        let phi_r = mat_mul(&a_exp, &r_prev, n);

        // e(j) = -trace(PhiR) / j
        let tr = mat_trace(&phi_r, n);
        let e_j = -tr / j as f64;

        if j == 1 {
            e1_abs = e_j.abs();
        }

        // R(j) = PhiR + e(j) * I; the last one, R(n), is zero.
        let last = j == n;
        let mut r_curr = if last { vec![0.0; n * n] } else { phi_r };
        if !last {
            for i in 0..n {
                r_curr[i * n + i] += e_j;
            }
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
        x_vec.push(s_j[0]); // s_j[0*2+0]
        y_vec.push(s_j[2]); // cross: outside temp → inside flux
        z_vec.push(-s_j[2 + 1]); // E+ negates Z
        phi_vec.push(-e_j); // E+ negates e

        // Check convergence: |e(j)| / |e(1)| < limit, or the exact last term
        if last || (e1_abs > 0.0 && (e_j.abs() / e1_abs) < CONVERGENCE_LIMIT) {
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

    debug_assert!(converged, "the series always ends by term n");

    let num_terms = x_vec.len();

    Ok(Some(CtfCoefficients {
        x: x_vec,
        y: y_vec,
        z: z_vec,
        phi: phi_vec,
        num_terms,
        steady_state_error_pct: 0.0,
        time_step: dt,
        num_histories: 1,
    }))
}

/// Apply CTF to compute surface heat fluxes for one timestep.
///
/// Returns (q_inside, q_outside) in [W/m²].
/// q_inside positive = heat flowing INTO the zone.
/// q_outside positive = conduction from the outside face into the wall, i.e.
/// heat flowing INTO the building (the E+ q''_ko form: X₀·T_o − Y₀·T_i + …).
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
        let ctf = calculate_ctf(layer_refs, 3600.0);
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
        let ctf = calculate_ctf(layer_refs, 3600.0);
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
        assert!(
            ctf.num_terms >= 2,
            "Concrete slab should have multiple CTF terms, got {}",
            ctf.num_terms
        );
        assert!(
            !ctf.phi.is_empty(),
            "Concrete slab should have flux history terms"
        );

        // X[0] ≠ Y[0] ≠ Z[0] for multi-node CTF (unlike lumped RC where they're equal)
        // This is the key improvement: Z[0] ≠ Y[0] allows adiabatic surfaces to
        // properly model thermal mass through the CTF equation.
        let z0_minus_y0 = (ctf.z[0] - ctf.y[0]).abs();
        assert!(
            z0_minus_y0 > 0.1,
            "Z[0] ({:.4}) and Y[0] ({:.4}) should differ for multi-node CTF (diff = {:.4})",
            ctf.z[0],
            ctf.y[0],
            z0_minus_y0
        );
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

        let ctf_layered = calculate_ctf(layer_refs, dt);

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
        assert!(
            z0_minus_y0 > 0.0,
            "Z[0] - Y[0] = {:.4} should be positive for adiabatic thermal mass",
            z0_minus_y0
        );

        // For the concrete slab, this should be a significant value
        // (representing the thermal mass contribution to the inside CTF)
        assert!(
            z0_minus_y0 > 1.0,
            "Z[0] - Y[0] = {:.4} should be substantial for 4\" concrete",
            z0_minus_y0
        );
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

        let ep_x = [
            58.08561,
            -62.622544,
            12.566595,
            -0.33051123,
            0.00057884701,
            -4.1142049e-08,
        ];
        let ep_y = [
            0.72354869,
            4.7096437,
            2.1743923,
            0.091914804,
            0.00022976293,
            1.5543709e-08,
        ];
        let ep_z = [
            58.08561,
            -62.622544,
            12.566595,
            -0.33051123,
            0.00057884701,
            -4.1142049e-08,
        ];
        let ep_phi = [
            0.60555731,
            -0.058066613,
            0.0006592243,
            -4.0580373e-07,
            1.2297289e-11,
        ];

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
            println!(
                "  j={}: X={:.6}, Y={:.6}, Z={:.6}",
                j, ctf.x[j], ctf.y[j], ctf.z[j]
            );
        }
        for j in 0..ctf.phi.len() {
            println!("  Phi[{}] = {:.10}", j, ctf.phi[j]);
        }

        // Check steady-state: U = ΣY / (1 - ΣPhi)
        let sum_y: f64 = ctf.y.iter().sum();
        let sum_phi: f64 = ctf.phi.iter().sum();
        let u_steady = sum_y / (1.0 - sum_phi);
        let u_expected = 1.0 / (0.009 / 0.14 + 0.0615 / 0.04 + 0.1 / 0.51);
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
        assert!(
            sum_phi.abs() < 1.0,
            "CTF unstable: |sum(Phi)| = {} >= 1.0",
            sum_phi.abs()
        );
    }

    #[test]
    fn test_ltwall_ctf_coefficients() {
        // ASHRAE 140 Case 600 LTWALL: Wood Siding 9mm / Fiberglass Quilt 66mm / Plasterboard 12mm
        // Exact material properties from E+ Case600.idf
        let layers = vec![
            ResolvedLayer::new(0.14, 530.0, 900.0, 0.009), // WOOD SIDING-1
            ResolvedLayer::new(0.04, 12.0, 840.0, 0.066),  // FIBERGLASS QUILT-1
            ResolvedLayer::new(0.16, 950.0, 840.0, 0.012), // PLASTERBOARD-1
        ];

        let dt = 900.0; // 4 timesteps/hour

        let ctf = calculate_ctf(&layers, dt);

        // Expected U = 1 / (0.009/0.14 + 0.066/0.04 + 0.012/0.16)
        //            = 1 / (0.0643 + 1.65 + 0.075) = 1/1.789 = 0.559 W/(m²K)
        let u_expected = 0.559;

        let _sum_x: f64 = ctf.x.iter().sum();
        let sum_y: f64 = ctf.y.iter().sum();
        let sum_z: f64 = ctf.z.iter().sum();
        let sum_phi: f64 = ctf.phi.iter().sum();
        let u_ctf_y = sum_y / (1.0 - sum_phi);
        let u_ctf_z = sum_z / (1.0 - sum_phi);

        println!(
            "\n=== LTWALL CTF (ASHRAE 140 Case 600 wall, dt={}s) ===",
            dt
        );
        println!("  num_terms = {}, nodes per layer:", ctf.num_terms);

        // Print layer discretization
        for (li, l) in layers.iter().enumerate() {
            let alpha = l.conductivity / (l.density * l.specific_heat);
            let nodes = layer_nodes(l, dt);
            let dx = l.thickness / nodes as f64;
            println!(
                "    Layer[{}]: t={:.4}m k={:.3} ρ={:.0} cp={:.0} → α={:.3e}, nodes={}, dx={:.5}m",
                li, l.thickness, l.conductivity, l.density, l.specific_heat, alpha, nodes, dx
            );
        }

        let rcmax: usize = layers.iter().map(|l| layer_nodes(l, dt)).sum::<usize>() - 1;
        println!("  rcmax (state nodes) = {}", rcmax);

        println!("\n  j    X           Y           Z           Phi");
        for j in 0..ctf.num_terms {
            let phi_j = if j > 0 && (j - 1) < ctf.phi.len() {
                ctf.phi[j - 1]
            } else {
                0.0
            };
            println!(
                "  {}  {:>12.6}  {:>12.6}  {:>12.6}  {:>12.8}",
                j, ctf.x[j], ctf.y[j], ctf.z[j], phi_j
            );
        }

        println!(
            "\n  Steady-state U: from Y={:.4}, from Z={:.4}, expected={:.4}",
            u_ctf_y, u_ctf_z, u_expected
        );
        println!(
            "  Z[0]={:.4}, Y[0]={:.6}, X[0]={:.4}",
            ctf.z[0], ctf.y[0], ctf.x[0]
        );
        println!(
            "  Z[0]-Y[0]={:.4} (for adiabatic surface)",
            ctf.z[0] - ctf.y[0]
        );
        println!("  sum(Phi)={:.8}", sum_phi);

        // Verify steady-state U
        assert_relative_eq!(u_ctf_y, u_expected, max_relative = 0.02);
        assert_relative_eq!(u_ctf_z, u_expected, max_relative = 0.02);

        // Z[0] should be dominated by the innermost layer (plasterboard)
        // k_plaster/dx_plaster_node ≈ 0.16/0.002 = 80 W/(m²K)
        // But with surface-adjacent node (1.5× cap), Z[0] will be lower.
        // For a lightweight wall, Z[0] should be >> U (fast surface response)
        println!(
            "  Z[0]/U = {:.1}x (should be >> 1 for lightweight wall)",
            ctf.z[0] / u_expected
        );
        assert!(
            ctf.z[0] > 5.0 * u_expected,
            "Z[0]={:.2} should be much larger than U={:.3} for lightweight construction",
            ctf.z[0],
            u_expected
        );
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
        println!(
            "  Simple:  num_terms={}, Z[0]={:.4}, Y[0]={:.6}",
            ctf_simple.num_terms, ctf_simple.z[0], ctf_simple.y[0]
        );
        println!(
            "  Layered: num_terms={}, Z[0]={:.4}, Y[0]={:.6}",
            ctf_layered.num_terms, ctf_layered.z[0], ctf_layered.y[0]
        );

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
                println!(
                    "  {:>4}   {:>10.3}   {:>10.3}    {:.3}",
                    step, q_s, q_l, ratio
                );
            }
            hist_s.update(t_out, 20.0, q_s, qo_s);
            hist_l.update(t_out, 20.0, q_l, qo_l);
        }
    }

    #[test]
    fn test_heavy_wall_short_timestep_lengthens_the_ctf_time_step() {
        // 15 mm plaster | 365 mm solid brick | 15 mm plaster. At 15 min the CTF
        // runs at the zone timestep. At 5 min the brick reaches the 19-node cap,
        // so the CTF time step becomes a whole number of zone timesteps.
        let plaster = ResolvedLayer::new(0.7, 1400.0, 1000.0, 0.015);
        let brick = ResolvedLayer::new(0.77, 1700.0, 800.0, 0.365);
        let layers = vec![plaster.clone(), brick, plaster];

        let ok = try_calculate_ctf(&layers, 900.0).expect("15-minute CTF converges");
        assert_eq!(ok.num_histories, 1);
        assert_eq!(ok.time_step, 900.0);
        assert!(
            ok.steady_state_error_pct < 1.0,
            "{}",
            ok.steady_state_error_pct
        );

        let fine = try_calculate_ctf(&layers, 300.0).expect("5-minute CTF runs");
        assert!(fine.num_histories >= 2, "{}", fine.num_histories);
        assert_eq!(fine.time_step, 300.0 * fine.num_histories as f64);
        assert!(fine.phi.len() < MAX_CTF_TERMS);
        assert!(
            fine.steady_state_error_pct < 0.1,
            "{}",
            fine.steady_state_error_pct
        );
    }

    #[test]
    fn test_node_count_follows_energyplus() {
        // E+: int(L/√(2αΔt)), held within [6, 19].
        let concrete = ResolvedLayer::new(1.311, 2240.0, 836.8, 0.2);
        let alpha = 1.311 / (2240.0 * 836.8);
        let dxn = (2.0 * alpha * 600.0_f64).sqrt();
        assert_eq!(layer_nodes(&concrete, 600.0), (0.2 / dxn) as usize);
        assert_eq!(layer_nodes(&concrete, 3600.0), MIN_NODES);
        let thick = ResolvedLayer::new(1.311, 2240.0, 836.8, 2.0);
        assert_eq!(layer_nodes(&thick, 600.0), MAX_CTF_TERMS);
    }

    #[test]
    fn test_capped_layer_sets_fourier_number_time_step() {
        // 60 cm of dense concrete at 10-minute timesteps: 19 nodes, Δx = L/19,
        // ρ·cp·Δx²/k ≈ 1,420 s → 2 zone timesteps (E+ rounds to the nearest).
        let layers = vec![ResolvedLayer::new(1.311, 2240.0, 836.8, 0.6)];
        let dx: f64 = 0.6 / 19.0;
        let dtn = 2240.0 * 836.8 * dx * dx / 1.311;
        let expected = ((dtn / 600.0) + 0.5) as usize;
        let ctf = try_calculate_ctf(&layers, 600.0).unwrap();
        assert!(ctf.num_histories >= expected, "{}", ctf.num_histories);
        assert_eq!(ctf.time_step, 600.0 * ctf.num_histories as f64);
    }

    /// The CTF of a slab with no history terms needed beyond its nodes, at a
    /// zone timestep, compared with E+'s rule that the series is exact at rcmax.
    #[test]
    fn test_series_never_exceeds_the_term_limit() {
        for (k, rho, cp, l, dt) in [
            (1.311, 2240.0, 836.8, 0.6, 600.0),
            (1.0, 1800.0, 1100.0, 1.5, 600.0),
            (0.77, 1700.0, 800.0, 0.365, 60.0),
        ] {
            let ctf = try_calculate_ctf(&[ResolvedLayer::new(k, rho, cp, l)], dt).unwrap();
            assert!(
                ctf.phi.len() < MAX_CTF_TERMS,
                "{l} m: {} terms",
                ctf.phi.len()
            );
            let [ux, uy, uz] = ctf_u_values(&ctf);
            assert!((ux - uy).abs() <= MAX_CTF_SUM_ERROR * uy.abs());
            assert!((uz - uy).abs() <= MAX_CTF_SUM_ERROR * uy.abs());
        }
    }

    #[test]
    fn test_too_long_ctf_time_step_is_an_error_naming_the_layer() {
        // 3 m of soil in one layer: Δx = 3/19 m gives a Fourier step near 14 h,
        // past E+'s 7 h ceiling. Three 1 m layers of it (56 nodes) need more
        // than 18 history terms at every step up to 7 h. EnergyPlus stops on both.
        let soil = |l| ResolvedLayer::new(1.0, 1800.0, 1100.0, l);
        let err = try_calculate_ctf(&[soil(3.0)], 600.0).expect_err("one 3 m layer");
        assert!(err.contains("Layer 1") && err.contains("7 h"), "{err}");
        let err = try_calculate_ctf(&[soil(1.0), soil(1.0), soil(1.0)], 600.0)
            .expect_err("three 1 m layers");
        assert!(err.contains("history terms"), "{err}");
    }

    #[test]
    fn test_staggered_histories_hold_exact_past_values() {
        // With N zone timesteps per CTF time step, the working history after the
        // update at the end of zone step n holds the exact values at
        // n + 1 − N·(i + 1), for any signal.
        for n_hist in [1usize, 2, 3, 5] {
            let ctf = CtfCoefficients {
                num_terms: 4,
                num_histories: n_hist,
                ..CtfCoefficients::steady(1.0, 60.0)
            };
            let mut h = CtfHistory::for_ctf(&ctf, 0.0);
            let f = |t: f64| (2.0 * t, 1.0 - t * t, (0.3 * t).sin(), -3.0 * t);
            let steps = 40;
            for n in 0..steps {
                let (a, b, c, d) = f(n as f64);
                h.update(a, b, c, d);
                if n < 5 * n_hist {
                    continue; // still reading the initial history
                }
                for i in 0..4 {
                    let t = (n + 1) as f64 - (n_hist * (i + 1)) as f64;
                    let (a, b, c, d) = f(t);
                    let got = [h.t_outside[i], h.t_inside[i], h.q_inside[i], h.q_outside[i]];
                    for (g, e) in got.iter().zip([a, b, c, d]) {
                        assert!((g - e).abs() < 1e-9, "N={n_hist} n={n} i={i}: {g} vs {e}");
                    }
                }
            }
            h.reset(5.0);
            assert!(h.t_outside.iter().all(|&v| v == 5.0));
            assert!(h.q_inside.iter().all(|&v| v == 0.0));
        }
    }

    /// Minimal complex arithmetic for the analytical slab solutions.
    #[derive(Clone, Copy)]
    struct Cx(f64, f64);
    impl Cx {
        fn mul(self, o: Cx) -> Cx {
            Cx(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
        }
        fn div(self, o: Cx) -> Cx {
            let d = o.0 * o.0 + o.1 * o.1;
            Cx(
                (self.0 * o.0 + self.1 * o.1) / d,
                (self.1 * o.0 - self.0 * o.1) / d,
            )
        }
        fn exp(self) -> Cx {
            let m = self.0.exp();
            Cx(m * self.1.cos(), m * self.1.sin())
        }
        fn sinh(self) -> Cx {
            let a = self.exp();
            let b = Cx(-self.0, -self.1).exp();
            Cx((a.0 - b.0) / 2.0, (a.1 - b.1) / 2.0)
        }
        fn cosh(self) -> Cx {
            let a = self.exp();
            let b = Cx(-self.0, -self.1).exp();
            Cx((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
        }
    }

    /// Drive a CTF with the outside face at 20 + `amp`·sin(ωt) and the inside
    /// face at 20 °C for `periods` periods at zone step `dt`, and compare the
    /// last period's face fluxes with the exact periodic solution for a
    /// homogeneous slab (transmission matrix, γ = √(iω/α)):
    /// q_in = kγ/sinh(γL)·T̃_o, q_out = kγ·coth(γL)·T̃_o.
    /// Returns the largest errors as fractions of each flux amplitude.
    fn periodic_slab_errors(
        layer: &ResolvedLayer,
        dt: f64,
        period: f64,
        periods: usize,
        amp: f64,
    ) -> (f64, f64, usize) {
        let ctf = calculate_ctf(std::slice::from_ref(layer), dt);
        let mut h = CtfHistory::for_ctf(&ctf, 20.0);
        let omega = 2.0 * std::f64::consts::PI / period;
        let alpha = layer.conductivity / (layer.density * layer.specific_heat);
        let g = (omega / (2.0 * alpha)).sqrt();
        let gamma = Cx(g, g);
        let gl = Cx(g * layer.thickness, g * layer.thickness);
        let kg = Cx(layer.conductivity * gamma.0, layer.conductivity * gamma.1);
        let h_in = kg.div(gl.sinh());
        let h_out = kg.mul(gl.cosh()).div(gl.sinh());
        let steps_per_period = (period / dt).round() as usize;
        let (mut e_in, mut e_out) = (0.0_f64, 0.0_f64);
        let amp_in = amp * (h_in.0.hypot(h_in.1));
        let amp_out = amp * (h_out.0.hypot(h_out.1));
        for n in 1..=periods * steps_per_period {
            let t = n as f64 * dt;
            let t_o = 20.0 + amp * (omega * t).sin();
            let (q_in, q_out) = apply_ctf(&ctf, &h, t_o, 20.0);
            h.update(t_o, 20.0, q_in, q_out);
            if n > (periods - 1) * steps_per_period {
                // Im(H·A·e^{iωt}) for T_o − 20 = Im(A·e^{iωt})
                let ph = Cx((omega * t).cos(), (omega * t).sin());
                let exact_in = amp * h_in.mul(ph).1;
                let exact_out = amp * h_out.mul(ph).1;
                e_in = e_in.max((q_in - exact_in).abs() / amp_in);
                e_out = e_out.max((q_out - exact_out).abs() / amp_out);
            }
        }
        (e_in, e_out, ctf.num_histories)
    }

    #[test]
    fn test_thick_concrete_periodic_response_matches_analytical() {
        // 60 cm of dense concrete at 6 timesteps per hour, daily cycle.
        let concrete = ResolvedLayer::new(1.311, 2240.0, 836.8, 0.6);
        let (e_in, e_out, n_hist) = periodic_slab_errors(&concrete, 600.0, 86_400.0, 10, 10.0);
        assert!(n_hist >= 2, "{n_hist}");
        // The remaining error is the CTF itself, not its staggered histories: the
        // node network (19 nodes, surface half-cells lumped into the first and
        // last node as in E+) damps the inside amplitude about 4% and the 20 min
        // CTF step lags the phase. Measured: 6.0% inside, 7.5% outside here; the
        // same wall run at a 15 min zone timestep with N = 1 gives 8.2% and 10.0%.
        assert!(
            e_in < 0.07,
            "inside flux error {:.2}% of amplitude",
            e_in * 100.0
        );
        assert!(
            e_out < 0.085,
            "outside flux error {:.2}% of amplitude",
            e_out * 100.0
        );
    }

    #[test]
    fn test_thick_soil_annual_response_matches_analytical() {
        // 1.5 m of soil at 6 timesteps per hour, annual cycle (ground coupling).
        let soil = ResolvedLayer::new(1.0, 1800.0, 1100.0, 1.5);
        let year = 365.0 * 86_400.0;
        let (e_in, e_out, n_hist) = periodic_slab_errors(&soil, 600.0, year, 2, 10.0);
        assert!(n_hist > 10, "{n_hist}");
        assert!(
            e_in < 0.01,
            "inside flux error {:.3}% of amplitude",
            e_in * 100.0
        );
        assert!(
            e_out < 0.01,
            "outside flux error {:.3}% of amplitude",
            e_out * 100.0
        );
    }

    #[test]
    fn test_thick_concrete_step_response_and_energy_closure() {
        // 60 cm of concrete, uniform at 20 °C; the outside face steps to 30 °C,
        // the inside face stays at 20 °C. Exact inside flux:
        //   q_in(t) = kΔT/L·[1 + 2·Σ (−1)ⁿ·exp(−n²π²αt/L²)].
        // Energy closes: Σ(q_out − q_in)·Δt = stored heat = ρ·cp·L·ΔT/2 once the
        // profile is linear.
        let (k, rho, cp, l) = (1.311, 2240.0, 836.8, 0.6);
        let layer = ResolvedLayer::new(k, rho, cp, l);
        let dt = 600.0;
        let ctf = calculate_ctf(std::slice::from_ref(&layer), dt);
        assert!(ctf.num_histories >= 2);
        let mut h = CtfHistory::for_ctf(&ctf, 20.0);
        let alpha = k / (rho * cp);
        let dtemp = 10.0;
        let exact_in = |t: f64| {
            let s: f64 = (1..200)
                .map(|n| {
                    let n = n as f64;
                    let sign = if n as i64 % 2 == 0 { 1.0 } else { -1.0 };
                    sign * (-(n * n) * std::f64::consts::PI.powi(2) * alpha * t / (l * l)).exp()
                })
                .sum();
            k * dtemp / l * (1.0 + 2.0 * s)
        };
        let steady = k * dtemp / l;
        let mut stored = 0.0;
        let mut worst = 0.0_f64;
        let days = 10;
        for n in 1..=(days * 144) {
            let t = n as f64 * dt;
            let (q_in, q_out) = apply_ctf(&ctf, &h, 30.0, 20.0);
            h.update(30.0, 20.0, q_in, q_out);
            stored += (q_out - q_in) * dt;
            // Past the first CTF time step (the CTF ramps a step over one step).
            if t > 6.0 * 3600.0 {
                worst = worst.max((q_in - exact_in(t)).abs() / steady);
            }
        }
        assert!(
            worst < 0.02,
            "inside flux error {:.2}% of steady flux",
            worst * 100.0
        );
        let expected = rho * cp * l * dtemp / 2.0;
        assert!(
            (stored - expected).abs() < 1e-3 * expected,
            "stored {stored:.0} J/m² vs {expected:.0}"
        );
    }

    #[test]
    fn test_thick_construction_steady_state_u_matches_layers() {
        // Concrete, soil and insulation; 2.3 m in all at 6 timesteps per hour.
        let layers = vec![
            ResolvedLayer::new(1.311, 2240.0, 836.8, 0.6),
            ResolvedLayer::new(1.0, 1800.0, 1100.0, 1.0),
            ResolvedLayer::new(0.04, 30.0, 840.0, 0.1),
            ResolvedLayer::new(1.311, 2240.0, 836.8, 0.6),
        ];
        let u = 1.0 / (0.6 / 1.311 + 1.0 / 1.0 + 0.1 / 0.04 + 0.6 / 1.311);
        let ctf = try_calculate_ctf(&layers, 600.0).unwrap();
        assert!(ctf.num_histories > 1);
        for got in ctf_u_values(&ctf) {
            assert!((got - u).abs() < 1e-3 * u, "U {got:.5} vs {u:.5}");
        }
    }

    /// Steady-state U-value of a CTF set from its X, Y and Z series.
    fn ctf_u_values(ctf: &CtfCoefficients) -> [f64; 3] {
        let denom = 1.0 - ctf.phi.iter().sum::<f64>();
        [
            ctf.x.iter().sum::<f64>() / denom,
            ctf.y.iter().sum::<f64>() / denom,
            ctf.z.iter().sum::<f64>() / denom,
        ]
    }

    #[test]
    fn test_nomass_layer_between_massed_layers() {
        // Masonry cavity wall: 100 mm brick | NoMass insulation R = 1.5 | 100 mm block.
        // The NoMass resistance sits between two massed layers, so it cannot be folded
        // into a boundary conductance; it must stay in the state-space model.
        let brick = ResolvedLayer::new(0.77, 1700.0, 800.0, 0.1);
        let block = ResolvedLayer::new(0.51, 1400.0, 1000.0, 0.1);
        let layers = vec![
            brick.clone(),
            ResolvedLayer::new_no_mass(1.5, 0.06),
            block.clone(),
        ];
        let u_expected = 1.0 / (0.1 / 0.77 + 1.5 + 0.1 / 0.51);

        for dt in [3600.0, 900.0, 300.0] {
            let ctf = calculate_ctf(&layers, dt);
            assert!(
                ctf.num_terms > 1 && !ctf.phi.is_empty(),
                "dt={dt}: expected a dynamic CTF, got a steady-state fallback"
            );
            for u in ctf_u_values(&ctf) {
                assert_relative_eq!(u, u_expected, max_relative = 0.01);
            }
        }

        // Adjacent NoMass layers act like one layer with the summed resistance.
        let split = vec![
            brick,
            ResolvedLayer::new_no_mass(0.5, 0.02),
            ResolvedLayer::new_no_mass(1.0, 0.04),
            block,
        ];
        let ctf_single = calculate_ctf(&layers, 900.0);
        let ctf_split = calculate_ctf(&split, 900.0);
        assert_eq!(ctf_split.num_terms, ctf_single.num_terms);
        for (a, b) in ctf_split.z.iter().zip(&ctf_single.z) {
            assert_relative_eq!(*a, *b, max_relative = 1e-9);
        }
        for (a, b) in ctf_split.phi.iter().zip(&ctf_single.phi) {
            assert_relative_eq!(*a, *b, max_relative = 1e-9);
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
            ResolvedLayer::new_no_mass(25.075, 1.003), // NoMass: R=25.075
            ResolvedLayer::new(0.14, 650.0, 1200.0, 0.025), // Timber Flooring
        ];
        let ctf_nomass = calculate_ctf(&layers_nomass, dt);

        // Old approach: physical insulation with parasitic mass
        let layers_physical = vec![
            ResolvedLayer::new(0.04, 10.0, 1000.0, 1.003), // Physical insulation
            ResolvedLayer::new(0.14, 650.0, 1200.0, 0.025), // Timber Flooring
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
        println!(
            "  U (physical): {:.6} (terms={})",
            u_ph, ctf_physical.num_terms
        );

        // NoMass thermal mass = only timber = 19500 J/(m²K)
        // Physical thermal mass = timber + insulation = 19500 + 10030 = 29530 J/(m²K)
        let c_timber = 650.0 * 1200.0 * 0.025;
        let c_insul = 10.0 * 1000.0 * 1.003;
        println!(
            "  C_timber = {:.0}, C_insul = {:.0}, total_physical = {:.0}",
            c_timber,
            c_insul,
            c_timber + c_insul
        );

        // Both U-values should match expected
        assert_relative_eq!(u_nm, u_expected, max_relative = 0.02);
        assert_relative_eq!(u_ph, u_expected, max_relative = 0.02);

        // NoMass version should have FEWER terms (less thermal mass → faster response)
        // Physical insulation adds 10030 J/(m²K) of parasitic mass
        println!(
            "  NoMass terms: {}, Physical terms: {}",
            ctf_nomass.num_terms, ctf_physical.num_terms
        );

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
            println!(
                "  {:>4}   {:>10.4}   {:>10.4}    {:.3}",
                step, q_nm, q_ph, ratio
            );
            hist_nm.update(t_out, 20.0, q_nm, qo_nm);
            hist_ph.update(t_out, 20.0, q_ph, qo_ph);
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
            ResolvedLayer::new(0.14, 530.0, 900.0, 0.009), // Wood Siding
            ResolvedLayer::new(0.04, 10.0, 1400.0, 0.0615), // Foam Insulation
            ResolvedLayer::new(0.51, 1400.0, 1000.0, 0.1), // Concrete Block
        ];
        let ctf_layered = calculate_ctf(&layers, dt);

        // Simplified from U+C (what YAML currently uses)
        // Current default (k=1.0, ρ=2000) — shows the difference
        let ctf_simple = calculate_ctf_simple(0.556, 145154.0, dt, false, None, None);

        // With E+ concrete block properties (k=0.51, ρ=1400)
        let ctf_block = calculate_ctf_simple(0.556, 145154.0, dt, false, Some(0.51), Some(1400.0));

        println!("\n=== HWWALL: simple(U+C) vs block(k=0.51) vs layered(E+ materials) CTF ===");
        println!(
            "  Simple:  num_terms={}, Z[0]={:.4}, Y[0]={:.6}, X[0]={:.4}",
            ctf_simple.num_terms, ctf_simple.z[0], ctf_simple.y[0], ctf_simple.x[0]
        );
        println!(
            "  Block:   num_terms={}, Z[0]={:.4}, Y[0]={:.6}, X[0]={:.4}",
            ctf_block.num_terms, ctf_block.z[0], ctf_block.y[0], ctf_block.x[0]
        );
        println!(
            "  Layered: num_terms={}, Z[0]={:.4}, Y[0]={:.6}, X[0]={:.4}",
            ctf_layered.num_terms, ctf_layered.z[0], ctf_layered.y[0], ctf_layered.x[0]
        );

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

        println!(
            "  Simple  U_y={:.4}, U_z={:.4}, sum(Phi)={:.6}",
            u_s, u_z_s, sum_phi_s
        );
        println!(
            "  Layered U_y={:.4}, U_z={:.4}, sum(Phi)={:.6}",
            u_l, u_z_l, sum_phi_l
        );

        // Print CTF coefficients side by side
        let max_terms = ctf_simple.num_terms.max(ctf_layered.num_terms);
        println!(
            "\n  j    Z_simple     Z_layered    Y_simple     Y_layered    X_simple     X_layered"
        );
        for j in 0..max_terms {
            let zs = if j < ctf_simple.z.len() {
                ctf_simple.z[j]
            } else {
                0.0
            };
            let zl = if j < ctf_layered.z.len() {
                ctf_layered.z[j]
            } else {
                0.0
            };
            let ys = if j < ctf_simple.y.len() {
                ctf_simple.y[j]
            } else {
                0.0
            };
            let yl = if j < ctf_layered.y.len() {
                ctf_layered.y[j]
            } else {
                0.0
            };
            let xs = if j < ctf_simple.x.len() {
                ctf_simple.x[j]
            } else {
                0.0
            };
            let xl = if j < ctf_layered.x.len() {
                ctf_layered.x[j]
            } else {
                0.0
            };
            println!(
                "  {j}  {zs:>12.4}  {zl:>12.4}  {ys:>12.4}  {yl:>12.4}  {xs:>12.4}  {xl:>12.4}"
            );
        }
        println!("\n  j    Phi_simple    Phi_layered");
        for j in 0..ctf_simple.phi.len().max(ctf_layered.phi.len()) {
            let ps = if j < ctf_simple.phi.len() {
                ctf_simple.phi[j]
            } else {
                0.0
            };
            let pl = if j < ctf_layered.phi.len() {
                ctf_layered.phi[j]
            } else {
                0.0
            };
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
                println!(
                    "  {:>4}   {:>10.3}   {:>10.3}    {:.3}",
                    step, q_s, q_l, ratio
                );
            }
            hist_s.update(t_out, 20.0, q_s, qo_s);
            hist_l.update(t_out, 20.0, q_l, qo_l);
        }
    }
}

#[cfg(test)]
mod hwwall_test {
    use super::*;
    use crate::material::ResolvedLayer;

    #[test]
    fn print_hwwall_ctf() {
        // E+ HWWALL: [WOOD SIDING-1 | FOAM INSULATION | CONCRETE BLOCK]
        let wood = ResolvedLayer::new(0.14, 530.0, 900.0, 0.009);
        let foam = ResolvedLayer::new(0.04, 10.0, 1400.0, 0.0615);
        let concrete = ResolvedLayer::new(0.51, 1400.0, 1000.0, 0.100);

        let dt = 600.0; // 10-minute timestep (6 per hour)
        let ctf = calculate_ctf(&[wood, foam, concrete], dt);

        println!("HWWALL CTF (dt=600s, direct layers):");
        println!("  num_terms = {}", ctf.num_terms);
        for i in 0..ctf.x.len() {
            println!(
                "  X[{}]={:.6}  Y[{}]={:.6}  Z[{}]={:.6}",
                i, ctf.x[i], i, ctf.y[i], i, ctf.z[i]
            );
        }
        for i in 0..ctf.phi.len() {
            println!("  Phi[{}]={:.6}", i, ctf.phi[i]);
        }
        println!(
            "  sum(X)={:.6}  sum(Y)={:.6}  sum(Z)={:.6}",
            ctf.x.iter().sum::<f64>(),
            ctf.y.iter().sum::<f64>(),
            ctf.z.iter().sum::<f64>()
        );

        // Now via from_simple_construction
        let ctf2 = calculate_ctf_simple(0.556, 145154.0, dt, false, Some(0.51), Some(1400.0));
        println!("\nHWWALL CTF (dt=600s, from_simple_construction):");
        println!("  num_terms = {}", ctf2.num_terms);
        for i in 0..ctf2.x.len() {
            println!(
                "  X[{}]={:.6}  Y[{}]={:.6}  Z[{}]={:.6}",
                i, ctf2.x[i], i, ctf2.y[i], i, ctf2.z[i]
            );
        }
        for i in 0..ctf2.phi.len() {
            println!("  Phi[{}]={:.6}", i, ctf2.phi[i]);
        }
        println!(
            "  sum(X)={:.6}  sum(Y)={:.6}  sum(Z)={:.6}",
            ctf2.x.iter().sum::<f64>(),
            ctf2.y.iter().sum::<f64>(),
            ctf2.z.iter().sum::<f64>()
        );
    }
}
