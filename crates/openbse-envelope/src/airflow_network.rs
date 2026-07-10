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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CpModel {
    /// Swami & Chandra (1988) for low-rise buildings (< 3 stories).
    #[default]
    SwamiChandra,
    /// Simplified high-rise model (cosine variation, constant leeward).
    HighRise,
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
    /// Newton-Raphson convergence tolerance [Pa].
    #[serde(default = "default_convergence_tol")]
    pub convergence_tolerance: f64,
    /// Maximum Newton-Raphson iterations.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    /// Damping factor for Newton-Raphson update (0-1, 1 = full step).
    #[serde(default = "default_damping")]
    pub damping: f64,
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
            convergence_tolerance: default_convergence_tol(),
            max_iterations: default_max_iterations(),
            damping: default_damping(),
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

/// Compute Cp for a surface given the angle between wind and surface normal.
fn compute_cp(theta_deg: f64, side_ratio: f64, model: CpModel) -> f64 {
    match model {
        CpModel::SwamiChandra => cp_swami_chandra(theta_deg, side_ratio),
        CpModel::HighRise => cp_high_rise(theta_deg),
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
/// `dp` = P_a - P_b + stack + wind [Pa].
/// `rho_avg` = average air density across the element [kg/m³].
///
/// Returns (mass_flow [kg/s], d(mass_flow)/d(dp) [kg/s/Pa]).
/// Positive flow = A → B.
pub fn flow_and_derivative(element: &FlowElement, dp: f64, rho_avg: f64) -> (f64, f64) {
    match element {
        FlowElement::PowerLawCrack {
            coefficient,
            exponent,
        } => {
            let c = *coefficient;
            let n = *exponent;
            let dp_abs = dp.abs().max(MIN_DP);
            let sign = if dp >= 0.0 { 1.0 } else { -1.0 };
            // Density correction: Q ∝ (ρ/ρ_ref)^(1-n) per ASHRAE
            let rho_corr = (rho_avg / RHO_REF).powf(1.0 - n);
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
            let dp_abs = dp.abs().max(MIN_DP);
            let sign = if dp >= 0.0 { 1.0 } else { -1.0 };
            let flow = sign * cd * a * (2.0 * rho_avg * dp_abs).sqrt();
            // dQ/ddp = Cd * A * sqrt(rho_avg / (2 * |dp|))
            let dflow = cd * a * (rho_avg / (2.0 * dp_abs)).sqrt();
            (flow, dflow)
        }
        FlowElement::FixedFlow { mass_flow } => {
            (*mass_flow, 0.0) // no pressure dependence
        }
    }
}

// ─── Network construction ───────────────────────────────────────────────────

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
    let mut processed_interzone = vec![false; surfaces.len()];

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
                    paths.push(FlowPath {
                        node_a: outdoor_node,
                        node_b: zone_node,
                        height,
                        cp: 0.0, // updated per timestep
                        azimuth,
                        element: FlowElement::LargeOpening {
                            discharge_coefficient: cd,
                            area: surf.input.area * frac,
                        },
                        source_surface: Some(si),
                        mass_flow: 0.0,
                    });
                } else {
                    // Default: power-law crack
                    let c = override_or(overrides, |o| o.crack_coefficient, {
                        let leakage = if surf.is_window {
                            config.window_leakage_per_area
                        } else {
                            config.wall_leakage_per_area
                        };
                        leakage * surf.net_area
                    });
                    let n = override_or_val(
                        overrides,
                        |o| o.crack_exponent,
                        config.default_crack_exponent,
                    );

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

                let c = override_or(overrides, |o| o.crack_coefficient, {
                    config.interzone_leakage_per_area * surf.input.area
                });
                let n = override_or_val(
                    overrides,
                    |o| o.crack_exponent,
                    config.default_crack_exponent,
                );

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

    // Natural ventilation openings (zone-level, not surface-level)
    for (zi, zone) in zones.iter().enumerate() {
        if let Some(ref nv) = zone.input.natural_ventilation {
            let zone_node = zone_to_node[zi];
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
    }
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

    let cp_model = network.config.cp_model;
    let side_ratio = network.side_ratio;

    for path in &mut network.paths {
        // Only exterior paths (one end is outdoor node)
        let is_exterior = path.node_a == outdoor || path.node_b == outdoor;
        if !is_exterior {
            continue;
        }

        // Compute Cp from wind direction vs surface azimuth
        let theta = wind_surface_angle(wind_direction, path.azimuth);
        path.cp = compute_cp(theta, side_ratio, cp_model);
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
    let damping = network.config.damping;
    let weather_wind_mod_coeff = crate::convection::DEFAULT_WEATHER_WIND_MOD_COEFF;

    // Initialize zone pressures to 0 (or keep from previous timestep)
    // Outdoor node pressure is always 0 (gauge reference).
    network.nodes[outdoor].pressure = 0.0;

    let mut converged = false;
    let mut iter = 0;

    for _it in 0..max_iter {
        iter = _it + 1;

        // Build residual vector R[zone_i] = sum of mass flows into zone i
        let mut residual = vec![0.0; n_zones];
        // Dense Jacobian J[i][j] = dR_i / dP_j
        let mut jacobian = vec![vec![0.0; n_zones]; n_zones];

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
            let rho_avg = 0.5 * (rho_a + rho_b);

            let (flow, dflow_ddp) = flow_and_derivative(&path.element, dp, rho_avg);

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

        // Check convergence
        let max_residual = residual.iter().map(|r| r.abs()).fold(0.0_f64, f64::max);
        if max_residual < tol * RHO_REF * 0.001 {
            // Tolerance scaled to mass flow units: ~0.1 Pa × typical flow sensitivity
            converged = true;
            break;
        }

        // Solve J × δP = -R using Gaussian elimination with partial pivoting
        let delta_p =
            solve_linear_system(&jacobian, &residual.iter().map(|r| -r).collect::<Vec<_>>());

        if let Some(dp_vec) = delta_p {
            let max_dp = dp_vec.iter().map(|d| d.abs()).fold(0.0_f64, f64::max);
            for zi in 0..n_zones {
                let node = &mut network.nodes[network.zone_to_node[zi]];
                node.pressure += damping * dp_vec[zi];
            }

            // Also check pressure update convergence
            if max_dp < tol {
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

    // Build a fast lookup for HVAC net paths so we can exclude them from
    // zone_outdoor_mass_flow accumulation (their OA is already tracked separately).
    let hvac_path_set: std::collections::HashSet<usize> = network
        .hvac_net_path_indices
        .iter()
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
        let rho_avg =
            0.5 * (network.nodes[path.node_a].density + network.nodes[path.node_b].density);
        let (flow, _) = flow_and_derivative(&path.element, dp, rho_avg);
        path.mass_flow = flow;

        // HVAC net injection paths drive zone pressure but their OA content is
        // already accounted for in zone.outdoor_air_mass_flow — skip accumulation.
        if hvac_path_set.contains(&pi) {
            continue;
        }

        // Accumulate into zone results
        let a_zone = network.nodes[path.node_a].zone_index;
        let b_zone = network.nodes[path.node_b].zone_index;
        let a_is_outdoor = path.node_a == outdoor;
        let b_is_outdoor = path.node_b == outdoor;

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
fn solve_linear_system(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
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
        let (flow, dflow) = flow_and_derivative(&elem, 4.0, RHO_REF);
        let expected = 0.001 * 4.0_f64.powf(0.65);
        assert_relative_eq!(flow, expected, max_relative = 1e-6);
        assert!(dflow > 0.0);

        // Negative ΔP → negative flow
        let (flow_neg, _) = flow_and_derivative(&elem, -4.0, RHO_REF);
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
        let (flow, _) = flow_and_derivative(&elem, 2.0, 1.2);
        let expected = 0.65 * (2.0 * 1.2 * 2.0_f64).sqrt();
        assert_relative_eq!(flow, expected, max_relative = 1e-6);
    }

    #[test]
    fn test_fixed_flow_element() {
        let elem = FlowElement::FixedFlow { mass_flow: 0.5 };
        let (flow, dflow) = flow_and_derivative(&elem, 100.0, 1.2);
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

        let (_, dflow_analytical) = flow_and_derivative(&elem, dp, rho);
        let (f_plus, _) = flow_and_derivative(&elem, dp + eps, rho);
        let (f_minus, _) = flow_and_derivative(&elem, dp - eps, rho);
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
