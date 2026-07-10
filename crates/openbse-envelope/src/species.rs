//! Passive species / contaminant transport on the airflow network (#84).
//!
//! CONTAM-style multizone transport of passive scalars (CO₂, radon, generic
//! tracers) riding the solved AFN flow field. Each timestep, after
//! `solve_pressures`, an implicit (backward Euler) mass balance is solved per
//! species across all zones:
//!
//!   (Mᵢ/Δt + Σṁ_out,i)·Cᵢ − Σⱼ ṁ_{j→i}·Cⱼ = (Mᵢ/Δt)·Cᵢ_prev + ṁ_od,i·C_out + Sᵢ
//!
//! with quasi-steady zone mass balance (Σ_out = Σ_in) — the same assumption
//! CONTAM makes. Interzone couplings include both directions of two-way
//! openings (#83) and the directional duct leakage paths (#85), so attic
//! contaminants reach conditioned zones through return-duct leaks.
//!
//! EnergyPlus (ZoneAirContaminantBalance: CO₂ + one generic contaminant) is
//! the minimum reference; species count here is unlimited.
//!
//! Concentrations are mass fractions [kg species / kg air].

use crate::airflow_network::{solve_linear_system, AirflowNetwork, FlowElement};
use serde::{Deserialize, Serialize};

/// A passive species tracked on the airflow network.
///
/// # Example (YAML)
/// ```yaml
/// simulation:
///   airflow_network:
///     enabled: true
///     species:
///       - name: co2
///         outdoor_concentration: 0.00063   # ~420 ppm as mass fraction
///         initial_concentration: 0.00063
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeciesConfig {
    /// Species name (referenced by zone `species_generation` entries).
    pub name: String,
    /// Outdoor (ambient) concentration [kg/kg].
    #[serde(default)]
    pub outdoor_concentration: f64,
    /// Initial zone concentration [kg/kg] (defaults to outdoor).
    #[serde(default)]
    pub initial_concentration: Option<f64>,
}

/// Per-zone species source.
///
/// # Example (YAML)
/// ```yaml
/// zones:
///   - name: Living
///     species_generation:
///       - species: co2
///         rate: 0.00001   # kg/s
///         schedule: occupancy
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeciesGenerationInput {
    /// Species name (must match a configured species).
    pub species: String,
    /// Generation rate [kg/s].
    pub rate: f64,
    /// Optional schedule modulating the rate [0-1].
    #[serde(default)]
    pub schedule: Option<String>,
}

/// Runtime state: per-species, per-zone concentrations.
#[derive(Debug, Clone)]
pub struct SpeciesTransport {
    /// Species names, parallel to the outer index of `concentrations`.
    pub names: Vec<String>,
    /// Outdoor concentration per species [kg/kg].
    pub outdoor: Vec<f64>,
    /// concentrations[species][zone] [kg/kg].
    pub concentrations: Vec<Vec<f64>>,
}

impl SpeciesTransport {
    /// Initialize from configured species for `n_zones` zones.
    pub fn new(species: &[SpeciesConfig], n_zones: usize) -> Self {
        let names = species.iter().map(|s| s.name.clone()).collect();
        let outdoor: Vec<f64> = species.iter().map(|s| s.outdoor_concentration).collect();
        let concentrations = species
            .iter()
            .map(|s| vec![s.initial_concentration.unwrap_or(s.outdoor_concentration); n_zones])
            .collect();
        Self {
            names,
            outdoor,
            concentrations,
        }
    }

    /// Index of a species by name.
    pub fn species_index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// Current concentration [kg/kg] of `species` in `zone`.
    pub fn concentration(&self, species: usize, zone: usize) -> f64 {
        self.concentrations[species][zone]
    }

    /// Advance all species one timestep on the solved flow field.
    ///
    /// # Arguments
    /// * `network` - AFN after `solve_pressures` (flows are current)
    /// * `zone_air_mass` - zone air mass Mᵢ = V·ρ [kg], per zone
    /// * `zone_oa_flow` - HVAC outdoor-air mass flow [kg/s], per zone
    /// * `zone_vent_flow` - scheduled ventilation mass flow [kg/s], per zone
    /// * `sources` - sources[species][zone] generation rate [kg/s]
    /// * `dt` - timestep [s]
    pub fn step(
        &mut self,
        network: &AirflowNetwork,
        zone_air_mass: &[f64],
        zone_oa_flow: &[f64],
        zone_vent_flow: &[f64],
        sources: &[Vec<f64>],
        dt: f64,
    ) {
        let n_zones = network.zone_to_node.len();
        if n_zones == 0 || self.names.is_empty() || dt <= 0.0 {
            return;
        }

        // Interzone couplings [kg/s]: inflow[i] += ṁ from zone j → zone i.
        // Thermal accumulation (zone_interzone_flows) already carries both
        // directions of two-way openings; duct leakage paths are excluded
        // there but DO carry species, so add them from their tracked indices.
        let mut interzone = vec![vec![0.0_f64; n_zones]; n_zones]; // [to][from]
        let mut outdoor_in = network.zone_outdoor_mass_flow.clone(); // at C_out

        for (zi, flows) in network.zone_interzone_flows.iter().enumerate() {
            for &(src, m) in flows {
                interzone[zi][src] += m;
            }
        }

        // Duct leakage paths (#85): supply leak carries zone air to the
        // ambient zone; return leak carries ambient-zone air to the zone.
        let duct_paths = network
            .duct_supply_leak_path_indices
            .iter()
            .chain(network.duct_return_leak_path_indices.iter())
            .filter_map(|&idx| idx);
        for path_idx in duct_paths {
            let path = &network.paths[path_idx];
            let m = match path.element {
                FlowElement::FixedFlow { mass_flow } => mass_flow,
                _ => path.mass_flow,
            };
            if m <= 0.0 {
                continue;
            }
            let from = network.nodes[path.node_a].zone_index;
            let to = network.nodes[path.node_b].zone_index;
            if let (Some(from), Some(to)) = (from, to) {
                interzone[to][from] += m;
            }
        }

        // HVAC outdoor air and scheduled ventilation enter at C_out.
        for (zi, outdoor) in outdoor_in.iter_mut().enumerate() {
            *outdoor += zone_oa_flow.get(zi).copied().unwrap_or(0.0).max(0.0)
                + zone_vent_flow.get(zi).copied().unwrap_or(0.0).max(0.0);
        }

        // Quasi-steady zone mass balance: total outflow = total inflow.
        let total_out: Vec<f64> = (0..n_zones)
            .map(|zi| outdoor_in[zi] + interzone[zi].iter().sum::<f64>())
            .collect();

        for (si, conc) in self.concentrations.iter_mut().enumerate() {
            let c_out = self.outdoor[si];

            let mut a = vec![vec![0.0_f64; n_zones]; n_zones];
            let mut b = vec![0.0_f64; n_zones];
            for zi in 0..n_zones {
                let m_over_dt = (zone_air_mass.get(zi).copied().unwrap_or(1.0) / dt).max(1e-9);
                a[zi][zi] = m_over_dt + total_out[zi];
                for zj in 0..n_zones {
                    if zj != zi {
                        a[zi][zj] -= interzone[zi][zj];
                    }
                }
                let source = sources
                    .get(si)
                    .and_then(|s| s.get(zi))
                    .copied()
                    .unwrap_or(0.0);
                b[zi] = m_over_dt * conc[zi] + outdoor_in[zi] * c_out + source;
            }

            if let Some(solution) = solve_linear_system(&a, &b) {
                for (zi, c) in solution.into_iter().enumerate() {
                    conc[zi] = c.max(0.0);
                }
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airflow_network::{AirflowNetwork, AirflowNetworkConfig, FlowPath, PressureNode};
    use approx::assert_relative_eq;

    const RHO: f64 = 1.2041;

    /// Minimal network scaffold: zones with prescribed outdoor inflows and
    /// interzone flows, no solving needed (we set accumulators directly).
    fn scaffold(n_zones: usize) -> AirflowNetwork {
        let mut nodes: Vec<PressureNode> = (0..n_zones)
            .map(|i| PressureNode {
                zone_index: Some(i),
                ref_height: 1.5,
                pressure: 0.0,
                temperature: 293.15,
                density: RHO,
            })
            .collect();
        nodes.push(PressureNode {
            zone_index: None,
            ref_height: 0.0,
            pressure: 0.0,
            temperature: 293.15,
            density: RHO,
        });
        AirflowNetwork {
            config: AirflowNetworkConfig::default(),
            outdoor_node: n_zones,
            nodes,
            paths: Vec::<FlowPath>::new(),
            zone_to_node: (0..n_zones).collect(),
            zone_outdoor_mass_flow: vec![0.0; n_zones],
            zone_interzone_flows: vec![vec![]; n_zones],
            side_ratio: 1.0,
            hvac_net_path_indices: vec![None; n_zones],
            scheduled_openings: vec![],
            duct_supply_leak_path_indices: vec![None; n_zones],
            duct_return_leak_path_indices: vec![None; n_zones],
        }
    }

    fn co2_species() -> Vec<SpeciesConfig> {
        vec![SpeciesConfig {
            name: "co2".to_string(),
            outdoor_concentration: 0.00063, // ~420 ppm
            initial_concentration: None,
        }]
    }

    /// Single zone with constant source and infiltration: concentration
    /// converges to the analytic steady state C = C_out + S/ṁ.
    #[test]
    fn test_single_zone_steady_state_dilution() {
        let mut network = scaffold(1);
        let m_inf = 0.05; // kg/s infiltration
        network.zone_outdoor_mass_flow[0] = m_inf;

        let mut transport = SpeciesTransport::new(&co2_species(), 1);
        let source = 0.00002; // kg/s CO2
        let mass = vec![50.0 * RHO]; // 50 m³ zone
        let sources = vec![vec![source]];

        // March to steady state (time constant ≈ M/ṁ ≈ 20 min)
        for _ in 0..2000 {
            transport.step(&network, &mass, &[0.0], &[0.0], &sources, 60.0);
        }

        let expected = 0.00063 + source / m_inf;
        assert_relative_eq!(transport.concentration(0, 0), expected, max_relative = 1e-6);
    }

    /// Two zones with one-way interzone flow: the species travels downstream
    /// only, and the downstream steady state follows the mixing balance.
    #[test]
    fn test_two_zone_transport_direction() {
        let mut network = scaffold(2);
        let m = 0.05;
        // Zone 0: infiltration in; zone 0 → zone 1 → (out via balance)
        network.zone_outdoor_mass_flow[0] = m;
        network.zone_interzone_flows[1].push((0, m));

        let mut transport = SpeciesTransport::new(&co2_species(), 2);
        let source = 0.00002; // kg/s in zone 0 only
        let mass = vec![50.0 * RHO, 50.0 * RHO];
        let sources = vec![vec![source, 0.0]];

        for _ in 0..3000 {
            transport.step(&network, &mass, &[0.0, 0.0], &[0.0, 0.0], &sources, 60.0);
        }

        // Zone 0: C_out + S/ṁ; zone 1 receives zone-0 air with no extra source
        let c0_expected = 0.00063 + source / m;
        assert_relative_eq!(
            transport.concentration(0, 0),
            c0_expected,
            max_relative = 1e-5
        );
        assert_relative_eq!(
            transport.concentration(0, 1),
            c0_expected,
            max_relative = 1e-5
        );
    }

    /// No sources: all zones decay to the outdoor concentration and never
    /// go negative.
    #[test]
    fn test_decay_to_outdoor() {
        let mut network = scaffold(1);
        network.zone_outdoor_mass_flow[0] = 0.1;

        let species = vec![SpeciesConfig {
            name: "tracer".to_string(),
            outdoor_concentration: 0.0,
            initial_concentration: Some(0.01),
        }];
        let mut transport = SpeciesTransport::new(&species, 1);
        let mass = vec![100.0 * RHO];
        let sources = vec![vec![0.0]];

        let mut prev = transport.concentration(0, 0);
        for _ in 0..500 {
            transport.step(&network, &mass, &[0.0], &[0.0], &sources, 60.0);
            let c = transport.concentration(0, 0);
            assert!(c >= 0.0 && c <= prev + 1e-15, "monotone decay violated");
            prev = c;
        }
        assert!(
            prev < 1e-6,
            "tracer should decay to outdoor level, got {prev}"
        );
    }

    /// Return-duct leakage (#85) carries attic species into the conditioned
    /// zone — the pathway E+'s simple duct model cannot represent.
    #[test]
    fn test_return_duct_leak_transports_attic_species() {
        let mut network = scaffold(2); // zone 0 = conditioned, zone 1 = attic
        let m_leak = 0.02;

        // Return leak path: attic (node 1) → zone (node 0)
        network.paths.push(FlowPath {
            node_a: 1,
            node_b: 0,
            height: 1.5,
            cp: 0.0,
            azimuth: 0.0,
            element: FlowElement::FixedFlow { mass_flow: m_leak },
            source_surface: None,
            mass_flow: m_leak,
        });
        network.duct_return_leak_path_indices[0] = Some(0);
        // Fresh air balances: zone gets nothing else; attic infiltrates.
        network.zone_outdoor_mass_flow[1] = m_leak;

        let species = vec![SpeciesConfig {
            name: "radon".to_string(),
            outdoor_concentration: 0.0,
            initial_concentration: Some(0.0),
        }];
        let mut transport = SpeciesTransport::new(&species, 2);
        let mass = vec![50.0 * RHO, 50.0 * RHO];
        let attic_source = 1e-8;
        let sources = vec![vec![0.0, attic_source]];

        for _ in 0..3000 {
            transport.step(&network, &mass, &[0.0, 0.0], &[0.0, 0.0], &sources, 60.0);
        }

        // Attic steady state: S/ṁ; the zone receives attic air through the
        // return leak and reaches the same concentration (its only supply).
        let attic_expected = attic_source / m_leak;
        assert_relative_eq!(
            transport.concentration(0, 1),
            attic_expected,
            max_relative = 1e-4
        );
        assert!(
            transport.concentration(0, 0) > 0.5 * attic_expected,
            "conditioned zone should accumulate attic species via return leak, got {} vs attic {}",
            transport.concentration(0, 0),
            transport.concentration(0, 1)
        );
    }
}
