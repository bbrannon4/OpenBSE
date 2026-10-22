//! Find `autosize` inputs that sizing left unresolved, so the run can stop.
//!
//! Air-side sizing runs only from design days, and a few fields have no
//! sizing at all yet. A field nothing sized used to simulate with the
//! `AUTOSIZE` sentinel (−99,999) or a silent fallback (a 0.5 kg/s fan, a 0 W
//! coil, a 3 kW VRF unit), which is how a data hall once ran at thousands
//! of °C. Plant sizing reports its own unresolved fields before this runs;
//! any plant sentinel left here is a backstop catch.

use std::collections::{HashMap, HashSet};

use openbse_airloop::LoopInfo;
use openbse_core::graph::{GraphComponent, SimulationGraph};
use openbse_core::types::is_autosize;
use openbse_io::input::ModelInput;

/// Whether air-side design-day sizing ran, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirSizing {
    Ran,
    /// The model has no `design_days`.
    NoDesignDays,
    /// Design-day sizing works on the envelope, and the model has none.
    NoEnvelope,
}

impl AirSizing {
    pub fn new(has_design_days: bool, has_envelope: bool) -> Self {
        if !has_envelope {
            Self::NoEnvelope
        } else if !has_design_days {
            Self::NoDesignDays
        } else {
            Self::Ran
        }
    }
}

/// What would have sized a field that stayed unsized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Remedy {
    /// Air-side design-day sizing sets this field.
    AirSizing(AirSizing),
    /// Nothing in OpenBSE sizes this field.
    NotAutosized,
}

fn message(owner: &str, field: &str, remedy: Remedy) -> String {
    let why = match remedy {
        Remedy::AirSizing(AirSizing::NoDesignDays) => {
            "the model has no design_days, so air-side sizing did not run. \
             Add design_days or give it an explicit value"
        }
        Remedy::AirSizing(AirSizing::NoEnvelope) => {
            "design-day sizing needs zones and surfaces, and the model has none. \
             Give it an explicit value"
        }
        Remedy::AirSizing(AirSizing::Ran) => {
            "design-day sizing ran but does not reach it. Give it an explicit value"
        }
        Remedy::NotAutosized => "OpenBSE does not autosize it yet. Give it an explicit value",
    };
    format!("'{field}' of {owner} is autosize but was never sized: {why}")
}

/// Sizing results the check reads, beyond the graph and the loops.
pub struct SizedValues<'a> {
    /// Design airflow per zone [kg/s]; still the sentinel where an
    /// `autosize` `design_zone_flow` was never replaced.
    pub zone_design_flows: &'a HashMap<String, f64>,
    /// Zone peak loads from design-day sizing, which size VRF indoor units.
    pub zone_peak_cooling: &'a HashMap<String, f64>,
    pub zone_peak_heating: &'a HashMap<String, f64>,
}

/// One message per `autosize` field that sizing left unresolved.
pub fn unsized_autosize_errors(
    model: &ModelInput,
    graph: &SimulationGraph,
    loop_infos: &[LoopInfo],
    sized: &SizedValues,
    air_sizing: AirSizing,
) -> Vec<String> {
    let mut errors = Vec::new();
    let air_sizing = Remedy::AirSizing(air_sizing);

    let air_loop_components: HashSet<&str> = loop_infos
        .iter()
        .flat_map(|li| li.component_names.iter().chain(li.terminal_boxes.values()))
        .map(String::as_str)
        .collect();
    for component in graph.components() {
        match component {
            GraphComponent::Air(c) => {
                let owner = format!("'{}'", c.name());
                for field in c.unsized_fields() {
                    // Design-day sizing never sets a PFP box's secondary fan.
                    let sized_by_air_sizing =
                        air_loop_components.contains(c.name()) && field != "secondary_fan_flow";
                    let remedy = if sized_by_air_sizing {
                        air_sizing
                    } else {
                        Remedy::NotAutosized
                    };
                    errors.push(message(&owner, field, remedy));
                }
            }
            GraphComponent::Plant(c) => {
                let owner = format!("'{}'", c.name());
                for field in c.unsized_fields() {
                    errors.push(message(&owner, field, Remedy::NotAutosized));
                }
            }
        }
    }

    for al in &model.air_loops {
        if !al.controls.design_zone_flow.is_autosize() {
            continue;
        }
        let unsized_zone = al.zone_terminals.iter().any(|zt| {
            sized
                .zone_design_flows
                .get(&zt.zone)
                .is_some_and(|&flow| is_autosize(flow))
        });
        if unsized_zone {
            let owner = format!("air loop '{}'", al.name);
            errors.push(message(&owner, "controls.design_zone_flow", air_sizing));
        }
    }

    let mut dd_boxes: Vec<_> = loop_infos
        .iter()
        .flat_map(|li| li.dd_boxes.values())
        .filter(|b| is_autosize(b.design_flow))
        .collect();
    dd_boxes.sort_by(|a, b| a.name.cmp(&b.name));
    for b in dd_boxes {
        let owner = format!("dual-duct box '{}'", b.name);
        errors.push(message(&owner, "design_flow", air_sizing));
    }

    // An unsized VRF indoor unit falls back to a 3 kW capacity rather than
    // keeping the sentinel; its zone missing from the sizing peaks shows it.
    for sys in &model.vrf_systems {
        for iu in &sys.indoor_units {
            let owner = format!("VRF indoor unit '{}'", iu.name);
            let checks = [
                (
                    "cooling_capacity",
                    iu.cooling_capacity,
                    sized.zone_peak_cooling,
                ),
                (
                    "heating_capacity",
                    iu.heating_capacity,
                    sized.zone_peak_heating,
                ),
            ];
            for (field, value, peaks) in checks {
                if value.is_autosize() && !peaks.contains_key(&iu.zone) {
                    errors.push(message(&owner, field, air_sizing));
                }
            }
        }
    }

    for rp in &model.radiant_panels {
        if rp.rated_capacity.is_autosize() {
            let owner = format!("radiant panel '{}'", rp.name);
            errors.push(message(&owner, "rated_capacity", Remedy::NotAutosized));
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_sizing_reports_the_missing_input() {
        assert_eq!(AirSizing::new(true, true), AirSizing::Ran);
        assert_eq!(AirSizing::new(false, true), AirSizing::NoDesignDays);
        // Without an envelope, design days alone would not help.
        assert_eq!(AirSizing::new(true, false), AirSizing::NoEnvelope);
        assert_eq!(AirSizing::new(false, false), AirSizing::NoEnvelope);
    }

    #[test]
    fn message_names_component_field_and_remedy() {
        let m = message(
            "'Supply Fan'",
            "design_flow_rate",
            Remedy::AirSizing(AirSizing::NoDesignDays),
        );
        assert_eq!(
            m,
            "'design_flow_rate' of 'Supply Fan' is autosize but was never sized: the model \
             has no design_days, so air-side sizing did not run. Add design_days or give it \
             an explicit value"
        );
        let m = message("'Boiler'", "capacity", Remedy::NotAutosized);
        assert!(m.contains("OpenBSE does not autosize it yet"), "{m}");
    }
}
