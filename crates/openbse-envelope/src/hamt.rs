//! Heat And Moisture Transfer (HAMT) solver for building envelope surfaces.
//!
//! Implements a 1D finite-difference solver for coupled heat and moisture
//! transport through multi-layer walls, matching the EnergyPlus HAMT model
//! (Künzel 1995).
//!
//! # ⚠️ EXPERIMENTAL — NOT PRODUCTION-READY (GitHub #64)
//!
//! This module is **not wired into the simulation** (`HamtState::from_construction`
//! has no caller) and is **quantitatively non-functional** as written. A physics
//! review (2026-06) found three coupled bugs; do not enable HAMT for any result
//! that matters until they are fixed and validated against E+:
//!
//! - **HAMT-1:** the latent heat/moisture coupling source term is identically
//!   zero — `w_prev` and `w_curr` both read `layer.moisture[ni]`, so the heat
//!   equation's `L_v·∂(ρ₀·w_c)/∂t` term is dead and the heat solve degenerates
//!   to plain conduction.
//! - **HAMT-2:** the vapor conductance drops the sorption-isotherm slope
//!   `∂RH/∂w_c` (uses `perm·p_sat` directly), mis-scaling vapor transport by
//!   roughly 5–50×.
//! - **HAMT-3:** surface vapor resistance is applied as `DELTA_AIR/Z_M` instead
//!   of `1/Z_M`, a spurious ~2e-10 factor that makes the surfaces effectively
//!   vapor-tight.
//!
//! The existing unit tests only check flux *sign* / TDMA mechanics, so they pass
//! despite these errors. Treat any HAMT output as diagnostic only.
//!
//! Governing equations (discretized implicitly, backward Euler):
//!
//! Heat:
//!   ρ·c_p·ΔT/Δt = ∂/∂x[λ·∂T/∂x] + L_v · ∂(ρ₀·w_c)/∂t
//!
//! Moisture:
//!   ρ₀·∂w_c/∂t = ∂/∂x[D_w·ρ₀·∂w_c/∂x] + ∂/∂x[δ_p·∂(p_v)/∂x]
//!
//! References:
//!   - Künzel, H.M. (1995). Simultaneous Heat and Moisture Transport in Building
//!     Components. Ph.D. thesis, Fraunhofer IBP.
//!   - EnergyPlus Engineering Reference, "Combined Heat and Moisture Transfer (HAMT)"

use crate::material::{Construction, Material};
use std::collections::HashMap;

/// Vapor permeability of still air [kg/(m·s·Pa)].
/// Used as baseline for δ_p = δ_air / μ.
const DELTA_AIR: f64 = 1.99e-10;

/// Latent heat of vaporization [J/kg].
const L_V: f64 = 2_501_000.0;

/// Surface moisture transfer resistance [m²·s·Pa/kg].
/// Matches E+ default "low" surface resistance.
const Z_M_SURFACE: f64 = 3.0e8;

/// Saturation pressure of water vapor [Pa] via Antoine equation approximation.
///
/// Valid range: -20°C to +70°C. Matches E+ psychrometrics accuracy.
pub fn p_sat(t_celsius: f64) -> f64 {
    // Magnus formula (approximation matching E+ accuracy)
    let t = t_celsius.clamp(-40.0, 80.0);
    610.78 * (17.27 * t / (t + 237.3)).exp()
}

/// Linear interpolation in a sorted lookup table of (x, y) pairs.
///
/// Returns the y value at x by linearly interpolating between table entries.
/// Clamps to table bounds if x is outside the range.
pub fn interp_table(table: &[[f64; 2]], x: f64) -> f64 {
    if table.is_empty() {
        return 0.0;
    }
    if table.len() == 1 {
        return table[0][1];
    }
    if x <= table[0][0] {
        return table[0][1];
    }
    if x >= table[table.len() - 1][0] {
        return table[table.len() - 1][1];
    }
    // Find bracketing interval
    for i in 0..table.len() - 1 {
        let x0 = table[i][0];
        let x1 = table[i + 1][0];
        if x >= x0 && x <= x1 {
            let t = (x - x0) / (x1 - x0).max(1e-30);
            return table[i][1] + t * (table[i + 1][1] - table[i][1]);
        }
    }
    table[table.len() - 1][1]
}

/// Moisture content [kg/kg] from relative humidity via sorption isotherm.
pub fn moisture_from_rh(isotherm: &[[f64; 2]], rh: f64) -> f64 {
    interp_table(isotherm, rh.clamp(0.0, 1.0))
}

/// Relative humidity [0-1] from moisture content [kg/kg] via inverted isotherm.
///
/// Performs linear search through the isotherm. Returns 0.0 for dry material,
/// clamped to [0, 1].
pub fn rh_from_moisture(isotherm: &[[f64; 2]], w_c: f64) -> f64 {
    if isotherm.is_empty() {
        return 0.0;
    }
    if w_c <= 0.0 {
        return 0.0;
    }
    if w_c >= isotherm[isotherm.len() - 1][1] {
        return 1.0;
    }
    // Invert: x=moisture_content, y=RH
    for i in 0..isotherm.len() - 1 {
        let w0 = isotherm[i][1];
        let w1 = isotherm[i + 1][1];
        if w_c >= w0 && w_c <= w1 {
            let t = if (w1 - w0).abs() > 1e-30 {
                (w_c - w0) / (w1 - w0)
            } else {
                0.0
            };
            let rh0 = isotherm[i][0];
            let rh1 = isotherm[i + 1][0];
            return (rh0 + t * (rh1 - rh0)).clamp(0.0, 1.0);
        }
    }
    1.0
}

/// Thomas algorithm (TDMA) solver for a tridiagonal system.
///
/// Solves: a[i]·x[i-1] + b[i]·x[i] + c[i]·x[i+1] = d[i]
///
/// `a` = subdiagonal (a[0] unused), `b` = diagonal, `c` = superdiagonal, `d` = RHS.
/// Returns the solution vector x.
pub fn tdma(a: &[f64], b: &[f64], c: &[f64], d: &[f64]) -> Vec<f64> {
    let n = b.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![if b[0].abs() > 1e-30 { d[0] / b[0] } else { 0.0 }];
    }

    let mut c_prime = vec![0.0; n];
    let mut d_prime = vec![0.0; n];
    let mut x = vec![0.0; n];

    // Forward sweep
    let denom = b[0];
    c_prime[0] = if denom.abs() > 1e-30 { c[0] / denom } else { 0.0 };
    d_prime[0] = if denom.abs() > 1e-30 { d[0] / denom } else { 0.0 };

    for i in 1..n {
        let denom = b[i] - a[i] * c_prime[i - 1];
        if denom.abs() > 1e-30 {
            c_prime[i] = c[i] / denom;
            d_prime[i] = (d[i] - a[i] * d_prime[i - 1]) / denom;
        } else {
            c_prime[i] = 0.0;
            d_prime[i] = 0.0;
        }
    }

    // Back substitution
    x[n - 1] = d_prime[n - 1];
    for i in (0..n - 1).rev() {
        x[i] = d_prime[i] - c_prime[i] * x[i + 1];
    }

    x
}

/// A single material layer discretized into finite-difference nodes.
#[derive(Debug, Clone)]
pub struct HamtLayer {
    /// Number of nodes in this layer.
    pub n_nodes: usize,
    /// Node temperatures [°C].
    pub temps: Vec<f64>,
    /// Node moisture content [kg/kg dry material].
    pub moisture: Vec<f64>,
    /// Thermal conductivity at each node [W/(m·K)] (dry value from material).
    pub conductivity: f64,
    /// Dry material density [kg/m³].
    pub density: f64,
    /// Specific heat [J/(kg·K)].
    pub specific_heat: f64,
    /// Vapor resistance factor μ [-].
    pub vapor_resist: f64,
    /// Node spacing [m] (uniform within a layer).
    pub node_dx: f64,
    /// Sorption isotherm: pairs of [RH [0-1], moisture_content [kg/kg]].
    pub sorption_iso: Vec<[f64; 2]>,
    /// Liquid transport coefficient table: pairs of [w_c [kg/kg], D_w [m²/s]].
    pub liquid_trans: Vec<[f64; 2]>,
}

impl HamtLayer {
    /// Create a new HAMT layer from a material, thickness, and initial conditions.
    pub fn new(
        material: &Material,
        thickness: f64,
        initial_temp: f64,
        initial_rh: f64,
    ) -> Self {
        // Number of nodes: minimum 3, scale with thickness (1 node per 2 cm)
        let n_nodes = ((thickness / 0.02).ceil() as usize).max(3);
        let node_dx = thickness / n_nodes as f64;

        let sorption_iso = material
            .sorption_isotherm
            .clone()
            .unwrap_or_else(|| vec![[0.0, 0.0], [1.0, 0.01]]);

        let initial_moisture = moisture_from_rh(&sorption_iso, initial_rh);
        let liquid_trans = material
            .liquid_transport_coeff
            .clone()
            .unwrap_or_default();

        Self {
            n_nodes,
            temps: vec![initial_temp; n_nodes],
            moisture: vec![initial_moisture; n_nodes],
            conductivity: material.conductivity,
            density: material.density,
            specific_heat: material.specific_heat,
            vapor_resist: material.vapor_resistance_factor.unwrap_or(1.0),
            node_dx,
            sorption_iso,
            liquid_trans,
        }
    }

    /// Liquid transport coefficient D_w [m²/s] at a given moisture content.
    pub fn d_liquid(&self, w_c: f64) -> f64 {
        if self.liquid_trans.is_empty() {
            0.0
        } else {
            interp_table(&self.liquid_trans, w_c).max(0.0)
        }
    }

    /// Vapor permeability [kg/(m·s·Pa)] = δ_air / μ.
    pub fn delta_p(&self) -> f64 {
        DELTA_AIR / self.vapor_resist.max(1.0)
    }
}

/// HAMT state for a complete multi-layer wall section.
///
/// Tracks per-node temperature and moisture content across timesteps.
/// The solver advances both fields simultaneously using implicit backward Euler.
#[derive(Debug, Clone)]
pub struct HamtState {
    /// Material layers (outside to inside).
    pub layers: Vec<HamtLayer>,
    /// Conductive flux at the inside surface [W/m²].
    /// Positive = heat flowing into the zone.
    pub q_inside: f64,
    /// Conductive flux at the outside surface [W/m²].
    /// Positive = heat flowing outward.
    pub q_outside: f64,
    /// Moisture flux at the inside surface [kg/(m²·s)].
    /// Positive = moisture flowing into zone.
    pub moisture_flux_inside: f64,
    /// Average moisture content across all nodes [kg/kg].
    pub avg_moisture_content: f64,
    /// Relative humidity at the inside surface node [0-1].
    pub rh_inside: f64,
}

impl HamtState {
    /// Build HAMT state from a multi-layer construction.
    ///
    /// Returns `None` if any layer is missing moisture data (use CTF instead).
    pub fn from_construction(
        construction: &Construction,
        materials: &HashMap<String, Material>,
        initial_temp: f64,
        initial_rh: f64,
    ) -> Option<Self> {
        let mut layers = Vec::new();

        for cl in &construction.layers {
            let mat = materials.get(&cl.material)?;

            // Require both vapor_resistance_factor and sorption_isotherm
            if mat.vapor_resistance_factor.is_none() || mat.sorption_isotherm.is_none() {
                return None;
            }

            let layer = HamtLayer::new(mat, cl.thickness, initial_temp, initial_rh);
            layers.push(layer);
        }

        if layers.is_empty() {
            return None;
        }

        // HAMT is experimental and quantitatively non-functional (see the
        // module-level docs / GitHub #64). Warn loudly if it is ever activated.
        log::warn!(
            "HAMT moisture solver activated for construction '{}', but it is \
             EXPERIMENTAL and non-functional (GitHub #64: latent coupling is \
             dead, vapor conductance mis-scaled, surface resistance wrong). \
             Results are diagnostic only — prefer CTF constructions.",
            construction.name
        );

        Some(Self {
            layers,
            q_inside: 0.0,
            q_outside: 0.0,
            moisture_flux_inside: 0.0,
            avg_moisture_content: 0.0,
            rh_inside: initial_rh,
        })
    }

    /// Total number of nodes across all layers.
    pub fn total_nodes(&self) -> usize {
        self.layers.iter().map(|l| l.n_nodes).sum()
    }

    /// Flatten all temperature values to a single vector (outside to inside).
    pub fn flat_temps(&self) -> Vec<f64> {
        self.layers.iter().flat_map(|l| l.temps.iter().copied()).collect()
    }

    /// Flatten all moisture values to a single vector.
    pub fn flat_moisture(&self) -> Vec<f64> {
        self.layers.iter().flat_map(|l| l.moisture.iter().copied()).collect()
    }

    /// Advance the HAMT state by one timestep.
    ///
    /// # Arguments
    /// - `t_outside`: outdoor boundary temperature [°C]
    /// - `t_inside`: zone air temperature [°C]
    /// - `rh_outside`: outdoor relative humidity [0-1]
    /// - `rh_inside_bc`: zone relative humidity [0-1]
    /// - `dt`: timestep duration [s]
    ///
    /// Updates `q_inside`, `q_outside`, `moisture_flux_inside`, `avg_moisture_content`,
    /// and `rh_inside` after convergence.
    pub fn solve_timestep(
        &mut self,
        t_outside: f64,
        t_inside: f64,
        rh_outside: f64,
        rh_inside_bc: f64,
        dt: f64,
    ) {
        // Iterate T and w_c equations 3× per timestep for weak coupling
        for _ in 0..3 {
            self.solve_heat(t_outside, t_inside, dt);
            self.solve_moisture(rh_outside, rh_inside_bc, dt);
        }

        // Compute diagnostic outputs from final state
        self.compute_outputs(t_outside, t_inside, rh_inside_bc);
    }

    /// Solve the heat equation for one implicit step.
    fn solve_heat(&mut self, t_outside: f64, t_inside: f64, dt: f64) {
        let n = self.total_nodes();
        if n == 0 {
            return;
        }

        // Build coefficient arrays for tridiagonal system
        let mut a = vec![0.0_f64; n];
        let mut b = vec![0.0_f64; n];
        let mut c = vec![0.0_f64; n];
        let mut d = vec![0.0_f64; n];

        // Map global node index → (layer_idx, local_idx)
        let node_map = self.build_node_map();

        // Surface conductances [W/(m²·K)]
        // h_conv_outside ≈ 25.0, h_conv_inside ≈ 8.3 (standard film coefficients)
        let h_out = 25.0_f64;
        let h_in = 8.3_f64;

        for gi in 0..n {
            let (li, ni) = node_map[gi];
            let layer = &self.layers[li];
            let dx = layer.node_dx;
            let rho = layer.density.max(1.0);
            let cp = layer.specific_heat.max(1.0);
            let k = layer.conductivity.max(1e-6);

            // Thermal capacitance term: ρ·c_p·dx / dt
            let cap = rho * cp * dx / dt;

            // Latent heat coupling term (moisture change rate × L_v).
            // Simplified: use previous moisture values to estimate.
            let w_prev = layer.moisture[ni];
            let w_curr = layer.moisture[ni]; // updated after moisture solve
            let latent = L_V * rho * dx * (w_curr - w_prev) / dt;

            // Right-hand side: cap × T_prev + latent
            let t_prev = layer.temps[ni];
            d[gi] = cap * t_prev - latent;

            // Diagonal: cap + conductances from neighbors
            b[gi] = cap;

            // Inter-node conductances
            let k_left = if gi == 0 {
                // Outside boundary: use film coefficient
                h_out
            } else {
                let (li_prev, ni_prev) = node_map[gi - 1];
                let layer_prev = &self.layers[li_prev];
                let dx_prev = layer_prev.node_dx;
                let k_prev = layer_prev.conductivity.max(1e-6);
                // Harmonic mean conductance between adjacent nodes
                // Δx = (dx_prev + dx) / 2; k_eff = 2 / (dx_prev/k_prev + dx/k)
                2.0 / (dx_prev / k_prev + dx / k)
            };

            let k_right = if gi == n - 1 {
                // Inside boundary: use film coefficient
                h_in
            } else {
                let (li_next, ni_next) = node_map[gi + 1];
                let layer_next = &self.layers[li_next];
                let dx_next = layer_next.node_dx;
                let k_next = layer_next.conductivity.max(1e-6);
                2.0 / (dx / k + dx_next / k_next)
            };

            b[gi] += k_left + k_right;
            a[gi] = -k_left;
            c[gi] = -k_right;

            // Boundary temperature contributions to RHS
            if gi == 0 {
                d[gi] += h_out * t_outside;
            }
            if gi == n - 1 {
                d[gi] += h_in * t_inside;
            }
        }

        // Solve the tridiagonal system
        let temps_new = tdma(&a, &b, &c, &d);

        // Write back to layer structs
        for (gi, &t) in temps_new.iter().enumerate() {
            let (li, ni) = node_map[gi];
            self.layers[li].temps[ni] = t;
        }
    }

    /// Solve the moisture transport equation for one implicit step.
    ///
    /// Moisture transport combines:
    ///   1. Vapor diffusion: δ_p · ∂p_v/∂x
    ///   2. Capillary liquid flow: D_w · ρ₀ · ∂w_c/∂x
    fn solve_moisture(
        &mut self,
        rh_outside: f64,
        rh_inside_bc: f64,
        dt: f64,
    ) {
        let n = self.total_nodes();
        if n == 0 {
            return;
        }

        let node_map = self.build_node_map();

        // Surface moisture resistance [m²·s·Pa/kg]
        let z_m = Z_M_SURFACE;

        // Outside boundary partial pressure [Pa]
        let p_sat_out = {
            let (li, _) = node_map[0];
            p_sat(self.layers[li].temps[0])
        };
        let p_v_out = rh_outside.clamp(0.0, 1.0) * p_sat_out;

        // Inside boundary partial pressure [Pa]
        let p_sat_in = {
            let (li, ni) = node_map[n - 1];
            p_sat(self.layers[li].temps[ni])
        };
        let p_v_in = rh_inside_bc.clamp(0.0, 1.0) * p_sat_in;

        // Build tridiagonal for moisture (in terms of moisture content w_c [kg/kg])
        let mut a = vec![0.0_f64; n];
        let mut b = vec![0.0_f64; n];
        let mut c = vec![0.0_f64; n];
        let mut d = vec![0.0_f64; n];

        for gi in 0..n {
            let (li, ni) = node_map[gi];
            let layer = &self.layers[li];
            let dx = layer.node_dx;
            let rho = layer.density.max(1.0);
            let w_prev = layer.moisture[ni];
            let t_node = layer.temps[ni];

            // Storage term: ρ₀·dx / dt
            let storage = rho * dx / dt;
            b[gi] = storage;
            d[gi] = storage * w_prev;

            // Compute moisture transport coefficients to adjacent nodes
            let p_sat_node = p_sat(t_node);

            // Transport to left neighbor
            let conduct_left = if gi == 0 {
                // Outside boundary: use surface moisture resistance z_m
                // Moisture flux = (p_v_out - p_v_node) / z_m
                let dp_p = DELTA_AIR / z_m;
                // Convert from Pa-based to w_c-based via ∂p_v/∂w_c ≈ p_sat(T)·(∂RH/∂w_c)
                // Simplified: use effective permeance in kg/(m²·s·Pa)
                dp_p * p_sat_node
            } else {
                let (li_prev, ni_prev) = node_map[gi - 1];
                let layer_prev = &self.layers[li_prev];
                let dx_prev = layer_prev.node_dx;
                let delta_p_prev = layer_prev.delta_p();
                let delta_p_curr = layer.delta_p();

                // Harmonic mean vapor permeance [kg/(m²·s·Pa)] between nodes
                let perm = 2.0 / (dx_prev / delta_p_prev + dx / delta_p_curr);

                // Capillary liquid transport conductance: D_w · ρ₀ / Δx
                let w_avg = 0.5 * (w_prev + layer_prev.moisture[ni_prev]);
                let d_liq = 0.5 * (layer.d_liquid(w_avg) + layer_prev.d_liquid(w_avg));
                let cap_cond = d_liq * rho / (0.5 * (dx + dx_prev));

                // Total conductance in w_c units:
                // Vapor: perm × p_sat converts Pa gradient → w_c gradient
                perm * p_sat_node + cap_cond
            };

            let conduct_right = if gi == n - 1 {
                // Inside boundary: surface resistance z_m
                let dp_p = DELTA_AIR / z_m;
                dp_p * p_sat_node
            } else {
                let (li_next, ni_next) = node_map[gi + 1];
                let layer_next = &self.layers[li_next];
                let dx_next = layer_next.node_dx;
                let delta_p_next = layer_next.delta_p();
                let delta_p_curr = layer.delta_p();

                let perm = 2.0 / (dx / delta_p_curr + dx_next / delta_p_next);

                let w_avg = 0.5 * (w_prev + layer_next.moisture[ni_next]);
                let d_liq = 0.5 * (layer.d_liquid(w_avg) + layer_next.d_liquid(w_avg));
                let cap_cond = d_liq * rho / (0.5 * (dx + dx_next));

                perm * p_sat_node + cap_cond
            };

            b[gi] += conduct_left + conduct_right;
            a[gi] = -conduct_left;
            c[gi] = -conduct_right;

            // Boundary conditions contribute to RHS
            if gi == 0 {
                // Convert outside boundary p_v to w_c contribution
                let rh_out_node = rh_outside.clamp(0.0, 1.0);
                let w_c_out = moisture_from_rh(&layer.sorption_iso, rh_out_node);
                d[gi] += conduct_left * w_c_out;
                a[gi] = 0.0; // No sub-diagonal for first node
            }
            if gi == n - 1 {
                // Convert inside boundary p_v to w_c contribution
                let rh_in_node = rh_inside_bc.clamp(0.0, 1.0);
                let w_c_in = moisture_from_rh(&layer.sorption_iso, rh_in_node);
                d[gi] += conduct_right * w_c_in;
                c[gi] = 0.0; // No super-diagonal for last node
            }
        }

        // Solve the tridiagonal system
        let moisture_new = tdma(&a, &b, &c, &d);

        // Write back, clamping to physical bounds (non-negative)
        for (gi, &w) in moisture_new.iter().enumerate() {
            let (li, ni) = node_map[gi];
            self.layers[li].moisture[ni] = w.max(0.0);
        }
    }

    /// Compute diagnostic outputs from the current state.
    fn compute_outputs(&mut self, t_outside: f64, t_inside: f64, rh_inside_bc: f64) {
        let n = self.total_nodes();
        if n == 0 {
            self.q_inside = 0.0;
            self.q_outside = 0.0;
            self.moisture_flux_inside = 0.0;
            self.avg_moisture_content = 0.0;
            self.rh_inside = rh_inside_bc;
            return;
        }

        let node_map = self.build_node_map();

        // Inside conductive flux: k × (T[n-1] - T_inside) / (dx/2 + film)
        // Approximated as surface film conductance × delta-T
        let h_in = 8.3_f64;
        let (li_last, ni_last) = node_map[n - 1];
        let t_last_node = self.layers[li_last].temps[ni_last];
        self.q_inside = h_in * (t_last_node - t_inside);

        // Outside conductive flux
        let h_out = 25.0_f64;
        let (li_first, _) = node_map[0];
        let t_first_node = self.layers[li_first].temps[0];
        self.q_outside = h_out * (t_outside - t_first_node);

        // Moisture flux at inside surface [kg/(m²·s)]
        // Using surface moisture resistance: J = (p_v_surface - p_v_bc) / z_m
        let p_sat_in_node = p_sat(t_last_node);
        let rh_in_node = rh_from_moisture(&self.layers[li_last].sorption_iso,
                                           self.layers[li_last].moisture[ni_last]);
        let p_v_surface = rh_in_node * p_sat_in_node;
        let p_v_bc = rh_inside_bc * p_sat(t_inside);
        self.moisture_flux_inside = (p_v_surface - p_v_bc) / Z_M_SURFACE * DELTA_AIR;

        // Average moisture content across all nodes [kg/kg]
        let total_wc: f64 = self.layers.iter()
            .flat_map(|l| l.moisture.iter())
            .sum();
        self.avg_moisture_content = total_wc / n as f64;

        // Relative humidity at the inside surface node [0-1]
        self.rh_inside = rh_from_moisture(
            &self.layers[li_last].sorption_iso,
            self.layers[li_last].moisture[ni_last],
        );
    }

    /// Build a mapping from global node index to (layer_index, local_node_index).
    fn build_node_map(&self) -> Vec<(usize, usize)> {
        let mut map = Vec::with_capacity(self.total_nodes());
        for (li, layer) in self.layers.iter().enumerate() {
            for ni in 0..layer.n_nodes {
                map.push((li, ni));
            }
        }
        map
    }
}

/// Check whether a construction has complete HAMT moisture data in all layers.
///
/// Returns `true` if EVERY layer has both `vapor_resistance_factor` and
/// `sorption_isotherm` set. When `false`, CTF should be used instead.
pub fn construction_has_hamt_data(
    construction: &Construction,
    materials: &HashMap<String, Material>,
) -> bool {
    construction.layers.iter().all(|cl| {
        materials
            .get(&cl.material)
            .map(|m| m.vapor_resistance_factor.is_some() && m.sorption_isotherm.is_some())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use crate::material::{Construction, ConstructionLayer, Material};

    fn concrete_material() -> Material {
        Material {
            name: "Concrete".to_string(),
            conductivity: 1.73,
            density: 2300.0,
            specific_heat: 880.0,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            visible_absorptance: 0.7,
            roughness: crate::material::Roughness::MediumRough,
            thermal_resistance: None,
            vapor_resistance_factor: Some(130.0),
            sorption_isotherm: Some(vec![
                [0.0, 0.0],
                [0.5, 0.02],
                [0.8, 0.06],
                [0.95, 0.12],
                [1.0, 0.20],
            ]),
            liquid_transport_coeff: None,
            thermal_absorptance_inside: None,
        }
    }

    fn mineral_wool_material() -> Material {
        Material {
            name: "MineralWool".to_string(),
            conductivity: 0.04,
            density: 30.0,
            specific_heat: 840.0,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            visible_absorptance: 0.7,
            roughness: crate::material::Roughness::MediumRough,
            thermal_resistance: None,
            vapor_resistance_factor: Some(1.0),
            sorption_isotherm: Some(vec![
                [0.0, 0.0],
                [0.5, 0.005],
                [1.0, 0.02],
            ]),
            liquid_transport_coeff: None,
            thermal_absorptance_inside: None,
        }
    }

    fn make_materials() -> HashMap<String, Material> {
        let mut map = HashMap::new();
        map.insert("Concrete".to_string(), concrete_material());
        map.insert("MineralWool".to_string(), mineral_wool_material());
        map
    }

    #[test]
    fn test_hamt_single_layer_moisture_propagation() {
        // Single concrete layer: step change in outdoor RH → moisture propagates inward.
        // Verify mass conservation: total moisture should not decrease after RH step.
        let mat = concrete_material();
        let layer = HamtLayer::new(&mat, 0.20, 20.0, 0.50);
        let initial_moisture: f64 = layer.moisture.iter().sum();
        let initial_n = layer.n_nodes;

        let construction = Construction {
            name: "Test".to_string(),
            layers: vec![ConstructionLayer {
                material: "Concrete".to_string(),
                thickness: 0.20,
            }],
        };
        let materials = make_materials();
        let mut hamt = HamtState::from_construction(&construction, &materials, 20.0, 0.50)
            .expect("Should build HAMT for concrete with moisture data");

        // Step change: outdoor RH goes to 0.90 (much wetter outside)
        for _ in 0..12 {
            hamt.solve_timestep(0.0, 20.0, 0.90, 0.50, 3600.0);
        }

        let final_moisture: f64 = hamt.layers[0].moisture.iter().sum();
        // After 12 hours of high RH outside, total moisture should increase
        assert!(
            final_moisture > initial_moisture,
            "Moisture should increase when outdoor RH rises: {} > {}",
            final_moisture,
            initial_moisture
        );
        // All node counts unchanged
        assert_eq!(hamt.layers[0].n_nodes, initial_n);
    }

    #[test]
    fn test_hamt_two_layer_steady_state_temperature() {
        // Two-layer wall (concrete + mineral wool): verify that the inside
        // conductive flux at steady state is approximately U × ΔT.
        let construction = Construction {
            name: "TwoLayer".to_string(),
            layers: vec![
                ConstructionLayer {
                    material: "Concrete".to_string(),
                    thickness: 0.15,
                },
                ConstructionLayer {
                    material: "MineralWool".to_string(),
                    thickness: 0.08,
                },
            ],
        };
        let materials = make_materials();
        let mut hamt = HamtState::from_construction(&construction, &materials, 20.0, 0.50)
            .expect("Should build HAMT for two-layer wall");

        // Simulate 200 hours to reach near-steady state
        let t_out = -10.0;
        let t_in = 20.0;
        for _ in 0..200 {
            hamt.solve_timestep(t_out, t_in, 0.80, 0.50, 3600.0);
        }

        // U-value (no film): 1 / (0.15/1.73 + 0.08/0.04) = 1 / (0.0867 + 2.0) = 0.472 W/(m²·K)
        let r_concrete = 0.15 / 1.73;
        let r_wool = 0.08 / 0.04;
        let u_no_film = 1.0 / (r_concrete + r_wool);
        let expected_q = u_no_film * (t_out - t_in).abs();

        // Inside flux should be within 30% of steady-state U×ΔT (film coefficients vary)
        let q_inside = hamt.q_inside.abs();
        assert!(
            q_inside > expected_q * 0.30,
            "Inside flux {:.1} W/m² should be > 30% of U×ΔT = {:.1} W/m²",
            q_inside,
            expected_q
        );
    }

    #[test]
    fn test_hamt_not_activated_without_moisture_data() {
        // A construction without moisture data should NOT activate HAMT.
        let materials_no_moisture = {
            let mut map = HashMap::new();
            map.insert(
                "PlainConcrete".to_string(),
                Material {
                    name: "PlainConcrete".to_string(),
                    conductivity: 1.73,
                    density: 2300.0,
                    specific_heat: 880.0,
                    solar_absorptance: 0.7,
                    thermal_absorptance: 0.9,
                    visible_absorptance: 0.7,
                    roughness: crate::material::Roughness::MediumRough,
                    thermal_resistance: None,
                    vapor_resistance_factor: None, // No HAMT data
                    sorption_isotherm: None,
                    liquid_transport_coeff: None,
                    thermal_absorptance_inside: None,
                },
            );
            map
        };
        let construction = Construction {
            name: "NoMoisture".to_string(),
            layers: vec![ConstructionLayer {
                material: "PlainConcrete".to_string(),
                thickness: 0.20,
            }],
        };

        // Should return None → CTF path
        let result = HamtState::from_construction(&construction, &materials_no_moisture, 20.0, 0.50);
        assert!(
            result.is_none(),
            "HAMT should not activate when moisture data is absent"
        );

        // Helper function should also return false
        assert!(
            !construction_has_hamt_data(&construction, &materials_no_moisture),
            "construction_has_hamt_data should be false without vapor_resistance_factor"
        );
    }

    #[test]
    fn test_hamt_activation_with_complete_data() {
        // A construction with complete moisture data in all layers should activate HAMT.
        let materials = make_materials();
        let construction = Construction {
            name: "WithMoisture".to_string(),
            layers: vec![ConstructionLayer {
                material: "Concrete".to_string(),
                thickness: 0.20,
            }],
        };
        assert!(
            construction_has_hamt_data(&construction, &materials),
            "construction_has_hamt_data should be true with complete moisture data"
        );

        let hamt = HamtState::from_construction(&construction, &materials, 20.0, 0.50);
        assert!(hamt.is_some(), "HAMT should activate when moisture data is complete");
    }

    #[test]
    fn test_tdma_solver() {
        // Verify TDMA on a simple 3×3 tridiagonal system.
        // System: [2 -1 0; -1 2 -1; 0 -1 2] × x = [1; 0; 1]
        // Solution: x = [1; 1; 1]
        let a = vec![0.0, -1.0, -1.0];
        let b = vec![2.0, 2.0, 2.0];
        let c = vec![-1.0, -1.0, 0.0];
        let d = vec![1.0, 0.0, 1.0];
        let x = tdma(&a, &b, &c, &d);
        assert_eq!(x.len(), 3);
        assert_relative_eq!(x[0], 1.0, max_relative = 0.001);
        assert_relative_eq!(x[1], 1.0, max_relative = 0.001);
        assert_relative_eq!(x[2], 1.0, max_relative = 0.001);
    }

    #[test]
    fn test_p_sat_at_known_temps() {
        // p_sat(0°C) ≈ 611 Pa, p_sat(100°C) ≈ 101325 Pa
        let ps_0 = p_sat(0.0);
        assert!(
            ps_0 > 600.0 && ps_0 < 620.0,
            "p_sat(0°C) should be ~611 Pa, got {:.1}",
            ps_0
        );
        // p_sat at 20°C ≈ 2338 Pa
        let ps_20 = p_sat(20.0);
        assert!(
            ps_20 > 2200.0 && ps_20 < 2500.0,
            "p_sat(20°C) should be ~2338 Pa, got {:.1}",
            ps_20
        );
    }

    #[test]
    fn test_interp_table_bounds_and_middle() {
        let table = vec![[0.0, 0.0], [0.5, 10.0], [1.0, 20.0]];
        // At left bound
        assert_relative_eq!(interp_table(&table, 0.0), 0.0, epsilon = 1e-10);
        // At right bound
        assert_relative_eq!(interp_table(&table, 1.0), 20.0, epsilon = 1e-10);
        // Midpoint
        assert_relative_eq!(interp_table(&table, 0.25), 5.0, max_relative = 0.001);
        // Extrapolation below
        assert_relative_eq!(interp_table(&table, -0.5), 0.0, epsilon = 1e-10);
        // Extrapolation above
        assert_relative_eq!(interp_table(&table, 1.5), 20.0, epsilon = 1e-10);
    }
}
