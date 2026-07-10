//! Multizone pressure network airflow solver.
//!
//! Newton-Raphson solver for pressure-driven airflow through building cracks,
//! large openings, and mechanical systems. Auto-generates the network from
//! building geometry when enabled via `airflow_network: enabled: true`.
//!
//! Flow models:
//!   - Power-law cracks: Q = C × |ΔP|^n × sign(ΔP), n ≈ 0.65
//!   - Large openings (orifice): Q = Cd × A × sqrt(2ρ|ΔP|) × sign(ΔP)
//!   - Fixed flows: exhaust fans, HVAC imbalance
//!
//! Wind pressure coefficients from Swami & Chandra (1988) for low-rise,
//! or simplified cosine model for high-rise buildings.
//!
//! Reference: ASHRAE Fundamentals Ch. 16, E+ AirflowNetwork model,
//! Swami & Chandra (1988) FSEC-CR-163-86.

use crate::convection::Terrain;
use crate::surface::{BoundaryCondition, SurfaceType};
use serde::{Deserialize, Serialize};

/// Gravitational acceleration [m/s²].
const G: f64 = 9.81;

/// Minimum |ΔP| to avoid singularity in Jacobian [Pa].
const MIN_DP: f64 = 1e-4;

/// Reference air density for flow coefficient normalization [kg/m³].
const RHO_REF: f64 = 1.2041; // at 20°C, 101325 Pa

// ─── Configuration (YAML input) ─────────────────────────────────────────────

/// Wind pressure coefficient model selection.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CpModel {
    /// Swami & Chandra (1988) for low-rise buildings (< 3 stories).
    #[default]
    SwamiChandra,
    /// Simplified high-rise model (cosine variation, constant leeward).
    HighRise,
    /// User-specified per-facade Cp table (#81), interpolated linearly
    /// from the EPW wind direction each timestep.
    Table(CpTable),
}

/// User-specified wind pressure coefficient table (#81).
///
/// `wind_angles` lists the wind directions (degrees from north, ascending,
/// within [0, 360)) at which Cp values are given — e.g. 0°–315° in 45° steps.
/// Each facade provides one Cp value per wind angle; a surface uses the
/// facade whose azimuth is nearest its own outward normal. Interpolation is
/// linear in wind direction with wraparound between the last and first angle.
///
/// # Example (YAML)
/// ```yaml
/// cp_model: !table
///   wind_angles: [0, 45, 90, 135, 180, 225, 270, 315]
///   facades:
///     - azimuth: 0     # north facade
///       cp: [0.6, 0.35, -0.4, -0.45, -0.3, -0.45, -0.4, 0.35]
///     - azimuth: 180   # south facade
///       cp: [-0.3, -0.45, -0.4, 0.35, 0.6, 0.35, -0.4, -0.45]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CpTable {
    /// Wind directions [degrees from north], ascending, within [0, 360).
    pub wind_angles: Vec<f64>,
    /// Per-facade Cp curves.
    pub facades: Vec<CpFacade>,
}

/// Cp values for one facade, parallel to `CpTable.wind_angles`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CpFacade {
    /// Facade outward normal azimuth [degrees from north, clockwise].
    pub azimuth: f64,
    /// Cp value per entry in `wind_angles`.
    pub cp: Vec<f64>,
}

/// ASHRAE construction leakage class (ASHRAE Fundamentals Ch. 16 Table 1).
///
/// Selects tabulated crack leakage per unit surface area instead of the
/// manual `*_leakage_per_area` fields. Accepts either the letter class
/// (`a`–`d`) or the descriptive alias (`tight`, `average`, `leaky`,
/// `very_leaky`) in YAML.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeakageClass {
    /// Class A — tight construction.
    #[serde(alias = "tight")]
    A,
    /// Class B — average construction.
    #[serde(alias = "average")]
    B,
    /// Class C — leaky construction.
    #[serde(alias = "leaky")]
    C,
    /// Class D — very leaky construction.
    #[serde(alias = "very_leaky")]
    D,
}

/// Leakage-class lookup tables: (coefficient [kg/s/m²/Pa^n], exponent n).
///
/// Coefficients are derived from ASHRAE Fundamentals Ch. 16 Table 1 unit
/// effective leakage areas (ELA at 4 Pa, Cd = 1) converted to power-law mass
/// flow coefficients at reference density:
///   C = ρ_ref × ELA × sqrt(2·ΔP_ref/ρ_ref) / ΔP_ref^n  with ΔP_ref = 4 Pa
/// Class D extends the table by doubling the class C leakage area.
impl LeakageClass {
    /// Exterior opaque walls (ELA ≈ 0.5 / 1.7 / 3.5 / 7.0 cm²/m²).
    pub fn wall_leakage(self) -> (f64, f64) {
        match self {
            LeakageClass::A => (0.000063, 0.65),
            LeakageClass::B => (0.00021, 0.65),
            LeakageClass::C => (0.00044, 0.65),
            LeakageClass::D => (0.00088, 0.65),
        }
    }

    /// Windows, per unit window area (ELA ≈ 1.1 / 2.8 / 5.6 / 11.0 cm²/m²).
    pub fn window_leakage(self) -> (f64, f64) {
        match self {
            LeakageClass::A => (0.00014, 0.65),
            LeakageClass::B => (0.00035, 0.65),
            LeakageClass::C => (0.00071, 0.65),
            LeakageClass::D => (0.0014, 0.65),
        }
    }

    /// Interzone partitions/ceilings (ELA ≈ 0.8 / 1.8 / 5.4 / 10.8 cm²/m²).
    pub fn interzone_leakage(self) -> (f64, f64) {
        match self {
            LeakageClass::A => (0.0001, 0.65),
            LeakageClass::B => (0.00023, 0.65),
            LeakageClass::C => (0.00068, 0.65),
            LeakageClass::D => (0.0014, 0.65),
        }
    }
}

/// Top-level airflow network configuration.
///
/// Added to `SimulationSettings` as an optional field. When `enabled: true`,
/// the pressure network replaces the Design Flow Rate infiltration model.
///
/// # Example (YAML)
/// ```yaml
/// simulation:
///   airflow_network:
///     enabled: true
///     cp_model: swami_chandra
///     wall_leakage_per_area: 0.00008
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirflowNetworkConfig {
    /// Enable the pressure network solver.
    #[serde(default)]
    pub enabled: bool,
    /// Cp model selection.
    #[serde(default)]
    pub cp_model: CpModel,
    /// Default crack flow exponent n (typically 0.65 for building cracks).
    #[serde(default = "default_crack_exponent")]
    pub default_crack_exponent: f64,
    /// Default crack leakage per unit area for opaque walls [kg/s/m²/Pa^n].
    /// Corresponds to tight construction (~1.5 ACH50).
    #[serde(default = "default_wall_leakage")]
    pub wall_leakage_per_area: f64,
    /// Default crack leakage per unit area for windows [kg/s/m²/Pa^n].
    #[serde(default = "default_window_leakage")]
    pub window_leakage_per_area: f64,
    /// Default crack leakage per unit perimeter for doors [kg/s/m/Pa^n].
    #[serde(default = "default_door_leakage")]
    pub door_leakage_per_perimeter: f64,
    /// Default interzone crack leakage per unit area [kg/s/m²/Pa^n].
    #[serde(default = "default_interzone_leakage")]
    pub interzone_leakage_per_area: f64,
    /// ASHRAE leakage class for exterior walls (#80). When set, overrides
    /// `wall_leakage_per_area` and `default_crack_exponent` for wall cracks.
    #[serde(default)]
    pub wall_leakage_class: Option<LeakageClass>,
    /// ASHRAE leakage class for windows (#80). When set, overrides
    /// `window_leakage_per_area` and `default_crack_exponent` for window cracks.
    #[serde(default)]
    pub window_leakage_class: Option<LeakageClass>,
    /// ASHRAE leakage class for interzone surfaces (#80). When set, overrides
    /// `interzone_leakage_per_area` and `default_crack_exponent` for interzone cracks.
    #[serde(default)]
    pub interzone_leakage_class: Option<LeakageClass>,
    /// Newton-Raphson convergence tolerance [Pa].
    #[serde(default = "default_convergence_tol")]
    pub convergence_tolerance: f64,
    /// Maximum Newton-Raphson iterations.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    /// Damping factor for Newton-Raphson update (0-1, 1 = full step).
    /// With `adaptive_damping` this is the starting factor.
    #[serde(default = "default_damping")]
    pub damping: f64,
    /// Adapt the Newton under-relaxation factor per iteration (#86):
    /// grow toward a full step while the residual falls, cut on overshoot.
    #[serde(default = "default_adaptive_damping")]
    pub adaptive_damping: bool,
    /// Relative convergence tolerance (#86): accept when the worst zone
    /// mass residual falls below this fraction of total network through-flow.
    #[serde(default = "default_relative_tolerance")]
    pub relative_tolerance: f64,
    /// Passive species tracked on the network flows (#84), e.g. CO₂.
    #[serde(default)]
    pub species: Vec<crate::species::SpeciesConfig>,
    /// Blower-door calibration (#92): measured air changes per hour at
    /// 50 Pa. When set, exterior crack coefficients are uniformly scaled
    /// after auto-generation so the whole-building envelope flow at 50 Pa
    /// equals ach50 × building volume (windows/openings excluded, as in
    /// the physical test).
    #[serde(default)]
    pub ach50: Option<f64>,
}

fn default_crack_exponent() -> f64 {
    0.65
}
fn default_wall_leakage() -> f64 {
    0.00008
}
fn default_window_leakage() -> f64 {
    0.00014
}
fn default_door_leakage() -> f64 {
    0.0006
}
fn default_interzone_leakage() -> f64 {
    0.00004
}
fn default_convergence_tol() -> f64 {
    0.1
}
fn default_max_iterations() -> usize {
    30
}
fn default_damping() -> f64 {
    0.75
}
fn default_adaptive_damping() -> bool {
    true
}
fn default_relative_tolerance() -> f64 {
    1e-4
}

impl Default for AirflowNetworkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cp_model: CpModel::default(),
            default_crack_exponent: default_crack_exponent(),
            wall_leakage_per_area: default_wall_leakage(),
            window_leakage_per_area: default_window_leakage(),
            door_leakage_per_perimeter: default_door_leakage(),
            interzone_leakage_per_area: default_interzone_leakage(),
            wall_leakage_class: None,
            window_leakage_class: None,
            interzone_leakage_class: None,
            convergence_tolerance: default_convergence_tol(),
            max_iterations: default_max_iterations(),
            damping: default_damping(),
            adaptive_damping: default_adaptive_damping(),
            relative_tolerance: default_relative_tolerance(),
            species: Vec::new(),
            ach50: None,
        }
    }
}

/// Per-surface override for airflow network auto-generation.
///
/// Allows users to override auto-generated crack/opening properties
/// for specific surfaces without manually specifying the entire network.
///
/// # Example (YAML)
/// ```yaml
/// surfaces:
///   - name: front_door
///     airflow:
///       large_opening: true
///       opening_fraction: 0.5
///       discharge_coefficient: 0.65
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SurfaceAirflowOverride {
    /// Override crack flow coefficient C [kg/s/Pa^n].
    #[serde(default)]
    pub crack_coefficient: Option<f64>,
    /// Override crack flow exponent n.
    #[serde(default)]
    pub crack_exponent: Option<f64>,
    /// Treat surface as a large opening (door, operable window).
    #[serde(default)]
    pub large_opening: Option<bool>,
    /// Override discharge coefficient for large opening.
    #[serde(default)]
    pub discharge_coefficient: Option<f64>,
    /// Override opening area as fraction of surface area [0-1].
    #[serde(default)]
    pub opening_fraction: Option<f64>,
    /// Schedule name modulating the opening fraction over time [0-1].
    /// The effective opening area is `area × opening_fraction × schedule`,
    /// re-evaluated each timestep; a schedule value of 0 closes the opening.
    #[serde(default)]
    pub opening_schedule: Option<String>,
    /// Override wind pressure coefficient Cp [-].
    #[serde(default)]
    pub cp: Option<f64>,
    /// Override opening midpoint height above ground [m].
    #[serde(default)]
    pub opening_height: Option<f64>,
}

// ─── Runtime structures ─────────────────────────────────────────────────────

/// A node in the pressure network.
#[derive(Debug, Clone)]
pub struct PressureNode {
    /// Zone index in BuildingEnvelope.zones (None = outdoor node).
    pub zone_index: Option<usize>,
    /// Reference height above ground datum [m].
    pub ref_height: f64,
    /// Current solved gauge pressure [Pa] (relative to outdoor at datum).
    pub pressure: f64,
    /// Zone air temperature [K].
    pub temperature: f64,
    /// Zone air density [kg/m³].
    pub density: f64,
}

/// Type of flow element connecting two nodes.
#[derive(Debug, Clone)]
pub enum FlowElement {
    /// Power-law crack: Q = C × |ΔP|^n × sign(ΔP).
    PowerLawCrack {
        /// Flow coefficient C [kg/s/Pa^n at reference density].
        coefficient: f64,
        /// Flow exponent n (0.5 = orifice, 0.65 = crack, 1.0 = laminar).
        exponent: f64,
    },
    /// Large opening (orifice equation): Q = Cd × A × sqrt(2ρ|ΔP|) × sign(ΔP).
    LargeOpening {
        /// Discharge coefficient Cd (typically 0.6-0.65).
        discharge_coefficient: f64,
        /// Opening area [m²].
        area: f64,
    },
    /// Fixed mass flow rate (exhaust fans, HVAC imbalance).
    /// Positive = flow from node_a to node_b.
    FixedFlow {
        /// Mass flow rate [kg/s].
        mass_flow: f64,
    },
    /// Horizontal two-way opening (#93): stairwell, atrium floor, or attic
    /// hatch. Net pressure-driven orifice flow plus buoyancy-driven
    /// instability exchange when the upper zone's air is denser
    /// (Cooper 1989 / Epstein 1988, as in the E+ AFE HorizontalOpening).
    /// Convention: node_a = lower zone, node_b = upper zone.
    HorizontalOpening {
        /// Discharge coefficient Cd for the pressure-driven component.
        discharge_coefficient: f64,
        /// Opening area [m²].
        area: f64,
    },
    /// Two-way large opening (#83): doorway or operable window resolving
    /// buoyancy-driven counterflow above and below the neutral plane.
    ///
    /// The pressure difference varies linearly over the opening height
    /// (slope g·(ρ_b − ρ_a)); the orifice equation is integrated in closed
    /// form on each side of the neutral plane, so warm air can leave through
    /// the top of the opening while cold air enters through the bottom.
    TwoWayOpening {
        /// Discharge coefficient Cd (typically 0.6-0.65).
        discharge_coefficient: f64,
        /// Opening width [m].
        width: f64,
        /// Opening height (vertical extent) [m], centered on the path height.
        height: f64,
    },
}

/// A flow path connecting two pressure nodes.
#[derive(Debug, Clone)]
pub struct FlowPath {
    /// Index of node A in `AirflowNetwork.nodes`.
    pub node_a: usize,
    /// Index of node B in `AirflowNetwork.nodes`.
    pub node_b: usize,
    /// Midpoint height of this opening above ground datum [m].
    pub height: f64,
    /// Wind pressure coefficient at this location [-].
    /// Only meaningful when one node is the outdoor node.
    pub cp: f64,
    /// Surface azimuth [degrees from north, clockwise].
    /// Used to update Cp each timestep based on wind direction.
    pub azimuth: f64,
    /// The flow element model.
    pub element: FlowElement,
    /// Source surface index in BuildingEnvelope.surfaces (for diagnostics).
    pub source_surface: Option<usize>,
    /// Last computed mass flow rate [kg/s] (positive = A→B).
    pub mass_flow: f64,
}

/// Interzone flow entering a zone: (source_zone_index, mass_flow_kg_s).
/// Mass flow is positive = into this zone from source zone.
pub type InterzoneFlow = (usize, f64);

/// A large opening whose area is modulated by a schedule each timestep.
#[derive(Debug, Clone)]
pub struct ScheduledOpening {
    /// Index of the opening's path in `AirflowNetwork.paths`.
    pub path_index: usize,
    /// Schedule name providing the opening fraction [0-1].
    pub schedule: String,
    /// Fully-open area [m²] (surface area × configured opening_fraction).
    pub base_area: f64,
}

/// The assembled airflow network.
#[derive(Debug, Clone)]
pub struct AirflowNetwork {
    pub config: AirflowNetworkConfig,
    pub nodes: Vec<PressureNode>,
    pub paths: Vec<FlowPath>,
    /// Index of the outdoor node in `nodes`.
    pub outdoor_node: usize,
    /// Mapping: zone_index → node_index.
    pub zone_to_node: Vec<usize>,
    /// Per-zone net outdoor mass flow [kg/s] (infiltration entering from outdoors).
    /// Positive = infiltration. Zero when building is pressurized (exfiltration occurs
    /// but is not counted here since zone air leaving has minimal heat-balance impact).
    /// Replaces `infiltration_mass_flow` in zone state when AFN is active.
    pub zone_outdoor_mass_flow: Vec<f64>,
    /// Per-zone interzone flows: list of (source_zone_index, mass_flow [kg/s]).
    pub zone_interzone_flows: Vec<Vec<InterzoneFlow>>,
    /// Building plan side ratio (along-wind / cross-wind) for Cp calculation.
    pub side_ratio: f64,
    /// Per-zone path indices for HVAC net injection (outdoor_air_supply - exhaust).
    /// These paths drive zone pressure but are excluded from zone_outdoor_mass_flow
    /// accumulation (the OA is already counted in zone.outdoor_air_mass_flow).
    pub hvac_net_path_indices: Vec<Option<usize>>,
    /// Large openings with schedule-driven opening fraction (#79).
    /// Areas are refreshed each timestep before solving.
    pub scheduled_openings: Vec<ScheduledOpening>,
    /// Per-zone path indices for supply duct leakage: zone → duct ambient
    /// zone (#82, #85). Supply air lost to the attic/crawlspace pressurizes
    /// the unconditioned space and depressurizes the zone. Excluded from
    /// thermal flow accumulation (leakage energy is handled by the duct
    /// component, #70) but visible to species transport.
    pub duct_supply_leak_path_indices: Vec<Option<usize>>,
    /// Per-zone path indices for return duct leakage: duct ambient zone →
    /// zone (#85). The return duct runs below ambient pressure and ingests
    /// unconditioned-space air, which is delivered to the zone. Same
    /// accumulation treatment as the supply leak paths.
    pub duct_return_leak_path_indices: Vec<Option<usize>>,
    /// Per-zone path indices for natural-ventilation openings (#88). The
    /// effective opening area is driven each timestep from the NV
    /// availability logic (schedule, temperature windows, wind limit); the
    /// zone-level wind-&-stack model is disabled while the AFN is active so
    /// the flow is not double-counted.
    pub nat_vent_path_indices: Vec<Option<usize>>,
}

impl AirflowNetwork {
    /// Update the HVAC net injection FixedFlow for a zone before each AFN solve.
    ///
    /// `net_kg_s` = outdoor_air_mass_flow − exhaust_mass_flow (from previous timestep).
    /// Positive → zone is being pressurized (OA supply > exhaust).
    /// Negative → zone is being depressurized (exhaust > OA supply).
    pub fn update_hvac_net_flow(&mut self, zone_idx: usize, net_kg_s: f64) {
        if let Some(Some(path_idx)) = self.hvac_net_path_indices.get(zone_idx) {
            if let Some(path) = self.paths.get_mut(*path_idx) {
                if let FlowElement::FixedFlow { ref mut mass_flow } = path.element {
                    *mass_flow = net_kg_s;
                }
            }
        }
    }

    /// Update the duct leakage FixedFlows for a zone before each AFN solve
    /// (#82, #85).
    ///
    /// `supply_kg_s` = supply_leakage_fraction × supply mass flow: supply air
    /// lost from the duct run into the ambient zone (zone → ambient).
    /// `return_kg_s` = return_leakage_fraction × supply mass flow: ambient
    /// air ingested by the return duct and delivered to the zone (ambient →
    /// zone). Both use the previous timestep's supply flow. The net pressure
    /// effect on the zone is (return − supply); a supply-dominated system
    /// depressurizes the zone, a return-dominated one pressurizes it.
    pub fn update_duct_leakage_flows(
        &mut self,
        zone_idx: usize,
        supply_kg_s: f64,
        return_kg_s: f64,
    ) {
        if let Some(Some(path_idx)) = self.duct_supply_leak_path_indices.get(zone_idx) {
            if let Some(path) = self.paths.get_mut(*path_idx) {
                if let FlowElement::FixedFlow { ref mut mass_flow } = path.element {
                    *mass_flow = supply_kg_s;
                }
            }
        }
        if let Some(Some(path_idx)) = self.duct_return_leak_path_indices.get(zone_idx) {
            if let Some(path) = self.paths.get_mut(*path_idx) {
                if let FlowElement::FixedFlow { ref mut mass_flow } = path.element {
                    *mass_flow = return_kg_s;
                }
            }
        }
    }

    /// Update a natural-ventilation opening's effective area before each
    /// AFN solve (#88). `area` = opening_area × availability (schedule
    /// fraction gated by temperature windows and wind limit); 0 = closed.
    pub fn update_nat_vent_opening(&mut self, zone_idx: usize, area: f64) {
        if let Some(Some(path_idx)) = self.nat_vent_path_indices.get(zone_idx) {
            if let Some(path) = self.paths.get_mut(*path_idx) {
                if let FlowElement::LargeOpening {
                    area: ref mut path_area,
                    ..
                } = path.element
                {
                    *path_area = area.max(0.0);
                }
            }
        }
    }

    /// Refresh schedule-driven opening areas before each AFN solve (#79).
    ///
    /// `fraction_of` maps a schedule name to the current fraction [0-1];
    /// the effective opening area is `base_area × fraction`. A fraction of 0
    /// closes the opening (zero area → zero flow).
    pub fn update_scheduled_openings<F>(&mut self, fraction_of: F)
    where
        F: Fn(&str) -> f64,
    {
        for opening in &self.scheduled_openings {
            let frac = fraction_of(&opening.schedule).clamp(0.0, 1.0);
            if let Some(path) = self.paths.get_mut(opening.path_index) {
                match path.element {
                    FlowElement::LargeOpening { ref mut area, .. } => {
                        *area = opening.base_area * frac;
                    }
                    // Two-way openings keep their vertical extent; the
                    // schedule narrows the width (sliding-sash behavior).
                    FlowElement::TwoWayOpening {
                        ref mut width,
                        height,
                        ..
                    } => {
                        *width = opening.base_area * frac / height.max(1e-6);
                    }
                    FlowElement::HorizontalOpening { ref mut area, .. } => {
                        *area = opening.base_area * frac;
                    }
                    _ => {}
                }
            }
        }
    }
}

// ─── Cp correlations ────────────────────────────────────────────────────────

/// Swami & Chandra (1988) wind pressure coefficient for low-rise buildings.
///
/// θ = angle between wind direction and surface outward normal [degrees].
/// `side_ratio` = L/W (plan dimension ratio along wind / across wind).
///
/// Reference: Swami & Chandra, FSEC-CR-163-86 (1988), Eq. 1.
/// Also: ASHRAE Fundamentals 2017, Ch. 24, Eq. 3.
pub fn cp_swami_chandra(theta_deg: f64, side_ratio: f64) -> f64 {
    let theta = theta_deg.to_radians();
    // G = natural log of the plan side ratio (Swami & Chandra Eq. 1). The two
    // side-ratio terms use G and G², not the raw ratio — the old code used
    // `side_ratio.ln().exp()` (a no-op identity) and `side_ratio.powi(2)`.
    let g = side_ratio.ln();
    let ln_arg = 1.248 - 0.703 * (theta / 2.0).sin() - 1.175 * theta.sin().powi(2)
        + 0.131 * (2.0 * theta * g).sin().powi(3)
        + 0.769 * (theta / 2.0).cos()
        + 0.07 * g.powi(2) * (theta / 2.0).sin().powi(2)
        + 0.717 * (theta / 2.0).cos().powi(2);

    // Protect against ln(≤0)
    if ln_arg > 0.0 {
        0.6 * ln_arg.ln()
    } else {
        -0.5 // conservative leeward value
    }
}

/// Simplified high-rise Cp model.
///
/// Windward (|θ| ≤ 90°): Cp = 0.6 × cos(θ)
/// Leeward (|θ| > 90°): Cp = -0.3
pub fn cp_high_rise(theta_deg: f64) -> f64 {
    let theta = theta_deg.to_radians();
    let cos_t = theta.cos();
    if cos_t > 0.0 {
        0.6 * cos_t
    } else {
        -0.3
    }
}

/// Interpolate Cp from a user table (#81).
///
/// Picks the facade whose azimuth is nearest `surface_azimuth` (shortest
/// angular distance), then linearly interpolates its Cp curve at `wind_dir`
/// with wraparound between the last and first wind angle.
pub fn cp_from_table(table: &CpTable, wind_dir: f64, surface_azimuth: f64) -> f64 {
    let facade = table.facades.iter().min_by(|a, b| {
        let da = angular_distance(a.azimuth, surface_azimuth);
        let db = angular_distance(b.azimuth, surface_azimuth);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    let facade = match facade {
        Some(f) => f,
        None => return 0.0,
    };

    let n = table.wind_angles.len().min(facade.cp.len());
    match n {
        0 => return 0.0,
        1 => return facade.cp[0],
        _ => {}
    }
    let angles = &table.wind_angles[..n];
    let values = &facade.cp[..n];

    let wd = wind_dir.rem_euclid(360.0);

    // Before the first or after the last tabulated angle: wrap around
    // between the last and first entries.
    if wd < angles[0] || wd >= angles[n - 1] {
        let span = 360.0 - angles[n - 1] + angles[0];
        let offset = if wd >= angles[n - 1] {
            wd - angles[n - 1]
        } else {
            360.0 - angles[n - 1] + wd
        };
        let t = if span > 0.0 { offset / span } else { 0.0 };
        return values[n - 1] + t * (values[0] - values[n - 1]);
    }

    // Interior segment: linear interpolation between bracketing angles.
    for i in 0..n - 1 {
        if wd >= angles[i] && wd < angles[i + 1] {
            let span = angles[i + 1] - angles[i];
            let t = if span > 0.0 {
                (wd - angles[i]) / span
            } else {
                0.0
            };
            return values[i] + t * (values[i + 1] - values[i]);
        }
    }
    values[n - 1]
}

/// Shortest angular distance between two compass directions [degrees, 0-180].
fn angular_distance(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Compute Cp for a surface from the configured model.
///
/// `wind_dir` and `azimuth` are absolute compass directions; the correlation
/// models use only their relative angle, while the table model also uses the
/// azimuth to select the facade curve.
fn compute_cp(model: &CpModel, wind_dir: f64, azimuth: f64, side_ratio: f64) -> f64 {
    match model {
        CpModel::SwamiChandra => {
            cp_swami_chandra(wind_surface_angle(wind_dir, azimuth), side_ratio)
        }
        CpModel::HighRise => cp_high_rise(wind_surface_angle(wind_dir, azimuth)),
        CpModel::Table(table) => cp_from_table(table, wind_dir, azimuth),
    }
}

/// Angle between wind direction and surface outward normal [degrees, 0-360].
///
/// `wind_dir` = direction wind is coming FROM [degrees from north, clockwise].
/// `azimuth` = surface outward normal direction [degrees from north, clockwise].
///
/// Returns angle in [0, 360).
fn wind_surface_angle(wind_dir: f64, azimuth: f64) -> f64 {
    let mut angle = (wind_dir - azimuth).abs() % 360.0;
    if angle > 180.0 {
        angle = 360.0 - angle;
    }
    angle
}

// ─── Flow element math ──────────────────────────────────────────────────────

/// Compute mass flow [kg/s] through a flow element given pressure difference.
///
/// `dp` = P_a - P_b + stack + wind [Pa], evaluated at the path height.
/// `rho_a`, `rho_b` = air density at node A / node B [kg/m³].
///
/// Returns (mass_flow [kg/s], d(mass_flow)/d(dp) [kg/s/Pa]).
/// Positive flow = A → B. For two-way openings the returned flow is the net
/// of the two directional components (see `two_way_components`).
pub fn flow_and_derivative(element: &FlowElement, dp: f64, rho_a: f64, rho_b: f64) -> (f64, f64) {
    let rho_avg = 0.5 * (rho_a + rho_b);
    match element {
        FlowElement::PowerLawCrack {
            coefficient,
            exponent,
        } => {
            let c = *coefficient;
            let n = *exponent;
            // Density correction: Q ∝ (ρ/ρ_ref)^(1-n) per ASHRAE
            let rho_corr = (rho_avg / RHO_REF).powf(1.0 - n);
            // Below MIN_DP, linearize through zero: a sign(ΔP)·|ΔP|^n clamp
            // is discontinuous at ΔP = 0 and traps Newton in a limit cycle
            // whenever the balance point needs a flow inside the jump (#93).
            if dp.abs() < MIN_DP {
                let slope = c * rho_corr * MIN_DP.powf(n) / MIN_DP;
                return (slope * dp, slope);
            }
            let dp_abs = dp.abs();
            let sign = if dp >= 0.0 { 1.0 } else { -1.0 };
            let flow = sign * c * rho_corr * dp_abs.powf(n);
            let dflow = c * rho_corr * n * dp_abs.powf(n - 1.0);
            (flow, dflow)
        }
        FlowElement::LargeOpening {
            discharge_coefficient,
            area,
        } => {
            let cd = *discharge_coefficient;
            let a = *area;
            // Linearize through zero below MIN_DP (see PowerLawCrack note).
            if dp.abs() < MIN_DP {
                let slope = cd * a * (2.0 * rho_avg * MIN_DP).sqrt() / MIN_DP;
                return (slope * dp, slope);
            }
            let dp_abs = dp.abs();
            let sign = if dp >= 0.0 { 1.0 } else { -1.0 };
            let flow = sign * cd * a * (2.0 * rho_avg * dp_abs).sqrt();
            // dQ/ddp = Cd * A * sqrt(rho_avg / (2 * |dp|))
            let dflow = cd * a * (rho_avg / (2.0 * dp_abs)).sqrt();
            (flow, dflow)
        }
        FlowElement::FixedFlow { mass_flow } => {
            (*mass_flow, 0.0) // no pressure dependence
        }
        FlowElement::TwoWayOpening { .. } => {
            let (m_ab, m_ba, dnet) = two_way_opening_flows(element, dp, rho_a, rho_b);
            (m_ab - m_ba, dnet)
        }
        FlowElement::HorizontalOpening { .. } => {
            let (m_ab, m_ba, dnet) = horizontal_opening_flows(element, dp, rho_a, rho_b);
            (m_ab - m_ba, dnet)
        }
    }
}

/// Directional mass flows through a horizontal opening (#93).
///
/// Convention: node A is the LOWER zone, node B the UPPER zone; positive
/// net flow is upward (A → B). Two components:
///   1. Pressure-driven orifice flow on ΔP using the upstream density.
///   2. Buoyancy-driven instability exchange when the upper air is denser
///      (ρ_b > ρ_a): Q_exch = 0.055·√(g·Δρ·D⁵/ρ̄) (Epstein/Cooper), ramped
///      down linearly with |ΔP| to zero at the flooding pressure
///      ΔP_flood = Cs²·g·Δρ·D⁵/(2A²), Cs = 0.942 (E+ AFE HorizontalOpening).
///
/// The returned derivative uses the orifice component only: the exchange
/// term is nearly pressure-independent and treating it as constant keeps
/// the Newton diagonal strictly positive.
///
/// Returns (ṁ_A→B, ṁ_B→A, d(ṁ_net)/d(ΔP)).
pub fn horizontal_opening_flows(
    element: &FlowElement,
    dp: f64,
    rho_a: f64,
    rho_b: f64,
) -> (f64, f64, f64) {
    let (cd, area) = match element {
        FlowElement::HorizontalOpening {
            discharge_coefficient,
            area,
        } => (*discharge_coefficient, *area),
        _ => return (0.0, 0.0, 0.0),
    };
    if area <= 0.0 {
        return (0.0, 0.0, 0.0);
    }

    // Pressure-driven orifice component (upstream density). Below MIN_DP,
    // linearize between the two ±MIN_DP endpoints so the flow is continuous
    // through zero — otherwise Newton limit-cycles when the balance point
    // needs a net flow smaller than the sign-flip jump (#93).
    let (mut m_ab, mut m_ba, dflow);
    if dp.abs() < MIN_DP {
        let f_up = cd * area * (2.0 * rho_a * MIN_DP).sqrt();
        let f_down = cd * area * (2.0 * rho_b * MIN_DP).sqrt();
        let slope = (f_up + f_down) / (2.0 * MIN_DP);
        let offset = (f_up - f_down) / 2.0;
        let net = slope * dp + offset;
        if net >= 0.0 {
            m_ab = net;
            m_ba = 0.0;
        } else {
            m_ab = 0.0;
            m_ba = -net;
        }
        dflow = slope;
    } else {
        let dp_abs = dp.abs();
        let rho_up = if dp >= 0.0 { rho_a } else { rho_b };
        let orifice = cd * area * (2.0 * rho_up * dp_abs).sqrt();
        dflow = cd * area * (rho_up / (2.0 * dp_abs)).sqrt();
        if dp >= 0.0 {
            m_ab = orifice;
            m_ba = 0.0;
        } else {
            m_ab = 0.0;
            m_ba = orifice;
        }
    }

    // Buoyancy instability exchange: dense air above light air
    let d_rho = rho_b - rho_a;
    if d_rho > 1e-9 {
        let rho_avg = 0.5 * (rho_a + rho_b);
        // Hydraulic diameter of the equivalent circular opening
        let dh = 2.0 * (area / std::f64::consts::PI).sqrt();
        let q_exch = 0.055 * (G * d_rho * dh.powi(5) / rho_avg).sqrt();
        const C_SHAPE: f64 = 0.942;
        let dp_flood = C_SHAPE * C_SHAPE * G * d_rho * dh.powi(5) / (2.0 * area * area);
        let fraction = (1.0 - dp.abs() / dp_flood.max(1e-12)).max(0.0);
        // Upward leg carries lower-zone air, downward leg upper-zone air
        m_ab += rho_a * q_exch * fraction;
        m_ba += rho_b * q_exch * fraction;
    }

    (m_ab, m_ba, dflow.max(1e-10))
}

/// Directional mass flows through a two-way opening (#83).
///
/// ΔP varies linearly over the opening height: ΔP(ζ) = dp + s·ζ with
/// ζ ∈ [−H/2, +H/2] and slope s = g·(ρ_b − ρ_a). The orifice equation is
/// integrated analytically on each side of the neutral plane:
///   ṁ_A→B = Cd·W·√(2ρ_a)·∫√(ΔP⁺) dζ,   ṁ_B→A = Cd·W·√(2ρ_b)·∫√(ΔP⁻) dζ
/// using ∫√u dζ = (2/(3|s|))·Δ(u^{3/2}) in u-space. Each component uses its
/// upstream density. Degenerates to the one-way orifice when |s| ≈ 0.
///
/// Returns (ṁ_A→B, ṁ_B→A, d(ṁ_net)/d(dp)), all ≥ 0 except the derivative
/// which is strictly positive.
pub fn two_way_opening_flows(
    element: &FlowElement,
    dp: f64,
    rho_a: f64,
    rho_b: f64,
) -> (f64, f64, f64) {
    let (cd, width, height) = match element {
        FlowElement::TwoWayOpening {
            discharge_coefficient,
            width,
            height,
        } => (*discharge_coefficient, *width, *height),
        _ => return (0.0, 0.0, 0.0),
    };
    if width <= 0.0 || height <= 0.0 {
        return (0.0, 0.0, 0.0);
    }

    let s = G * (rho_b - rho_a); // d(ΔP)/dz across the opening

    // Negligible density difference → uniform ΔP → one-way orifice using
    // the upstream density, linearized through zero below MIN_DP (#93).
    if s.abs() < 1e-9 {
        let area = width * height;
        if dp.abs() < MIN_DP {
            let f_up = cd * area * (2.0 * rho_a * MIN_DP).sqrt();
            let f_down = cd * area * (2.0 * rho_b * MIN_DP).sqrt();
            let slope = (f_up + f_down) / (2.0 * MIN_DP);
            let net = slope * dp + (f_up - f_down) / 2.0;
            return if net >= 0.0 {
                (net, 0.0, slope)
            } else {
                (0.0, -net, slope)
            };
        }
        let dp_abs = dp.abs();
        let rho_up = if dp >= 0.0 { rho_a } else { rho_b };
        let mag = cd * area * (2.0 * rho_up * dp_abs).sqrt();
        let dflow = cd * area * (rho_up / (2.0 * dp_abs)).sqrt();
        return if dp >= 0.0 {
            (mag, 0.0, dflow)
        } else {
            (0.0, mag, dflow)
        };
    }

    // ΔP at the bottom and top edges of the opening.
    let u_bot = dp - s * height / 2.0;
    let u_top = dp + s * height / 2.0;
    let (lo, hi) = if u_bot < u_top {
        (u_bot, u_top)
    } else {
        (u_top, u_bot)
    };
    let s_abs = s.abs();

    // A→B component (region where ΔP > 0)
    let (m_ab, dm_ab) = if hi > 0.0 {
        let a = lo.max(0.0);
        let integral = (2.0 / (3.0 * s_abs)) * (hi.powf(1.5) - a.powf(1.5));
        let dintegral = (hi.sqrt() - if lo > 0.0 { lo.sqrt() } else { 0.0 }) / s_abs;
        let k = cd * width * (2.0 * rho_a).sqrt();
        (k * integral, k * dintegral)
    } else {
        (0.0, 0.0)
    };

    // B→A component (region where ΔP < 0)
    let (m_ba, dm_ba) = if lo < 0.0 {
        let b = hi.min(0.0);
        let integral = (2.0 / (3.0 * s_abs)) * ((-lo).powf(1.5) - (-b).powf(1.5));
        // d(ṁ_BA)/d(dp) is negative: raising dp shrinks the ΔP<0 region.
        let dintegral = ((-lo).sqrt() - if hi < 0.0 { (-hi).sqrt() } else { 0.0 }) / s_abs;
        let k = cd * width * (2.0 * rho_b).sqrt();
        (k * integral, k * dintegral)
    } else {
        (0.0, 0.0)
    };

    // d(net)/d(dp) = d(ṁ_AB)/ddp − d(ṁ_BA)/ddp; both contributions increase
    // net flow with dp. Floor keeps the Jacobian non-singular near stagnation.
    let dnet = (dm_ab + dm_ba).max(1e-10);
    (m_ab, m_ba, dnet)
}

// ─── Network construction ───────────────────────────────────────────────────

/// Effective per-area crack leakage: (coefficient [kg/s/m²/Pa^n], exponent n).
struct EffectiveLeakage {
    wall: (f64, f64),
    window: (f64, f64),
    interzone: (f64, f64),
}

/// Resolve per-area crack leakage from the config (#80).
///
/// ASHRAE leakage classes take precedence; the manual `*_leakage_per_area`
/// fields (with `default_crack_exponent`) remain the fallback.
fn effective_leakage(config: &AirflowNetworkConfig) -> EffectiveLeakage {
    let n = config.default_crack_exponent;
    EffectiveLeakage {
        wall: config
            .wall_leakage_class
            .map(LeakageClass::wall_leakage)
            .unwrap_or((config.wall_leakage_per_area, n)),
        window: config
            .window_leakage_class
            .map(LeakageClass::window_leakage)
            .unwrap_or((config.window_leakage_per_area, n)),
        interzone: config
            .interzone_leakage_class
            .map(LeakageClass::interzone_leakage)
            .unwrap_or((config.interzone_leakage_per_area, n)),
    }
}

/// Build the airflow network from building geometry.
///
/// Auto-generates pressure nodes (one per zone + outdoor) and flow paths
/// (one per exterior/interzone surface) from existing surface/zone data.
///
/// # Arguments
/// * `zones` - All zone states (need centroid_height, temperature)
/// * `surfaces` - All surface states (need area, azimuth, centroid_height, boundary)
/// * `config` - Airflow network configuration
/// * `zone_index` - Zone name → index mapping
/// * `interzone_pairs` - interzone_pairs[i] = Some(j) means surfaces i,j are paired
/// * `airflow_overrides` - Optional per-surface overrides (parallel to surfaces)
/// * `envelope_areas` - Building envelope areas for side ratio estimation
pub fn build_network(
    zones: &[crate::zone::ZoneState],
    surfaces: &[crate::surface::SurfaceState],
    config: &AirflowNetworkConfig,
    zone_index: &std::collections::HashMap<String, usize>,
    interzone_pairs: &[Option<usize>],
    airflow_overrides: &[Option<SurfaceAirflowOverride>],
    envelope_areas: &crate::geometry::EnvelopeAreas,
) -> AirflowNetwork {
    let n_zones = zones.len();

    // Create zone nodes
    let mut nodes: Vec<PressureNode> = zones
        .iter()
        .enumerate()
        .map(|(i, z)| PressureNode {
            zone_index: Some(i),
            ref_height: z.centroid_height,
            pressure: 0.0,
            temperature: z.input.volume.max(1.0) * 0.0 + 293.15, // 20°C default
            density: RHO_REF,
        })
        .collect();

    // Outdoor node
    let outdoor_node = nodes.len();
    nodes.push(PressureNode {
        zone_index: None,
        ref_height: 0.0,
        pressure: 0.0, // set each timestep from wind
        temperature: 293.15,
        density: RHO_REF,
    });

    let zone_to_node: Vec<usize> = (0..n_zones).collect(); // 1:1 mapping

    // Estimate building side ratio from envelope areas
    let side_ratio = estimate_side_ratio(envelope_areas);

    let mut paths = Vec::new();
    let mut scheduled_openings = Vec::new();
    let mut processed_interzone = vec![false; surfaces.len()];
    let leakage = effective_leakage(config);

    for (si, surf) in surfaces.iter().enumerate() {
        let zone_name = &surf.input.zone;
        let zi = match zone_index.get(zone_name) {
            Some(&i) => i,
            None => continue,
        };
        let zone_node = zone_to_node[zi];
        let overrides = airflow_overrides.get(si).and_then(|o| o.as_ref());

        match &surf.input.boundary {
            BoundaryCondition::Outdoor => {
                // Skip floors (slab-on-grade has no air path)
                if surf.input.surface_type == SurfaceType::Floor {
                    continue;
                }

                let height = override_or(overrides, |o| o.opening_height, surf.centroid_height);
                let azimuth = surf.input.azimuth;

                // Check if user wants a large opening
                let is_large_opening = overrides.and_then(|o| o.large_opening).unwrap_or(false);

                if is_large_opening {
                    let frac = overrides.and_then(|o| o.opening_fraction).unwrap_or(1.0);
                    let cd = overrides
                        .and_then(|o| o.discharge_coefficient)
                        .unwrap_or(0.65);
                    let base_area = surf.input.area * frac;
                    if let Some(schedule) = overrides.and_then(|o| o.opening_schedule.clone()) {
                        scheduled_openings.push(ScheduledOpening {
                            path_index: paths.len(),
                            schedule,
                            base_area,
                        });
                    }
                    // Two-way opening (#83): vertical extent from the full
                    // surface (square-equivalent); the opening fraction
                    // narrows the width, matching a sliding sash.
                    let opening_height = surf.input.area.sqrt();
                    paths.push(FlowPath {
                        node_a: outdoor_node,
                        node_b: zone_node,
                        height,
                        cp: 0.0, // updated per timestep
                        azimuth,
                        element: FlowElement::TwoWayOpening {
                            discharge_coefficient: cd,
                            width: base_area / opening_height,
                            height: opening_height,
                        },
                        source_surface: Some(si),
                        mass_flow: 0.0,
                    });
                } else {
                    // Default: power-law crack
                    let (per_area, exponent) = if surf.is_window {
                        leakage.window
                    } else {
                        leakage.wall
                    };
                    let c =
                        override_or(overrides, |o| o.crack_coefficient, per_area * surf.net_area);
                    let n = override_or_val(overrides, |o| o.crack_exponent, exponent);

                    paths.push(FlowPath {
                        node_a: outdoor_node,
                        node_b: zone_node,
                        height,
                        cp: 0.0, // updated per timestep
                        azimuth,
                        element: FlowElement::PowerLawCrack {
                            coefficient: c,
                            exponent: n,
                        },
                        source_surface: Some(si),
                        mass_flow: 0.0,
                    });
                }
            }

            BoundaryCondition::Zone(other_zone) => {
                // Only create one path per interzone pair
                if processed_interzone[si] {
                    continue;
                }
                if let Some(paired) = interzone_pairs[si] {
                    processed_interzone[si] = true;
                    processed_interzone[paired] = true;
                }

                let other_zi = match zone_index.get(other_zone) {
                    Some(&i) => i,
                    None => continue,
                };
                let other_node = zone_to_node[other_zi];
                let height = surf.centroid_height;

                // Interzone doorways (#83) and horizontal openings (#93):
                // `large_opening` turns the surface into a two-way opening.
                // Vertical surfaces resolve buoyancy counterflow over the
                // opening height; horizontal surfaces (floors, ceilings —
                // stairwells, hatches) use the Cooper instability model.
                let is_large_opening = overrides.and_then(|o| o.large_opening).unwrap_or(false);
                if is_large_opening {
                    let frac = overrides.and_then(|o| o.opening_fraction).unwrap_or(1.0);
                    let cd = overrides
                        .and_then(|o| o.discharge_coefficient)
                        .unwrap_or(0.65);
                    let base_area = surf.input.area * frac;
                    if let Some(schedule) = overrides.and_then(|o| o.opening_schedule.clone()) {
                        scheduled_openings.push(ScheduledOpening {
                            path_index: paths.len(),
                            schedule,
                            base_area,
                        });
                    }
                    let is_horizontal = matches!(
                        surf.input.surface_type,
                        SurfaceType::Floor | SurfaceType::Ceiling | SurfaceType::Roof
                    );
                    if is_horizontal {
                        // Orient node_a = lower zone, node_b = upper zone
                        let (low_node, high_node) =
                            if zones[zi].centroid_height <= zones[other_zi].centroid_height {
                                (zone_node, other_node)
                            } else {
                                (other_node, zone_node)
                            };
                        paths.push(FlowPath {
                            node_a: low_node,
                            node_b: high_node,
                            height,
                            cp: 0.0,
                            azimuth: 0.0,
                            element: FlowElement::HorizontalOpening {
                                discharge_coefficient: cd,
                                area: base_area,
                            },
                            source_surface: Some(si),
                            mass_flow: 0.0,
                        });
                        continue;
                    }
                    let opening_height = surf.input.area.sqrt();
                    paths.push(FlowPath {
                        node_a: zone_node,
                        node_b: other_node,
                        height,
                        cp: 0.0,
                        azimuth: 0.0, // irrelevant for interzone
                        element: FlowElement::TwoWayOpening {
                            discharge_coefficient: cd,
                            width: base_area / opening_height,
                            height: opening_height,
                        },
                        source_surface: Some(si),
                        mass_flow: 0.0,
                    });
                    continue;
                }

                let (per_area, exponent) = leakage.interzone;
                let c = override_or(
                    overrides,
                    |o| o.crack_coefficient,
                    per_area * surf.input.area,
                );
                let n = override_or_val(overrides, |o| o.crack_exponent, exponent);

                paths.push(FlowPath {
                    node_a: zone_node,
                    node_b: other_node,
                    height,
                    cp: 0.0,
                    azimuth: 0.0, // irrelevant for interzone
                    element: FlowElement::PowerLawCrack {
                        coefficient: c,
                        exponent: n,
                    },
                    source_surface: Some(si),
                    mass_flow: 0.0,
                });
            }

            // No air paths for ground or adiabatic surfaces
            BoundaryCondition::Ground | BoundaryCondition::Adiabatic => {}
        }
    }

    // Natural ventilation openings (zone-level, not surface-level).
    // The effective area is driven each timestep from the NV availability
    // logic (#88); while the AFN is active the zone-level wind-&-stack
    // model is disabled so the flow is not double-counted.
    let mut nat_vent_path_indices: Vec<Option<usize>> = vec![None; n_zones];
    for (zi, zone) in zones.iter().enumerate() {
        if let Some(ref nv) = zone.input.natural_ventilation {
            let zone_node = zone_to_node[zi];
            nat_vent_path_indices[zi] = Some(paths.len());
            paths.push(FlowPath {
                node_a: outdoor_node,
                node_b: zone_node,
                height: zone.centroid_height + nv.height_difference / 2.0,
                cp: 0.0,
                azimuth: nv.effective_angle,
                element: FlowElement::LargeOpening {
                    discharge_coefficient: nv.discharge_coefficient,
                    area: nv.opening_area,
                },
                source_surface: None,
                mass_flow: 0.0,
            });
        }
    }

    // Exhaust fans (added as FixedFlow, mass_flow updated each timestep)
    for (zi, zone) in zones.iter().enumerate() {
        if zone.input.exhaust_fan.is_some() {
            let zone_node = zone_to_node[zi];
            paths.push(FlowPath {
                node_a: zone_node,
                node_b: outdoor_node,
                height: zone.centroid_height,
                cp: 0.0,
                azimuth: 0.0,
                element: FlowElement::FixedFlow { mass_flow: 0.0 },
                source_surface: None,
                mass_flow: 0.0,
            });
        }
    }

    // HVAC net injection paths: one per zone (outdoor → zone).
    // mass_flow = OA_supply − exhaust, updated each timestep before solving.
    // Positive → pressurisation; negative → depressurisation.
    // Excluded from zone_outdoor_mass_flow so OA isn't double-counted.
    let mut hvac_net_path_indices: Vec<Option<usize>> = vec![None; n_zones];
    for zi in 0..n_zones {
        let zone_node = zone_to_node[zi];
        let path_idx = paths.len();
        paths.push(FlowPath {
            node_a: outdoor_node,
            node_b: zone_node,
            height: zones[zi].centroid_height,
            cp: 0.0,
            azimuth: 0.0,
            element: FlowElement::FixedFlow { mass_flow: 0.0 },
            source_surface: None,
            mass_flow: 0.0,
        });
        hvac_net_path_indices[zi] = Some(path_idx);
    }

    // Duct leakage paths (#82, #85): two directional FixedFlows per zone
    // with duct leakage, updated each timestep before solving.
    //   Supply leak: zone → ambient (supply air spilled into the attic).
    //   Return leak: ambient → zone (unconditioned air ingested by the
    //   return duct and delivered to the zone).
    // Excluded from thermal flow accumulation — the leakage energy is
    // already handled by the duct component (#70) — but species transport
    // (#84) sees them, so attic contaminants can reach conditioned zones.
    let mut duct_supply_leak_path_indices: Vec<Option<usize>> = vec![None; n_zones];
    let mut duct_return_leak_path_indices: Vec<Option<usize>> = vec![None; n_zones];
    for (zi, zone) in zones.iter().enumerate() {
        if let Some(ref dl) = zone.input.duct_leakage {
            let ambient_zi = match zone_index.get(&dl.ambient_zone) {
                Some(&i) if i != zi => i,
                _ => continue, // unknown ambient zone, or ducts in this zone
            };
            let height = zones[ambient_zi].centroid_height;
            let supply_idx = paths.len();
            paths.push(FlowPath {
                node_a: zone_to_node[zi],
                node_b: zone_to_node[ambient_zi],
                height,
                cp: 0.0,
                azimuth: 0.0,
                element: FlowElement::FixedFlow { mass_flow: 0.0 },
                source_surface: None,
                mass_flow: 0.0,
            });
            duct_supply_leak_path_indices[zi] = Some(supply_idx);

            let return_idx = paths.len();
            paths.push(FlowPath {
                node_a: zone_to_node[ambient_zi],
                node_b: zone_to_node[zi],
                height,
                cp: 0.0,
                azimuth: 0.0,
                element: FlowElement::FixedFlow { mass_flow: 0.0 },
                source_surface: None,
                mass_flow: 0.0,
            });
            duct_return_leak_path_indices[zi] = Some(return_idx);
        }
    }

    // Blower-door calibration (#92)
    if let Some(ach50) = config.ach50 {
        let building_volume: f64 = zones.iter().map(|z| z.input.volume).sum();
        let scale = calibrate_ach50(&mut paths, outdoor_node, ach50, building_volume);
        if (scale - 1.0).abs() > 1e-9 {
            log::info!(
                "AFN blower-door calibration: exterior crack coefficients scaled ×{scale:.3} to match ACH50 = {ach50}"
            );
        }
    }

    AirflowNetwork {
        config: config.clone(),
        nodes,
        paths,
        outdoor_node,
        zone_to_node,
        zone_outdoor_mass_flow: vec![0.0; n_zones],
        zone_interzone_flows: vec![Vec::new(); n_zones],
        side_ratio,
        hvac_net_path_indices,
        scheduled_openings,
        duct_supply_leak_path_indices,
        duct_return_leak_path_indices,
        nat_vent_path_indices,
    }
}

/// Blower-door calibration (#92): uniformly scale exterior crack
/// coefficients so the envelope flow at 50 Pa matches the measured ACH50.
///
/// Only exterior power-law cracks participate — large openings and fixed
/// flows are excluded, matching the physical test (windows closed, fans
/// off). Returns the applied scale factor (1.0 if no calibration applied).
fn calibrate_ach50(
    paths: &mut [FlowPath],
    outdoor_node: usize,
    ach50: f64,
    building_volume: f64,
) -> f64 {
    if ach50 <= 0.0 || building_volume <= 0.0 {
        return 1.0;
    }
    // Current envelope mass flow at 50 Pa across exterior cracks
    let mut q50_current = 0.0;
    for path in paths.iter() {
        let is_exterior = path.node_a == outdoor_node || path.node_b == outdoor_node;
        if !is_exterior {
            continue;
        }
        if let FlowElement::PowerLawCrack {
            coefficient,
            exponent,
        } = path.element
        {
            q50_current += coefficient * 50.0_f64.powf(exponent);
        }
    }
    if q50_current <= 0.0 {
        return 1.0;
    }
    // Target mass flow: ACH50 × V × ρ / 3600
    let q50_target = ach50 * building_volume * RHO_REF / 3600.0;
    let scale = q50_target / q50_current;
    for path in paths.iter_mut() {
        let is_exterior = path.node_a == outdoor_node || path.node_b == outdoor_node;
        if !is_exterior {
            continue;
        }
        if let FlowElement::PowerLawCrack {
            ref mut coefficient,
            ..
        } = path.element
        {
            *coefficient *= scale;
        }
    }
    scale
}

/// Estimate building plan side ratio from envelope wall areas.
/// L/W ≈ (N+S wall area) / (E+W wall area) or vice versa.
fn estimate_side_ratio(areas: &crate::geometry::EnvelopeAreas) -> f64 {
    // EnvelopeAreas.wall_area: [N, E, S, W]
    let ns = areas.wall_area[0] + areas.wall_area[2]; // north + south
    let ew = areas.wall_area[1] + areas.wall_area[3]; // east + west
    if ns > 0.0 && ew > 0.0 {
        (ns / ew).max(0.25).min(4.0)
    } else {
        1.0
    }
}

fn override_or<F>(overrides: Option<&SurfaceAirflowOverride>, f: F, default: f64) -> f64
where
    F: Fn(&SurfaceAirflowOverride) -> Option<f64>,
{
    overrides.and_then(f).unwrap_or(default)
}

fn override_or_val<F>(overrides: Option<&SurfaceAirflowOverride>, f: F, default: f64) -> f64
where
    F: Fn(&SurfaceAirflowOverride) -> Option<f64>,
{
    overrides.and_then(f).unwrap_or(default)
}

// ─── Newton-Raphson solver ──────────────────────────────────────────────────

/// Update wind pressures on all exterior flow paths for current conditions.
pub fn update_wind_pressures(
    network: &mut AirflowNetwork,
    _wind_speed_met: f64,
    wind_direction: f64,
    t_outdoor: f64,
    rho_outdoor: f64,
    _terrain: Terrain,
) {
    let outdoor = network.outdoor_node;
    network.nodes[outdoor].temperature = t_outdoor + 273.15;
    network.nodes[outdoor].density = rho_outdoor;

    let cp_model = network.config.cp_model.clone();
    let side_ratio = network.side_ratio;

    for path in &mut network.paths {
        // Only exterior paths (one end is outdoor node)
        let is_exterior = path.node_a == outdoor || path.node_b == outdoor;
        if !is_exterior {
            continue;
        }

        // Compute Cp from wind direction vs surface azimuth
        path.cp = compute_cp(&cp_model, wind_direction, path.azimuth, side_ratio);
    }

    // Update zone node temperatures/densities
    // (caller should set these before calling solve_pressures)
}

/// Solve the pressure network for zone pressures using Newton-Raphson.
///
/// Updates `network.paths[*].mass_flow`, `network.zone_outdoor_mass_flow`,
/// and `network.zone_interzone_flows`.
///
/// # Arguments
/// * `network` - Mutable reference to the assembled network
/// * `wind_speed_met` - Meteorological wind speed [m/s]
/// * `wind_direction` - Wind direction (from) [degrees from north, clockwise]
/// * `t_outdoor` - Outdoor air temperature [°C]
/// * `rho_outdoor` - Outdoor air density [kg/m³]
/// * `terrain` - Site terrain type for wind profile
///
/// # Returns
/// (converged, iterations_taken)
pub fn solve_pressures(
    network: &mut AirflowNetwork,
    wind_speed_met: f64,
    wind_direction: f64,
    t_outdoor: f64,
    rho_outdoor: f64,
    terrain: Terrain,
) -> (bool, usize) {
    // Update wind pressures and outdoor conditions
    update_wind_pressures(
        network,
        wind_speed_met,
        wind_direction,
        t_outdoor,
        rho_outdoor,
        terrain,
    );

    let n_zones = network.zone_to_node.len();
    if n_zones == 0 {
        return (true, 0);
    }

    let outdoor = network.outdoor_node;
    let max_iter = network.config.max_iterations;
    let tol = network.config.convergence_tolerance;
    let adaptive = network.config.adaptive_damping;
    let rel_tol = network.config.relative_tolerance;
    let weather_wind_mod_coeff = crate::convection::DEFAULT_WEATHER_WIND_MOD_COEFF;

    // Initialize zone pressures to 0 (or keep from previous timestep)
    // Outdoor node pressure is always 0 (gauge reference).
    network.nodes[outdoor].pressure = 0.0;

    let mut converged = false;
    let mut iter = 0;

    // Adaptive under-relaxation (#86): grow the Newton step toward a full
    // step while the residual falls, cut it when the residual rises
    // (overshoot/oscillation). Mirrors the E+ AIRNET strategy.
    let mut damp = network.config.damping;
    let mut prev_residual = f64::INFINITY;
    let afn_debug = std::env::var("AFN_DEBUG").is_ok();

    for _it in 0..max_iter {
        iter = _it + 1;

        // Build residual vector R[zone_i] = sum of mass flows into zone i
        let mut residual = vec![0.0; n_zones];
        // Dense Jacobian J[i][j] = dR_i / dP_j
        let mut jacobian = vec![vec![0.0; n_zones]; n_zones];
        // Total through-flow magnitude for the relative convergence test
        let mut total_flow = 0.0_f64;

        for path in &network.paths {
            let a = path.node_a;
            let b = path.node_b;

            // Compute total pressure difference: P_a - P_b + stack + wind
            let dp = compute_path_dp(
                path,
                &network.nodes,
                outdoor,
                wind_speed_met,
                terrain,
                weather_wind_mod_coeff,
            );

            let rho_a = network.nodes[a].density;
            let rho_b = network.nodes[b].density;

            let (flow, dflow_ddp) = flow_and_derivative(&path.element, dp, rho_a, rho_b);
            total_flow += flow.abs();

            // flow > 0 means A → B: leaves A, enters B
            let a_is_zone = network.nodes[a].zone_index;
            let b_is_zone = network.nodes[b].zone_index;

            // Node A: loses flow (flow leaves A)
            if let Some(zi_a) = a_is_zone {
                residual[zi_a] -= flow;
                // dR_a/dP_a: dp increases → more flow A→B → R_a decreases
                jacobian[zi_a][zi_a] -= dflow_ddp;
                // dR_a/dP_b: dp = Pa - Pb, so dP_b → -dflow_ddp on dp → R_a increases
                if let Some(zi_b) = b_is_zone {
                    jacobian[zi_a][zi_b] += dflow_ddp;
                }
            }

            // Node B: gains flow (flow enters B)
            if let Some(zi_b) = b_is_zone {
                residual[zi_b] += flow;
                // dR_b/dP_b: dp = Pa - Pb, so increased Pb decreases dp,
                // which decreases flow, which decreases R_b
                jacobian[zi_b][zi_b] -= dflow_ddp;
                if let Some(zi_a) = a_is_zone {
                    jacobian[zi_b][zi_a] += dflow_ddp;
                }
            }
        }

        // Dual convergence test (#86): absolute (scaled to mass-flow units,
        // ~0.1 Pa × typical flow sensitivity) OR relative to the network's
        // total through-flow, so large open-window flows aren't held to the
        // tight-crack absolute tolerance. Mirrors E+'s dual criterion.
        let max_residual = residual.iter().map(|r| r.abs()).fold(0.0_f64, f64::max);
        let abs_ok = max_residual < tol * RHO_REF * 0.001;
        let rel_ok = total_flow > 0.0 && max_residual < rel_tol * total_flow;
        if abs_ok || rel_ok {
            converged = true;
            break;
        }

        if afn_debug {
            eprintln!(
                "iter {iter}: max_residual={max_residual:.6}, total_flow={total_flow:.4}, damp={damp:.3}, P={:?}",
                network.zone_to_node.iter().map(|&n| network.nodes[n].pressure).collect::<Vec<_>>()
            );
        }
        // Adaptive under-relaxation (#86)
        if adaptive {
            if max_residual > prev_residual {
                damp = (damp * 0.5).max(0.1);
            } else if max_residual < 0.3 * prev_residual {
                damp = (damp * 1.5).min(1.0);
            }
        }
        prev_residual = max_residual;

        // Solve J × δP = -R using Gaussian elimination with partial pivoting
        let delta_p =
            solve_linear_system(&jacobian, &residual.iter().map(|r| -r).collect::<Vec<_>>());

        if let Some(dp_vec) = delta_p {
            let max_dp = dp_vec.iter().map(|d| d.abs()).fold(0.0_f64, f64::max);
            for zi in 0..n_zones {
                let node = &mut network.nodes[network.zone_to_node[zi]];
                node.pressure += damp * dp_vec[zi];
            }

            // Pressure-update convergence: only trust a small Newton step if
            // the mass residual is also in the right neighborhood (#86).
            // Near the |ΔP| → 0 orifice singularity the Jacobian is steep,
            // so steps can be tiny while the balance is still far off.
            let residual_near =
                max_residual < 10.0 * (tol * RHO_REF * 0.001).max(rel_tol * total_flow);
            if max_dp < tol && residual_near {
                converged = true;
                break;
            }
        } else {
            // Singular Jacobian — shouldn't happen with proper network
            break;
        }
    }

    // Post-solve: compute final mass flows and accumulate per-zone results
    post_solve(network, wind_speed_met, terrain);

    (converged, iter)
}

/// Compute total pressure difference across a flow path [Pa].
///
/// ΔP = P_a - P_b + stack_a - stack_b + wind_a - wind_b
///
/// Stack pressure at height h: ΔP_stack = -ρ g (h - h_ref)
/// Wind pressure (outdoor only): P_wind = 0.5 ρ Cp V²
fn compute_path_dp(
    path: &FlowPath,
    nodes: &[PressureNode],
    outdoor: usize,
    wind_speed_met: f64,
    terrain: Terrain,
    weather_wind_mod_coeff: f64,
) -> f64 {
    let a = &nodes[path.node_a];
    let b = &nodes[path.node_b];

    let mut dp = a.pressure - b.pressure;

    // Stack effect: pressure difference due to height difference between
    // path midpoint and each node's reference height.
    // Using Boussinesq approximation: ΔP_stack = ρ₀ g (1/T_a - 1/T_b)(h_path - h_ref)
    // Simplified: ΔP = -ρ_a g (h - h_ref_a) + ρ_b g (h - h_ref_b)
    let _rho_avg = 0.5 * (a.density + b.density);
    let h = path.height;
    dp += -a.density * G * (h - a.ref_height) + b.density * G * (h - b.ref_height);

    // Wind pressure (only on paths connected to outdoor)
    if path.node_a == outdoor {
        // Wind at path height
        let v_local = crate::convection::wind_speed_at_height(
            wind_speed_met,
            h.max(0.5),
            weather_wind_mod_coeff,
            terrain.wind_exp(),
            terrain.wind_bl_height(),
        );
        dp += 0.5 * a.density * path.cp * v_local * v_local;
    }
    if path.node_b == outdoor {
        let v_local = crate::convection::wind_speed_at_height(
            wind_speed_met,
            h.max(0.5),
            weather_wind_mod_coeff,
            terrain.wind_exp(),
            terrain.wind_bl_height(),
        );
        dp -= 0.5 * b.density * path.cp * v_local * v_local;
    }

    dp
}

/// Post-solve: recompute final mass flows and accumulate per-zone results.
fn post_solve(network: &mut AirflowNetwork, wind_speed_met: f64, terrain: Terrain) {
    let outdoor = network.outdoor_node;
    let n_zones = network.zone_to_node.len();
    let weather_wind_mod_coeff = crate::convection::DEFAULT_WEATHER_WIND_MOD_COEFF;

    // Build a fast lookup for HVAC net and duct leakage paths so we can
    // exclude them from flow accumulation (HVAC OA is already tracked in
    // zone.outdoor_air_mass_flow; duct leakage energy is handled by the
    // duct component, #70).
    let excluded_path_set: std::collections::HashSet<usize> = network
        .hvac_net_path_indices
        .iter()
        .chain(network.duct_supply_leak_path_indices.iter())
        .chain(network.duct_return_leak_path_indices.iter())
        .filter_map(|&idx| idx)
        .collect();

    // Reset accumulators
    for i in 0..n_zones {
        network.zone_outdoor_mass_flow[i] = 0.0;
        network.zone_interzone_flows[i].clear();
    }

    // Compute final flows on each path
    for (pi, path) in network.paths.iter_mut().enumerate() {
        let dp = compute_path_dp(
            path,
            &network.nodes,
            outdoor,
            wind_speed_met,
            terrain,
            weather_wind_mod_coeff,
        );
        let rho_a = network.nodes[path.node_a].density;
        let rho_b = network.nodes[path.node_b].density;
        let (flow, _) = flow_and_derivative(&path.element, dp, rho_a, rho_b);
        path.mass_flow = flow;

        // HVAC net injection and duct leakage paths drive zone pressure but
        // their mass/energy content is accounted for elsewhere — skip accumulation.
        if excluded_path_set.contains(&pi) {
            continue;
        }

        // Accumulate into zone results
        let a_zone = network.nodes[path.node_a].zone_index;
        let b_zone = network.nodes[path.node_b].zone_index;
        let a_is_outdoor = path.node_a == outdoor;
        let b_is_outdoor = path.node_b == outdoor;

        // Two-way openings (#83) and horizontal openings (#93): record BOTH
        // directional components, not just the net — a doorway or stairwell
        // exchanges air in both directions simultaneously, and an open
        // window admits outdoor air even at net outflow.
        let two_way = match path.element {
            FlowElement::TwoWayOpening { .. } => {
                Some(two_way_opening_flows(&path.element, dp, rho_a, rho_b))
            }
            FlowElement::HorizontalOpening { .. } => {
                Some(horizontal_opening_flows(&path.element, dp, rho_a, rho_b))
            }
            _ => None,
        };
        if let Some((m_ab, m_ba, _)) = two_way {
            if m_ab > 0.0 {
                if let Some(zi_b) = b_zone {
                    if a_is_outdoor {
                        network.zone_outdoor_mass_flow[zi_b] += m_ab;
                    } else if let Some(zi_a) = a_zone {
                        network.zone_interzone_flows[zi_b].push((zi_a, m_ab));
                    }
                }
            }
            if m_ba > 0.0 {
                if let Some(zi_a) = a_zone {
                    if b_is_outdoor {
                        network.zone_outdoor_mass_flow[zi_a] += m_ba;
                    } else if let Some(zi_b) = b_zone {
                        network.zone_interzone_flows[zi_a].push((zi_b, m_ba));
                    }
                }
            }
            continue;
        }

        // flow > 0 means A → B
        if flow > 0.0 {
            // Flow enters B
            if let Some(zi_b) = b_zone {
                if a_is_outdoor {
                    network.zone_outdoor_mass_flow[zi_b] += flow;
                } else if let Some(zi_a) = a_zone {
                    network.zone_interzone_flows[zi_b].push((zi_a, flow));
                }
            }
        } else if flow < 0.0 {
            // Flow enters A (flow is negative A→B, so |flow| enters A)
            if let Some(zi_a) = a_zone {
                if b_is_outdoor {
                    network.zone_outdoor_mass_flow[zi_a] += -flow;
                } else if let Some(zi_b) = b_zone {
                    network.zone_interzone_flows[zi_a].push((zi_b, -flow));
                }
            }
        }
    }
}

// ─── Linear algebra ─────────────────────────────────────────────────────────

/// Solve A × x = b using Gaussian elimination with partial pivoting.
///
/// For N < 100 zones, this dense O(N³) solver is more than adequate.
/// Returns None if the matrix is singular.
pub(crate) fn solve_linear_system(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    if n == 0 {
        return Some(vec![]);
    }

    // Augmented matrix [A | b]
    let mut aug: Vec<Vec<f64>> = a
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.push(b[i]);
            r
        })
        .collect();

    // Forward elimination with partial pivoting
    for col in 0..n {
        // Find pivot
        let mut max_val = aug[col][col].abs();
        let mut max_row = col;
        for row in (col + 1)..n {
            let v = aug[row][col].abs();
            if v > max_val {
                max_val = v;
                max_row = row;
            }
        }

        if max_val < 1e-15 {
            return None; // singular
        }

        // Swap rows
        if max_row != col {
            aug.swap(col, max_row);
        }

        // Eliminate
        let pivot = aug[col][col];
        for row in (col + 1)..n {
            let factor = aug[row][col] / pivot;
            for j in col..=n {
                aug[row][j] -= factor * aug[col][j];
            }
        }
    }

    // Back substitution
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = aug[i][n];
        for j in (i + 1)..n {
            sum -= aug[i][j] * x[j];
        }
        x[i] = sum / aug[i][i];
    }

    Some(x)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_power_law_crack_flow() {
        // Q = C × |ΔP|^n, C = 0.001 kg/s/Pa^0.65, n = 0.65, ΔP = 4 Pa
        // Q = 0.001 × 4^0.65 = 0.001 × 2.6390 ≈ 0.002639
        let elem = FlowElement::PowerLawCrack {
            coefficient: 0.001,
            exponent: 0.65,
        };
        let (flow, dflow) = flow_and_derivative(&elem, 4.0, RHO_REF, RHO_REF);
        let expected = 0.001 * 4.0_f64.powf(0.65);
        assert_relative_eq!(flow, expected, max_relative = 1e-6);
        assert!(dflow > 0.0);

        // Negative ΔP → negative flow
        let (flow_neg, _) = flow_and_derivative(&elem, -4.0, RHO_REF, RHO_REF);
        assert_relative_eq!(flow_neg, -expected, max_relative = 1e-6);
    }

    #[test]
    fn test_large_opening_flow() {
        // Q = Cd × A × sqrt(2ρ|ΔP|), Cd = 0.65, A = 1.0, ρ = 1.2, ΔP = 2 Pa
        // Q = 0.65 × 1.0 × sqrt(2 × 1.2 × 2) = 0.65 × sqrt(4.8) = 0.65 × 2.1909 ≈ 1.4241
        let elem = FlowElement::LargeOpening {
            discharge_coefficient: 0.65,
            area: 1.0,
        };
        let (flow, _) = flow_and_derivative(&elem, 2.0, 1.2, 1.2);
        let expected = 0.65 * (2.0 * 1.2 * 2.0_f64).sqrt();
        assert_relative_eq!(flow, expected, max_relative = 1e-6);
    }

    #[test]
    fn test_fixed_flow_element() {
        let elem = FlowElement::FixedFlow { mass_flow: 0.5 };
        let (flow, dflow) = flow_and_derivative(&elem, 100.0, 1.2, 1.2);
        assert_eq!(flow, 0.5);
        assert_eq!(dflow, 0.0);
    }

    #[test]
    fn test_wind_surface_angle() {
        // Wind from north (0°), surface facing south (180°) → θ = 180°
        assert_relative_eq!(wind_surface_angle(0.0, 180.0), 180.0, max_relative = 1e-10);
        // Wind from north, surface facing north → θ = 0°
        assert_relative_eq!(wind_surface_angle(0.0, 0.0), 0.0, max_relative = 1e-10);
        // Wind from east (90°), surface facing south (180°) → θ = 90°
        assert_relative_eq!(wind_surface_angle(90.0, 180.0), 90.0, max_relative = 1e-10);
        // Wind from 350°, surface facing 10° → θ = 20°
        assert_relative_eq!(wind_surface_angle(350.0, 10.0), 20.0, max_relative = 1e-10);
    }

    #[test]
    fn test_cp_swami_chandra_windward() {
        // Windward face (θ=0) should have positive Cp (~0.6)
        let cp = cp_swami_chandra(0.0, 1.0);
        assert!(cp > 0.4, "Windward Cp should be positive, got {cp}");
        assert!(cp < 0.8, "Windward Cp should be < 0.8, got {cp}");
    }

    #[test]
    fn test_cp_swami_chandra_leeward() {
        // Leeward face (θ=180) should have negative Cp
        let cp = cp_swami_chandra(180.0, 1.0);
        assert!(cp < 0.0, "Leeward Cp should be negative, got {cp}");
    }

    #[test]
    fn test_cp_swami_chandra_side_ratio_at_90deg() {
        // At θ=90° the side-ratio (G = ln(side_ratio)) terms are active — the
        // sin(θ/2)-weighted terms don't vanish as they do at θ=0/180. This
        // exercises the #63 fix (G = ln(side_ratio), not the raw ratio).
        // Hand-computed from Swami & Chandra Eq. 1 with side_ratio = 2:
        //   G = ln(2) = 0.693147,  Cp = 0.6·ln(0.567569) ≈ -0.33974.
        let cp_sr2 = cp_swami_chandra(90.0, 2.0);
        assert_relative_eq!(cp_sr2, -0.33974, epsilon = 1e-3);

        // With side_ratio = 1, G = 0 so both side-ratio terms drop out, giving a
        // distinctly different value (≈ -0.44272). If the side ratio were applied
        // raw (or via the old ln().exp() identity) these would not match theory.
        let cp_sr1 = cp_swami_chandra(90.0, 1.0);
        assert_relative_eq!(cp_sr1, -0.44272, epsilon = 1e-3);
        assert!(
            (cp_sr2 - cp_sr1).abs() > 0.05,
            "side ratio must change Cp at θ=90°: sr1={cp_sr1}, sr2={cp_sr2}"
        );
    }

    #[test]
    fn test_cp_high_rise() {
        // Windward
        assert_relative_eq!(cp_high_rise(0.0), 0.6, max_relative = 1e-10);
        // Leeward
        assert_relative_eq!(cp_high_rise(180.0), -0.3, max_relative = 1e-10);
        // Side (90°)
        assert_relative_eq!(cp_high_rise(90.0), 0.0, epsilon = 0.01);
    }

    #[test]
    fn test_linear_solver_2x2() {
        // 2x + y = 5, x + 3y = 10 → x = 1, y = 3
        let a = vec![vec![2.0, 1.0], vec![1.0, 3.0]];
        let b = vec![5.0, 10.0];
        let x = solve_linear_system(&a, &b).unwrap();
        assert_relative_eq!(x[0], 1.0, max_relative = 1e-10);
        assert_relative_eq!(x[1], 3.0, max_relative = 1e-10);
    }

    #[test]
    fn test_linear_solver_singular() {
        // Singular matrix: rows are linearly dependent
        let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let b = vec![3.0, 6.0];
        assert!(solve_linear_system(&a, &b).is_none());
    }

    #[test]
    fn test_single_zone_crack_analytic() {
        // Single zone connected to outdoor by one crack.
        // No wind, no stack effect (same height).
        // Exhaust fan removes 0.1 kg/s from zone.
        // Crack: C = 0.01, n = 0.65
        // At steady state: crack flow = 0.1 kg/s into zone.
        // Q = C × |ΔP|^n → |ΔP| = (Q/C)^(1/n) = (0.1/0.01)^(1/0.65)
        // = 10^(1.538) = 34.5 Pa (zone is negative)

        let config = AirflowNetworkConfig {
            enabled: true,
            max_iterations: 50,
            convergence_tolerance: 0.01,
            damping: 0.75,
            ..Default::default()
        };

        let mut network = AirflowNetwork {
            config,
            nodes: vec![
                PressureNode {
                    // zone 0
                    zone_index: Some(0),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
                PressureNode {
                    // outdoor
                    zone_index: None,
                    ref_height: 1.5, // same height to eliminate stack
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
            ],
            paths: vec![
                FlowPath {
                    // crack from outdoor to zone
                    node_a: 1, // outdoor
                    node_b: 0, // zone
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.01,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                FlowPath {
                    // exhaust fan: zone → outdoor, 0.1 kg/s
                    node_a: 0, // zone
                    node_b: 1, // outdoor
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::FixedFlow { mass_flow: 0.1 },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node: 1,
            zone_to_node: vec![0],
            zone_outdoor_mass_flow: vec![0.0],
            zone_interzone_flows: vec![vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![],
            duct_return_leak_path_indices: vec![],
            nat_vent_path_indices: vec![],
        };

        let (converged, _iters) =
            solve_pressures(&mut network, 0.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);

        assert!(converged, "Solver should converge");

        // Zone should be at negative pressure (exhaust pulls air out)
        let p_zone = network.nodes[0].pressure;
        assert!(
            p_zone < 0.0,
            "Zone pressure should be negative, got {p_zone}"
        );

        // Crack inflow should balance exhaust
        let outdoor_flow = network.zone_outdoor_mass_flow[0];
        assert_relative_eq!(outdoor_flow, 0.1, max_relative = 0.05);
    }

    #[test]
    fn test_mass_conservation() {
        // Two zones connected by crack, both connected to outdoor.
        // Zone 0 has exhaust fan (0.05 kg/s), zone 1 does not.
        // At steady state, sum of flows at each zone node should be ~0.

        let config = AirflowNetworkConfig {
            enabled: true,
            max_iterations: 50,
            convergence_tolerance: 0.001,
            damping: 0.75,
            ..Default::default()
        };

        let mut network = AirflowNetwork {
            config,
            nodes: vec![
                PressureNode {
                    zone_index: Some(0),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
                PressureNode {
                    zone_index: Some(1),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
                PressureNode {
                    zone_index: None,
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
            ],
            paths: vec![
                // Outdoor → Zone 0
                FlowPath {
                    node_a: 2,
                    node_b: 0,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 180.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.005,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Outdoor → Zone 1
                FlowPath {
                    node_a: 2,
                    node_b: 1,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.005,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Zone 0 → Zone 1 (interzone crack)
                FlowPath {
                    node_a: 0,
                    node_b: 1,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.003,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Exhaust fan: Zone 0 → outdoor
                FlowPath {
                    node_a: 0,
                    node_b: 2,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::FixedFlow { mass_flow: 0.05 },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node: 2,
            zone_to_node: vec![0, 1],
            zone_outdoor_mass_flow: vec![0.0, 0.0],
            zone_interzone_flows: vec![vec![], vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None, None],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![],
            duct_return_leak_path_indices: vec![],
            nat_vent_path_indices: vec![],
        };

        let (converged, _) =
            solve_pressures(&mut network, 0.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);
        assert!(converged);

        // Check mass conservation at each zone node
        let mut zone_balance = [0.0; 2];
        for path in &network.paths {
            let a_zone = network.nodes[path.node_a].zone_index;
            let b_zone = network.nodes[path.node_b].zone_index;
            if let Some(zi) = a_zone {
                zone_balance[zi] -= path.mass_flow;
            }
            if let Some(zi) = b_zone {
                zone_balance[zi] += path.mass_flow;
            }
        }

        for (zi, balance) in zone_balance.iter().enumerate() {
            assert!(
                balance.abs() < 0.001,
                "Zone {zi} mass balance should be ~0, got {balance}"
            );
        }

        // Zone 0 should be negative pressure (exhaust)
        assert!(
            network.nodes[0].pressure < network.nodes[1].pressure,
            "Zone with exhaust should have lower pressure"
        );
    }

    #[test]
    fn test_stack_effect_direction() {
        // Two-story building: zone 0 (ground floor) and zone 1 (upper floor).
        // Both connected to outdoor at their respective heights.
        // Zone 1 is warmer than zone 0.
        // Stack effect: warm air should flow out at top, cold air in at bottom.

        let config = AirflowNetworkConfig {
            enabled: true,
            max_iterations: 50,
            convergence_tolerance: 0.001,
            damping: 0.75,
            ..Default::default()
        };

        let mut network = AirflowNetwork {
            config,
            nodes: vec![
                PressureNode {
                    zone_index: Some(0),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: 1.205,
                }, // 20°C
                PressureNode {
                    zone_index: Some(1),
                    ref_height: 4.5,
                    pressure: 0.0,
                    temperature: 303.15,
                    density: 1.165,
                }, // 30°C
                PressureNode {
                    zone_index: None,
                    ref_height: 0.0,
                    pressure: 0.0,
                    temperature: 273.15,
                    density: 1.292,
                }, // 0°C outdoor
            ],
            paths: vec![
                // Outdoor → Zone 0 (low crack at 1.0 m)
                FlowPath {
                    node_a: 2,
                    node_b: 0,
                    height: 1.0,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.005,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Outdoor → Zone 1 (high crack at 5.0 m)
                FlowPath {
                    node_a: 2,
                    node_b: 1,
                    height: 5.0,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.005,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Zone 0 → Zone 1 (interzone at 3.0 m)
                FlowPath {
                    node_a: 0,
                    node_b: 1,
                    height: 3.0,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.003,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node: 2,
            zone_to_node: vec![0, 1],
            zone_outdoor_mass_flow: vec![0.0, 0.0],
            zone_interzone_flows: vec![vec![], vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None, None],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![],
            duct_return_leak_path_indices: vec![],
            nat_vent_path_indices: vec![],
        };

        let (converged, _) = solve_pressures(&mut network, 0.0, 0.0, 0.0, 1.292, Terrain::Country);
        assert!(converged);

        // Cold air enters at bottom (outdoor → zone 0): positive flow on path 0
        let bottom_flow = network.paths[0].mass_flow;
        assert!(
            bottom_flow > 0.0,
            "Cold air should enter at bottom, got flow = {bottom_flow}"
        );

        // Warm air exits at top (zone 1 → outdoor): negative flow on path 1
        // (outdoor→zone1 path, so negative means zone1→outdoor)
        let top_flow = network.paths[1].mass_flow;
        assert!(
            top_flow < 0.0,
            "Warm air should exit at top, got flow = {top_flow}"
        );
    }

    #[test]
    fn test_jacobian_numerical() {
        // Verify analytical Jacobian against finite-difference for a simple
        // power-law element.
        let elem = FlowElement::PowerLawCrack {
            coefficient: 0.005,
            exponent: 0.65,
        };
        let dp = 3.0;
        let eps = 1e-6;
        let rho = RHO_REF;

        let (_, dflow_analytical) = flow_and_derivative(&elem, dp, rho, rho);
        let (f_plus, _) = flow_and_derivative(&elem, dp + eps, rho, rho);
        let (f_minus, _) = flow_and_derivative(&elem, dp - eps, rho, rho);
        let dflow_numerical = (f_plus - f_minus) / (2.0 * eps);

        assert_relative_eq!(dflow_analytical, dflow_numerical, max_relative = 1e-4);
    }

    #[test]
    fn test_estimate_side_ratio() {
        let areas = crate::geometry::EnvelopeAreas {
            wall_area: [100.0, 50.0, 100.0, 50.0], // N, E, S, W
            window_area: [0.0, 0.0, 0.0, 0.0],
        };
        let sr = estimate_side_ratio(&areas);
        assert_relative_eq!(sr, 2.0, max_relative = 1e-10);
    }

    /// ASHRAE leakage classes (#80): coefficients grow monotonically from
    /// tight (A) to very leaky (D) for every surface category.
    #[test]
    fn test_leakage_class_monotonic() {
        let classes = [
            LeakageClass::A,
            LeakageClass::B,
            LeakageClass::C,
            LeakageClass::D,
        ];
        for pair in classes.windows(2) {
            let (tighter, leakier) = (pair[0], pair[1]);
            assert!(tighter.wall_leakage().0 < leakier.wall_leakage().0);
            assert!(tighter.window_leakage().0 < leakier.window_leakage().0);
            assert!(tighter.interzone_leakage().0 < leakier.interzone_leakage().0);
        }
        // Crack exponent is 0.65 across the table
        for class in classes {
            assert_eq!(class.wall_leakage().1, 0.65);
            assert_eq!(class.window_leakage().1, 0.65);
            assert_eq!(class.interzone_leakage().1, 0.65);
        }
    }

    /// Leakage classes override the manual per-area fields; without a class,
    /// the manual values (with the default exponent) remain in effect.
    #[test]
    fn test_leakage_class_overrides_manual_values() {
        let manual = AirflowNetworkConfig {
            wall_leakage_per_area: 0.0005,
            window_leakage_per_area: 0.0007,
            interzone_leakage_per_area: 0.0009,
            default_crack_exponent: 0.6,
            ..Default::default()
        };
        let eff = effective_leakage(&manual);
        assert_eq!(eff.wall, (0.0005, 0.6));
        assert_eq!(eff.window, (0.0007, 0.6));
        assert_eq!(eff.interzone, (0.0009, 0.6));

        let classed = AirflowNetworkConfig {
            wall_leakage_class: Some(LeakageClass::C),
            window_leakage_class: Some(LeakageClass::B),
            interzone_leakage_class: Some(LeakageClass::D),
            ..manual
        };
        let eff = effective_leakage(&classed);
        assert_eq!(eff.wall, LeakageClass::C.wall_leakage());
        assert_eq!(eff.window, LeakageClass::B.window_leakage());
        assert_eq!(eff.interzone, LeakageClass::D.interzone_leakage());
    }

    /// Leakage classes parse from YAML as either letters or descriptive names.
    #[test]
    fn test_leakage_class_yaml_aliases() {
        let yaml = "enabled: true\nwall_leakage_class: b\nwindow_leakage_class: tight\ninterzone_leakage_class: very_leaky\n";
        let config: AirflowNetworkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.wall_leakage_class, Some(LeakageClass::B));
        assert_eq!(config.window_leakage_class, Some(LeakageClass::A));
        assert_eq!(config.interzone_leakage_class, Some(LeakageClass::D));
    }

    fn sample_cp_table() -> CpTable {
        // Symmetric two-facade table, 45° steps.
        CpTable {
            wind_angles: vec![0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0],
            facades: vec![
                CpFacade {
                    azimuth: 0.0, // north
                    cp: vec![0.6, 0.35, -0.4, -0.45, -0.3, -0.45, -0.4, 0.35],
                },
                CpFacade {
                    azimuth: 180.0, // south
                    cp: vec![-0.3, -0.45, -0.4, 0.35, 0.6, 0.35, -0.4, -0.45],
                },
            ],
        }
    }

    /// Cp table lookup (#81): exact angles, linear interpolation between
    /// tabulated angles, and wraparound past the last entry.
    #[test]
    fn test_cp_table_interpolation() {
        let table = sample_cp_table();

        // Exact tabulated angles
        assert_relative_eq!(cp_from_table(&table, 0.0, 0.0), 0.6, epsilon = 1e-12);
        assert_relative_eq!(cp_from_table(&table, 180.0, 0.0), -0.3, epsilon = 1e-12);

        // Midpoint between 0° and 45°: (0.6 + 0.35) / 2
        assert_relative_eq!(cp_from_table(&table, 22.5, 0.0), 0.475, epsilon = 1e-12);

        // Wraparound: 337.5° is midway between 315° (0.35) and 360°→0° (0.6)
        assert_relative_eq!(cp_from_table(&table, 337.5, 0.0), 0.475, epsilon = 1e-12);

        // Negative wind directions normalize into [0, 360)
        assert_relative_eq!(
            cp_from_table(&table, -22.5, 0.0),
            cp_from_table(&table, 337.5, 0.0),
            epsilon = 1e-12
        );
    }

    /// Cp table facade selection (#81): a surface uses the facade whose
    /// azimuth is nearest its own, with compass wraparound.
    #[test]
    fn test_cp_table_facade_selection() {
        let table = sample_cp_table();

        // South-facing surface picks the south facade curve
        assert_relative_eq!(cp_from_table(&table, 180.0, 180.0), 0.6, epsilon = 1e-12);

        // 350° is nearer north (0°) than south (180°) across the wrap
        assert_relative_eq!(cp_from_table(&table, 0.0, 350.0), 0.6, epsilon = 1e-12);

        // Empty table degrades to 0.0
        let empty = CpTable {
            wind_angles: vec![],
            facades: vec![],
        };
        assert_eq!(cp_from_table(&empty, 90.0, 0.0), 0.0);
    }

    /// Cp model parses from YAML both as a correlation name and as a table.
    #[test]
    fn test_cp_model_yaml_parse() {
        let config: AirflowNetworkConfig =
            serde_yaml::from_str("enabled: true\ncp_model: swami_chandra\n").unwrap();
        assert_eq!(config.cp_model, CpModel::SwamiChandra);

        // Variant with data uses YAML tag syntax, like `boundary: !zone`.
        let yaml = "
enabled: true
cp_model: !table
  wind_angles: [0, 90, 180, 270]
  facades:
    - azimuth: 0
      cp: [0.6, -0.4, -0.3, -0.4]
";
        let config: AirflowNetworkConfig = serde_yaml::from_str(yaml).unwrap();
        match config.cp_model {
            CpModel::Table(ref table) => {
                assert_eq!(table.wind_angles.len(), 4);
                assert_eq!(table.facades.len(), 1);
                assert_relative_eq!(table.facades[0].cp[0], 0.6, epsilon = 1e-12);
            }
            ref other => panic!("expected CpModel::Table, got {other:?}"),
        }
    }

    /// Cp table drives path Cp values inside the solver's Cp update loop.
    #[test]
    fn test_cp_table_in_wind_pressure_update() {
        let config = AirflowNetworkConfig {
            cp_model: CpModel::Table(sample_cp_table()),
            ..Default::default()
        };
        let mut network = AirflowNetwork {
            config,
            nodes: vec![
                PressureNode {
                    zone_index: Some(0),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
                PressureNode {
                    zone_index: None,
                    ref_height: 0.0,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
            ],
            paths: vec![
                FlowPath {
                    node_a: 1,
                    node_b: 0,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0, // north facade
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.001,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                FlowPath {
                    node_a: 1,
                    node_b: 0,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 180.0, // south facade
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.001,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node: 1,
            zone_to_node: vec![0],
            zone_outdoor_mass_flow: vec![0.0],
            zone_interzone_flows: vec![vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![],
            duct_return_leak_path_indices: vec![],
            nat_vent_path_indices: vec![],
        };

        // Wind from north: north facade windward (0.6), south leeward (-0.3)
        update_wind_pressures(&mut network, 4.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);
        assert_relative_eq!(network.paths[0].cp, 0.6, epsilon = 1e-12);
        assert_relative_eq!(network.paths[1].cp, -0.3, epsilon = 1e-12);

        // Wind from south: roles reverse
        update_wind_pressures(&mut network, 4.0, 180.0, 20.0, RHO_REF, Terrain::Suburbs);
        assert_relative_eq!(network.paths[0].cp, -0.3, epsilon = 1e-12);
        assert_relative_eq!(network.paths[1].cp, 0.6, epsilon = 1e-12);
    }

    /// Schedule-driven opening fraction (#79): the effective opening area
    /// follows the schedule fraction, and a fraction of 0 closes the opening.
    #[test]
    fn test_scheduled_opening_area_update() {
        let mut network = AirflowNetwork {
            config: AirflowNetworkConfig::default(),
            nodes: vec![
                PressureNode {
                    zone_index: Some(0),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
                PressureNode {
                    zone_index: None,
                    ref_height: 0.0,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
            ],
            paths: vec![FlowPath {
                node_a: 1,
                node_b: 0,
                height: 1.5,
                cp: 0.0,
                azimuth: 180.0,
                element: FlowElement::LargeOpening {
                    discharge_coefficient: 0.65,
                    area: 2.0,
                },
                source_surface: Some(0),
                mass_flow: 0.0,
            }],
            outdoor_node: 1,
            zone_to_node: vec![0],
            zone_outdoor_mass_flow: vec![0.0],
            zone_interzone_flows: vec![vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None],
            scheduled_openings: vec![ScheduledOpening {
                path_index: 0,
                schedule: "window_opening".to_string(),
                base_area: 2.0,
            }],
            duct_supply_leak_path_indices: vec![],
            duct_return_leak_path_indices: vec![],
            nat_vent_path_indices: vec![],
        };

        // Half-open
        network.update_scheduled_openings(|name| {
            assert_eq!(name, "window_opening");
            0.5
        });
        match network.paths[0].element {
            FlowElement::LargeOpening { area, .. } => {
                assert_relative_eq!(area, 1.0, max_relative = 1e-12)
            }
            _ => panic!("expected LargeOpening"),
        }

        // Closed: zero area → zero flow at any ΔP
        network.update_scheduled_openings(|_| 0.0);
        match network.paths[0].element {
            FlowElement::LargeOpening { area, .. } => assert_eq!(area, 0.0),
            _ => panic!("expected LargeOpening"),
        }
        let (flow, dflow) = flow_and_derivative(&network.paths[0].element, 10.0, RHO_REF, RHO_REF);
        assert_eq!(flow, 0.0);
        assert_eq!(dflow, 0.0);

        // Schedule values outside [0,1] are clamped
        network.update_scheduled_openings(|_| 1.7);
        match network.paths[0].element {
            FlowElement::LargeOpening { area, .. } => {
                assert_relative_eq!(area, 2.0, max_relative = 1e-12)
            }
            _ => panic!("expected LargeOpening"),
        }
    }

    /// Builds the single-zone exhaust-fan network used by the solver
    /// robustness tests (#86).
    fn exhaust_fan_network(config: AirflowNetworkConfig) -> AirflowNetwork {
        AirflowNetwork {
            config,
            nodes: vec![
                PressureNode {
                    zone_index: Some(0),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
                PressureNode {
                    zone_index: None,
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
            ],
            paths: vec![
                FlowPath {
                    node_a: 1,
                    node_b: 0,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.01,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                FlowPath {
                    node_a: 0,
                    node_b: 1,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::FixedFlow { mass_flow: 0.1 },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node: 1,
            zone_to_node: vec![0],
            zone_outdoor_mass_flow: vec![0.0],
            zone_interzone_flows: vec![vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![None],
            duct_return_leak_path_indices: vec![None],
            nat_vent_path_indices: vec![None],
        }
    }

    /// Horizontal opening (#93): dense air above light air drives
    /// bidirectional exchange even at zero pressure difference.
    #[test]
    fn test_horizontal_opening_unstable_exchange() {
        let elem = FlowElement::HorizontalOpening {
            discharge_coefficient: 0.65,
            area: 2.0,
        };
        let rho_lower = 1.15; // warm lower zone
        let rho_upper = 1.25; // cold (denser) upper zone — unstable

        let (m_up, m_down, _) = horizontal_opening_flows(&elem, 0.0, rho_lower, rho_upper);
        assert!(
            m_up > 0.01 && m_down > 0.01,
            "unstable stratification should exchange air both ways: up={m_up:.4}, down={m_down:.4}"
        );
        // Each leg carries its source-zone air density (small tolerance for
        // the linearized orifice contribution near ΔP = 0)
        assert_relative_eq!(m_down / m_up, rho_upper / rho_lower, max_relative = 0.02);

        // Hand check against Q = 0.055·√(g·Δρ·D⁵/ρ̄)
        let dh = 2.0 * (2.0_f64 / std::f64::consts::PI).sqrt();
        let q = 0.055 * (G * 0.1 * dh.powi(5) / 1.2).sqrt();
        assert_relative_eq!(m_up, rho_lower * q, max_relative = 0.02);
    }

    /// Horizontal opening (#93): stable stratification (light air above)
    /// gives pure orifice behavior; flooding pressure kills the exchange.
    #[test]
    fn test_horizontal_opening_stable_and_flooding() {
        let elem = FlowElement::HorizontalOpening {
            discharge_coefficient: 0.65,
            area: 2.0,
        };

        // Stable: upper zone lighter → no buoyancy exchange
        let (m_up, m_down, _) = horizontal_opening_flows(&elem, 3.0, 1.25, 1.15);
        let orifice = 0.65 * 2.0 * (2.0 * 1.25 * 3.0_f64).sqrt();
        assert_relative_eq!(m_up, orifice, max_relative = 1e-9);
        assert_eq!(m_down, 0.0);

        // Unstable but far beyond the flooding pressure: exchange suppressed
        let (m_up, m_down, _) = horizontal_opening_flows(&elem, 100.0, 1.15, 1.25);
        let orifice = 0.65 * 2.0 * (2.0 * 1.15 * 100.0_f64).sqrt();
        assert_relative_eq!(m_up, orifice, max_relative = 1e-9);
        assert_eq!(m_down, 0.0);

        // Net flow from flow_and_derivative is consistent
        let (net, dnet) = flow_and_derivative(&elem, 3.0, 1.25, 1.15);
        assert_relative_eq!(
            net,
            0.65 * 2.0 * (2.0 * 1.25 * 3.0_f64).sqrt(),
            max_relative = 1e-9
        );
        assert!(dnet > 0.0);
    }

    /// Stacked zones joined by a hatch (#93): the solved network records
    /// counterflow through the horizontal opening when the upper zone is
    /// colder (denser) than the lower zone.
    #[test]
    fn test_stairwell_exchange_in_network() {
        let mut network = AirflowNetwork {
            config: AirflowNetworkConfig {
                max_iterations: 200,
                ..Default::default()
            },
            nodes: vec![
                PressureNode {
                    zone_index: Some(0), // lower, warm 25°C
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 298.15,
                    density: 1.184,
                },
                PressureNode {
                    zone_index: Some(1), // upper, cold 10°C
                    ref_height: 4.5,
                    pressure: 0.0,
                    temperature: 283.15,
                    density: 1.247,
                },
                PressureNode {
                    zone_index: None,
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
            ],
            paths: vec![
                FlowPath {
                    node_a: 2,
                    node_b: 0,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.01,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                FlowPath {
                    node_a: 2,
                    node_b: 1,
                    height: 4.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.01,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Stair hatch: lower (0) → upper (1)
                FlowPath {
                    node_a: 0,
                    node_b: 1,
                    height: 3.0,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::HorizontalOpening {
                        discharge_coefficient: 0.65,
                        area: 1.0,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node: 2,
            zone_to_node: vec![0, 1],
            zone_outdoor_mass_flow: vec![0.0, 0.0],
            zone_interzone_flows: vec![vec![], vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None, None],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![None, None],
            duct_return_leak_path_indices: vec![None, None],
            nat_vent_path_indices: vec![None, None],
        };

        let (converged, _) = solve_pressures(&mut network, 0.0, 0.0, 15.0, 1.22, Terrain::Suburbs);
        assert!(converged, "solver should converge with horizontal opening");

        let lower_gets_cold = network.zone_interzone_flows[0]
            .iter()
            .any(|&(src, m)| src == 1 && m > 1e-3);
        let upper_gets_warm = network.zone_interzone_flows[1]
            .iter()
            .any(|&(src, m)| src == 0 && m > 1e-3);
        assert!(
            lower_gets_cold && upper_gets_warm,
            "hatch should exchange both ways: lower={:?}, upper={:?}",
            network.zone_interzone_flows[0],
            network.zone_interzone_flows[1]
        );
    }

    /// Blower-door calibration (#92): exterior crack coefficients scale so
    /// the envelope flow at 50 Pa matches ACH50 × volume; openings and
    /// interzone cracks are untouched.
    #[test]
    fn test_ach50_calibration() {
        let crack = |a: usize, b: usize, c: f64| FlowPath {
            node_a: a,
            node_b: b,
            height: 1.5,
            cp: 0.0,
            azimuth: 0.0,
            element: FlowElement::PowerLawCrack {
                coefficient: c,
                exponent: 0.65,
            },
            source_surface: None,
            mass_flow: 0.0,
        };
        // Nodes: 0,1 = zones, 2 = outdoor
        let mut paths = vec![
            crack(2, 0, 0.001), // exterior
            crack(2, 1, 0.003), // exterior
            crack(0, 1, 0.002), // interzone — must NOT scale
            // NV opening — must NOT scale (blower door runs windows closed)
            FlowPath {
                node_a: 2,
                node_b: 0,
                height: 1.5,
                cp: 0.0,
                azimuth: 0.0,
                element: FlowElement::LargeOpening {
                    discharge_coefficient: 0.65,
                    area: 1.0,
                },
                source_surface: None,
                mass_flow: 0.0,
            },
        ];

        let volume = 300.0;
        let ach50 = 5.0;
        let scale = calibrate_ach50(&mut paths, 2, ach50, volume);
        assert!(scale > 0.0);

        // Envelope crack flow at 50 Pa now equals the target
        let q50: f64 = paths
            .iter()
            .filter(|p| p.node_a == 2 || p.node_b == 2)
            .filter_map(|p| match p.element {
                FlowElement::PowerLawCrack {
                    coefficient,
                    exponent,
                } => Some(coefficient * 50.0_f64.powf(exponent)),
                _ => None,
            })
            .sum();
        let target = ach50 * volume * RHO_REF / 3600.0;
        assert_relative_eq!(q50, target, max_relative = 1e-12);

        // Interzone crack untouched
        match paths[2].element {
            FlowElement::PowerLawCrack { coefficient, .. } => {
                assert_relative_eq!(coefficient, 0.002, max_relative = 1e-12)
            }
            _ => panic!("expected crack"),
        }
        // Opening untouched
        match paths[3].element {
            FlowElement::LargeOpening { area, .. } => assert_eq!(area, 1.0),
            _ => panic!("expected opening"),
        }
        // No calibration requested → no-op
        assert_eq!(calibrate_ach50(&mut paths, 2, 0.0, volume), 1.0);
    }

    /// Natural-ventilation openings under AFN (#88): the availability logic
    /// drives the opening area; closed conditions give zero area (zero flow).
    #[test]
    fn test_nat_vent_opening_area_update() {
        let mut network = exhaust_fan_network(AirflowNetworkConfig::default());
        // Treat path 0 (the crack) slot as an NV opening for zone 0
        network.paths[0].element = FlowElement::LargeOpening {
            discharge_coefficient: 0.65,
            area: 2.0,
        };
        network.nat_vent_path_indices = vec![Some(0)];

        // Half-available → half area
        network.update_nat_vent_opening(0, 1.0);
        match network.paths[0].element {
            FlowElement::LargeOpening { area, .. } => {
                assert_relative_eq!(area, 1.0, max_relative = 1e-12)
            }
            _ => panic!("expected LargeOpening"),
        }

        // Unavailable → closed → zero flow at any ΔP
        network.update_nat_vent_opening(0, 0.0);
        let (flow, dflow) = flow_and_derivative(&network.paths[0].element, 10.0, RHO_REF, RHO_REF);
        assert_eq!(flow, 0.0);
        assert_eq!(dflow, 0.0);

        // Negative input clamps to zero
        network.update_nat_vent_opening(0, -3.0);
        match network.paths[0].element {
            FlowElement::LargeOpening { area, .. } => assert_eq!(area, 0.0),
            _ => panic!("expected LargeOpening"),
        }
    }

    /// Adaptive damping (#86): converges at least as fast as the fixed
    /// factor on the same problem, and within a tight iteration budget.
    #[test]
    fn test_adaptive_damping_converges_no_slower() {
        let tight = AirflowNetworkConfig {
            enabled: true,
            max_iterations: 100,
            convergence_tolerance: 1e-6,
            relative_tolerance: 1e-12, // isolate the absolute criterion
            damping: 0.75,
            ..Default::default()
        };

        let mut fixed = exhaust_fan_network(AirflowNetworkConfig {
            adaptive_damping: false,
            ..tight.clone()
        });
        let (conv_fixed, iters_fixed) =
            solve_pressures(&mut fixed, 0.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);

        let mut adaptive = exhaust_fan_network(AirflowNetworkConfig {
            adaptive_damping: true,
            ..tight
        });
        let (conv_adaptive, iters_adaptive) =
            solve_pressures(&mut adaptive, 0.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);

        assert!(conv_fixed && conv_adaptive, "both variants should converge");
        assert!(
            iters_adaptive <= iters_fixed,
            "adaptive damping should not be slower: {iters_adaptive} vs {iters_fixed} iterations"
        );
        assert!(
            iters_adaptive < 20,
            "adaptive damping should converge quickly, took {iters_adaptive} iterations"
        );

        // Same physical answer
        assert_relative_eq!(
            adaptive.nodes[0].pressure,
            fixed.nodes[0].pressure,
            max_relative = 1e-3
        );
    }

    /// Relative convergence criterion (#86): a network dominated by a large
    /// open-window flow converges via the relative test rather than being
    /// held to the tight-crack absolute tolerance.
    #[test]
    fn test_relative_convergence_large_flows() {
        let mut network = exhaust_fan_network(AirflowNetworkConfig {
            enabled: true,
            max_iterations: 30,
            ..Default::default()
        });
        // Replace the crack with a wide-open window: flows ~2-3 orders of
        // magnitude larger than crack flows.
        network.paths[0].element = FlowElement::TwoWayOpening {
            discharge_coefficient: 0.65,
            width: 1.5,
            height: 1.5,
        };
        network.paths[1].element = FlowElement::FixedFlow { mass_flow: 2.0 };

        let (converged, iters) =
            solve_pressures(&mut network, 0.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);
        assert!(converged, "large-flow network should converge");
        assert!(
            iters < 20,
            "relative criterion should accept quickly, took {iters} iterations"
        );
        // Window inflow balances the exhaust
        assert_relative_eq!(network.zone_outdoor_mass_flow[0], 2.0, max_relative = 0.01);
    }

    /// Two-way opening (#83): pure buoyancy exchange with equal node
    /// pressures at mid-height matches the classic doorway formula
    /// ṁ = (Cd·W/3)·√(g·Δρ·ρ_upstream)·H^{3/2}.
    #[test]
    fn test_two_way_opening_buoyancy_exchange() {
        let elem = FlowElement::TwoWayOpening {
            discharge_coefficient: 0.65,
            width: 0.9,
            height: 2.0,
        };
        let rho_a = 1.25; // cold side
        let rho_b = 1.15; // warm side

        let (m_ab, m_ba, dnet) = two_way_opening_flows(&elem, 0.0, rho_a, rho_b);

        // Classic doorway exchange, upstream density per direction
        let g_drho = G * (rho_a - rho_b);
        let expected_ab = (0.65 * 0.9 / 3.0) * (g_drho * rho_a).sqrt() * 2.0_f64.powf(1.5);
        let expected_ba = (0.65 * 0.9 / 3.0) * (g_drho * rho_b).sqrt() * 2.0_f64.powf(1.5);
        assert_relative_eq!(m_ab, expected_ab, max_relative = 1e-9);
        assert_relative_eq!(m_ba, expected_ba, max_relative = 1e-9);

        // Both directions flow simultaneously; derivative is positive
        assert!(m_ab > 0.0 && m_ba > 0.0);
        assert!(dnet > 0.0);

        // Consistency with the net flow from flow_and_derivative
        let (net, _) = flow_and_derivative(&elem, 0.0, rho_a, rho_b);
        assert_relative_eq!(net, m_ab - m_ba, max_relative = 1e-12);
    }

    /// Two-way opening (#83): analytic d(net)/d(ΔP) matches finite
    /// differences with the neutral plane inside and outside the opening.
    #[test]
    fn test_two_way_opening_jacobian_numerical() {
        let elem = FlowElement::TwoWayOpening {
            discharge_coefficient: 0.6,
            width: 1.2,
            height: 1.8,
        };
        let rho_a = 1.28;
        let rho_b = 1.16;
        let eps = 1e-6;

        for dp in [-0.4, 0.3, 5.0, -6.0] {
            let (_, _, dnet_analytic) = two_way_opening_flows(&elem, dp, rho_a, rho_b);
            let (f_plus, _) = flow_and_derivative(&elem, dp + eps, rho_a, rho_b);
            let (f_minus, _) = flow_and_derivative(&elem, dp - eps, rho_a, rho_b);
            let dnet_numeric = (f_plus - f_minus) / (2.0 * eps);
            assert_relative_eq!(dnet_analytic, dnet_numeric, max_relative = 1e-4);
        }
    }

    /// Two-way opening (#83): with equal densities it degenerates to the
    /// one-way orifice with area = width × height.
    #[test]
    fn test_two_way_opening_degenerate_equal_density() {
        let two_way = FlowElement::TwoWayOpening {
            discharge_coefficient: 0.65,
            width: 0.5,
            height: 2.0,
        };
        let orifice = FlowElement::LargeOpening {
            discharge_coefficient: 0.65,
            area: 1.0,
        };
        let (f_two_way, d_two_way) = flow_and_derivative(&two_way, 3.0, 1.2, 1.2);
        let (f_orifice, d_orifice) = flow_and_derivative(&orifice, 3.0, 1.2, 1.2);
        assert_relative_eq!(f_two_way, f_orifice, max_relative = 1e-9);
        assert_relative_eq!(d_two_way, d_orifice, max_relative = 1e-9);
    }

    /// Doorway between a warm and a cold zone (#83): the solved network
    /// records simultaneous counterflow — each zone receives air from the
    /// other through the same opening.
    #[test]
    fn test_doorway_counterflow_in_network() {
        let mut network = AirflowNetwork {
            config: AirflowNetworkConfig::default(),
            nodes: vec![
                PressureNode {
                    zone_index: Some(0), // cold zone, 15°C
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 288.15,
                    density: 1.225,
                },
                PressureNode {
                    zone_index: Some(1), // warm zone, 25°C
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 298.15,
                    density: 1.184,
                },
                PressureNode {
                    zone_index: None,
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
            ],
            paths: vec![
                // Cracks to outdoor keep the pressure problem well-posed
                FlowPath {
                    node_a: 2,
                    node_b: 0,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.002,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                FlowPath {
                    node_a: 2,
                    node_b: 1,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.002,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Doorway between the zones
                FlowPath {
                    node_a: 0,
                    node_b: 1,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::TwoWayOpening {
                        discharge_coefficient: 0.65,
                        width: 0.9,
                        height: 2.0,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node: 2,
            zone_to_node: vec![0, 1],
            zone_outdoor_mass_flow: vec![0.0, 0.0],
            zone_interzone_flows: vec![vec![], vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None, None],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![None, None],
            duct_return_leak_path_indices: vec![None, None],
            nat_vent_path_indices: vec![None, None],
        };

        let (converged, _) =
            solve_pressures(&mut network, 0.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);
        assert!(converged, "solver should converge with two-way opening");

        // Counterflow: cold zone receives warm air, warm zone receives cold air
        let cold_receives_from_warm = network.zone_interzone_flows[0]
            .iter()
            .any(|&(src, m)| src == 1 && m > 1e-4);
        let warm_receives_from_cold = network.zone_interzone_flows[1]
            .iter()
            .any(|&(src, m)| src == 0 && m > 1e-4);
        assert!(
            cold_receives_from_warm,
            "cold zone should receive warm air through the doorway: {:?}",
            network.zone_interzone_flows[0]
        );
        assert!(
            warm_receives_from_cold,
            "warm zone should receive cold air through the doorway: {:?}",
            network.zone_interzone_flows[1]
        );

        // The exchange dwarfs the crack flows (doorway ≫ cracks)
        let doorway_exchange: f64 = network.zone_interzone_flows[0]
            .iter()
            .map(|&(_, m)| m)
            .sum();
        assert!(
            doorway_exchange > 0.1,
            "doorway exchange should be substantial, got {doorway_exchange:.4} kg/s"
        );
    }

    /// Directional duct leakage (#82, #85): a supply-dominated system moves
    /// net air from the conditioned zone to the attic, pressurizing the attic
    /// and depressurizing the zone. Both duct paths are excluded from
    /// interzone flow accumulation (their energy is handled by the duct
    /// component).
    #[test]
    fn test_duct_leakage_pressurizes_unconditioned_zone() {
        let zone_node = 0usize; // conditioned zone
        let attic_node = 1usize; // unconditioned duct ambient zone
        let outdoor_node = 2usize;
        let supply_path_idx = 2usize;
        let return_path_idx = 3usize;

        let mut network = AirflowNetwork {
            config: AirflowNetworkConfig::default(),
            nodes: vec![
                PressureNode {
                    zone_index: Some(0),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
                PressureNode {
                    zone_index: Some(1),
                    ref_height: 1.5, // same height to isolate the duct effect
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
                PressureNode {
                    zone_index: None,
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15,
                    density: RHO_REF,
                },
            ],
            paths: vec![
                // Outdoor → zone crack
                FlowPath {
                    node_a: outdoor_node,
                    node_b: zone_node,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.005,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Outdoor → attic crack
                FlowPath {
                    node_a: outdoor_node,
                    node_b: attic_node,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: 0.005,
                        exponent: 0.65,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Supply duct leak: zone → attic (mass flow set below)
                FlowPath {
                    node_a: zone_node,
                    node_b: attic_node,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::FixedFlow { mass_flow: 0.0 },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // Return duct leak: attic → zone (mass flow set below)
                FlowPath {
                    node_a: attic_node,
                    node_b: zone_node,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::FixedFlow { mass_flow: 0.0 },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node,
            zone_to_node: vec![zone_node, attic_node],
            zone_outdoor_mass_flow: vec![0.0, 0.0],
            zone_interzone_flows: vec![vec![], vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None, None],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![Some(supply_path_idx), None],
            duct_return_leak_path_indices: vec![Some(return_path_idx), None],
            nat_vent_path_indices: vec![Some(return_path_idx), None],
        };

        // Supply-dominated: 0.06 × 0.5 kg/s supply leak, 0.02 × 0.5 return leak
        network.update_duct_leakage_flows(0, 0.03, 0.01);
        match network.paths[supply_path_idx].element {
            FlowElement::FixedFlow { mass_flow } => assert_eq!(mass_flow, 0.03),
            _ => panic!("expected FixedFlow"),
        }
        match network.paths[return_path_idx].element {
            FlowElement::FixedFlow { mass_flow } => assert_eq!(mass_flow, 0.01),
            _ => panic!("expected FixedFlow"),
        }

        let (converged, _) =
            solve_pressures(&mut network, 0.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);
        assert!(converged, "solver should converge");

        // Supply-dominated leakage pressurizes the attic; the conditioned
        // zone is depressurized (make-up air enters through its crack).
        let p_zone = network.nodes[zone_node].pressure;
        let p_attic = network.nodes[attic_node].pressure;
        assert!(
            p_attic > 0.0,
            "duct ambient zone should be pressurized, got {p_attic:.3} Pa"
        );
        assert!(
            p_zone < 0.0,
            "conditioned zone should be depressurized, got {p_zone:.3} Pa"
        );

        // Make-up air infiltrates the zone at the NET imbalance (0.02 kg/s);
        // the attic exfiltrates the same amount.
        assert_relative_eq!(network.zone_outdoor_mass_flow[0], 0.02, max_relative = 0.05);
        assert!(network.zone_outdoor_mass_flow[1] < 1e-9);

        // Both duct leakage paths are excluded from interzone accumulation.
        assert!(network.zone_interzone_flows[0].is_empty());
        assert!(network.zone_interzone_flows[1].is_empty());

        // Return-dominated case: reverse the fractions — the zone is now
        // pressurized by attic air delivered through the return leak.
        network.update_duct_leakage_flows(0, 0.01, 0.03);
        let (converged, _) =
            solve_pressures(&mut network, 0.0, 0.0, 20.0, RHO_REF, Terrain::Suburbs);
        assert!(converged);
        assert!(
            network.nodes[zone_node].pressure > 0.0,
            "return-dominated leakage should pressurize the zone, got {:.3} Pa",
            network.nodes[zone_node].pressure
        );
        assert!(
            network.nodes[attic_node].pressure < 0.0,
            "return-dominated leakage should depressurize the attic, got {:.3} Pa",
            network.nodes[attic_node].pressure
        );
    }

    /// Pressurised building: HVAC supplies more OA than it exhausts.
    /// The solver should raise zone pressure until crack flows balance the net
    /// injection, driving infiltration to zero (all cracks produce exfiltration).
    #[test]
    fn test_pressurized_building_zero_infiltration() {
        // Single zone, two crack paths (outdoor→zone), one HVAC net path.
        // Crack coefficient chosen so a small overpressure produces ~0.05 kg/s out.
        let crack_c = 0.001_f64; // power-law C [kg/s/Pa^n]
        let crack_n = 0.65_f64;

        let zone_node = 0usize;
        let outdoor_node = 1usize;
        let hvac_path_idx = 2usize; // third path = HVAC net injection

        let mut network = AirflowNetwork {
            config: AirflowNetworkConfig::default(),
            nodes: vec![
                PressureNode {
                    zone_index: Some(0),
                    ref_height: 1.5,
                    pressure: 0.0,
                    temperature: 293.15, // 20°C
                    density: RHO_REF,
                },
                PressureNode {
                    zone_index: None, // outdoor
                    ref_height: 0.0,
                    pressure: 0.0,
                    temperature: 273.15, // 0°C
                    density: 1.292,
                },
            ],
            paths: vec![
                // Two cracks: outdoor→zone
                FlowPath {
                    node_a: outdoor_node,
                    node_b: zone_node,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: crack_c,
                        exponent: crack_n,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                FlowPath {
                    node_a: outdoor_node,
                    node_b: zone_node,
                    height: 2.5,
                    cp: 0.0,
                    azimuth: 180.0,
                    element: FlowElement::PowerLawCrack {
                        coefficient: crack_c,
                        exponent: crack_n,
                    },
                    source_surface: None,
                    mass_flow: 0.0,
                },
                // HVAC net injection: 0.05 kg/s OA surplus → pressurisation
                FlowPath {
                    node_a: outdoor_node,
                    node_b: zone_node,
                    height: 1.5,
                    cp: 0.0,
                    azimuth: 0.0,
                    element: FlowElement::FixedFlow { mass_flow: 0.05 },
                    source_surface: None,
                    mass_flow: 0.0,
                },
            ],
            outdoor_node,
            zone_to_node: vec![zone_node],
            zone_outdoor_mass_flow: vec![0.0],
            zone_interzone_flows: vec![vec![]],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![Some(hvac_path_idx)],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![],
            duct_return_leak_path_indices: vec![],
            nat_vent_path_indices: vec![],
        };

        let (converged, _) = solve_pressures(&mut network, 0.0, 0.0, 0.0, 1.292, Terrain::Suburbs);
        assert!(converged, "solver should converge");

        // Zone should be at positive pressure (above outdoor = 0 Pa)
        let zone_pressure = network.nodes[zone_node].pressure;
        assert!(
            zone_pressure > 0.0,
            "pressurised zone should have positive gauge pressure, got {zone_pressure:.3} Pa"
        );

        // Infiltration reported to zone should be zero — cracks flow outward
        let infiltration = network.zone_outdoor_mass_flow[0];
        assert!(
            infiltration < 1e-6,
            "pressurised building should have zero infiltration, got {infiltration:.4} kg/s"
        );

        // The two crack paths should each carry negative flow (exfiltration)
        for (i, path) in network.paths[0..2].iter().enumerate() {
            assert!(
                path.mass_flow < 0.0,
                "crack path {i} should carry exfiltration (negative), got {:.4}",
                path.mass_flow
            );
        }
    }
}
