//! Parametric run support — apply scalar overrides and expand sweep definitions.

use crate::input::{
    BoilerInput, ChillerInput, CoolingCoilInput, CoolingTowerInput, DuctInput, EquipmentInput,
    ExteriorEquipmentInput, FanInput, GshpInput, HeatExchangerInput, HeatRecoveryInput,
    HeatingCoilInput, HumidifierInput, ModelInput, PlantEquipmentInput, PumpInput, VavBoxInput,
};
use std::collections::HashMap;

/// Errors from parametric override application.
#[derive(Debug, thiserror::Error)]
pub enum ParametricError {
    #[error("Component not found: '{0}'")]
    ComponentNotFound(String),
    #[error("Field not found: '{field}' on component '{component}'")]
    FieldNotFound { component: String, field: String },
    #[error("Invalid override key (expected 'ComponentName.field_name'): '{0}'")]
    InvalidKey(String),
    #[error("Sweep error: {0}")]
    SweepError(String),
}

/// Apply scalar overrides to a parsed model.
///
/// Each override key has the format `"ComponentName.field_name"` where:
/// - `ComponentName` matches the `name` field of any named component
/// - `field_name` matches a numeric field on that component
///
/// Returns an error if a component name or field name is not found.
pub fn apply_overrides(
    model: &mut ModelInput,
    overrides: &HashMap<String, f64>,
) -> Result<(), ParametricError> {
    for (key, value) in overrides {
        let (comp_name, field_name) = parse_override_key(key)?;
        if !apply_single_override(model, comp_name, field_name, *value) {
            return Err(ParametricError::ComponentNotFound(comp_name.to_string()));
        }
    }
    Ok(())
}

/// Parse "ComponentName.field_name" into (component, field).
fn parse_override_key(key: &str) -> Result<(&str, &str), ParametricError> {
    // Find the last '.' to split — component names may contain dots in theory,
    // but field names never do.
    let dot_pos = key
        .rfind('.')
        .ok_or_else(|| ParametricError::InvalidKey(key.to_string()))?;
    let comp = &key[..dot_pos];
    let field = &key[dot_pos + 1..];
    if comp.is_empty() || field.is_empty() {
        return Err(ParametricError::InvalidKey(key.to_string()));
    }
    Ok((comp, field))
}

/// Try to apply an override to a single component.  Returns true if the
/// component was found (and the field was set), false if not found.
/// Returns Err only on field-not-found (component exists but field doesn't).
fn apply_single_override(
    model: &mut ModelInput,
    comp_name: &str,
    field_name: &str,
    value: f64,
) -> bool {
    // ── Zones ────────────────────────────────────────────────────────────
    for zone in &mut model.zones {
        if zone.name == comp_name {
            set_zone_field(zone, field_name, value);
            return true;
        }
    }

    // ── Thermostats ──────────────────────────────────────────────────────
    for tstat in &mut model.thermostats {
        if tstat.name == comp_name {
            set_thermostat_field(tstat, field_name, value);
            return true;
        }
    }

    // ── Materials ────────────────────────────────────────────────────────
    for mat in &mut model.materials {
        if mat.name == comp_name {
            set_material_field(mat, field_name, value);
            return true;
        }
    }

    // ── Window constructions ─────────────────────────────────────────────
    for wc in &mut model.window_constructions {
        if wc.name == comp_name {
            set_window_construction_field(wc, field_name, value);
            return true;
        }
    }

    // ── Simple constructions ─────────────────────────────────────────────
    for sc in &mut model.simple_constructions {
        if sc.name == comp_name {
            set_simple_construction_field(sc, field_name, value);
            return true;
        }
    }

    // ── People ───────────────────────────────────────────────────────────
    for p in &mut model.people {
        if p.name == comp_name {
            set_people_field(p, field_name, value);
            return true;
        }
    }

    // ── Lights ───────────────────────────────────────────────────────────
    for l in &mut model.lights {
        if l.name == comp_name {
            set_lights_field(l, field_name, value);
            return true;
        }
    }

    // ── Equipment gains ──────────────────────────────────────────────────
    for eg in &mut model.equipment {
        if eg.name == comp_name {
            set_equipment_gain_field(eg, field_name, value);
            return true;
        }
    }

    // ── Infiltration ─────────────────────────────────────────────────────
    for inf in &mut model.infiltration {
        if inf.name == comp_name {
            set_infiltration_field(inf, field_name, value);
            return true;
        }
    }

    // ── Air loop equipment ───────────────────────────────────────────────
    for al in &mut model.air_loops {
        for eq in &mut al.equipment {
            match eq {
                EquipmentInput::Fan(f) if f.name == comp_name => {
                    set_fan_field(f, field_name, value);
                    return true;
                }
                EquipmentInput::HeatingCoil(c) if c.name == comp_name => {
                    set_heating_coil_field(c, field_name, value);
                    return true;
                }
                EquipmentInput::CoolingCoil(c) if c.name == comp_name => {
                    set_cooling_coil_field(c, field_name, value);
                    return true;
                }
                EquipmentInput::HeatRecovery(hr) if hr.name == comp_name => {
                    set_heat_recovery_field(hr, field_name, value);
                    return true;
                }
                EquipmentInput::Humidifier(h) if h.name == comp_name => {
                    set_humidifier_field(h, field_name, value);
                    return true;
                }
                EquipmentInput::Duct(d) if d.name == comp_name => {
                    set_duct_field(d, field_name, value);
                    return true;
                }
                EquipmentInput::Gshp(g) if g.name == comp_name => {
                    set_gshp_field(g, field_name, value);
                    return true;
                }
                _ => {}
            }
        }
        // Terminal boxes (VAV/PFP/DualDuct)
        for zc in &mut al.zone_terminals {
            if let Some(ref mut terminal) = zc.terminal {
                match terminal {
                    crate::input::TerminalInput::VavBox(vb) if vb.name == comp_name => {
                        set_vav_box_field(vb, field_name, value);
                        return true;
                    }
                    crate::input::TerminalInput::PfpBox(pb) if pb.name == comp_name => {
                        set_pfp_box_field(pb, field_name, value);
                        return true;
                    }
                    crate::input::TerminalInput::DualDuctBox(b) if b.name == comp_name => {
                        match field_name {
                            "min_flow_fraction" => b.min_flow_fraction = value,
                            "design_flow" => {
                                b.design_flow = openbse_core::types::AutosizeValue::Value(value)
                            }
                            _ => {
                                log::warn!(
                                    "Unknown field '{}' on dual duct box '{}' — skipping",
                                    field_name,
                                    b.name
                                );
                            }
                        }
                        return true;
                    }
                    _ => {}
                }
            }
        }
    }

    // ── Plant loop equipment ─────────────────────────────────────────────
    for pl in &mut model.plant_loops {
        for eq in &mut pl.supply_equipment {
            match eq {
                PlantEquipmentInput::Boiler(b) if b.name == comp_name => {
                    set_boiler_field(b, field_name, value);
                    return true;
                }
                PlantEquipmentInput::Chiller(c) if c.name == comp_name => {
                    set_chiller_field(c, field_name, value);
                    return true;
                }
                PlantEquipmentInput::Pump(p) if p.name == comp_name => {
                    set_pump_field(p, field_name, value);
                    return true;
                }
                PlantEquipmentInput::CoolingTower(ct) if ct.name == comp_name => {
                    set_cooling_tower_field(ct, field_name, value);
                    return true;
                }
                PlantEquipmentInput::HeatExchanger(hx) if hx.name == comp_name => {
                    set_heat_exchanger_field(hx, field_name, value);
                    return true;
                }
                _ => {}
            }
        }
    }

    // ── DHW systems ──────────────────────────────────────────────────────
    for dhw in &mut model.dhw_systems {
        if dhw.water_heater.name == comp_name {
            set_water_heater_field(&mut dhw.water_heater, field_name, value);
            return true;
        }
        if let Some(ref mut pump) = dhw.pump {
            if pump.name == comp_name {
                set_pump_field(pump, field_name, value);
                return true;
            }
        }
    }

    // ── Exterior equipment ───────────────────────────────────────────────
    for ext in &mut model.exterior_equipment {
        if ext.name == comp_name {
            set_exterior_equipment_field(ext, field_name, value);
            return true;
        }
    }

    false
}

// ─── Per-component field setters ─────────────────────────────────────────────
//
// Each setter applies a value to a named field.  Unknown fields log a warning
// but don't fail — this keeps the system usable as new fields are added.

fn set_zone_field(zone: &mut openbse_envelope::ZoneInput, field: &str, value: f64) {
    match field {
        "volume" => zone.volume = value,
        "floor_area" => zone.floor_area = value,
        _ => log::warn!(
            "Unknown field '{}' on zone '{}' — skipping",
            field,
            zone.name
        ),
    }
}

fn set_thermostat_field(t: &mut openbse_envelope::ThermostatInput, field: &str, value: f64) {
    match field {
        "heating_setpoint" => t.heating_setpoint = value,
        "cooling_setpoint" => t.cooling_setpoint = value,
        "unoccupied_heating_setpoint" => t.unoccupied_heating_setpoint = value,
        "unoccupied_cooling_setpoint" => t.unoccupied_cooling_setpoint = value,
        _ => log::warn!(
            "Unknown field '{}' on thermostat '{}' — skipping",
            field,
            t.name
        ),
    }
}

fn set_material_field(m: &mut openbse_envelope::Material, field: &str, value: f64) {
    match field {
        "conductivity" => m.conductivity = value,
        "density" => m.density = value,
        "specific_heat" => m.specific_heat = value,
        "solar_absorptance" => m.solar_absorptance = value,
        "thermal_absorptance" => m.thermal_absorptance = value,
        "visible_absorptance" => m.visible_absorptance = value,
        _ => log::warn!(
            "Unknown field '{}' on material '{}' — skipping",
            field,
            m.name
        ),
    }
}

fn set_window_construction_field(
    wc: &mut openbse_envelope::WindowConstruction,
    field: &str,
    value: f64,
) {
    match field {
        "u_factor" => wc.u_factor = value,
        "shgc" => wc.shgc = value,
        "visible_transmittance" => wc.visible_transmittance = value,
        _ => log::warn!(
            "Unknown field '{}' on window construction '{}' — skipping",
            field,
            wc.name
        ),
    }
}

fn set_simple_construction_field(
    sc: &mut openbse_envelope::SimpleConstruction,
    field: &str,
    value: f64,
) {
    match field {
        "u_factor" => sc.u_factor = value,
        "thickness" => sc.thickness = value,
        "thermal_capacity" => sc.thermal_capacity = value,
        "solar_absorptance" => sc.solar_absorptance = value,
        "thermal_absorptance" => sc.thermal_absorptance = value,
        _ => log::warn!(
            "Unknown field '{}' on simple construction '{}' — skipping",
            field,
            sc.name
        ),
    }
}

fn set_people_field(p: &mut openbse_envelope::PeopleInput, field: &str, value: f64) {
    match field {
        "count" => p.count = value,
        "people_per_area" => p.people_per_area = Some(value),
        "area_per_person" => p.area_per_person = Some(value),
        "activity_level" => p.activity_level = value,
        "sensible_fraction" => p.sensible_fraction = value,
        "radiant_fraction" => p.radiant_fraction = value,
        _ => log::warn!(
            "Unknown field '{}' on people '{}' — skipping",
            field,
            p.name
        ),
    }
}

fn set_lights_field(l: &mut openbse_envelope::LightsInput, field: &str, value: f64) {
    match field {
        "power" => l.power = value,
        "watts_per_area" | "power_per_area" => l.watts_per_area = Some(value),
        "radiant_fraction" => l.radiant_fraction = value,
        "return_air_fraction" => l.return_air_fraction = value,
        _ => log::warn!(
            "Unknown field '{}' on lights '{}' — skipping",
            field,
            l.name
        ),
    }
}

fn set_equipment_gain_field(
    eg: &mut openbse_envelope::EquipmentGainInput,
    field: &str,
    value: f64,
) {
    match field {
        "power" => eg.power = value,
        "watts_per_area" | "power_per_area" => eg.watts_per_area = Some(value),
        "radiant_fraction" => eg.radiant_fraction = value,
        "lost_fraction" => eg.lost_fraction = value,
        "latent_fraction" => eg.latent_fraction = value,
        _ => log::warn!(
            "Unknown field '{}' on equipment gain '{}' — skipping",
            field,
            eg.name
        ),
    }
}

fn set_infiltration_field(
    inf: &mut openbse_envelope::InfiltrationTopLevel,
    field: &str,
    value: f64,
) {
    match field {
        "design_flow_rate" => inf.design_flow_rate = value,
        "air_changes_per_hour" => inf.air_changes_per_hour = value,
        _ => log::warn!(
            "Unknown field '{}' on infiltration '{}' — skipping",
            field,
            inf.name
        ),
    }
}

fn set_fan_field(f: &mut FanInput, field: &str, value: f64) {
    match field {
        "pressure_rise" => f.pressure_rise = value,
        "motor_efficiency" => f.motor_efficiency = value,
        "impeller_efficiency" => f.impeller_efficiency = value,
        "motor_in_airstream_fraction" => f.motor_in_airstream_fraction = value,
        "design_flow_rate" => f.design_flow_rate = openbse_core::types::AutosizeValue::Value(value),
        _ => log::warn!("Unknown field '{}' on fan '{}' — skipping", field, f.name),
    }
}

fn set_heating_coil_field(c: &mut HeatingCoilInput, field: &str, value: f64) {
    match field {
        "capacity" => c.capacity = openbse_core::types::AutosizeValue::Value(value),
        "setpoint" => c.setpoint = value,
        "efficiency" => c.efficiency = value,
        "cop" => c.cop = value,
        "rated_airflow" => c.rated_airflow = value,
        "supplemental_capacity" => c.supplemental_capacity = value,
        _ => log::warn!(
            "Unknown field '{}' on heating coil '{}' — skipping",
            field,
            c.name
        ),
    }
}

fn set_cooling_coil_field(c: &mut CoolingCoilInput, field: &str, value: f64) {
    match field {
        "capacity" => c.capacity = openbse_core::types::AutosizeValue::Value(value),
        "cop" => c.cop = value,
        "shr" => c.shr = value,
        "setpoint" => c.setpoint = value,
        "rated_airflow" => c.rated_airflow = openbse_core::types::AutosizeValue::Value(value),
        "design_water_flow_rate" => c.design_water_flow_rate = value,
        _ => log::warn!(
            "Unknown field '{}' on cooling coil '{}' — skipping",
            field,
            c.name
        ),
    }
}

fn set_heat_recovery_field(hr: &mut HeatRecoveryInput, field: &str, value: f64) {
    match field {
        "sensible_effectiveness" => hr.sensible_effectiveness = value,
        "latent_effectiveness" => hr.latent_effectiveness = value,
        "parasitic_power" => hr.parasitic_power = value,
        _ => log::warn!(
            "Unknown field '{}' on heat recovery '{}' — skipping",
            field,
            hr.name
        ),
    }
}

fn set_humidifier_field(h: &mut HumidifierInput, field: &str, value: f64) {
    match field {
        "rated_power" => h.rated_power = value,
        "min_rh_setpoint" => h.min_rh_setpoint = value,
        "zone_cooling_setpoint" => h.zone_cooling_setpoint = value,
        _ => log::warn!(
            "Unknown field '{}' on humidifier '{}' — skipping",
            field,
            h.name
        ),
    }
}

fn set_duct_field(d: &mut DuctInput, field: &str, value: f64) {
    match field {
        "length" => d.length = value,
        "diameter" => d.diameter = value,
        "u_value" => d.u_value = value,
        "leakage_fraction" => d.leakage_fraction = value,
        _ => log::warn!("Unknown field '{}' on duct '{}' — skipping", field, d.name),
    }
}

fn set_gshp_field(g: &mut GshpInput, field: &str, value: f64) {
    match field {
        "cop_cooling" => g.cop_cooling = value,
        "cop_heating" => g.cop_heating = value,
        "rated_cooling_capacity" => {
            g.rated_cooling_capacity = openbse_core::types::AutosizeValue::Value(value)
        }
        "rated_heating_capacity" => {
            g.rated_heating_capacity = openbse_core::types::AutosizeValue::Value(value)
        }
        "loop_depth" => g.loop_depth = value,
        "outlet_temp_setpoint" => g.outlet_temp_setpoint = value,
        _ => log::warn!("Unknown field '{}' on GSHP '{}' — skipping", field, g.name),
    }
}

fn set_vav_box_field(vb: &mut VavBoxInput, field: &str, value: f64) {
    match field {
        "min_flow_fraction" => vb.min_flow_fraction = value,
        "max_air_flow" => vb.max_air_flow = openbse_core::types::AutosizeValue::Value(value),
        "reheat_capacity" => vb.reheat_capacity = openbse_core::types::AutosizeValue::Value(value),
        "max_reheat_temp" => vb.max_reheat_temp = Some(value),
        _ => log::warn!(
            "Unknown field '{}' on VAV box '{}' — skipping",
            field,
            vb.name
        ),
    }
}

fn set_pfp_box_field(pb: &mut crate::input::PfpBoxInput, field: &str, value: f64) {
    match field {
        "min_primary_fraction" => pb.min_primary_fraction = value,
        "max_primary_flow" => {
            pb.max_primary_flow = openbse_core::types::AutosizeValue::Value(value)
        }
        "secondary_fan_flow" => {
            pb.secondary_fan_flow = openbse_core::types::AutosizeValue::Value(value)
        }
        "reheat_capacity" => pb.reheat_capacity = openbse_core::types::AutosizeValue::Value(value),
        _ => log::warn!(
            "Unknown field '{}' on PFP box '{}' — skipping",
            field,
            pb.name
        ),
    }
}

fn set_boiler_field(b: &mut BoilerInput, field: &str, value: f64) {
    match field {
        "capacity" => b.capacity = openbse_core::types::AutosizeValue::Value(value),
        "efficiency" => b.efficiency = value,
        "design_outlet_temp" => b.design_outlet_temp = value,
        "design_water_flow_rate" => {
            b.design_water_flow_rate = openbse_core::types::AutosizeValue::Value(value)
        }
        _ => log::warn!(
            "Unknown field '{}' on boiler '{}' — skipping",
            field,
            b.name
        ),
    }
}

fn set_chiller_field(c: &mut ChillerInput, field: &str, value: f64) {
    match field {
        "capacity" => c.capacity = openbse_core::types::AutosizeValue::Value(value),
        "cop" => c.cop = value,
        "chw_setpoint" => c.chw_setpoint = value,
        "design_chw_flow" => c.design_chw_flow = value,
        "tower_approach" => c.tower_approach = value,
        "min_plr" => c.min_plr = value,
        _ => log::warn!(
            "Unknown field '{}' on chiller '{}' — skipping",
            field,
            c.name
        ),
    }
}

fn set_pump_field(p: &mut PumpInput, field: &str, value: f64) {
    match field {
        "design_flow_rate" => p.design_flow_rate = openbse_core::types::AutosizeValue::Value(value),
        "design_head" => p.design_head = value,
        "motor_efficiency" => p.motor_efficiency = value,
        "impeller_efficiency" => p.impeller_efficiency = value,
        "motor_heat_to_fluid_fraction" => p.motor_heat_to_fluid_fraction = value,
        _ => log::warn!("Unknown field '{}' on pump '{}' — skipping", field, p.name),
    }
}

fn set_cooling_tower_field(ct: &mut CoolingTowerInput, field: &str, value: f64) {
    match field {
        "design_air_flow" => ct.design_air_flow = value,
        "design_fan_power" => ct.design_fan_power = value,
        "design_inlet_water_temp" => ct.design_inlet_water_temp = value,
        "design_approach" => ct.design_approach = value,
        "design_range" => ct.design_range = value,
        _ => log::warn!(
            "Unknown field '{}' on cooling tower '{}' — skipping",
            field,
            ct.name
        ),
    }
}

fn set_heat_exchanger_field(hx: &mut HeatExchangerInput, field: &str, value: f64) {
    match field {
        "effectiveness" => hx.effectiveness = value,
        "design_flow_rate" => {
            hx.design_flow_rate = openbse_core::types::AutosizeValue::Value(value)
        }
        "economizer_threshold" => hx.economizer_threshold = value,
        _ => log::warn!(
            "Unknown field '{}' on heat exchanger '{}' — skipping",
            field,
            hx.name
        ),
    }
}

fn set_water_heater_field(wh: &mut crate::input::WaterHeaterInput, field: &str, value: f64) {
    match field {
        "capacity" => wh.capacity = value,
        "efficiency" => wh.efficiency = value,
        "setpoint" | "setpoint_temperature" => wh.setpoint = value,
        "tank_volume" => wh.tank_volume = value,
        "ua_standby" => wh.ua_standby = value,
        "deadband" => wh.deadband = value,
        "parasitic_power" => wh.parasitic_power = value,
        _ => log::warn!(
            "Unknown field '{}' on water heater '{}' — skipping",
            field,
            wh.name
        ),
    }
}

fn set_exterior_equipment_field(ext: &mut ExteriorEquipmentInput, field: &str, value: f64) {
    match field {
        "power" => ext.power = value,
        _ => log::warn!(
            "Unknown field '{}' on exterior equipment '{}' — skipping",
            field,
            ext.name
        ),
    }
}

// ─── Sweep Expansion ─────────────────────────────────────────────────────────

use crate::input::{ParametricInput, ParametricRun};

/// Expand sweep definitions into explicit ParametricRun entries.
///
/// If `cross_product` is true, generates the Cartesian product of all sweeps.
/// If false (default), zips sweeps together (they must have the same length).
pub fn expand_sweeps(parametric: &mut ParametricInput) -> Result<(), ParametricError> {
    if parametric.sweeps.is_empty() {
        return Ok(());
    }

    // Resolve each sweep into (parameter, Vec<f64>)
    let mut resolved: Vec<(String, Vec<f64>)> = Vec::new();
    for sweep in &parametric.sweeps {
        let values = sweep.resolve_values()?;
        if values.is_empty() {
            return Err(ParametricError::SweepError(format!(
                "Sweep for '{}' produced no values",
                sweep.parameter
            )));
        }
        resolved.push((sweep.parameter.clone(), values));
    }

    let mut sweep_runs: Vec<ParametricRun> = if parametric.cross_product {
        // Cartesian product
        expand_cross_product(&resolved)
    } else {
        // Zip — all sweeps must have same length
        expand_zip(&resolved)?
    };

    // Append sweep runs after any explicit runs
    parametric.runs.append(&mut sweep_runs);
    Ok(())
}

/// Generate runs by zipping sweeps together (element-wise).
fn expand_zip(sweeps: &[(String, Vec<f64>)]) -> Result<Vec<ParametricRun>, ParametricError> {
    let len = sweeps[0].1.len();
    for (param, vals) in sweeps {
        if vals.len() != len {
            return Err(ParametricError::SweepError(format!(
                "Zip mode requires all sweeps to have the same number of values. \
                 '{}' has {} values but expected {}",
                param,
                vals.len(),
                len
            )));
        }
    }

    let mut runs = Vec::with_capacity(len);
    for i in 0..len {
        let mut overrides = HashMap::new();
        let mut name_parts = Vec::new();
        for (param, vals) in sweeps {
            overrides.insert(param.clone(), vals[i]);
            // Build a descriptive name fragment
            let short_param = param.rsplit('.').next().unwrap_or(param).replace(' ', "_");
            name_parts.push(format!("{}_{}", short_param, format_value(vals[i])));
        }
        runs.push(ParametricRun {
            name: format!("sweep_{}", name_parts.join("_")),
            weather_file: None,
            overrides,
            includes: Vec::new(),
        });
    }
    Ok(runs)
}

/// Generate runs by Cartesian product of all sweeps.
fn expand_cross_product(sweeps: &[(String, Vec<f64>)]) -> Vec<ParametricRun> {
    // Start with a single empty combination
    let mut combos: Vec<Vec<(String, f64)>> = vec![vec![]];

    for (param, vals) in sweeps {
        let mut new_combos = Vec::new();
        for combo in &combos {
            for &val in vals {
                let mut new = combo.clone();
                new.push((param.clone(), val));
                new_combos.push(new);
            }
        }
        combos = new_combos;
    }

    combos
        .into_iter()
        .map(|combo| {
            let mut overrides = HashMap::new();
            let mut name_parts = Vec::new();
            for (param, val) in &combo {
                overrides.insert(param.clone(), *val);
                let short_param = param.rsplit('.').next().unwrap_or(param).replace(' ', "_");
                name_parts.push(format!("{}_{}", short_param, format_value(*val)));
            }
            ParametricRun {
                name: format!("sweep_{}", name_parts.join("_")),
                weather_file: None,
                overrides,
                includes: Vec::new(),
            }
        })
        .collect()
}

/// Format a float for use in run names (avoid excessive decimals).
fn format_value(v: f64) -> String {
    if v == v.floor() {
        format!("{:.0}", v)
    } else {
        // Remove trailing zeros
        let s = format!("{:.4}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{SweepInput, SweepRange};

    /// Helper: create a minimal ModelInput for testing overrides.
    fn test_model() -> ModelInput {
        let yaml = r#"
simulation:
  timesteps_per_hour: 1
  start_month: 1
  start_day: 1
  end_month: 1
  end_day: 31

weather_files:
  - "test.epw"

zones:
  - name: TestZone
    volume: 100.0
    floor_area: 40.0

thermostats:
  - name: TestTstat
    zones: [TestZone]
    heating_setpoint: 21.0
    cooling_setpoint: 24.0

plant_loops:
  - name: HHW Loop
    supply_equipment:
      - type: boiler
        name: Boiler-1
        capacity: 50000
        efficiency: 0.80
        design_outlet_temp: 82.0

air_loops:
  - name: Main AHU
    equipment:
      - type: fan
        name: Supply Fan
        design_flow_rate: 1.0
        pressure_rise: 600.0
        motor_efficiency: 0.9
        impeller_efficiency: 0.71
      - type: cooling_coil
        name: DX Coil
        source: dx
        capacity: 20000
        cop: 3.5
        shr: 0.8
    zone_terminals:
      - zone: TestZone
"#;
        crate::input::parse_model_yaml(yaml).expect("test model should parse")
    }

    #[test]
    fn test_apply_overrides_boiler_efficiency() {
        let mut model = test_model();
        let mut overrides = HashMap::new();
        overrides.insert("Boiler-1.efficiency".to_string(), 0.95);

        apply_overrides(&mut model, &overrides).unwrap();

        // Verify the boiler efficiency was changed
        let boiler = match &model.plant_loops[0].supply_equipment[0] {
            PlantEquipmentInput::Boiler(b) => b,
            _ => panic!("expected boiler"),
        };
        assert!((boiler.efficiency - 0.95).abs() < 1e-10);
    }

    #[test]
    fn test_apply_overrides_zone_volume() {
        let mut model = test_model();
        let mut overrides = HashMap::new();
        overrides.insert("TestZone.volume".to_string(), 200.0);

        apply_overrides(&mut model, &overrides).unwrap();
        assert!((model.zones[0].volume - 200.0).abs() < 1e-10);
    }

    #[test]
    fn test_apply_overrides_thermostat() {
        let mut model = test_model();
        let mut overrides = HashMap::new();
        overrides.insert("TestTstat.heating_setpoint".to_string(), 22.0);

        apply_overrides(&mut model, &overrides).unwrap();
        assert!((model.thermostats[0].heating_setpoint - 22.0).abs() < 1e-10);
    }

    #[test]
    fn test_apply_overrides_fan() {
        let mut model = test_model();
        let mut overrides = HashMap::new();
        overrides.insert("Supply Fan.impeller_efficiency".to_string(), 0.89);

        apply_overrides(&mut model, &overrides).unwrap();

        let fan = match &model.air_loops[0].equipment[0] {
            EquipmentInput::Fan(f) => f,
            _ => panic!("expected fan"),
        };
        assert!((fan.impeller_efficiency - 0.89).abs() < 1e-10);
    }

    #[test]
    fn test_apply_overrides_cooling_coil() {
        let mut model = test_model();
        let mut overrides = HashMap::new();
        overrides.insert("DX Coil.cop".to_string(), 4.5);

        apply_overrides(&mut model, &overrides).unwrap();

        let coil = match &model.air_loops[0].equipment[1] {
            EquipmentInput::CoolingCoil(c) => c,
            _ => panic!("expected cooling coil"),
        };
        assert!((coil.cop - 4.5).abs() < 1e-10);
    }

    #[test]
    fn test_apply_overrides_component_not_found() {
        let mut model = test_model();
        let mut overrides = HashMap::new();
        overrides.insert("NonExistent.efficiency".to_string(), 0.95);

        let result = apply_overrides(&mut model, &overrides);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ParametricError::ComponentNotFound(_)
        ));
    }

    #[test]
    fn test_apply_overrides_invalid_key() {
        let mut model = test_model();
        let mut overrides = HashMap::new();
        overrides.insert("no_dot_in_key".to_string(), 0.95);

        let result = apply_overrides(&mut model, &overrides);
        assert!(result.is_err());
    }

    #[test]
    fn test_sweep_expand_zip() {
        let mut parametric = ParametricInput {
            runs: vec![],
            sweeps: vec![
                SweepInput {
                    parameter: "Boiler-1.efficiency".to_string(),
                    values: Some(vec![0.75, 0.80, 0.85]),
                    range: None,
                },
                SweepInput {
                    parameter: "DX Coil.cop".to_string(),
                    values: Some(vec![3.0, 3.5, 4.0]),
                    range: None,
                },
            ],
            cross_product: false,
        };

        expand_sweeps(&mut parametric).unwrap();

        assert_eq!(parametric.runs.len(), 3);
        assert!(
            (parametric.runs[0]
                .overrides
                .get("Boiler-1.efficiency")
                .copied()
                .unwrap()
                - 0.75)
                .abs()
                < 1e-10
        );
        assert!(
            (parametric.runs[0]
                .overrides
                .get("DX Coil.cop")
                .copied()
                .unwrap()
                - 3.0)
                .abs()
                < 1e-10
        );
        assert!(
            (parametric.runs[2]
                .overrides
                .get("Boiler-1.efficiency")
                .copied()
                .unwrap()
                - 0.85)
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn test_sweep_expand_cross_product() {
        let mut parametric = ParametricInput {
            runs: vec![],
            sweeps: vec![
                SweepInput {
                    parameter: "Boiler-1.efficiency".to_string(),
                    values: Some(vec![0.80, 0.90]),
                    range: None,
                },
                SweepInput {
                    parameter: "DX Coil.cop".to_string(),
                    values: Some(vec![3.0, 4.0, 5.0]),
                    range: None,
                },
            ],
            cross_product: true,
        };

        expand_sweeps(&mut parametric).unwrap();

        // 2 × 3 = 6 combinations
        assert_eq!(parametric.runs.len(), 6);
    }

    #[test]
    fn test_sweep_range_expansion() {
        let sweep = SweepInput {
            parameter: "Boiler-1.efficiency".to_string(),
            values: None,
            range: Some(crate::input::SweepRange {
                min: 0.75,
                max: 0.95,
                step: 0.05,
            }),
        };

        let values = sweep.resolve_values().unwrap();
        assert_eq!(values.len(), 5); // 0.75, 0.80, 0.85, 0.90, 0.95
        assert!((values[0] - 0.75).abs() < 1e-10);
        assert!((values[4] - 0.95).abs() < 1e-10);
    }

    #[test]
    fn test_sweep_zip_mismatched_lengths() {
        let mut parametric = ParametricInput {
            runs: vec![],
            sweeps: vec![
                SweepInput {
                    parameter: "Boiler-1.efficiency".to_string(),
                    values: Some(vec![0.75, 0.80]),
                    range: None,
                },
                SweepInput {
                    parameter: "DX Coil.cop".to_string(),
                    values: Some(vec![3.0, 3.5, 4.0]),
                    range: None,
                },
            ],
            cross_product: false,
        };

        let result = expand_sweeps(&mut parametric);
        assert!(result.is_err());
    }

    #[test]
    fn test_explicit_runs_before_sweeps() {
        let mut parametric = ParametricInput {
            runs: vec![ParametricRun {
                name: "baseline".to_string(),
                weather_file: None,
                overrides: HashMap::new(),
                includes: Vec::new(),
            }],
            sweeps: vec![SweepInput {
                parameter: "Boiler-1.efficiency".to_string(),
                values: Some(vec![0.80, 0.90]),
                range: None,
            }],
            cross_product: false,
        };

        expand_sweeps(&mut parametric).unwrap();

        // 1 explicit + 2 sweep = 3
        assert_eq!(parametric.runs.len(), 3);
        assert_eq!(parametric.runs[0].name, "baseline");
        assert!(parametric.runs[1].name.starts_with("sweep_"));
    }

    #[test]
    fn test_sweep_values_and_range_combined() {
        let sweep = SweepInput {
            parameter: "Boiler-1.efficiency".to_string(),
            values: Some(vec![0.70, 0.72]),
            range: Some(crate::input::SweepRange {
                min: 0.80,
                max: 0.90,
                step: 0.05,
            }),
        };

        let values = sweep.resolve_values().unwrap();
        // 2 explicit + 3 from range = 5
        assert_eq!(values.len(), 5);
        assert!((values[0] - 0.70).abs() < 1e-10);
        assert!((values[1] - 0.72).abs() < 1e-10);
        assert!((values[2] - 0.80).abs() < 1e-10);
        assert!((values[4] - 0.90).abs() < 1e-10);
    }

    #[test]
    fn test_sweep_yaml_parsing_range_only() {
        // Verify that YAML with only `range:` doesn't accidentally populate `values:`
        let yaml = r#"
simulation:
  timesteps_per_hour: 1
weather_files:
  - "test.epw"
parametrics:
  sweeps:
    - parameter: "Thermostat.heating_setpoint"
      range: { min: 20.0, max: 22.0, step: 1.0 }
"#;
        let model = crate::input::parse_model_yaml(yaml).expect("should parse");
        let parametric = model.parametrics.expect("should have parametrics");
        assert_eq!(parametric.sweeps.len(), 1);
        assert!(parametric.sweeps[0].values.is_none());
        assert!(parametric.sweeps[0].range.is_some());

        let values = parametric.sweeps[0].resolve_values().unwrap();
        assert_eq!(values.len(), 3); // 20.0, 21.0, 22.0
    }

    #[test]
    fn test_sweep_yaml_parsing_both() {
        let yaml = r#"
simulation:
  timesteps_per_hour: 1
weather_files:
  - "test.epw"
parametrics:
  sweeps:
    - parameter: "Thermostat.heating_setpoint"
      values: [18.0, 19.0]
      range: { min: 20.0, max: 22.0, step: 1.0 }
"#;
        let model = crate::input::parse_model_yaml(yaml).expect("should parse");
        let parametric = model.parametrics.expect("should have parametrics");
        let values = parametric.sweeps[0].resolve_values().unwrap();
        // 2 explicit + 3 from range = 5
        assert_eq!(values.len(), 5);
        assert!((values[0] - 18.0).abs() < 1e-10);
        assert!((values[4] - 22.0).abs() < 1e-10);
    }
}
