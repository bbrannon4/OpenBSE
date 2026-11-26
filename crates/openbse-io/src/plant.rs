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

use openbse_components::chiller::ChillerModel;
use openbse_core::types::is_autosize;

use crate::input::{
    ChillerHeatRecoveryInput, ChillerInput, CoolingCoilInput, CurveInput, EquipmentInput,
    HeatingCoilInput, ModelInput, PlantEquipmentInput, PlantLoopInput, PumpTypeInput,
    RadiantPanelSourceInput, TerminalInput, ValidationResult, VavBoxInput,
};

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

/// Water flow [kg/s] the given air-side components draw this HVAC iteration,
/// each scaled by its zone multiplier. Components without a water side report
/// none.
pub fn coil_plant_flow(
    components: &[String],
    outputs: &HashMap<String, HashMap<String, f64>>,
    zone_multiplier: &HashMap<String, f64>,
) -> f64 {
    components
        .iter()
        .filter_map(|name| {
            let m = outputs.get(name)?.get("water_mass_flow").copied()?;
            Some(m * zone_multiplier.get(name).copied().unwrap_or(1.0))
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

    for al in &model.air_loops {
        for eq in &al.equipment {
            if let EquipmentInput::HeatingCoil(c) = eq {
                if c.ua_model.is_some() {
                    result.push_warning(format!(
                        "Heating coil '{}': ua_model is not read; a hot-water coil on \
                         a plant loop always uses the UA (ε-NTU) model",
                        c.name
                    ));
                }
            }
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

/// Cooling tower inputs the tower model reads, or would ignore.
pub fn validate_cooling_towers(model: &ModelInput, result: &mut ValidationResult) {
    use openbse_components::cooling_tower::CoolingTowerType;
    let towers = model
        .plant_loops
        .iter()
        .flat_map(|pl| &pl.supply_equipment)
        .filter_map(|eq| match eq {
            PlantEquipmentInput::CoolingTower(ct) => Some(ct),
            _ => None,
        });
    for ct in towers {
        let unused: Vec<&str> = [
            ("design_air_flow", ct.design_air_flow),
            ("design_inlet_water_temp", ct.design_inlet_water_temp),
            ("design_approach", ct.design_approach),
        ]
        .into_iter()
        .filter_map(|(name, v)| v.map(|_| name))
        .collect();
        if !unused.is_empty() {
            result.push_warning(format!(
                "Cooling tower '{}': {} not used; the tower rejects its load down to \
                 2 K above the wet-bulb, with fan power from the load over its design \
                 capacity (#252)",
                ct.name,
                unused.join(", ")
            ));
        }
        if ct.fan_power_curve.is_some() && ct.tower_type != CoolingTowerType::VariableSpeed {
            result.push_severe(format!(
                "Cooling tower '{}': fan_power_curve applies only to a variable_speed tower",
                ct.name
            ));
        }
    }
}

/// Thermal storage is charged by its loop's chillers, so it must sit on a
/// loop that has one.
pub fn validate_thermal_storage(model: &ModelInput, result: &mut ValidationResult) {
    for pl in &model.plant_loops {
        let has_chiller = pl
            .supply_equipment
            .iter()
            .any(|eq| matches!(eq, PlantEquipmentInput::Chiller(_)));
        for eq in &pl.supply_equipment {
            if let PlantEquipmentInput::ThermalStorage(ts) = eq {
                if !has_chiller {
                    result.push_severe(format!(
                        "Thermal storage '{}' is on plant loop '{}', which has no chiller \
                         to charge it; put it on a chilled-water loop with a chiller",
                        ts.name, pl.name
                    ));
                }
            }
        }
    }
}

/// A part-load curve applies only to a variable-speed pump; E+'s
/// `Pump:ConstantSpeed` has none, so one given there would be ignored.
pub fn validate_pumps(model: &ModelInput, result: &mut ValidationResult) {
    let plant = model
        .plant_loops
        .iter()
        .flat_map(|pl| &pl.supply_equipment)
        .filter_map(|eq| match eq {
            PlantEquipmentInput::Pump(p) => Some(p),
            _ => None,
        });
    let dhw = model.dhw_systems.iter().filter_map(|d| d.pump.as_ref());
    for p in dhw.clone() {
        if p.loop_side == crate::input::PumpLoopSide::Demand {
            result.push_severe(format!(
                "DHW pump '{}': loop_side demand applies only to plant loop pumps",
                p.name
            ));
        }
    }
    for p in plant.chain(dhw) {
        if p.pump_type == PumpTypeInput::ConstantSpeed && p.power_curve.is_some() {
            result.push_severe(format!(
                "Pump '{}': power_curve applies only to a variable_speed pump; a \
                 constant_speed pump draws its design power whenever it runs",
                p.name
            ));
        }
    }
}

/// A load-range operation scheme names units its loop supplies with and that
/// take a load (boilers, chillers, thermal storage, external plant), over
/// ranges with `0 <= lower <= upper`.
pub fn validate_plant_operation(model: &ModelInput, result: &mut ValidationResult) {
    for pl in &model.plant_loops {
        let Some(op) = &pl.operation else { continue };
        if op.load_ranges.is_empty() {
            result.push_severe(format!(
                "Plant loop '{}': operation has no load_ranges, so no unit would run",
                pl.name
            ));
        }
        for r in &op.load_ranges {
            if !(r.lower >= 0.0 && r.lower <= r.upper) {
                result.push_severe(format!(
                    "Plant loop '{}': load range [{}, {}] W needs 0 <= lower <= upper",
                    pl.name, r.lower, r.upper
                ));
            }
            for name in &r.equipment {
                let unit = pl.supply_equipment.iter().find(|eq| eq.name() == name);
                match unit {
                    Some(
                        PlantEquipmentInput::Boiler(_)
                        | PlantEquipmentInput::Chiller(_)
                        | PlantEquipmentInput::ThermalStorage(_)
                        | PlantEquipmentInput::ExternalPlant(_),
                    ) => {}
                    Some(_) => result.push_severe(format!(
                        "Plant loop '{}': operation names '{name}', which takes no load; \
                         list boilers, chillers, thermal storage or external plant",
                        pl.name
                    )),
                    None => result.push_severe(format!(
                        "Plant loop '{}': operation names '{name}', which is not in its \
                         supply_equipment",
                        pl.name
                    )),
                }
            }
        }
    }
}

/// Check chiller inputs the parser cannot: the reformulated EIR model's
/// requirements, the min unloading ratio and condenser heat recovery.
pub fn validate_chiller_models(model: &ModelInput, result: &mut ValidationResult) {
    for pl in &model.plant_loops {
        for eq in &pl.supply_equipment {
            if let PlantEquipmentInput::Chiller(c) = eq {
                match c.chw_setpoint {
                    Some(sp) if (sp - pl.design_supply_temp).abs() > 1e-6 => {
                        result.push_severe(format!(
                            "Chiller '{}': chw_setpoint {sp} differs from plant loop '{}' \
                             design_supply_temp {}; a chiller leaves at its loop's setpoint, \
                             so remove chw_setpoint or make them equal",
                            c.name, pl.name, pl.design_supply_temp
                        ))
                    }
                    _ => {}
                }
            }
        }
    }
    let chillers = model
        .plant_loops
        .iter()
        .flat_map(|pl| &pl.supply_equipment)
        .filter_map(|eq| match eq {
            PlantEquipmentInput::Chiller(c) => Some(c),
            _ => None,
        });
    for c in chillers {
        if c.model == ChillerModel::ReformulatedEir && c.a205_file.is_none() {
            validate_reformulated_eir(model, c, result);
        }
        if let Some(r) = c.min_unloading_ratio {
            if !(c.min_plr..=1.0).contains(&r) {
                result.push_severe(format!(
                    "Chiller '{}': min_unloading_ratio {r} must be between min_plr ({}) and 1, \
                     as in E+",
                    c.name, c.min_plr
                ));
            }
        }
        if let Some(hr) = c.heat_recovery.as_ref() {
            validate_heat_recovery(model, c, hr, result);
        }
    }
}

/// E+ Chiller:Electric:ReformulatedEIR is water-cooled only and requires all
/// three curves, with EIR-f(PLR) a function of leaving condenser temperature
/// and PLR. The fallbacks used by the EIR model are referenced to the
/// entering condenser temperature, so there is nothing to fall back on.
fn validate_reformulated_eir(model: &ModelInput, c: &ChillerInput, result: &mut ValidationResult) {
    if c.condenser_type != "water_cooled" {
        result.push_severe(format!(
            "Chiller '{}' uses model reformulated_eir, which is water-cooled only; \
             set condenser_type: water_cooled or use model: eir",
            c.name
        ));
    }
    let missing: Vec<&str> = [
        ("capft", c.capft.is_some() || c.capft_curve.is_some()),
        ("eirft", c.eirft.is_some() || c.eirft_curve.is_some()),
        ("eirfplr", c.eirfplr.is_some() || c.eirfplr_curve.is_some()),
    ]
    .into_iter()
    .filter(|(_, set)| !set)
    .map(|(field, _)| field)
    .collect();
    if !missing.is_empty() {
        result.push_severe(format!(
            "Chiller '{}' uses model reformulated_eir but has no {} curve; this model \
             needs capft, eirft and eirfplr, all of leaving condenser water temperature",
            c.name,
            missing.join(", ")
        ));
    }
    if !eirfplr_is_two_variable(model, c) {
        result.push_severe(format!(
            "Chiller '{}' uses model reformulated_eir, so its eirfplr must be a function \
             of leaving condenser water temperature (x) and PLR (y): a bicubic (10 \
             coefficients), a biquadratic (6) or a two-axis table",
            c.name
        ));
    }
}

/// Heat recovery follows E+ Chiller:Electric:EIR, which offers it only with a
/// water-cooled condenser.
fn validate_heat_recovery(
    model: &ModelInput,
    c: &ChillerInput,
    hr: &ChillerHeatRecoveryInput,
    result: &mut ValidationResult,
) {
    let mut problem = |msg: String| result.push_severe(format!("Chiller '{}': {msg}", c.name));
    if c.a205_file.is_some() {
        problem("heat_recovery is not supported with a205_file".to_string());
    }
    if c.model != ChillerModel::Eir {
        problem("heat_recovery needs model: eir (E+ Chiller:Electric:EIR)".to_string());
    }
    if c.condenser_type != "water_cooled" {
        problem("heat_recovery needs condenser_type: water_cooled, as in E+".to_string());
    }
    if !model.plant_loops.iter().any(|pl| pl.name == hr.plant_loop) {
        problem(format!(
            "heat_recovery plant_loop '{}' not found in plant_loops",
            hr.plant_loop
        ));
    }
    if !(hr.capacity_fraction > 0.0 && hr.capacity_fraction <= 1.0) {
        problem(format!(
            "heat_recovery capacity_fraction {} must be in (0, 1]",
            hr.capacity_fraction
        ));
    }
    if hr.design_flow < 0.0 {
        problem("heat_recovery design_flow must not be negative".to_string());
    }
}

/// Whether a chiller's EIR-f(PLR) curve takes two variables (true when it
/// has none: that is reported as missing).
fn eirfplr_is_two_variable(model: &ModelInput, c: &ChillerInput) -> bool {
    if let Some(name) = c.eirfplr_curve.as_deref() {
        return model
            .performance_curves
            .iter()
            .find(|pc| pc.name() == name)
            .is_none_or(|pc| pc.dimensions() == 2);
    }
    c.eirfplr
        .as_ref()
        .is_none_or(|ci: &CurveInput| matches!(ci.coefficients.len(), 6 | 10))
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
        // E+ sizes a coil's or boiler's water flow at its conversion temperature.
        let conversion_temp = pl.water_conversion_temp();
        let rho_cp = openbse_psychrometrics::water_rho(conversion_temp)
            * openbse_psychrometrics::water_cp(conversion_temp);
        let to_flow = |watts: f64| watts / (rho_cp * pl.design_delta_t.max(1.0));
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

    /// Load [W] a pump is sized for: the loop design load, so that its flow is
    /// the sum of its demand components' design flows (E+ `Pumps::SizePump`,
    /// `PlantSizData.DesVolFlowRate`). A loop with no sized demand falls back to
    /// its installed boilers and chillers.
    fn pump_basis(&mut self, pl: &PlantLoopInput, loop_load: f64) -> f64 {
        if loop_load > 0.0 {
            return loop_load;
        }
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
    }
}

/// A supply unit's load limits for distribution [W]: E+ `MinLoad`,
/// `MaxLoad` and `OptLoad` (rated capacity times the minimum, maximum and
/// optimum part load ratios).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitLoadLimits {
    pub min: f64,
    pub max: f64,
    pub opt: f64,
}

/// Loads below this are no load [W] (E+ `HVAC::SmallLoad`).
const SMALL_LOAD: f64 = 1.0;

/// The load range a loop load magnitude falls in (E+ `ManagePlantLoadDistribution`
/// for range-based schemes): the first range with `lower <= load <= upper`;
/// above that of the last stage (the range with the highest upper limit),
/// the last stage. Returns the range's index, or `None` when no range holds
/// the load and none runs.
pub fn select_load_range(ranges: &[crate::input::LoadRangeInput], load: f64) -> Option<usize> {
    let last_stage = last_load_stage(ranges)?;
    ranges
        .iter()
        .enumerate()
        .find(|(i, r)| (r.lower..=r.upper).contains(&load) || (*i == last_stage && load > r.upper))
        .map(|(i, _)| i)
}

/// The range with the highest upper limit (E+ `EquipListNumForLastStage`).
pub fn last_load_stage(ranges: &[crate::input::LoadRangeInput]) -> Option<usize> {
    ranges
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.upper.total_cmp(&b.1.upper))
        .map(|(i, _)| i)
}

/// Each unit's share of a loop load magnitude `demand` [W] under an E+ Load
/// Distribution Scheme (`PlantCondLoopOperation.cc`, `DistributePlantLoad`).
/// `stage_cap` is the last stage's upper limit when the range is the last
/// stage, which caps each unit's load (`AdjustChangeInLoadForLastStageUpperRangeLimit`).
/// A unit with no capacity limit (infinite maximum) takes what is left.
pub fn distribute_plant_load(
    scheme: crate::input::LoadDistribution,
    units: &[UnitLoadLimits],
    demand: f64,
    stage_cap: Option<f64>,
) -> Vec<f64> {
    use crate::input::LoadDistribution as D;
    let n = units.len();
    let mut loads = vec![0.0; n];
    let mut rem = demand.max(0.0);
    if n == 0 || rem < SMALL_LOAD {
        return loads;
    }
    let cap = |x: f64| stage_cap.map_or(x, |c| x.min(c)).max(0.0);
    let limited = |limit: f64, rem: f64| if limit > 0.0 { limit.min(rem) } else { rem };
    let settle = |rem: &mut f64| {
        if *rem < SMALL_LOAD {
            *rem = 0.0;
        }
    };
    match scheme {
        D::Sequential => {
            for (l, u) in loads.iter_mut().zip(units) {
                let change = cap(limited(u.max, rem));
                *l = change;
                rem -= change;
                settle(&mut rem);
            }
        }
        D::Optimal => {
            for (l, u) in loads.iter_mut().zip(units) {
                let change = cap(limited(u.opt, rem));
                *l = change;
                rem -= change;
                settle(&mut rem);
            }
            if rem > SMALL_LOAD {
                let divide = rem / n as f64;
                for (l, u) in loads.iter_mut().zip(units) {
                    let new = u.max.min(*l + divide);
                    rem -= new - *l;
                    *l = new;
                    settle(&mut rem);
                }
            }
            if rem > SMALL_LOAD {
                for (l, u) in loads.iter_mut().zip(units) {
                    let change = rem.min(u.max - *l).max(0.0);
                    *l += change;
                    rem -= change;
                    settle(&mut rem);
                }
            }
        }
        D::Uniform => {
            let share = rem / n as f64;
            for (l, u) in loads.iter_mut().zip(units) {
                let change = cap(if u.max > 0.0 { u.max.min(share) } else { rem });
                *l = change;
                rem -= change;
                settle(&mut rem);
            }
            if rem > SMALL_LOAD {
                for (l, u) in loads.iter_mut().zip(units) {
                    let change = (u.max - *l).min(rem).max(0.0);
                    *l += change;
                    rem -= change;
                    settle(&mut rem);
                }
            }
        }
        D::UniformPlr | D::SequentialUniformPlr => {
            // Plant capacity and the largest minimum part load, accumulated in
            // order: all units (UniformPLR), or as far as covers the load.
            let mut accrued: Vec<(f64, f64)> = Vec::with_capacity(n);
            let (mut capacity, mut largest_min) = (0.0_f64, 0.0_f64);
            for u in units {
                capacity += u.max;
                let min_plr = if u.max < SMALL_LOAD {
                    0.0
                } else {
                    u.min / u.max
                };
                largest_min = largest_min.max(min_plr);
                accrued.push((capacity, largest_min));
                if scheme == D::SequentialUniformPlr && rem <= capacity {
                    break;
                }
            }
            if scheme == D::UniformPlr {
                // From all units on down to one, drop the last units while the
                // load is under the remaining set's minimum.
                for i in (0..accrued.len()).rev() {
                    let (c, m) = accrued[i];
                    if i == 0 || rem < m * c {
                        capacity = c;
                        if i == 0 {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            let plr = if capacity > 0.0 {
                (rem / capacity).min(1.0)
            } else {
                0.0
            };
            for (l, u) in loads.iter_mut().zip(units) {
                let change = cap(if u.max > 0.0 {
                    rem.min(plr * u.max)
                } else {
                    rem
                });
                *l = change;
                rem -= change;
                settle(&mut rem);
            }
        }
    }
    loads
}

#[cfg(test)]
mod load_distribution_tests {
    use super::*;
    use crate::input::{LoadDistribution as D, LoadRangeInput};

    fn unit(cap: f64, min_plr: f64, opt_plr: f64) -> UnitLoadLimits {
        UnitLoadLimits {
            min: cap * min_plr,
            max: cap,
            opt: cap * opt_plr,
        }
    }

    fn close(a: &[f64], b: &[f64]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 1e-6, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn ranges_pick_the_first_holding_the_load_and_the_last_stage_above_all() {
        let r = |lower, upper| LoadRangeInput {
            lower,
            upper,
            equipment: vec![],
        };
        // DOE LargeOffice: chiller 1 to 1.57 MW, both above.
        let ranges = [r(0.0, 1.57e6), r(1.57e6, 1.0e14)];
        assert_eq!(select_load_range(&ranges, 1.0e6), Some(0));
        assert_eq!(select_load_range(&ranges, 1.57e6), Some(0));
        assert_eq!(select_load_range(&ranges, 2.0e6), Some(1));
        let capped = [r(0.0, 1.0e6), r(1.0e6, 2.0e6)];
        assert_eq!(select_load_range(&capped, 3.0e6), Some(1));
        let gap = [r(1.0e5, 1.0e6)];
        assert_eq!(select_load_range(&gap, 5.0e4), None);
    }

    #[test]
    fn sequential_loads_units_in_order_to_their_maximum() {
        let u = [unit(1000.0, 0.25, 1.0), unit(1000.0, 0.25, 1.0)];
        close(
            &distribute_plant_load(D::Sequential, &u, 1500.0, None),
            &[1000.0, 500.0],
        );
        close(
            &distribute_plant_load(D::Sequential, &u, 600.0, None),
            &[600.0, 0.0],
        );
    }

    #[test]
    fn uniform_splits_equally_then_fills_in_order() {
        let u = [unit(1000.0, 0.25, 1.0), unit(1000.0, 0.25, 1.0)];
        close(
            &distribute_plant_load(D::Uniform, &u, 1500.0, None),
            &[750.0, 750.0],
        );
        let uneven = [unit(400.0, 0.0, 1.0), unit(1000.0, 0.0, 1.0)];
        // Equal 600 each: the first takes 400; the leftover 200 goes in order.
        close(
            &distribute_plant_load(D::Uniform, &uneven, 1200.0, None),
            &[400.0, 800.0],
        );
    }

    #[test]
    fn optimal_loads_to_the_optimum_then_shares_the_rest() {
        let u = [unit(1000.0, 0.1, 0.6), unit(1000.0, 0.1, 0.6)];
        // 600 + 600 at optimum, the remaining 300 split equally.
        close(
            &distribute_plant_load(D::Optimal, &u, 1500.0, None),
            &[750.0, 750.0],
        );
        close(
            &distribute_plant_load(D::Optimal, &u, 500.0, None),
            &[500.0, 0.0],
        );
    }

    #[test]
    fn uniform_plr_runs_units_at_one_part_load() {
        let u = [unit(1000.0, 0.25, 1.0), unit(3000.0, 0.25, 1.0)];
        close(
            &distribute_plant_load(D::UniformPlr, &u, 2000.0, None),
            &[500.0, 1500.0],
        );
        // Under both units' minimum (0.25 × 4000) only the first runs.
        close(
            &distribute_plant_load(D::UniformPlr, &u, 800.0, None),
            &[800.0, 0.0],
        );
    }

    #[test]
    fn sequential_uniform_plr_uses_as_few_units_as_cover_the_load() {
        let u = [
            unit(1000.0, 0.0, 1.0),
            unit(1000.0, 0.0, 1.0),
            unit(1000.0, 0.0, 1.0),
        ];
        close(
            &distribute_plant_load(D::SequentialUniformPlr, &u, 1500.0, None),
            &[750.0, 750.0, 0.0],
        );
    }

    #[test]
    fn last_stage_limit_caps_each_unit_and_no_load_runs_nothing() {
        let u = [unit(1000.0, 0.0, 1.0)];
        close(
            &distribute_plant_load(D::Sequential, &u, 900.0, Some(500.0)),
            &[500.0],
        );
        close(&distribute_plant_load(D::Uniform, &u, 0.5, None), &[0.0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_model_yaml;
    use openbse_core::types::AUTOSIZE;
    use openbse_psychrometrics::{
        water_cp, water_rho, COLD_WATER_CONVERSION_TEMP, HOT_WATER_CONVERSION_TEMP,
    };

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
      - {type: cooling_tower, name: Tower, design_water_flow: autosize, design_fan_power: 5000.0}
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
    fn operation_scheme_names_load_taking_units_of_its_loop() {
        let boiler =
            "      - {type: boiler, name: Boiler, capacity: autosize, design_water_flow_rate: autosize}";
        let with = |ranges: &str| {
            PLANT_MODEL.replace(
                boiler,
                &format!(
                    "{boiler}\n    operation: {{distribution: uniform, load_ranges: [{ranges}]}}"
                ),
            )
        };
        let mut r = ValidationResult::new();
        validate_plant_operation(
            &model(&with("{lower: 0, upper: 1.0e9, equipment: [Boiler]}")),
            &mut r,
        );
        assert_eq!(r.error_count(), 0, "{}", r.to_err_file());

        let mut r = ValidationResult::new();
        validate_plant_operation(
            &model(&with("{lower: 5, upper: 1, equipment: [HHW Pump, Nope]}")),
            &mut r,
        );
        let err = r.to_err_file();
        assert!(err.contains("needs 0 <= lower <= upper"), "{err}");
        assert!(err.contains("'HHW Pump', which takes no load"), "{err}");
        assert!(
            err.contains("'Nope', which is not in its supply_equipment"),
            "{err}"
        );
    }

    #[test]
    fn pump_inputs_are_checked() {
        let pump = "{type: pump, name: HHW Pump, design_flow_rate: autosize}";
        let with = |p: &str| PLANT_MODEL.replace(pump, p);

        let yaml = with(
            "{type: pump, name: HHW Pump, design_flow_rate: autosize, \
                         pump_type: constant_speed, power_curve: [0, 1, 0, 0]}",
        );
        let mut r = ValidationResult::new();
        validate_pumps(&model(&yaml), &mut r);
        assert!(r
            .to_err_file()
            .contains("power_curve applies only to a variable_speed pump"));

        let mut r = ValidationResult::new();
        let yaml = with(
            "{type: pump, name: HHW Pump, design_flow_rate: autosize, \
                         power_curve: [0, 1, 0, 0]}",
        );
        validate_pumps(&model(&yaml), &mut r);
        assert_eq!(r.error_count(), 0, "{}", r.to_err_file());

        // A curve with the wrong number of coefficients, or an unknown pump
        // type, is refused rather than replaced by the default.
        for bad in [
            "{type: pump, name: HHW Pump, design_flow_rate: autosize, power_curve: [0, 1, 0]}",
            "{type: pump, name: HHW Pump, design_flow_rate: autosize, pump_type: headered}",
            "{type: pump, name: HHW Pump, design_flow_rate: autosize, role: primary}",
        ] {
            assert!(parse_model_yaml(&with(bad)).is_err(), "{bad}");
        }
    }

    const CHILLER: &str = "{type: chiller, name: Chiller, capacity: autosize, cop: 4.0, \
                           condenser_type: water_cooled, condenser_plant_loop: CW}";

    fn validate_chillers(yaml: &str) -> ValidationResult {
        let m = model(yaml);
        let mut result = ValidationResult::new();
        validate_chiller_models(&m, &mut result);
        result
    }

    #[test]
    fn cooling_tower_inputs_are_checked() {
        let tower =
            "{type: cooling_tower, name: Tower, design_water_flow: autosize, design_fan_power: 5000.0}";
        let check = |t: &str| {
            let mut r = ValidationResult::new();
            validate_cooling_towers(&model(&PLANT_MODEL.replace(tower, t)), &mut r);
            r
        };
        assert_eq!(check(tower).warning_count(), 0);
        let r = check(
            "{type: cooling_tower, name: Tower, design_fan_power: 5000.0, design_air_flow: 10.0, \
             design_approach: 3.9}",
        );
        assert!(r
            .to_err_file()
            .contains("design_air_flow, design_approach not used"));
        let r = check(
            "{type: cooling_tower, name: Tower, design_fan_power: 5000.0, tower_type: two_speed, \
             fan_power_curve: [0, 0, 0, 1, 0]}",
        );
        assert!(r.to_err_file().contains("only to a variable_speed tower"));
        for bad in [
            "{type: cooling_tower, name: Tower, design_fan_power: 5000.0, tower_type: cell}",
            "{type: cooling_tower, name: Tower, design_fan_power: 5000.0, fan_power_curve: [0, 1]}",
        ] {
            assert!(
                parse_model_yaml(&PLANT_MODEL.replace(tower, bad)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn thermal_storage_needs_a_chiller_on_its_loop() {
        let pump = "{type: pump, name: HHW Pump, design_flow_rate: autosize}";
        let tes = "{type: thermal_storage, name: TES, tes_type: ice, capacity_wh: 1000, \
                   max_charge_rate: 100, max_discharge_rate: 100, control_strategy: full_storage}";
        let yaml = PLANT_MODEL.replace(pump, &format!("{pump}\n      - {tes}"));
        let mut r = ValidationResult::new();
        validate_thermal_storage(&model(&yaml), &mut r);
        assert!(
            r.to_err_file().contains("no chiller to charge it"),
            "{}",
            r.to_err_file()
        );
    }

    #[test]
    fn chw_setpoint_must_match_its_loop() {
        let yaml = PLANT_MODEL.replace(
            CHILLER,
            "{type: chiller, name: Chiller, capacity: autosize, cop: 4.0, chw_setpoint: 5.0, \
             condenser_type: water_cooled, condenser_plant_loop: CW}",
        );
        let err = validate_chillers(&yaml).to_err_file();
        assert!(
            err.contains("chw_setpoint 5 differs from plant loop"),
            "{err}"
        );
        assert_eq!(validate_chillers(PLANT_MODEL).error_count(), 0);
    }

    #[test]
    fn reformulated_chiller_needs_water_cooling_and_two_variable_curves() {
        let yaml = PLANT_MODEL.replace(
            CHILLER,
            "{type: chiller, name: Chiller, capacity: autosize, cop: 4.0, \
             model: reformulated_eir, eirfplr: {coefficients: [0.1, 0.9, 0.0]}}",
        );
        let err = validate_chillers(&yaml).to_err_file();
        assert!(err.contains("'Chiller' uses model reformulated_eir, which is water-cooled only"));
        assert!(err.contains("has no capft, eirft curve"), "{err}");
        assert!(
            err.contains("eirfplr must be a function of leaving condenser"),
            "{err}"
        );
    }

    #[test]
    fn reformulated_chiller_with_its_curves_validates() {
        let yaml = PLANT_MODEL.replace(
            CHILLER,
            "{type: chiller, name: Chiller, capacity: autosize, cop: 4.0, \
             condenser_type: water_cooled, condenser_plant_loop: CW, model: reformulated_eir, \
             capft: {coefficients: [1, 0, 0, 0, 0, 0]}, \
             eirft: {coefficients: [1, 0, 0, 0, 0, 0]}, \
             eirfplr_curve: EIRFPLR}",
        ) + "performance_curves:\n  - {name: EIRFPLR, curve_type: bicubic, \
              coefficients: [0, 0, 0, 1, 0, 0, 0, 0, 0, 0]}\n";
        let r = validate_chillers(&yaml);
        assert_eq!(r.error_count(), 0, "{}", r.to_err_file());
        // The same chiller as model eir needs none of this.
        let r = validate_chillers(PLANT_MODEL);
        assert_eq!(r.error_count(), 0, "{}", r.to_err_file());
    }

    #[test]
    fn chiller_heat_recovery_validates_against_eplus_rules() {
        let with_hr = |extra: &str| {
            PLANT_MODEL.replace(
                CHILLER,
                &format!(
                    "{{type: chiller, name: Chiller, capacity: autosize, cop: 4.0, \
                     condenser_type: water_cooled, condenser_plant_loop: CW, {extra}}}"
                ),
            )
        };
        let ok = with_hr(
            "min_plr: 0.1, min_unloading_ratio: 0.2, \
             heat_recovery: {plant_loop: HHW, leaving_setpoint: 48.89}",
        );
        let r = validate_chillers(&ok);
        assert_eq!(r.error_count(), 0, "{}", r.to_err_file());

        let bad = with_hr(
            "model: reformulated_eir, min_plr: 0.3, min_unloading_ratio: 0.2, \
             heat_recovery: {plant_loop: HWX, leaving_setpoint: 48.89, capacity_fraction: 1.5}",
        );
        let err = validate_chillers(&bad).to_err_file();
        assert!(
            err.contains("min_unloading_ratio 0.2 must be between min_plr (0.3)"),
            "{err}"
        );
        assert!(err.contains("heat_recovery needs model: eir"), "{err}");
        assert!(err.contains("plant_loop 'HWX' not found"), "{err}");
        assert!(
            err.contains("capacity_fraction 1.5 must be in (0, 1]"),
            "{err}"
        );
        let air_cooled = ok.replace("condenser_type: water_cooled, ", "");
        let err = validate_chillers(&air_cooled).to_err_file();
        assert!(
            err.contains("heat_recovery needs condenser_type: water_cooled"),
            "{err}"
        );
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
        let sizes = size_plant(&m, &inputs).expect("sizes");

        assert_eq!(sizes.loop_design_load["HHW"], 40_000.0);
        assert_eq!(sizes.capacity["Boiler"], 40_000.0);
        assert_eq!(sizes.capacity["Chiller"], 80_000.0);
        // Condenser loop rejects the chiller's capacity plus compressor heat.
        assert_eq!(sizes.loop_design_load["CW"], 100_000.0);

        let flow = |w: f64, dt: f64, t: f64| w / (water_rho(t) * water_cp(t) * dt);
        let (hot, cold) = (HOT_WATER_CONVERSION_TEMP, COLD_WATER_CONVERSION_TEMP);
        assert_eq!(sizes.design_flow["Boiler"], flow(40_000.0, 10.0, hot));
        // The pump carries what the coils on its loop draw, as the boiler does.
        assert_eq!(sizes.design_flow["HHW Pump"], sizes.design_flow["Boiler"]);
        assert_eq!(sizes.design_flow["Tower"], flow(100_000.0, 5.0, cold));
        assert_eq!(sizes.design_flow["CW Pump"], flow(100_000.0, 5.0, cold));
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
        let hot = HOT_WATER_CONVERSION_TEMP;
        assert_eq!(hhw_pump, 40_000.0 / (water_rho(hot) * water_cp(hot) * 10.0));
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
        let hot = HOT_WATER_CONVERSION_TEMP;
        assert_eq!(flow, 40_000.0 / (water_rho(hot) * water_cp(hot) * 10.0));
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
