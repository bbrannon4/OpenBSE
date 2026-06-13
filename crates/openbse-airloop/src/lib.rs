//! Air-loop control signal builders.
//!
//! One builder per HVAC system type (PSZ, CRAC, CRAH, DOAS, FCU, VAV,
//! dual-duct). Each builder reads the current zone/outdoor state and produces
//! a [`ControlSignals`] describing what the loop's components should do this
//! timestep (coil setpoints, air mass flows, mixed-air temperature, OA
//! fraction, PLR).
//!
//! Extracted from the CLI driver (`openbse-cli`) so the control logic lives in
//! a focused, separately testable crate. This crate sits above `openbse-io`
//! in the dependency graph (it consumes the YAML input enums), so it cannot
//! live in the lower-level `openbse-controls` crate.

// The per-system-type builders take many per-zone state maps by reference;
// grouping them into a context struct is a future cleanup (CR-4 follow-up).
#![allow(clippy::too_many_arguments)]

use std::collections::{HashMap, HashSet};

use openbse_core::ports::ComponentKind;
use openbse_core::simulation::ControlSignals;
use openbse_envelope::schedule::ScheduleManager;
use openbse_io::input::AirLoopSystemType;

// ─── Loop Descriptor ─────────────────────────────────────────────────────────
//
// Captures the static properties of an air loop that the control logic needs
// at every timestep. Built once at startup from the model input.

#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields populated for upcoming cycling/supply temp logic
pub struct LoopInfo {
    pub name: String,
    pub system_type: AirLoopSystemType,
    /// Component names in simulation order (fan → coils)
    pub component_names: Vec<String>,
    /// Names of fan components in this loop (for PLR-exempt identification)
    pub fan_names: HashSet<String>,
    /// Names of components whose electric power carries the compressor
    /// cycling penalty (RTF = PLR/PLF) at the system level: DX cooling
    /// coils (single/multi-speed) and WSHP. Heat pump heating coils apply
    /// PLF internally; electric/gas/HW heating coils have no PLF in E+.
    pub dx_compressor_names: HashSet<String>,
    /// Component kinds by name, from the loop's equipment list. Used by the
    /// signal builders to dispatch coil control on type instead of name
    /// substrings (code review CR-1).
    pub component_kinds: HashMap<String, ComponentKind>,
    /// Zones served by this loop
    pub served_zones: Vec<String>,
    /// Minimum outdoor air fraction [0-1]. DOAS always 1.0.
    /// Resolved from controls.minimum_damper_position or auto-calculated.
    pub min_oa_fraction: f64,
    /// Minimum VAV box flow fraction [0-1]. Only used for VAV.
    pub min_vav_fraction: f64,
    /// HVAC availability schedule name. When schedule value is 0, system is OFF.
    pub availability_schedule: Option<String>,
    /// Design heating supply air temperature [°C] (from air loop controls)
    pub heating_supply_temp: f64,
    /// Design cooling supply air temperature [°C] (from air loop controls)
    pub cooling_supply_temp: f64,
    /// Capacity control method (from air loop controls)
    pub cycling: openbse_io::input::CyclingMethod,
    /// Fan operating mode: cycling (fan cycles with coils) or continuous
    /// (fan runs at full speed always, coils cycle ON/OFF).
    pub fan_operating_mode: openbse_io::input::FanOperatingMode,
    /// Terminal box component names per zone (zone_name -> component_name).
    /// Only populated for loops with VAV/PFP terminal boxes defined in YAML.
    pub terminal_boxes: HashMap<String, String>,
    /// Dual-duct mixing box objects per zone (zone_name -> DualDuctBox).
    /// Only populated for DualDuct system type loops.
    pub dd_boxes: HashMap<String, openbse_components::dual_duct_box::DualDuctBox>,
    /// True when the user explicitly set `minimum_damper_position` in YAML.
    /// Prevents post-sizing auto-recalculation from overriding the user value.
    pub explicit_min_oa: bool,
    /// Name of the heat recovery component (if any) in this loop.
    /// Used for pre-processing heat recovery before the signal builder.
    pub heat_recovery_name: Option<String>,
    /// Efficiency of the boiler serving this loop's HW coils.
    /// Used to convert HR thermal credit to gas savings.
    pub hhw_boiler_efficiency: f64,
    /// Demand-controlled ventilation enabled for this loop.
    pub dcv: bool,
    /// Cooling SAT reset configuration (cloned from AirLoopControls).
    pub cooling_sat_reset: Option<openbse_io::input::SatResetConfig>,
    /// Heating SAT reset configuration (cloned from AirLoopControls).
    pub heating_sat_reset: Option<openbse_io::input::SatResetConfig>,
    /// Per-zone OA data for ASHRAE 62.1 VRP and DCV calculations.
    /// Always populated from zone connections (per_person_oa, per_area_oa).
    pub zone_oa_data: Vec<ZoneOaData>,
    /// Design supply air flow rate [m³/s] for this loop (used to compute dynamic OA fraction)
    pub design_supply_flow: f64,
    /// Economizer type for this loop.
    pub economizer_type: openbse_io::input::EconomizerType,
    /// Economizer high-limit shutoff temperature [°C] (for FixedDryBulb / EnthalpyWithHighLimit).
    pub economizer_high_limit: Option<f64>,
    /// Economizer high-limit shutoff enthalpy [J/kg] (for FixedEnthalpy / EnthalpyWithHighLimit).
    pub economizer_high_limit_enthalpy: Option<f64>,
}

/// Control role of a component in a signal builder's coil dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoilRole {
    Cooling,
    Heating,
    Humidifier,
    Other,
}

impl LoopInfo {
    /// Control role for a component, dispatched on its `ComponentKind` from
    /// the loop's equipment list. Falls back to the legacy name-substring
    /// heuristic for components not in the map (e.g., hand-built test loops),
    /// so unconventional names still resolve the way they used to.
    pub fn coil_role(&self, name: &str) -> CoilRole {
        match self.component_kinds.get(name) {
            Some(ComponentKind::CoolingCoil) | Some(ComponentKind::EvapCooler) => {
                return CoilRole::Cooling
            }
            Some(ComponentKind::HeatingCoil) => return CoilRole::Heating,
            Some(ComponentKind::Humidifier) => return CoilRole::Humidifier,
            Some(_) => return CoilRole::Other,
            None => {}
        }
        let l = name.to_lowercase();
        if l.contains("cool") || l.contains("dx") || l.starts_with("cc ") || l.starts_with("cc_") {
            CoilRole::Cooling
        } else if l.contains("heat")
            || l.contains("furnace")
            || (l.contains("hw") && !l.contains("chw"))
            || l.starts_with("hc ")
            || l.starts_with("hc_")
        {
            CoilRole::Heating
        } else if l.contains("humid") {
            CoilRole::Humidifier
        } else {
            CoilRole::Other
        }
    }
}

/// Per-zone data for ASHRAE 62.1 ventilation rate procedure.
/// Used for both DCV (dynamic occupancy) and multi-zone VRP (Ev correction).
#[derive(Debug, Clone)]
pub struct ZoneOaData {
    pub zone_name: String,
    pub design_people: f64,
    pub per_person_oa: f64, // [m³/s per person]
    pub per_area_oa: f64,   // [m³/s per m²]
    pub floor_area: f64,    // [m²]
    pub people_schedule: Option<String>,
}

// ─── HVAC Mode ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HvacMode {
    Heating,
    Cooling,
    Deadband,
}

pub fn hvac_mode(zone_temp: f64, heat_sp: f64, cool_sp: f64) -> HvacMode {
    if zone_temp < heat_sp {
        HvacMode::Heating
    } else if zone_temp > cool_sp {
        HvacMode::Cooling
    } else {
        HvacMode::Deadband
    }
}
pub fn build_psz_signals(
    li: &LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    t_outdoor: f64,
    zone_cooling_loads: &HashMap<String, f64>,
    zone_heating_loads: &HashMap<String, f64>,
    effective_min_oa: f64,
    predictor_modes: &HashMap<String, HvacMode>,
    w_outdoor: f64,
    zone_humidity_ratios: &HashMap<String, f64>,
    zone_max_rh: &HashMap<String, f64>,
    zone_min_rh: &HashMap<String, f64>,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // Control zone = first served zone
    let control_zone = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
    let control_temp = zone_temps.get(control_zone).copied().unwrap_or(21.0);
    let heat_sp = zone_heat_sp.get(control_zone).copied().unwrap_or(21.1);
    let cool_sp = zone_cool_sp.get(control_zone).copied().unwrap_or(23.9);
    let zone_cool_load = zone_cooling_loads.get(control_zone).copied().unwrap_or(0.0);
    let zone_heat_load = zone_heating_loads.get(control_zone).copied().unwrap_or(0.0);

    // Use predictor mode (from frozen ideal loads) to prevent mode
    // flip-flopping during HVAC↔envelope iteration loop.
    let predictor_mode = predictor_modes
        .get(control_zone)
        .copied()
        .unwrap_or_else(|| {
            // Fallback: temperature-based with load-informed deadband tiebreaker
            if control_temp > cool_sp {
                HvacMode::Cooling
            } else if control_temp < heat_sp {
                HvacMode::Heating
            } else if zone_cool_load > zone_heat_load && zone_cool_load > 100.0 {
                HvacMode::Cooling
            } else if zone_heat_load > zone_cool_load && zone_heat_load > 100.0 {
                HvacMode::Heating
            } else {
                HvacMode::Deadband
            }
        });

    // Safety override: prevent heating when zone is already above cooling
    // setpoint (and vice versa).  With on/off cycling at high capacity,
    // the predictor mode can be stale by one timestep, causing the system
    // to fire heating into an already-warm zone.  This guard prevents the
    // resulting temperature oscillation.
    let mut mode = match predictor_mode {
        HvacMode::Heating if control_temp > cool_sp => HvacMode::Cooling,
        HvacMode::Cooling if control_temp < heat_sp => HvacMode::Heating,
        other => other,
    };

    // RH override: if zone is over-humid and in deadband, force Cooling so the
    // DX coil activates and dehumidifies. The coil setpoint is adjusted below
    // (zone_temp - 0.5) to minimize sensible cooling while operating in wet region.
    let zone_w = zone_humidity_ratios
        .get(control_zone)
        .copied()
        .unwrap_or(0.008);
    let zone_rh_pct =
        openbse_psychrometrics::rh_fn_tdb_w_pb(control_temp, zone_w, 101325.0) * 100.0;
    let mut dehumidify_only = false;
    if let Some(&max_rh) = zone_max_rh.get(control_zone) {
        if zone_rh_pct > max_rh && mode == HvacMode::Deadband {
            mode = HvacMode::Cooling;
            dehumidify_only = true;
        }
    }

    // Total design flow for this loop
    let mut total_flow = 0.0f64;
    for zone_name in &li.served_zones {
        total_flow += zone_design_flows.get(zone_name).copied().unwrap_or(0.5);
    }
    total_flow = total_flow.max(0.01);

    // ── Part-Load Ratio (PLR) for ON/OFF Fan Cycling ──
    //
    // PLR is computed AFTER component simulation in simulate_all_loops
    // using load-based PLR: PLR = zone_load / system_capacity.
    //
    // Components are simulated at full flow (PLR = 1.0), then outputs
    // are scaled by PLR to represent the time-averaged effect.
    //
    // Here we just set PLR = 1.0 as a placeholder; the actual load-based
    // PLR is computed in simulate_all_loops after we know the system
    // capacity from the component simulation.
    let plr = 1.0_f64; // Placeholder — real PLR computed post-simulation

    // Components run at FULL design flow (fan ON at full speed when cycling)
    let flow = total_flow;

    // ── Heating DAT ──
    // On/Off: E+ PSZ-AC with Fan:OnOff fires the heating coil at full
    //   capacity whenever the system is ON.  PLR controls runtime, not
    //   supply temperature.  Fixed DAT = heating_supply_temp.
    // Proportional: modulate supply temp based on deviation from setpoint.
    //   DAT ramps from setpoint to max over a 5°C error band, giving
    //   smooth modulation for systems with variable-capacity burners.
    let heating_dat = match li.cycling {
        openbse_io::input::CyclingMethod::OnOff => li.heating_supply_temp,
        openbse_io::input::CyclingMethod::Proportional => {
            let error = (heat_sp - control_temp).max(0.0);
            (heat_sp + (li.heating_supply_temp - heat_sp) * (error / 5.0).min(1.0))
                .clamp(heat_sp, li.heating_supply_temp)
        }
    };

    // ── Cooling control ──
    // Economizer target: modulate OA to achieve the supply air temperature
    // (SAT) in the mixed air, minimizing cooling coil work.  This matches
    // E+'s Controller:OutdoorAir behavior where the OA damper targets the
    // mixed-air setpoint derived from the cooling-coil leaving-air temp.
    // Use the loop's cooling SAT as the economizer target.
    let econ_target = li.cooling_supply_temp;
    // Coil setpoint: -10°C forces the DX coil to run at full physical capacity.
    // In dehumidification-only mode, use zone_temp - 0.5 to minimize sensible
    // cooling while still operating the coil in the wet region.
    let cooling_coil_sp = if mode == HvacMode::Cooling {
        if dehumidify_only {
            control_temp - 0.5 // dehumidify with minimal sensible cooling
        } else {
            -10.0
        }
    } else {
        99.0
    };

    // ── Economizer: respects loop economizer type ──
    // FixedDryBulb: OA used when OAT < high_limit
    // DifferentialDryBulb: OA used when OAT < return air temp
    // DifferentialEnthalpy: OA used when OA enthalpy < return air enthalpy
    // FixedEnthalpy: OA used when OA enthalpy < high_limit_enthalpy
    // EnthalpyWithHighLimit: differential enthalpy AND OAT < high_limit
    // NoEconomizer: always minimum OA
    let return_air_temp = control_temp;
    let return_w = zone_humidity_ratios
        .get(control_zone)
        .copied()
        .unwrap_or(0.008);
    let return_enthalpy = openbse_psychrometrics::h_fn_tdb_w(return_air_temp, return_w);
    let outdoor_enthalpy = openbse_psychrometrics::h_fn_tdb_w(t_outdoor, w_outdoor);
    use openbse_io::input::EconomizerType;
    let psz_econ_available = match li.economizer_type {
        EconomizerType::NoEconomizer => false,
        EconomizerType::FixedDryBulb => {
            let limit = li.economizer_high_limit.unwrap_or(23.889);
            t_outdoor < limit
        }
        EconomizerType::DifferentialDryBulb => t_outdoor < return_air_temp,
        EconomizerType::DifferentialEnthalpy => outdoor_enthalpy < return_enthalpy,
        EconomizerType::FixedEnthalpy => {
            let limit = li.economizer_high_limit_enthalpy.unwrap_or(65_200.0);
            outdoor_enthalpy < limit
        }
        EconomizerType::EnthalpyWithHighLimit => {
            let temp_limit = li.economizer_high_limit.unwrap_or(23.889);
            outdoor_enthalpy < return_enthalpy && t_outdoor < temp_limit
        }
    };
    let oa_frac = if psz_econ_available && mode != HvacMode::Heating {
        // Economizer: modulate OA to approach SAT target in mixed air.
        // Active in both Cooling and Deadband — provides free cooling from
        // outdoor air, reducing or eliminating mechanical cooling.  Matches
        // E+'s economizer which operates whenever OA conditions are favorable,
        // regardless of whether the cooling coil is currently active.
        let delta = return_air_temp - t_outdoor;
        if delta > 0.1 {
            let needed = (return_air_temp - econ_target) / delta;
            needed.clamp(effective_min_oa, 1.0)
        } else {
            effective_min_oa
        }
    } else {
        effective_min_oa
    };
    let mixed_air_temp = return_air_temp * (1.0 - oa_frac) + t_outdoor * oa_frac;

    for name in &li.component_names {
        let role = li.coil_role(name);
        match mode {
            HvacMode::Heating => {
                // Proportional heating DAT: ramps from setpoint toward max (40°C)
                // based on zone heating error. At small errors, furnace delivers
                // warm but not hot air; at large errors, full-fire to recover.
                if role == CoilRole::Heating {
                    signals.coil_setpoints.insert(name.clone(), heating_dat);
                } else if role == CoilRole::Cooling {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
            HvacMode::Cooling => {
                // DX coil runs at full capacity when ON (PLR controls runtime).
                // The coil setpoint is set very low so capacity is the limiter.
                if role == CoilRole::Cooling {
                    signals.coil_setpoints.insert(name.clone(), cooling_coil_sp);
                } else if role == CoilRole::Heating {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                }
            }
            HvacMode::Deadband => {
                if role == CoilRole::Heating {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                } else if role == CoilRole::Cooling {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
        }
        // Humidification control: if zone RH < min_rh, activate humidifier
        // by setting its w_setpoint to the target humidity ratio.
        if role == CoilRole::Humidifier {
            if let Some(&min_rh) = zone_min_rh.get(control_zone) {
                if zone_rh_pct < min_rh {
                    let w_target = openbse_psychrometrics::w_fn_tdb_rh_pb(
                        control_temp,
                        min_rh / 100.0,
                        101325.0,
                    );
                    signals.coil_setpoints.insert(name.clone(), w_target);
                }
            }
        }
        signals.air_mass_flows.insert(name.clone(), flow);
    }

    // Inject mixed air temperature, OA fraction, and PLR
    signals.mixed_air_temp = Some(mixed_air_temp);
    signals.oa_fraction = Some(oa_frac);
    signals.loop_plr = Some(plr);

    signals
}

/// CRAC: self-contained DX cooling unit.
///
/// Key differences from PSZ-AC:
/// - No outdoor air mixing (OA fraction = 0 unless explicitly set)
/// - No economizer
/// - Always in cooling-only mode (data centers have no heating)
/// - SHR = 0.98 default (high sensible for IT environments)
/// - Supply air setpoint from `cooling_supply_temp` or rack_inlet_temp_max_c
pub fn build_crac_signals(
    li: &LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    t_outdoor: f64,
    zone_cooling_loads: &HashMap<String, f64>,
    predictor_modes: &HashMap<String, HvacMode>,
    zone_humidity_ratios: &HashMap<String, f64>,
    zone_max_rh: &HashMap<String, f64>,
    zone_dc_return_temps: &HashMap<String, f64>,
) -> ControlSignals {
    let _ = t_outdoor; // CRAC uses outdoor temp for condenser, not OA mixing
    let mut signals = ControlSignals::default();

    let zone_name = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
    let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(24.0);
    let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);

    let total_flow: f64 = li
        .served_zones
        .iter()
        .map(|z| zone_design_flows.get(z).copied().unwrap_or(0.5))
        .sum::<f64>()
        .max(0.01);

    // CRAC is cooling-only: mode is always Cooling when zone is warm or has load
    let zone_cool_load = zone_cooling_loads.get(zone_name).copied().unwrap_or(0.0);
    let predictor_mode = predictor_modes.get(zone_name).copied().unwrap_or_else(|| {
        if zone_temp > cool_sp || zone_cool_load > 100.0 {
            HvacMode::Cooling
        } else {
            HvacMode::Deadband
        }
    });
    let mode = match predictor_mode {
        HvacMode::Heating => HvacMode::Cooling, // CRAC never heats
        other => other,
    };

    // RH override: force cooling for dehumidification if zone is over-humid
    let zone_w = zone_humidity_ratios
        .get(zone_name)
        .copied()
        .unwrap_or(0.008);
    let zone_rh_pct = openbse_psychrometrics::rh_fn_tdb_w_pb(zone_temp, zone_w, 101325.0) * 100.0;
    let mut dehumidify_only = false;
    let mode = if let Some(&max_rh) = zone_max_rh.get(zone_name) {
        if zone_rh_pct > max_rh && mode == HvacMode::Deadband {
            dehumidify_only = true;
            HvacMode::Cooling
        } else {
            mode
        }
    } else {
        mode
    };

    // CRAC recirculates room air (no OA). For DC zones, return air temp
    // is warmer than zone average due to rack heat — use pre-computed value.
    let oa_frac = li
        .component_names
        .is_empty()
        .then_some(0.0_f64)
        .unwrap_or(0.0_f64);
    let return_air_temp = zone_dc_return_temps
        .get(zone_name)
        .copied()
        .unwrap_or(zone_temp);
    let mixed_air_temp = return_air_temp;

    for name in &li.component_names {
        let role = li.coil_role(name);
        match mode {
            HvacMode::Cooling => {
                if role == CoilRole::Cooling {
                    let sp = if dehumidify_only {
                        zone_temp - 0.5
                    } else {
                        li.cooling_supply_temp // target rack_inlet_temp_max_c like CRAH
                    };
                    signals.coil_setpoints.insert(name.clone(), sp);
                }
            }
            HvacMode::Deadband | HvacMode::Heating => {
                if role == CoilRole::Cooling {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
        }
        signals.air_mass_flows.insert(name.clone(), total_flow);
    }

    signals.mixed_air_temp = Some(mixed_air_temp);
    signals.oa_fraction = Some(oa_frac);
    signals.loop_plr = Some(1.0);

    signals
}

/// CRAH: chilled-water air handler for data centers.
///
/// Identical to CRAC in control logic but uses a chilled-water coil.
/// No OA mixing, no economizer, cooling-only, high sensible (SHR ≈ 0.98).
pub fn build_crah_signals(
    li: &LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    zone_cooling_loads: &HashMap<String, f64>,
    predictor_modes: &HashMap<String, HvacMode>,
    zone_humidity_ratios: &HashMap<String, f64>,
    zone_max_rh: &HashMap<String, f64>,
    zone_dc_return_temps: &HashMap<String, f64>,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    let zone_name = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
    let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(24.0);
    let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);

    let total_flow: f64 = li
        .served_zones
        .iter()
        .map(|z| zone_design_flows.get(z).copied().unwrap_or(0.5))
        .sum::<f64>()
        .max(0.01);

    let zone_cool_load = zone_cooling_loads.get(zone_name).copied().unwrap_or(0.0);
    let predictor_mode = predictor_modes.get(zone_name).copied().unwrap_or_else(|| {
        if zone_temp > cool_sp || zone_cool_load > 100.0 {
            HvacMode::Cooling
        } else {
            HvacMode::Deadband
        }
    });
    let mode = match predictor_mode {
        HvacMode::Heating => HvacMode::Cooling,
        other => other,
    };

    let zone_w = zone_humidity_ratios
        .get(zone_name)
        .copied()
        .unwrap_or(0.008);
    let zone_rh_pct = openbse_psychrometrics::rh_fn_tdb_w_pb(zone_temp, zone_w, 101325.0) * 100.0;
    let mut dehumidify_only = false;
    let mode = if let Some(&max_rh) = zone_max_rh.get(zone_name) {
        if zone_rh_pct > max_rh && mode == HvacMode::Deadband {
            dehumidify_only = true;
            HvacMode::Cooling
        } else {
            mode
        }
    } else {
        mode
    };

    // CRAH recirculates room air (no OA). For DC zones, return air temp
    // is warmer than zone average due to rack heat — use pre-computed value.
    let return_air_temp = zone_dc_return_temps
        .get(zone_name)
        .copied()
        .unwrap_or(zone_temp);
    let mixed_air_temp = return_air_temp;

    for name in &li.component_names {
        let role = li.coil_role(name);
        match mode {
            HvacMode::Cooling => {
                if role == CoilRole::Cooling {
                    let sp = if dehumidify_only {
                        zone_temp - 0.5
                    } else {
                        li.cooling_supply_temp // CHW coil targets SAT setpoint
                    };
                    signals.coil_setpoints.insert(name.clone(), sp);
                }
            }
            HvacMode::Deadband | HvacMode::Heating => {
                if role == CoilRole::Cooling {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
        }
        signals.air_mass_flows.insert(name.clone(), total_flow);
    }

    signals.mixed_air_temp = Some(mixed_air_temp);
    signals.oa_fraction = Some(0.0);
    signals.loop_plr = Some(1.0);

    signals
}

/// DOAS: 100% outdoor air, fixed supply setpoints, always on.
///
/// Supply temperature setpoints:
///   Heating:  max zone heating setpoint + 2°C (ensures OA is delivered above zone setpoint)
///   Cooling:  min zone cooling setpoint - 2°C (dehumidified neutral air)
///
/// This prevents the DOAS from delivering supply air that is colder than the zone
/// heating setpoint in winter (which would add heating load to the zones).
pub fn build_doas_signals(
    li: &LoopInfo,
    zone_design_flows: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    t_outdoor: f64,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // Total ventilation airflow = 30% of zone design flows
    let vent_flow_total: f64 = li
        .served_zones
        .iter()
        .map(|z| zone_design_flows.get(z).copied().unwrap_or(0.1))
        .sum::<f64>()
        * 0.30;
    let vent_flow = vent_flow_total.max(0.05);

    // Supply setpoints: heat to 2°C above zone heating setpoint,
    // cool to 2°C below zone cooling setpoint.
    // Clamp: never heat if OA is already above heating setpoint; never cool if below.
    let max_heat_sp = li
        .served_zones
        .iter()
        .map(|z| zone_heat_sp.get(z).copied().unwrap_or(21.0))
        .fold(f64::NEG_INFINITY, f64::max);
    let min_cool_sp = li
        .served_zones
        .iter()
        .map(|z| zone_cool_sp.get(z).copied().unwrap_or(24.0))
        .fold(f64::INFINITY, f64::min);

    // DOAS heating setpoint: 2°C above zone heating setpoint (deliver warm neutral air)
    let t_supply_heat = max_heat_sp + 2.0;
    // DOAS cooling setpoint: 2°C below zone cooling setpoint (deliver cool dehumidified air)
    let t_supply_cool = (min_cool_sp - 2.0).max(14.0); // 14°C minimum for dehumidification

    for name in &li.component_names {
        let role = li.coil_role(name);
        if role == CoilRole::Heating {
            // Fire only if OA is below heating target
            if t_outdoor < t_supply_heat {
                signals.coil_setpoints.insert(name.clone(), t_supply_heat);
            } else {
                signals.coil_setpoints.insert(name.clone(), -99.0); // off
            }
        } else if role == CoilRole::Cooling {
            // Fire only if OA is above cooling target (summer dehumidification)
            if t_outdoor > t_supply_cool {
                signals.coil_setpoints.insert(name.clone(), t_supply_cool);
            } else {
                signals.coil_setpoints.insert(name.clone(), 99.0); // off
            }
        }
        signals.air_mass_flows.insert(name.clone(), vent_flow);
    }

    // DOAS inlet is always 100% outdoor air
    signals.oa_fraction = Some(1.0);

    signals
}

/// FCU: recirculating fan coil, per-zone thermostat (one zone per FCU loop).
pub fn build_fcu_signals(
    li: &LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    t_outdoor: f64,
    zone_heating_loads: &HashMap<String, f64>,
    zone_cooling_loads: &HashMap<String, f64>,
    predictor_modes: &HashMap<String, HvacMode>,
    zone_humidity_ratios: &HashMap<String, f64>,
    zone_max_rh: &HashMap<String, f64>,
    zone_min_rh: &HashMap<String, f64>,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // FCU serves one zone (its name is the zone)
    let zone_name = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
    let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(21.0);
    let heat_sp = zone_heat_sp.get(zone_name).copied().unwrap_or(21.1);
    let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);

    let design_flow = zone_design_flows.get(zone_name).copied().unwrap_or(0.3);

    // Use predictor mode (from frozen ideal loads) to prevent mode
    // flip-flopping during HVAC↔envelope iteration.
    let mut mode = predictor_modes
        .get(zone_name)
        .copied()
        .unwrap_or_else(|| hvac_mode(zone_temp, heat_sp, cool_sp));

    // RH override: if zone is over-humid and in deadband, force Cooling so the
    // DX coil activates and dehumidifies.
    let zone_w_fcu = zone_humidity_ratios
        .get(zone_name)
        .copied()
        .unwrap_or(0.008);
    let zone_rh_pct_fcu =
        openbse_psychrometrics::rh_fn_tdb_w_pb(zone_temp, zone_w_fcu, 101325.0) * 100.0;
    let mut dehumidify_only_fcu = false;
    if let Some(&max_rh) = zone_max_rh.get(zone_name) {
        if zone_rh_pct_fcu > max_rh && mode == HvacMode::Deadband {
            mode = HvacMode::Cooling;
            dehumidify_only_fcu = true;
        }
    }

    // PTAC: Fan runs at design flow when heating or cooling (mode != Deadband).
    // In deadband with cycling fan the system is off.
    // In deadband with continuous fan, fan runs at design flow recirculating
    // zone air (fan heat only — coils disabled).  This matches E+ behaviour
    // where Supply Air Fan Operating Mode Schedule = 1 (continuous).
    // E+ PTAC heating uses water coil modulation (PLR=1, valve throttles).
    // E+ PTAC cooling uses DX ON/OFF cycling (PLR < 1).
    // PTHP: identical flow/OA/setpoint dispatch to PTAC, but heating uses a
    // heat pump coil with ON/OFF cycling (PLR < 1) just like DX cooling.
    //
    // FCU: modulates fan speed proportionally.
    let is_pthp = li.system_type == AirLoopSystemType::Pthp;
    let is_ptac = li.system_type == AirLoopSystemType::Ptac || is_pthp;
    let is_continuous_fan_mode = li.fan_operating_mode
        == openbse_io::input::FanOperatingMode::Continuous
        || li.fan_operating_mode == openbse_io::input::FanOperatingMode::ContinuousNoLoadOff;
    let is_no_load_off_mode =
        li.fan_operating_mode == openbse_io::input::FanOperatingMode::ContinuousNoLoadOff;
    let flow = if is_ptac {
        match mode {
            HvacMode::Deadband => {
                if is_continuous_fan_mode && !is_no_load_off_mode {
                    design_flow // continuous fan: recirculate zone air
                } else {
                    0.0 // cycling fan: system off
                }
            }
            _ => design_flow,
        }
    } else {
        // FCU modulates fan speed: deadband = 20%, heating/cooling = proportional
        match mode {
            HvacMode::Deadband => design_flow * 0.20,
            HvacMode::Heating => {
                let error = (heat_sp - zone_temp).clamp(0.0, 5.0);
                let frac = 0.30 + 0.70 * (error / 5.0); // 30-100% of design
                design_flow * frac
            }
            HvacMode::Cooling => {
                let error = (zone_temp - cool_sp).clamp(0.0, 5.0);
                let frac = 0.30 + 0.70 * (error / 5.0); // 30-100% of design
                design_flow * frac
            }
        }
    };

    // PTAC OA = 0 (matching E+): PTAC recirculates zone air only.
    // Zone ventilation is handled independently by zone outdoor_air spec
    // (equivalent to E+ separate ERV with 0% effectiveness).
    // FCU: also recirculates zone air only (OA fraction = 0).
    let oa_frac = if is_ptac { li.min_oa_fraction } else { 0.0 };
    let mixed_air_temp = (1.0 - oa_frac) * zone_temp + oa_frac * t_outdoor;

    // PTAC uses ON/OFF cycling with PLR modulation (like PSZ-AC):
    // coils target design supply temps at full capacity, then PLR
    // scales the output to match the zone load.
    //
    // FCU uses proportional modulation: coil setpoint varies with zone error.
    for name in &li.component_names {
        let role = li.coil_role(name);
        if is_ptac {
            // PTAC / PTHP control matching EnergyPlus:
            //
            // Heating: coil targets the design supply temp at full capacity.
            // PLR cycling (computed in simulate_all_loops) sets the ON/OFF
            // duty cycle to match the zone load.  For PTAC this is a water
            // coil; for PTHP this is a heat pump coil — both use the same
            // ON/OFF PLR path.
            //
            // Cooling (DX coil): same ON/OFF PLR approach.
            match mode {
                HvacMode::Heating => {
                    // E+ PTAC (Fan:OnOff cycling): run heating coil at
                    // design supply temp during ON-period, off during
                    // OFF-period.  PLR sets the duty cycle.
                    if role == CoilRole::Heating {
                        signals
                            .coil_setpoints
                            .insert(name.clone(), li.heating_supply_temp);
                    } else if role == CoilRole::Cooling {
                        signals.coil_setpoints.insert(name.clone(), 99.0);
                    }
                }
                HvacMode::Cooling => {
                    // DX cooling: run at full capacity, PLR handles cycling.
                    if role == CoilRole::Cooling {
                        signals
                            .coil_setpoints
                            .insert(name.clone(), li.cooling_supply_temp);
                    } else if role == CoilRole::Heating {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    }
                }
                HvacMode::Deadband => {
                    if role == CoilRole::Heating {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    } else if role == CoilRole::Cooling {
                        signals.coil_setpoints.insert(name.clone(), 99.0);
                    }
                }
            }
        } else {
            // FCU: proportional modulation
            match mode {
                HvacMode::Heating => {
                    let error = heat_sp - zone_temp;
                    let target = (heat_sp + error.min(14.0)).clamp(heat_sp, 45.0);
                    if role == CoilRole::Heating {
                        signals.coil_setpoints.insert(name.clone(), target);
                    } else if role == CoilRole::Cooling {
                        signals.coil_setpoints.insert(name.clone(), 99.0);
                    }
                }
                HvacMode::Cooling => {
                    let error = zone_temp - cool_sp;
                    let target = (cool_sp - error.min(10.0)).clamp(12.0, cool_sp);
                    if role == CoilRole::Cooling {
                        signals.coil_setpoints.insert(name.clone(), target);
                    } else if role == CoilRole::Heating {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    }
                }
                HvacMode::Deadband => {
                    if role == CoilRole::Heating {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    } else if role == CoilRole::Cooling {
                        signals.coil_setpoints.insert(name.clone(), 99.0);
                    }
                }
            }
        }
        // Dehumidification-only: override cooling coil setpoint to minimize sensible cooling
        if dehumidify_only_fcu && (role == CoilRole::Cooling) {
            signals.coil_setpoints.insert(name.clone(), zone_temp - 0.5);
        }
        // Humidification control
        if role == CoilRole::Humidifier {
            if let Some(&min_rh) = zone_min_rh.get(zone_name) {
                if zone_rh_pct_fcu < min_rh {
                    let w_target =
                        openbse_psychrometrics::w_fn_tdb_rh_pb(zone_temp, min_rh / 100.0, 101325.0);
                    signals.coil_setpoints.insert(name.clone(), w_target);
                }
            }
        }
        signals.air_mass_flows.insert(name.clone(), flow);
    }

    signals.mixed_air_temp = Some(mixed_air_temp);
    signals.oa_fraction = Some(oa_frac);

    signals
}

/// VAV: central AHU + per-zone VAV boxes with reheat.
///
/// ASHRAE Guideline 36 §5.2 / §5.16 — Dual-Maximum VAV control:
///
///   **Zone-level (VAV box):**
///   - Cooling: airflow ramps from V_min up to V_cool_max (100% design) proportional to error
///   - Deadband: airflow at V_min (ventilation minimum)
///   - Heating: airflow ramps from V_min up to V_heat_max (50% design), AND reheat coil fires
///     This is "dual-maximum" — heating has its own max, not the single-maximum of old systems
///
///   **AHU-level:**
///   - SAT reset (G36 §5.16): reset supply temp from 13°C (max cooling) to 18°C (min cooling)
///     based on cooling demand across all zones. Saves energy in mild weather.
///   - Economizer: differential dry-bulb (100% OA when OA < return in cooling)
///   - Preheat: frost protection when mixed air < 4°C
pub fn build_vav_signals(
    li: &LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    t_outdoor: f64,
    effective_min_oa: f64,
    economizer_lockout: bool,
    raw_t_outdoor: f64,
    schedule_mgr: Option<&ScheduleManager>,
    hour: u32,
    day_of_week: u32,
    zone_cooling_loads: &HashMap<String, f64>,
    zone_heating_loads: &HashMap<String, f64>,
    _supply_air_temp: f64,
    zone_thermal_caps: &HashMap<String, f64>,
    w_outdoor: f64,
    zone_humidity_ratios: &HashMap<String, f64>,
    zone_max_rh: &HashMap<String, f64>,
    zone_min_rh: &HashMap<String, f64>,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // ── SetpointManager:Warmest SAT calculation ──
    //
    // E+ finds the HIGHEST supply air temp that satisfies ALL cooling zones
    // at their current VAV flow. For each cooling zone:
    //   SAT_zone = T_zone - Q_cool / (Cp × m_max)
    // System SAT = min(SAT_max, min(SAT_zone across all cooling zones))
    //
    // This keeps SAT as warm as possible, minimizing both cooling coil work
    // AND reheat energy (the key to avoiding simultaneous heating/cooling).
    let cp = 1005.0_f64;
    let v_heat_max_frac = 0.50;
    let sat_min = li.cooling_supply_temp; // E+ SetpointManager:Warmest MinimumTemperature
    let sat_max = 15.6_f64; // E+ SetpointManager:Warmest MaximumTemperature

    let mut sat_setpoint = sat_max; // start warm, only drop if a zone needs it
    let mut any_cooling_zone = false;

    for zone_name in &li.served_zones {
        let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(21.0);
        let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);
        let design_flow = zone_design_flows.get(zone_name).copied().unwrap_or(0.5);
        let cool_load = zone_cooling_loads.get(zone_name).copied().unwrap_or(0.0);

        if zone_temp > cool_sp && cool_load > 100.0 {
            any_cooling_zone = true;
            // What SAT would satisfy this zone at max VAV flow?
            // Q = m_max × Cp × (T_zone - SAT)
            // SAT = T_zone - Q / (m_max × Cp)
            let sat_needed = zone_temp - cool_load / (design_flow * cp);
            sat_setpoint = sat_setpoint.min(sat_needed);
        }
    }
    // Clamp to E+ SetpointManager range
    let sat_setpoint = sat_setpoint.clamp(sat_min, sat_max);

    // ── Compute zone flows using the SAT-derived supply temp ──
    //
    // Now compute load-based zone airflows using the actual SAT that the
    // cooling coil will target. This ensures mass balance: fan flow = Σ(zone flows).
    let mut total_flow = 0.0f64;
    let mut max_cooling_demand = 0.0f64;

    for zone_name in &li.served_zones {
        let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(21.0);
        let heat_sp = zone_heat_sp.get(zone_name).copied().unwrap_or(21.1);
        let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);
        let design_flow = zone_design_flows.get(zone_name).copied().unwrap_or(0.5);

        let base_mode = hvac_mode(zone_temp, heat_sp, cool_sp);
        // RH override: if zone is over-humid and in deadband, force Cooling
        // to increase VAV airflow and drive the DX coil for dehumidification.
        let zone_w_vav = zone_humidity_ratios
            .get(zone_name)
            .copied()
            .unwrap_or(0.008);
        let zone_rh_vav =
            openbse_psychrometrics::rh_fn_tdb_w_pb(zone_temp, zone_w_vav, 101325.0) * 100.0;
        let mode = if let Some(&max_rh) = zone_max_rh.get(zone_name) {
            if zone_rh_vav > max_rh && base_mode == HvacMode::Deadband {
                any_cooling_zone = true;
                HvacMode::Cooling
            } else {
                base_mode
            }
        } else {
            base_mode
        };

        let zone_flow = match mode {
            HvacMode::Cooling => {
                let cool_load = zone_cooling_loads.get(zone_name).copied().unwrap_or(0.0);
                if cool_load > 100.0 {
                    // m = Q / (Cp × (T_zone - SAT))
                    let dt = (zone_temp - sat_setpoint).max(1.0);
                    let m_needed = cool_load / (cp * dt);
                    let min_flow = design_flow * li.min_vav_fraction;
                    let flow = m_needed.clamp(min_flow, design_flow);
                    let frac =
                        ((flow - min_flow) / (design_flow - min_flow).max(0.001)).clamp(0.0, 1.0);
                    max_cooling_demand = max_cooling_demand.max(frac);
                    flow
                } else {
                    // Dehumidification-only: run at minimum flow to activate DX coil
                    design_flow * li.min_vav_fraction
                }
            }
            HvacMode::Heating => {
                let error = (heat_sp - zone_temp).clamp(0.0, 5.0);
                let frac =
                    li.min_vav_fraction + (v_heat_max_frac - li.min_vav_fraction) * (error / 5.0);
                design_flow * frac
            }
            HvacMode::Deadband => design_flow * li.min_vav_fraction,
        };

        signals.zone_air_flows.insert(zone_name.clone(), zone_flow);
        total_flow += zone_flow;
    }
    total_flow = total_flow.max(0.05);

    // ── ASHRAE 62.1 §6.2.5 Multi-Zone VRP: System Ventilation Efficiency ──
    //
    // In a multi-zone recirculating system (VAV), all zones share the same
    // mixed air (same OA fraction). When zones are at part load (minimum
    // flow), they receive less absolute OA than needed. The VRP corrects
    // by increasing the system OA fraction based on the "critical zone"
    // — the zone with the highest required discharge OA fraction (Zd).
    //
    // E+ implements this via Controller:MechanicalVentilation.
    let vrp_min_oa = if !li.zone_oa_data.is_empty() {
        let air_density = 1.204_f64; // kg/m³ at standard conditions
        let mut vou = 0.0_f64; // uncorrected total OA [m³/s]
        let mut max_zd = 0.0_f64; // critical zone discharge OA fraction

        for oa in &li.zone_oa_data {
            // Occupancy fraction from people schedule (design occupancy if no schedule)
            let occ_frac = if let Some(ref sched_name) = oa.people_schedule {
                schedule_mgr
                    .map(|sm| sm.fraction(sched_name, hour, day_of_week))
                    .unwrap_or(1.0)
            } else {
                1.0
            };

            // Breathing zone OA [m³/s]: ASHRAE 62.1 Eq 6-1
            let vbz =
                oa.per_person_oa * oa.design_people * occ_frac + oa.per_area_oa * oa.floor_area;
            // Zone OA with distribution effectiveness: Voz = Vbz / Ez
            // Ez = 1.0 for well-mixed ceiling supply (ASHRAE 62.1 Table 6-2)
            let voz = vbz;
            vou += voz;

            // Zone discharge OA fraction: Zd = Voz / Vdz
            // Vdz = actual zone airflow [m³/s]
            let vdz_kg = signals
                .zone_air_flows
                .get(&oa.zone_name)
                .copied()
                .unwrap_or(0.1);
            let vdz = vdz_kg / air_density; // kg/s → m³/s
            if vdz > 0.001 {
                let zd = voz / vdz;
                max_zd = max_zd.max(zd);
            }
        }

        // System ventilation efficiency: ASHRAE 62.1 Eq 6-6
        // Ev = 1 + Xs - max(Zd)
        let vps = total_flow / air_density; // total supply [m³/s]
        let xs = if vps > 0.01 { vou / vps } else { 0.0 };
        let ev = (1.0 + xs - max_zd).clamp(0.15, 1.0);

        // Corrected system OA: Vot = Vou / Ev
        let vot = vou / ev;
        let ys = if vps > 0.01 {
            vot / vps
        } else {
            effective_min_oa
        };

        // VRP OA fraction: never less than the original design OA
        ys.clamp(effective_min_oa, 1.0)
    } else {
        effective_min_oa
    };

    // ── Return air temperature (flow-weighted average of zone temps) ──
    let avg_zone_temp = if li.served_zones.is_empty() {
        21.0
    } else {
        li.served_zones
            .iter()
            .map(|z| zone_temps.get(z).copied().unwrap_or(21.0))
            .sum::<f64>()
            / li.served_zones.len() as f64
    };

    // ── Economizer: modulating differential dry-bulb ──
    // In cooling mode: modulate OA fraction to achieve SAT setpoint.
    // If OA can fully satisfy SAT, no mechanical cooling needed (free cooling).
    //
    // IMPORTANT: The economizer decides OA fraction based on RAW outdoor
    // temperature (not post-HR effective temperature).  The economizer benefits
    // from cold OA for free cooling — the HR's preheating effect would mislead
    // the economizer into thinking OA is warmer than it actually is.
    //
    // The mixed air calculation then uses effective_t_outdoor (= t_outdoor param)
    // which already includes the HR preheating effect.
    // Economizer activation: run when any served zone has cooling load.
    // E+ uses LockoutWithHeating: economizer locks out when the AHU
    // preheat coil would fire (mixed air < SAT). In practice, this
    // means the economizer only runs when OA is warm enough that the
    // mixed air doesn't need preheating.
    //
    // Additionally, the economizer only activates when cooling-dominant
    // (more zones need cooling than heating). This approximates E+'s
    // behavior where the economizer provides free cooling only when
    // beneficial to the system as a whole.
    let any_served_cooling = any_cooling_zone
        || li
            .served_zones
            .iter()
            .any(|z| zone_cooling_loads.get(z).copied().unwrap_or(0.0) > 100.0);
    // Economizer only activates when cooling is dominant (more zones
    // need cooling than heating). This approximates E+'s LockoutWithHeating:
    // when many perimeter zones need heating, bringing in cold OA would
    // force excessive VAV reheat. Locking out the economizer keeps the
    // mixed air warm, reducing both preheat and reheat energy.
    // Economizer lockout: only activate when more zones need cooling
    // than heating. This approximates E+'s LockoutWithHeating behavior
    // and balances free-cooling against reheat penalty.
    let cooling_dominant = {
        let n_cool = li
            .served_zones
            .iter()
            .filter(|z| zone_cooling_loads.get(*z).copied().unwrap_or(0.0) > 100.0)
            .count();
        let n_heat = li
            .served_zones
            .iter()
            .filter(|z| zone_heating_loads.get(*z).copied().unwrap_or(0.0) > 100.0)
            .count();
        n_cool > n_heat
    };
    let any_cooling = any_served_cooling && cooling_dominant;
    let avg_zone_w = if li.served_zones.is_empty() {
        0.008
    } else {
        li.served_zones
            .iter()
            .map(|z| zone_humidity_ratios.get(z).copied().unwrap_or(0.008))
            .sum::<f64>()
            / li.served_zones.len() as f64
    };
    let return_enthalpy_vav = openbse_psychrometrics::h_fn_tdb_w(avg_zone_temp, avg_zone_w);
    let outdoor_enthalpy_vav = openbse_psychrometrics::h_fn_tdb_w(raw_t_outdoor, w_outdoor);
    use openbse_io::input::EconomizerType;
    let econ_available = match li.economizer_type {
        EconomizerType::NoEconomizer => false,
        EconomizerType::FixedDryBulb => {
            let limit = li.economizer_high_limit.unwrap_or(23.889);
            raw_t_outdoor < limit
        }
        EconomizerType::DifferentialDryBulb => raw_t_outdoor < avg_zone_temp,
        EconomizerType::DifferentialEnthalpy => outdoor_enthalpy_vav < return_enthalpy_vav,
        EconomizerType::FixedEnthalpy => {
            let limit = li.economizer_high_limit_enthalpy.unwrap_or(65_200.0);
            outdoor_enthalpy_vav < limit
        }
        EconomizerType::EnthalpyWithHighLimit => {
            let temp_limit = li.economizer_high_limit.unwrap_or(23.889);
            outdoor_enthalpy_vav < return_enthalpy_vav && raw_t_outdoor < temp_limit
        }
    };
    // ── E+ LockoutWithHeating economizer logic ──
    //
    // Step 1: compute the economizer OA fraction for free cooling.
    // Step 2: check if the resulting mixed air needs preheating.
    //         If so, lock to minimum OA (LockoutWithHeating).
    //
    // This prevents the economizer from bringing in cold OA that
    // then requires reheat at every perimeter zone, wasting energy.
    let oa_frac = if economizer_lockout {
        // HR active → economizer locked to minimum OA
        vrp_min_oa
    } else if any_cooling && econ_available {
        // Economizer: modulate OA for free cooling
        let delta = avg_zone_temp - raw_t_outdoor;
        let econ_oa = if delta > 0.1 {
            let needed = (avg_zone_temp - sat_setpoint) / delta;
            needed.clamp(vrp_min_oa, 1.0)
        } else {
            vrp_min_oa
        };

        // LockoutWithHeating: if the resulting mixed air is below SAT,
        // the preheat coil would fire. Lock economizer to minimum OA instead.
        let trial_mixed = avg_zone_temp * (1.0 - econ_oa) + t_outdoor * econ_oa;
        if trial_mixed < sat_setpoint {
            // Preheat would fire → lock to minimum OA
            vrp_min_oa
        } else {
            econ_oa
        }
    } else {
        vrp_min_oa
    };
    // Mixed air uses effective (post-HR) outdoor temperature
    let mixed_air_temp = avg_zone_temp * (1.0 - oa_frac) + t_outdoor * oa_frac;

    // ── AHU coil control ──
    for name in &li.component_names {
        let role = li.coil_role(name);
        if role == CoilRole::Cooling {
            if any_cooling {
                // AHU cooling coil targets the SAT setpoint
                signals.coil_setpoints.insert(name.clone(), sat_setpoint);
            } else {
                // No cooling demand — coil off
                signals.coil_setpoints.insert(name.clone(), 99.0);
            }
        } else if role == CoilRole::Heating {
            // AHU heating coil: frost protection only.
            //
            // E+ data shows the VAV_MID heating coil rarely fires — the
            // economizer provides free cooling by mixing cold OA with warm
            // return air. The mixed air goes directly to VAV boxes without
            // being heated to SAT. This avoids wasting preheat energy.
            //
            // Only fire the preheat coil for frost protection (mixed air < 2°C)
            // to prevent freezing in the AHU. Zone reheat handles the warming.
            let frost_protection_temp = 2.0_f64;
            if mixed_air_temp < frost_protection_temp {
                signals
                    .coil_setpoints
                    .insert(name.clone(), frost_protection_temp);
            } else {
                signals.coil_setpoints.insert(name.clone(), -99.0);
            }
        }
        // Humidification control: if any served zone is below min_rh, activate humidifier
        if role == CoilRole::Humidifier {
            for zone_name in &li.served_zones {
                if let Some(&min_rh) = zone_min_rh.get(zone_name) {
                    let zone_temp_h = zone_temps.get(zone_name).copied().unwrap_or(21.0);
                    let zone_w_h = zone_humidity_ratios
                        .get(zone_name)
                        .copied()
                        .unwrap_or(0.008);
                    let zone_rh_h =
                        openbse_psychrometrics::rh_fn_tdb_w_pb(zone_temp_h, zone_w_h, 101325.0)
                            * 100.0;
                    if zone_rh_h < min_rh {
                        let w_target = openbse_psychrometrics::w_fn_tdb_rh_pb(
                            zone_temp_h,
                            min_rh / 100.0,
                            101325.0,
                        );
                        signals.coil_setpoints.insert(name.clone(), w_target);
                        break; // Set based on first zone needing humidification
                    }
                }
            }
        }
        signals.air_mass_flows.insert(name.clone(), total_flow);
    }

    // Inject mixed air temp + OA fraction
    signals.mixed_air_temp = Some(mixed_air_temp);
    signals.oa_fraction = Some(oa_frac);

    // Store SAT setpoint for heat recovery credit cap calculation
    signals.sat_setpoint = sat_setpoint;

    signals
}

// ─── Dual-Duct Signal Builder ────────────────────────────────────────────────
//
// Each zone has a mixing box with two dampers (hot and cold deck).
// The box blends hot and cold supply air at constant total flow (CAV).
// The signal builder:
//   1. Determines zone mode (Heating / Cooling / Deadband) from predictor temps.
//   2. Computes zone PLR from the zone's estimated load.
//   3. Calls DualDuctBox::simulate() to get blended supply temp and flow.
//   4. Stores per-zone supply temps in signals.zone_supply_temps and
//      per-zone flows in signals.zone_air_flows.
//   5. Sets AHU coil setpoints:
//      - Hot deck coil: target = heating_supply_temp when any zone needs heat
//      - Cold deck coil: target = cooling_supply_temp when any zone needs cool
//      - Fan: receives total design flow (Σ zone design_flows)
#[allow(clippy::too_many_arguments)]
pub fn build_dual_duct_signals(
    li: &mut LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    t_outdoor: f64,
    effective_min_oa: f64,
    zone_cooling_loads: &HashMap<String, f64>,
    zone_heating_loads: &HashMap<String, f64>,
    zone_humidity_ratios: &HashMap<String, f64>,
    zone_max_rh: &HashMap<String, f64>,
    zone_min_rh: &HashMap<String, f64>,
) -> ControlSignals {
    let mut signals = ControlSignals::default();
    let cp = 1005.0_f64;
    let hot_deck_temp = li.heating_supply_temp;
    let cold_deck_temp = li.cooling_supply_temp;

    let mut total_flow = 0.0_f64;
    let mut any_heating = false;
    let mut any_cooling = false;

    for zone_name in &li.served_zones {
        let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(21.0);
        let heat_sp = zone_heat_sp.get(zone_name).copied().unwrap_or(21.1);
        let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);
        let heat_load = zone_heating_loads.get(zone_name).copied().unwrap_or(0.0);
        let cool_load = zone_cooling_loads.get(zone_name).copied().unwrap_or(0.0);

        let mode = hvac_mode(zone_temp, heat_sp, cool_sp);

        // RH override: if zone is over-humid and in deadband, force cooling
        let zone_w = zone_humidity_ratios
            .get(zone_name)
            .copied()
            .unwrap_or(0.008);
        let zone_rh = openbse_psychrometrics::rh_fn_tdb_w_pb(zone_temp, zone_w, 101325.0) * 100.0;
        let mode = if let Some(&max_rh) = zone_max_rh.get(zone_name) {
            if zone_rh > max_rh && mode == HvacMode::Deadband {
                HvacMode::Cooling
            } else {
                mode
            }
        } else {
            mode
        };

        let heating = mode == HvacMode::Heating;
        let cooling = mode == HvacMode::Cooling;
        if heating {
            any_heating = true;
        }
        if cooling {
            any_cooling = true;
        }

        // PLR: fraction of available ΔT used
        let plr = match mode {
            HvacMode::Heating if heat_load > 0.0 => {
                // Estimate PLR from load vs. max capacity at design flow
                let design_flow = zone_design_flows.get(zone_name).copied().unwrap_or(
                    li.dd_boxes
                        .get(zone_name)
                        .map(|b| b.design_flow)
                        .unwrap_or(0.5),
                );
                let q_max = design_flow * cp * (hot_deck_temp - heat_sp).max(1.0);
                (heat_load / q_max).clamp(0.0, 1.0)
            }
            HvacMode::Cooling if cool_load > 0.0 => {
                let design_flow = zone_design_flows.get(zone_name).copied().unwrap_or(
                    li.dd_boxes
                        .get(zone_name)
                        .map(|b| b.design_flow)
                        .unwrap_or(0.5),
                );
                let q_max = design_flow * cp * (cool_sp - cold_deck_temp).max(1.0);
                (cool_load / q_max).clamp(0.0, 1.0)
            }
            _ => 0.0,
        };

        // Get or create a DualDuctBox for this zone.
        // If not already in li.dd_boxes, use design_flow from zone_design_flows.
        let (supply_temp, zone_flow) = if let Some(dd_box) = li.dd_boxes.get_mut(zone_name) {
            dd_box.simulate(heating, cooling, plr, hot_deck_temp, cold_deck_temp)
        } else {
            // No box registered (e.g., during warmup before autosizing)
            let fallback_flow = zone_design_flows.get(zone_name).copied().unwrap_or(0.5);
            let min_flow = fallback_flow * 0.20;
            let (hot_flow, cold_flow) = if heating {
                let hf = min_flow + plr * (fallback_flow - min_flow);
                (hf, fallback_flow - hf)
            } else if cooling {
                let cf = min_flow + plr * (fallback_flow - min_flow);
                (fallback_flow - cf, cf)
            } else {
                (fallback_flow / 2.0, fallback_flow / 2.0)
            };
            let blended = (hot_flow * hot_deck_temp + cold_flow * cold_deck_temp) / fallback_flow;
            (blended, fallback_flow)
        };

        signals
            .zone_supply_temps
            .insert(zone_name.clone(), supply_temp);
        signals.zone_air_flows.insert(zone_name.clone(), zone_flow);
        total_flow += zone_flow;
    }
    total_flow = total_flow.max(0.05);

    // ── AHU coil setpoints ──
    // Hot deck: target heating_supply_temp when any zone in heating mode
    // Cold deck: target cooling_supply_temp when any zone in cooling mode
    // Both operate simultaneously — each deck heats/cools its own portion of air
    for name in &li.component_names {
        let role = li.coil_role(name);
        if role == CoilRole::Cooling {
            // Cold deck coil
            if any_cooling {
                signals.coil_setpoints.insert(name.clone(), cold_deck_temp);
            } else {
                signals.coil_setpoints.insert(name.clone(), 99.0);
            }
        } else if role == CoilRole::Heating {
            // Hot deck coil
            if any_heating {
                signals.coil_setpoints.insert(name.clone(), hot_deck_temp);
            } else {
                signals.coil_setpoints.insert(name.clone(), -99.0);
            }
        }
        signals.air_mass_flows.insert(name.clone(), total_flow);
    }

    // Mixed air for AHU inlet: blend outdoor and return air at min_oa_fraction
    let avg_zone_temp = if li.served_zones.is_empty() {
        21.0
    } else {
        li.served_zones
            .iter()
            .map(|z| zone_temps.get(z).copied().unwrap_or(21.0))
            .sum::<f64>()
            / li.served_zones.len() as f64
    };
    let mixed_air_temp = avg_zone_temp * (1.0 - effective_min_oa) + t_outdoor * effective_min_oa;
    signals.mixed_air_temp = Some(mixed_air_temp);
    signals.oa_fraction = Some(effective_min_oa);

    // Check RH min override for humidifier
    for name in &li.component_names {
        let role = li.coil_role(name);
        if role == CoilRole::Humidifier {
            for zone_name in &li.served_zones {
                if let Some(&min_rh) = zone_min_rh.get(zone_name) {
                    let zone_temp_h = zone_temps.get(zone_name).copied().unwrap_or(21.0);
                    let zone_w_h = zone_humidity_ratios
                        .get(zone_name)
                        .copied()
                        .unwrap_or(0.008);
                    let zone_rh_h =
                        openbse_psychrometrics::rh_fn_tdb_w_pb(zone_temp_h, zone_w_h, 101325.0)
                            * 100.0;
                    if zone_rh_h < min_rh {
                        let w_target = openbse_psychrometrics::w_fn_tdb_rh_pb(
                            zone_temp_h,
                            min_rh / 100.0,
                            101325.0,
                        );
                        signals.coil_setpoints.insert(name.clone(), w_target);
                        break;
                    }
                }
            }
        }
    }

    signals
}
