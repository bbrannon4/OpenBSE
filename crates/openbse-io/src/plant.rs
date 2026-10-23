//! Plant-loop connections and plant sizing.
//!
//! The demand side of a plant loop is every water coil, hot-water reheat
//! terminal and water radiant panel that names it in `plant_loop`. This module
//! is the single place that enumerates those connections, so load collection,
//! sizing and validation cannot disagree about what a loop serves.
//!
//! Sizing follows EnergyPlus `Sizing:Plant` with its default *NonCoincident*
//! option: a loop's design flow is the sum of its demand components' design
//! water flows, and an autosized boiler or chiller is sized to that flow times
//! the loop ΔT (`SizeBoiler`, `SizeElectricEIRChiller`). With every coil using
//! the loop ΔT that is the sum of the coils' design capacities. E+'s
//! *Coincident* option needs an HVAC sizing simulation, which OpenBSE does not
//! run.

use std::collections::{HashMap, HashSet};

use openbse_core::types::is_autosize;

use crate::input::{
    CoolingCoilInput, EquipmentInput, HeatingCoilInput, ModelInput, PlantEquipmentInput,
    PlantLoopInput, RadiantPanelSourceInput, TerminalInput, ValidationResult, VavBoxInput,
};

/// Water properties for design flows: the values the plant-loop solver uses,
/// so a design flow and the flow the loop later carries agree.
const RHO_WATER: f64 = 998.0;
const CP_WATER: f64 = 4186.0;

impl HeatingCoilInput {
    /// Whether this coil is heated by a hot-water plant loop.
    pub fn is_hot_water(&self) -> bool {
        matches!(self.source.as_str(), "hot_water" | "HotWater")
    }
}

impl CoolingCoilInput {
    /// Whether this coil is cooled by a chilled-water plant loop.
    pub fn is_chilled_water(&self) -> bool {
        matches!(
            self.source.as_str(),
            "chilled_water" | "ChilledWater" | "chw"
        )
    }
}

impl VavBoxInput {
    /// Whether the reheat coil is a hot-water coil.
    pub fn has_hot_water_reheat(&self) -> bool {
        matches!(self.reheat_type.as_str(), "hot_water" | "HotWater" | "hw")
    }
}

/// A component that can draw on a plant loop.
#[derive(Debug, Clone, PartialEq)]
pub struct PlantDemand<'a> {
    pub name: &'a str,
    /// Object type for messages, e.g. "heating coil".
    pub object: &'static str,
    /// Its `source` (or `reheat_type`) as written.
    pub source: &'a str,
    /// Whether that source is hot or chilled water.
    pub water: bool,
    pub plant_loop: Option<&'a str>,
    /// Radiant panels are simulated outside the air-side component graph.
    pub radiant_panel: bool,
}

impl PlantDemand<'_> {
    /// Whether this component loads plant loop `loop_name`. A non-water
    /// component never does, even if it names a loop (validation rejects that).
    pub fn serves(&self, loop_name: &str) -> bool {
        self.water && self.plant_loop == Some(loop_name)
    }
}

/// Every heating coil, cooling coil, VAV box and radiant panel in the model,
/// with its water source and plant loop.
pub fn plant_demands(model: &ModelInput) -> Vec<PlantDemand<'_>> {
    let mut demands = Vec::new();
    for al in &model.air_loops {
        for eq in &al.equipment {
            match eq {
                EquipmentInput::HeatingCoil(c) => demands.push(PlantDemand {
                    name: &c.name,
                    object: "heating coil",
                    source: &c.source,
                    water: c.is_hot_water(),
                    plant_loop: c.plant_loop.as_deref(),
                    radiant_panel: false,
                }),
                EquipmentInput::CoolingCoil(c) => demands.push(PlantDemand {
                    name: &c.name,
                    object: "cooling coil",
                    source: &c.source,
                    water: c.is_chilled_water(),
                    plant_loop: c.plant_loop.as_deref(),
                    radiant_panel: false,
                }),
                _ => {}
            }
        }
        for zc in &al.zone_terminals {
            if let Some(TerminalInput::VavBox(vb)) = &zc.terminal {
                demands.push(PlantDemand {
                    name: &vb.name,
                    object: "VAV box",
                    source: &vb.reheat_type,
                    water: vb.has_hot_water_reheat(),
                    plant_loop: vb.plant_loop.as_deref(),
                    radiant_panel: false,
                });
            }
        }
    }
    for rp in &model.radiant_panels {
        let (source, water) = match rp.source {
            RadiantPanelSourceInput::HotWater => ("hot_water", true),
            RadiantPanelSourceInput::ChilledWater => ("chilled_water", true),
            RadiantPanelSourceInput::Electric => ("electric", false),
        };
        demands.push(PlantDemand {
            name: &rp.name,
            object: "radiant panel",
            source,
            water,
            plant_loop: rp.plant_loop.as_deref(),
            radiant_panel: true,
        });
    }
    demands
}

/// Air-side components (water coils and hot-water reheat terminals) whose
/// thermal output is a load on each plant loop, keyed by loop name.
pub fn air_side_demands_by_loop(model: &ModelInput) -> HashMap<String, Vec<String>> {
    let demands = plant_demands(model);
    model
        .plant_loops
        .iter()
        .map(|pl| {
            let names = demands
                .iter()
                .filter(|d| !d.radiant_panel && d.serves(&pl.name))
                .map(|d| d.name.to_string())
                .collect();
            (pl.name.clone(), names)
        })
        .collect()
}

/// Plant load [W] from the given air-side components this HVAC iteration:
/// the sum of their `thermal_output`, each scaled by its zone multiplier.
/// Heating loads are positive and cooling loads negative.
pub fn coil_plant_load(
    components: &[String],
    outputs: &HashMap<String, HashMap<String, f64>>,
    zone_multiplier: &HashMap<String, f64>,
) -> f64 {
    components
        .iter()
        .filter_map(|name| {
            let q = outputs.get(name)?.get("thermal_output").copied()?;
            Some(q * zone_multiplier.get(name).copied().unwrap_or(1.0))
        })
        .sum()
}

/// Check that every water coil, reheat terminal and radiant panel is connected
/// to an existing plant loop, and that every plant loop has some demand.
///
/// A water component with no `plant_loop` is accepted only in a model without
/// plant loops, where it stands for an ideal water source (as in the ASHRAE
/// 140 AE cases). In a model with plant loops it would heat or cool for free
/// while the loop's equipment sat idle, so it is a severe error.
pub fn validate_plant_connections(model: &ModelInput, result: &mut ValidationResult) {
    let loop_names: Vec<&str> = model.plant_loops.iter().map(|p| p.name.as_str()).collect();
    let loop_list = loop_names
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let exists = |name: &str| loop_names.contains(&name);

    let demands = plant_demands(model);
    for d in &demands {
        match (d.water, d.plant_loop) {
            (true, Some(l)) if !exists(l) => result.push_severe(format!(
                "Plant loop '{l}' referenced by {} '{}' not found in plant_loops",
                d.object, d.name
            )),
            (false, Some(l)) => result.push_severe(format!(
                "{} '{}' names plant_loop '{l}' but its source is '{}', which draws no \
                 water; use a hot_water or chilled_water source, or remove plant_loop",
                d.object, d.name, d.source
            )),
            (true, None) if !loop_names.is_empty() => result.push_severe(format!(
                "{} '{}' uses {} but names no plant_loop, so no plant equipment would \
                 supply it; set plant_loop to one of: {loop_list}",
                d.object, d.name, d.source
            )),
            _ => {}
        }
    }

    for pl in &model.plant_loops {
        for eq in &pl.supply_equipment {
            let (kind, name, target) = match eq {
                PlantEquipmentInput::Chiller(c) => match c.condenser_plant_loop.as_deref() {
                    Some(l) => ("condenser_plant_loop", &c.name, l),
                    None => continue,
                },
                PlantEquipmentInput::HeatExchanger(hx) => {
                    ("source_loop", &hx.name, hx.source_loop.as_str())
                }
                _ => continue,
            };
            if !exists(target) {
                result.push_severe(format!(
                    "Plant loop '{target}' named as {kind} of '{name}' (in plant loop '{}') \
                     not found in plant_loops",
                    pl.name
                ));
            }
        }
    }

    let drawn_on = loops_drawn_on(model, &demands);
    for pl in &model.plant_loops {
        if !drawn_on.contains(pl.name.as_str()) && !pl.supply_equipment.is_empty() {
            result.push_warning(format!(
                "Plant loop '{}' has no demand: no coil, terminal, radiant panel, chiller \
                 condenser or heat exchanger draws on it, so its equipment never runs \
                 (anything on it marked autosize is sized to zero)",
                pl.name
            ));
        }
    }
}

/// Inputs to [`size_plant`] gathered from the built, air-side-sized model.
#[derive(Debug, Default)]
pub struct PlantSizingInputs {
    /// Design capacity [W] of each water demand component after air-side
    /// sizing, scaled by its zone multiplier. A component that is missing or
    /// still at the autosize sentinel is unsized.
    pub demand_capacity: HashMap<String, f64>,
    /// Capacity [W] of each boiler and chiller as built; the autosize sentinel
    /// marks one to be sized here.
    pub equipment_capacity: HashMap<String, f64>,
    /// Coincident building peak zone heating / cooling load on the design days
    /// [W], zero without design days. Primary pumps keep sizing from these.
    pub coincident_peak_heating: f64,
    pub coincident_peak_cooling: f64,
}

/// Results of [`size_plant`].
#[derive(Debug, Default)]
pub struct PlantSizes {
    /// Design load of each plant loop [W].
    pub loop_design_load: HashMap<String, f64>,
    /// Capacity [W] for each autosized boiler and chiller.
    pub capacity: HashMap<String, f64>,
    /// Design water flow [m³/s] for each autosized boiler water flow, pump,
    /// cooling tower and heat exchanger.
    pub design_flow: HashMap<String, f64>,
}

/// Resolve every `autosize` in the plant loops' supply equipment.
///
/// Each loop's design load is the sum of its demand components' design
/// capacities, plus the design load of any loop drawing on it through a heat
/// exchanger, plus the heat rejected by chillers whose condenser it is
/// (capacity × (1 + 1/COP)). Every autosized boiler or chiller on a loop is
/// sized to the whole loop design load, as E+ does with its default sizing
/// factor of 1.0.
///
/// Returns one message per autosize that cannot be resolved, for the `.err`
/// file as severe errors.
pub fn size_plant(
    model: &ModelInput,
    inputs: &PlantSizingInputs,
) -> Result<PlantSizes, Vec<String>> {
    let mut errors = Vec::new();
    let demands = plant_demands(model);

    // Demand directly on each loop from its coils, terminals and panels.
    let mut direct: HashMap<&str, f64> = HashMap::new();
    for pl in &model.plant_loops {
        let mut sum = 0.0;
        for d in demands.iter().filter(|d| d.serves(&pl.name)) {
            match inputs.demand_capacity.get(d.name) {
                Some(&cap) if !is_autosize(cap) => sum += cap.abs(),
                _ => errors.push(format!(
                    "{} '{}' on plant loop '{}' has no sized capacity, so the loop cannot be \
                     sized; give it a capacity or add design_days to autosize it",
                    d.object, d.name, pl.name
                )),
            }
        }
        direct.insert(&pl.name, sum);
    }

    let mut sizer = LoopSizer {
        model,
        inputs,
        direct,
        memo: HashMap::new(),
        visiting: HashSet::new(),
        errors: Vec::new(),
    };
    let mut sizes = PlantSizes::default();
    for pl in &model.plant_loops {
        let load = sizer.loop_load(&pl.name);
        sizes.loop_design_load.insert(pl.name.clone(), load);
    }
    errors.append(&mut sizer.errors);

    let drawn_on = loops_drawn_on(model, &demands);
    for pl in &model.plant_loops {
        let load = sizes.loop_design_load[&pl.name];
        // A loop nothing draws on never runs (validation warns). As in E+,
        // its autosized equipment sizes to zero instead of failing the run.
        let idle = !drawn_on.contains(pl.name.as_str());
        let to_flow = |watts: f64| watts / (RHO_WATER * CP_WATER * pl.design_delta_t.max(1.0));
        for eq in &pl.supply_equipment {
            let autosized_capacity = matches!(
                eq,
                PlantEquipmentInput::Boiler(_) | PlantEquipmentInput::Chiller(_)
            ) && inputs
                .equipment_capacity
                .get(eq.name())
                .copied()
                .is_some_and(is_autosize);
            if autosized_capacity {
                match resolved(load, idle) {
                    Some(cap) => {
                        sizes.capacity.insert(eq.name().to_string(), cap);
                    }
                    None => errors.push(no_demand_error(eq, "capacity", pl)),
                }
            }
            let flow_basis = match eq {
                PlantEquipmentInput::Boiler(b) if b.design_water_flow_rate.is_autosize() => {
                    let basis = if load > 0.0 {
                        load
                    } else {
                        sizer.capacity(eq, pl).unwrap_or(0.0)
                    };
                    Some(("design_water_flow_rate", basis))
                }
                PlantEquipmentInput::Pump(p) if p.design_flow_rate.is_autosize() => {
                    Some(("design_flow_rate", sizer.pump_basis(pl, load)))
                }
                PlantEquipmentInput::CoolingTower(ct) if ct.design_water_flow.is_autosize() => {
                    Some(("design_water_flow", load))
                }
                PlantEquipmentInput::HeatExchanger(hx) if hx.design_flow_rate.is_autosize() => {
                    Some(("design_flow_rate", load))
                }
                _ => None,
            };
            if let Some((field, basis)) = flow_basis {
                match resolved(basis, idle) {
                    Some(w) => {
                        sizes.design_flow.insert(eq.name().to_string(), to_flow(w));
                    }
                    None => errors.push(no_demand_error(eq, field, pl)),
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(sizes)
    } else {
        Err(errors)
    }
}

/// A sizing basis [W] as a size: the basis when positive, zero on an idle
/// loop, `None` when a loop with demand still has nothing to size from.
fn resolved(basis: f64, idle: bool) -> Option<f64> {
    if basis > 0.0 {
        Some(basis)
    } else if idle {
        Some(0.0)
    } else {
        None
    }
}

/// Names of the plant loops something draws on: a connected water component,
/// a chiller rejecting heat to it, or a heat exchanger using it as source.
fn loops_drawn_on<'a>(model: &'a ModelInput, demands: &[PlantDemand<'a>]) -> HashSet<&'a str> {
    let supply_side = model
        .plant_loops
        .iter()
        .flat_map(|pl| &pl.supply_equipment)
        .filter_map(|eq| match eq {
            PlantEquipmentInput::Chiller(c) => c.condenser_plant_loop.as_deref(),
            PlantEquipmentInput::HeatExchanger(hx) => Some(hx.source_loop.as_str()),
            _ => None,
        });
    demands
        .iter()
        .filter(|d| d.water)
        .filter_map(|d| d.plant_loop)
        .chain(supply_side)
        .collect()
}

fn no_demand_error(eq: &PlantEquipmentInput, field: &str, pl: &PlantLoopInput) -> String {
    format!(
        "Cannot autosize {field} of '{}' in plant loop '{}': the loop has no connected demand \
         with a design capacity to size it from",
        eq.name(),
        pl.name
    )
}

/// Memoized recursion over the loop graph: a loop's load depends on the loops
/// drawing on it through heat exchangers and on the chillers rejecting to it.
struct LoopSizer<'a> {
    model: &'a ModelInput,
    inputs: &'a PlantSizingInputs,
    direct: HashMap<&'a str, f64>,
    memo: HashMap<String, f64>,
    visiting: HashSet<String>,
    errors: Vec<String>,
}

impl<'a> LoopSizer<'a> {
    fn loop_load(&mut self, name: &str) -> f64 {
        if let Some(&load) = self.memo.get(name) {
            return load;
        }
        if !self.visiting.insert(name.to_string()) {
            self.errors.push(format!(
                "Plant loop '{name}' depends on its own design load through heat exchangers \
                 or chiller condensers, so it cannot be sized"
            ));
            return 0.0;
        }
        let mut load = self.direct.get(name).copied().unwrap_or(0.0);
        let model = self.model;
        for other in &model.plant_loops {
            for eq in &other.supply_equipment {
                match eq {
                    PlantEquipmentInput::HeatExchanger(hx) if hx.source_loop == name => {
                        load += self.loop_load(&other.name);
                    }
                    PlantEquipmentInput::Chiller(c)
                        if c.condenser_plant_loop.as_deref() == Some(name) =>
                    {
                        let cap = self.capacity(eq, other).unwrap_or(0.0);
                        let cop = if c.cop > 0.0 { c.cop } else { f64::INFINITY };
                        load += cap * (1.0 + 1.0 / cop);
                    }
                    _ => {}
                }
            }
        }
        self.visiting.remove(name);
        self.memo.insert(name.to_string(), load);
        load
    }

    /// Capacity of a boiler or chiller: as built, or its loop's design load
    /// when autosized.
    fn capacity(&mut self, eq: &PlantEquipmentInput, pl: &PlantLoopInput) -> Option<f64> {
        let built = self.inputs.equipment_capacity.get(eq.name()).copied()?;
        if is_autosize(built) {
            Some(self.loop_load(&pl.name))
        } else {
            Some(built)
        }
    }

    /// Load [W] a pump is sized for. Condenser loops use their heat-rejection
    /// load; heating-only and cooling-only loops keep the coincident building
    /// peak from the design days; other loops use the installed boiler and
    /// chiller capacity. The loop design load is the fallback when that basis
    /// is zero (no design days, or a loop with neither boilers nor chillers).
    fn pump_basis(&mut self, pl: &PlantLoopInput, loop_load: f64) -> f64 {
        let is_condenser = self.model.plant_loops.iter().any(|ol| {
            ol.supply_equipment.iter().any(|eq| {
                matches!(eq, PlantEquipmentInput::Chiller(c)
                    if c.condenser_plant_loop.as_deref() == Some(pl.name.as_str()))
            })
        });
        let has_boiler = pl
            .supply_equipment
            .iter()
            .any(|eq| matches!(eq, PlantEquipmentInput::Boiler(_)));
        let has_chiller = pl
            .supply_equipment
            .iter()
            .any(|eq| matches!(eq, PlantEquipmentInput::Chiller(_)));
        let basis = if is_condenser {
            loop_load
        } else if has_boiler && !has_chiller {
            self.inputs.coincident_peak_heating
        } else if has_chiller && !has_boiler {
            self.inputs.coincident_peak_cooling
        } else {
            pl.supply_equipment
                .iter()
                .filter(|eq| {
                    matches!(
                        eq,
                        PlantEquipmentInput::Boiler(_) | PlantEquipmentInput::Chiller(_)
                    )
                })
                .filter_map(|eq| self.capacity(eq, pl))
                .filter(|c| *c > 0.0)
                .sum()
        };
        if basis > 0.0 {
            basis
        } else {
            loop_load
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_model_yaml;
    use openbse_core::types::AUTOSIZE;

    /// HHW loop: preheat coil + hot-water VAV reheat. CHW loop: a CHW coil
    /// with a water-cooled chiller rejecting to a condenser loop. A DX coil
    /// and an electric-reheat VAV box sit alongside but never load a loop.
    const PLANT_MODEL: &str = r#"
simulation: {}
weather_files: []
air_loops:
  - name: AHU
    equipment:
      - {type: heating_coil, name: Preheat, source: hot_water, capacity: autosize, plant_loop: HHW}
      - {type: cooling_coil, name: CHW Coil, source: chilled_water, capacity: autosize, plant_loop: CHW}
      - {type: cooling_coil, name: DX Coil, capacity: autosize}
    zone_terminals:
      - zone: A
        terminal: {type: vav_box, name: Box A, max_air_flow: autosize, reheat_type: hot_water, plant_loop: HHW}
      - zone: B
        terminal: {type: vav_box, name: Box B, max_air_flow: autosize, reheat_type: electric}
plant_loops:
  - name: HHW
    design_delta_t: 10.0
    supply_equipment:
      - {type: pump, name: HHW Pump, design_flow_rate: autosize}
      - {type: boiler, name: Boiler, capacity: autosize, design_water_flow_rate: autosize}
  - name: CHW
    design_delta_t: 5.0
    supply_equipment:
      - {type: chiller, name: Chiller, capacity: autosize, cop: 4.0, condenser_type: water_cooled, condenser_plant_loop: CW}
  - name: CW
    design_delta_t: 5.0
    supply_equipment:
      - {type: pump, name: CW Pump, design_flow_rate: autosize}
      - {type: cooling_tower, name: Tower, design_water_flow: autosize, design_air_flow: 10.0, design_fan_power: 5000.0}
"#;

    fn model(yaml: &str) -> ModelInput {
        parse_model_yaml(yaml).expect("test model parses")
    }

    fn validate(yaml: &str) -> ValidationResult {
        let m = model(yaml);
        let mut result = ValidationResult::new();
        validate_plant_connections(&m, &mut result);
        result
    }

    fn sizing_inputs(pairs: &[(&str, f64)]) -> PlantSizingInputs {
        PlantSizingInputs {
            demand_capacity: pairs.iter().map(|(n, c)| (n.to_string(), *c)).collect(),
            equipment_capacity: [("Boiler", AUTOSIZE), ("Chiller", AUTOSIZE)]
                .iter()
                .map(|(n, c)| (n.to_string(), *c))
                .collect(),
            coincident_peak_heating: 0.0,
            coincident_peak_cooling: 0.0,
        }
    }

    #[test]
    fn air_side_demands_are_the_connected_water_components() {
        let m = model(PLANT_MODEL);
        let by_loop = air_side_demands_by_loop(&m);
        assert_eq!(by_loop["HHW"], vec!["Preheat", "Box A"]);
        assert_eq!(by_loop["CHW"], vec!["CHW Coil"]);
        assert!(by_loop["CW"].is_empty());
    }

    #[test]
    fn coil_load_sums_thermal_output_with_zone_multipliers() {
        let outputs: HashMap<String, HashMap<String, f64>> =
            [("Preheat", 3000.0), ("Box A", 500.0), ("DX Coil", -9000.0)]
                .iter()
                .map(|(n, q)| {
                    let row = [("thermal_output".to_string(), *q)].into_iter().collect();
                    (n.to_string(), row)
                })
                .collect();
        let mult: HashMap<String, f64> = [("Box A".to_string(), 4.0)].into_iter().collect();
        let hhw = vec!["Preheat".to_string(), "Box A".to_string()];
        assert_eq!(coil_plant_load(&hhw, &outputs, &mult), 5000.0);
        // A component with no outputs this iteration contributes nothing.
        let absent = vec!["CHW Coil".to_string()];
        assert_eq!(coil_plant_load(&absent, &outputs, &mult), 0.0);
    }

    #[test]
    fn connected_model_validates_cleanly() {
        let r = validate(PLANT_MODEL);
        assert_eq!(r.error_count(), 0, "{}", r.to_err_file());
        assert_eq!(r.warning_count(), 0, "{}", r.to_err_file());
    }

    #[test]
    fn unconnected_water_coil_is_severe_when_plant_loops_exist() {
        let yaml = PLANT_MODEL.replace(", plant_loop: HHW}", "}");
        let r = validate(&yaml);
        let err = r.to_err_file();
        assert!(err.contains("heating coil 'Preheat' uses hot_water but names no plant_loop"));
        assert!(err.contains("VAV box 'Box A' uses hot_water"), "{err}");
        assert!(err.contains("Plant loop 'HHW' has no demand"), "{err}");
    }

    #[test]
    fn unconnected_water_coil_is_fine_without_plant_loops() {
        // ASHRAE 140 AE cases: a chilled-water coil on an ideal water source.
        let yaml = r#"
simulation: {}
weather_files: []
air_loops:
  - name: AHU
    equipment:
      - {type: cooling_coil, name: CHW Coil, source: chilled_water, capacity: 5000}
"#;
        let r = validate(yaml);
        assert_eq!(r.error_count(), 0, "{}", r.to_err_file());
    }

    #[test]
    fn plant_loop_on_a_dx_coil_or_unknown_loop_is_severe() {
        let yaml = PLANT_MODEL
            .replace(
                "{type: cooling_coil, name: DX Coil, capacity: autosize}",
                "{type: cooling_coil, name: DX Coil, capacity: autosize, plant_loop: CHW}",
            )
            .replace("plant_loop: CHW}", "plant_loop: CHWX}");
        let err = validate(&yaml).to_err_file();
        assert!(
            err.contains("cooling coil 'DX Coil' names plant_loop 'CHWX' but its source is 'dx'")
        );
        assert!(err.contains("Plant loop 'CHWX' referenced by cooling coil 'CHW Coil' not found"));
    }

    #[test]
    fn autosized_plant_is_sized_from_connected_coils() {
        let m = model(PLANT_MODEL);
        let mut inputs = sizing_inputs(&[
            ("Preheat", 30_000.0),
            ("Box A", 10_000.0),
            ("CHW Coil", 80_000.0),
        ]);
        inputs.coincident_peak_heating = 25_000.0;
        let sizes = size_plant(&m, &inputs).expect("sizes");

        assert_eq!(sizes.loop_design_load["HHW"], 40_000.0);
        assert_eq!(sizes.capacity["Boiler"], 40_000.0);
        assert_eq!(sizes.capacity["Chiller"], 80_000.0);
        // Condenser loop rejects the chiller's capacity plus compressor heat.
        assert_eq!(sizes.loop_design_load["CW"], 100_000.0);

        let flow = |w: f64, dt: f64| w / (RHO_WATER * CP_WATER * dt);
        assert_eq!(sizes.design_flow["Boiler"], flow(40_000.0, 10.0));
        // Heating-only loop pump keeps the coincident building peak.
        assert_eq!(sizes.design_flow["HHW Pump"], flow(25_000.0, 10.0));
        assert_eq!(sizes.design_flow["Tower"], flow(100_000.0, 5.0));
        assert_eq!(sizes.design_flow["CW Pump"], flow(100_000.0, 5.0));
    }

    #[test]
    fn hard_sized_equipment_is_left_alone_and_feeds_the_condenser() {
        let m = model(PLANT_MODEL);
        let mut inputs = sizing_inputs(&[
            ("Preheat", 30_000.0),
            ("Box A", 10_000.0),
            ("CHW Coil", 80_000.0),
        ]);
        inputs
            .equipment_capacity
            .insert("Chiller".to_string(), 120_000.0);
        let sizes = size_plant(&m, &inputs).expect("sizes");
        assert!(!sizes.capacity.contains_key("Chiller"));
        assert_eq!(sizes.loop_design_load["CW"], 150_000.0);
        // No design days: the heating pump falls back to the loop design load.
        let hhw_pump = sizes.design_flow["HHW Pump"];
        assert_eq!(hhw_pump, 40_000.0 / (RHO_WATER * CP_WATER * 10.0));
    }

    #[test]
    fn hard_sized_boiler_still_gets_its_autosized_water_flow() {
        // The DOE prototypes hard-size the boiler and autosize its flow.
        let m = model(PLANT_MODEL);
        let mut inputs = sizing_inputs(&[
            ("Preheat", 30_000.0),
            ("Box A", 10_000.0),
            ("CHW Coil", 80_000.0),
        ]);
        inputs
            .equipment_capacity
            .insert("Boiler".to_string(), 55_000.0);
        let sizes = size_plant(&m, &inputs).expect("sizes");
        assert!(!sizes.capacity.contains_key("Boiler"));
        let flow = sizes.design_flow["Boiler"];
        assert_eq!(flow, 40_000.0 / (RHO_WATER * CP_WATER * 10.0));
    }

    #[test]
    fn idle_loop_equipment_sizes_to_zero() {
        // A loop nothing draws on never runs; like E+, its autosized
        // equipment sizes to zero rather than stopping the run.
        let yaml = r#"
simulation: {}
weather_files: []
plant_loops:
  - name: Spare
    supply_equipment:
      - {type: pump, name: Spare Pump, design_flow_rate: autosize}
      - {type: boiler, name: Boiler, capacity: autosize}
"#;
        let m = model(yaml);
        let sizes = size_plant(&m, &sizing_inputs(&[])).expect("sizes");
        assert_eq!(sizes.capacity["Boiler"], 0.0);
        assert_eq!(sizes.design_flow["Spare Pump"], 0.0);
    }

    #[test]
    fn unresolvable_autosize_is_reported() {
        let m = model(PLANT_MODEL);
        // The CHW coil was never sized, and nothing else feeds the CHW loop.
        let inputs = sizing_inputs(&[("Preheat", 30_000.0), ("Box A", 10_000.0)]);
        let errors = size_plant(&m, &inputs).expect_err("chiller cannot be sized");
        let all = errors.join("\n");
        assert!(all.contains("cooling coil 'CHW Coil' on plant loop 'CHW' has no sized capacity"));
        assert!(
            all.contains("Cannot autosize capacity of 'Chiller'"),
            "{all}"
        );
        assert!(
            all.contains("Cannot autosize design_water_flow of 'Tower'"),
            "{all}"
        );
    }
}
