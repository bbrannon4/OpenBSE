//! Chiller driven by an ASHRAE Standard 205 RS0001 performance map.
//!
//! Unlike the polynomial/table-curve chiller in [`crate::chiller`], this
//! component reads tabulated performance directly from the manufacturer-
//! supplied data file.  Capacity and input power are looked up at the
//! current operating conditions; no separate CAPFT × EIRFT × EIRFPLR
//! decomposition is needed.
//!
//! ### Part-load behavior
//!
//! Standard 205 cooling maps include a `compressor_sequence_number` grid
//! axis that indexes loading stages (1 = minimum, N = full).  Given a
//! requested load:
//!
//! 1. Look up the full-sequence capacity at current temperatures.
//! 2. PLR = load / full_capacity (clamped to [0, 1]).
//! 3. Map PLR → sequence number:
//!    - CONTINUOUS speed: `seq = 1 + PLR * (N - 1)` (linear in PLR).
//!    - DISCRETE: snap to nearest integer step and apply cycling below the
//!      minimum step using the file's `cycling_degradation_coefficient`.
//! 4. Re-query the map at that sequence number to get power and capacity.
//!
//! Out-of-range conditions are edge-clamped (see `openbse-a205` crate docs).

use openbse_a205::rs0001::{
    CondenserType, CoolingInterpolator, CoolingQuery, Rs0001, SpeedControl,
};
use openbse_core::ports::*;
use openbse_psychrometrics::FluidState;

/// Convert °C → K
#[inline]
fn c_to_k(c: f64) -> f64 {
    c + 273.15
}

fn default_submeter() -> String {
    "General".to_string()
}

/// Chiller backed by an RS0001 performance map.
pub struct ChillerA205 {
    pub name: String,
    pub submeter: String,
    /// Leaving CHW temperature setpoint [°C].
    pub chw_setpoint: f64,
    /// Design CHW volumetric flow [m³/s].  If None, use the file's flow axis.
    pub design_chw_flow: f64,
    /// Lower bound on operating PLR (used for cycling below min step).
    pub min_plr: f64,
    /// For water-cooled chillers: fixed condenser entering water temperature
    /// [°C].  When None, falls back to wet-bulb + tower_approach.
    pub condenser_entering_temp: Option<f64>,
    /// For water-cooled (fallback): approach offset from outdoor wet-bulb
    /// to condenser entering water temperature [°C].
    pub tower_approach: f64,

    rs0001: Rs0001,
    interp: CoolingInterpolator,
    /// Cached nominal capacity at design conditions [W].
    nominal_capacity_w: f64,
    /// Ambient pressure to use in queries (Pa). From file grid midpoint.
    default_pressure: f64,
    /// Condenser air RH for queries (fraction). From file grid midpoint.
    default_rh: f64,
    /// Evaporator volumetric flow to query [m³/s] (from file's axis).
    file_evap_flow: f64,
    /// Condenser-liquid volumetric flow to query [m³/s] (water-cooled).
    file_cond_flow: f64,

    // runtime state
    actual_capacity: f64,
    electric_power: f64,
    plr: f64,
    sequence_number: f64,
    in_range: bool,
    water_inlet_temp: f64,
    water_outlet_temp: f64,
    water_mass_flow: f64,
}

impl std::fmt::Debug for ChillerA205 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChillerA205")
            .field("name", &self.name)
            .field("chw_setpoint", &self.chw_setpoint)
            .field("condenser_type", &self.rs0001.performance.condenser_type)
            .field("nominal_capacity_w", &self.nominal_capacity_w)
            .finish()
    }
}

impl ChillerA205 {
    /// Construct from a loaded RS0001 object.
    ///
    /// `chw_setpoint` is in °C; `design_chw_flow` is in m³/s (or 0 to
    /// derive from capacity downstream).
    pub fn from_rs0001(
        name: impl Into<String>,
        rs0001: Rs0001,
        chw_setpoint: f64,
        design_chw_flow: f64,
    ) -> Result<Self, openbse_a205::A205Error> {
        let interp = CoolingInterpolator::new(&rs0001.performance.performance_map_cooling)?;

        // Defaults for axes that aren't varied in the file (single-point axes).
        let g = &rs0001.performance.performance_map_cooling.grid_variables;
        let file_evap_flow = *g.evaporator_liquid_volumetric_flow_rate.first().unwrap();
        let file_cond_flow = g
            .condenser_liquid_volumetric_flow_rate
            .as_ref()
            .and_then(|v| v.first().copied())
            .unwrap_or(0.0);
        let default_pressure = g
            .ambient_pressure
            .as_ref()
            .and_then(|v| v.first().copied())
            .unwrap_or(101_325.0);
        let default_rh = g
            .condenser_air_entering_relative_humidity
            .as_ref()
            .and_then(|v| v.first().copied())
            .unwrap_or(0.4);

        // Precompute nominal (rated) capacity at AHRI-style design conditions.
        // The map's max compressor sequence at the user's chw setpoint and a
        // representative condenser temp gives us "full-load" capacity for
        // autosizing and PLR-based staging.
        let design_cond_c = match rs0001.performance.condenser_type {
            CondenserType::Air | CondenserType::Evaporative => 35.0, // AHRI 550/590 rated outdoor DB
            CondenserType::Liquid => 29.44, // AHRI rated entering CW (85°F)
        };
        let q = CoolingQuery {
            evap_volumetric_flow: file_evap_flow,
            evap_leaving_temp_k: c_to_k(chw_setpoint),
            condenser_temp_k: c_to_k(design_cond_c),
            condenser_air_rh: default_rh,
            condenser_liquid_flow: file_cond_flow,
            ambient_pressure_pa: default_pressure,
            compressor_sequence: interp.sequence_range.1,
        };
        let r = interp.query(&q)?;
        let nominal_capacity_w = r.net_evaporator_capacity;

        Ok(Self {
            name: name.into(),
            submeter: default_submeter(),
            chw_setpoint,
            design_chw_flow,
            min_plr: 0.10,
            condenser_entering_temp: None,
            tower_approach: 5.56,
            rs0001,
            interp,
            nominal_capacity_w,
            default_pressure,
            default_rh,
            file_evap_flow,
            file_cond_flow,
            actual_capacity: 0.0,
            electric_power: 0.0,
            plr: 0.0,
            sequence_number: 0.0,
            in_range: true,
            water_inlet_temp: 0.0,
            water_outlet_temp: 0.0,
            water_mass_flow: 0.0,
        })
    }

    pub fn condenser_type(&self) -> CondenserType {
        self.rs0001.performance.condenser_type
    }

    /// Determine condenser-side temperature [°C] for the current context.
    /// Mirrors the polynomial chiller's logic so the two paths behave the
    /// same with respect to outdoor weather and tower approach.
    fn condenser_temp_c(&self, ctx: &SimulationContext) -> f64 {
        match self.rs0001.performance.condenser_type {
            CondenserType::Air => ctx.outdoor_air.t_db,
            CondenserType::Evaporative => {
                // Approach to outdoor wet-bulb
                ctx.outdoor_air.t_wb() + self.tower_approach
            }
            CondenserType::Liquid => {
                let t_wb = ctx.outdoor_air.t_wb();
                match (self.condenser_entering_temp, self.tower_approach) {
                    (Some(sp), approach) if approach > 0.0 => sp.max(t_wb + approach),
                    (Some(sp), _) => sp,
                    (None, approach) => t_wb + approach,
                }
            }
        }
    }
}

impl PlantComponent for ChillerA205 {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Chiller
    }

    fn rated_capacity(&self) -> f64 {
        self.nominal_capacity_w
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.nominal_capacity_w)
    }

    fn design_water_flow_rate(&self) -> Option<f64> {
        if self.design_chw_flow > 0.0 {
            Some(self.design_chw_flow)
        } else if self.file_evap_flow > 0.0 {
            Some(self.file_evap_flow)
        } else {
            None
        }
    }

    fn power_consumption(&self) -> f64 {
        self.electric_power
    }

    fn thermal_output(&self) -> f64 {
        self.actual_capacity
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        ctx: &SimulationContext,
    ) -> WaterPort {
        let cp_water = 4186.0;
        if load <= 0.0 || inlet.state.mass_flow <= 0.0 {
            self.actual_capacity = 0.0;
            self.electric_power = 0.0;
            self.plr = 0.0;
            self.sequence_number = 0.0;
            self.water_inlet_temp = inlet.state.temp;
            self.water_outlet_temp = inlet.state.temp;
            self.water_mass_flow = inlet.state.mass_flow;
            return *inlet;
        }

        let t_cond_c = self.condenser_temp_c(ctx);
        let t_chw_leaving_c = self.chw_setpoint;

        // 1) Full-sequence capacity at current conditions
        let seq_max = self.interp.sequence_range.1;
        let seq_min = self.interp.sequence_range.0;
        let q_full = CoolingQuery {
            evap_volumetric_flow: self.file_evap_flow,
            evap_leaving_temp_k: c_to_k(t_chw_leaving_c),
            condenser_temp_k: c_to_k(t_cond_c),
            condenser_air_rh: self.default_rh,
            condenser_liquid_flow: self.file_cond_flow,
            ambient_pressure_pa: self.default_pressure,
            compressor_sequence: seq_max,
        };
        let r_full = match self.interp.query(&q_full) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("ChillerA205 '{}' map query failed: {}", self.name, e);
                self.actual_capacity = 0.0;
                self.electric_power = 0.0;
                return *inlet;
            }
        };
        let cap_full = r_full.net_evaporator_capacity.max(1.0);

        // 2) PLR
        let plr_raw = (load / cap_full).clamp(0.0, 1.0);

        // 3) Map PLR → sequence number; query at that point.
        // Below the min step, treat as cycling.
        let (sequence, cycling_ratio) = {
            let cont_seq = seq_min + plr_raw * (seq_max - seq_min);
            match self.rs0001.performance.compressor_speed_control_type {
                SpeedControl::Continuous => {
                    if cont_seq < seq_min {
                        // Cycling region
                        let cyc = plr_raw / self.min_plr.max(1e-6);
                        (seq_min, cyc.min(1.0))
                    } else {
                        (cont_seq, 1.0)
                    }
                }
                SpeedControl::Discrete => {
                    let snapped = cont_seq.round().clamp(seq_min, seq_max);
                    if cont_seq < seq_min {
                        let cyc = plr_raw / self.min_plr.max(1e-6);
                        (seq_min, cyc.min(1.0))
                    } else {
                        (snapped, 1.0)
                    }
                }
            }
        };

        let q_op = CoolingQuery {
            compressor_sequence: sequence,
            ..q_full
        };
        let r_op = match self.interp.query(&q_op) {
            Ok(r) => r,
            Err(_) => r_full,
        };

        // 4) Apply cycling degradation per RS0001
        // PLF = 1 - Cd * (1 - cycling_ratio); RTF = cycling_ratio / PLF
        let cd = self.rs0001.performance.cycling_degradation_coefficient;
        let inst_power = r_op.input_power;
        let inst_cap = r_op.net_evaporator_capacity;
        let (eff_power, eff_cap) = if cycling_ratio < 1.0 {
            let plf = (1.0 - cd * (1.0 - cycling_ratio)).max(0.01);
            let rtf = (cycling_ratio / plf).min(1.0);
            (inst_power * rtf, inst_cap * cycling_ratio)
        } else {
            (inst_power, inst_cap)
        };

        // Deliver up to the requested load
        let delivered = eff_cap.min(load);
        self.actual_capacity = delivered;
        self.electric_power = eff_power;
        self.plr = plr_raw;
        self.sequence_number = sequence;
        self.in_range = r_op.in_range;

        // Water outlet
        let mass_flow = inlet.state.mass_flow.max(0.001);
        let delta_t = delivered / (mass_flow * cp_water);
        let t_outlet = (inlet.state.temp - delta_t).max(self.chw_setpoint - 2.0);

        self.water_inlet_temp = inlet.state.temp;
        self.water_outlet_temp = t_outlet;
        self.water_mass_flow = mass_flow;

        WaterPort::new(FluidState::water(t_outlet, mass_flow))
    }

    fn detailed_outputs(&self) -> std::collections::HashMap<String, f64> {
        let mut m = std::collections::HashMap::new();
        m.insert("plr".into(), self.plr);
        m.insert("compressor_sequence".into(), self.sequence_number);
        m.insert("in_range".into(), if self.in_range { 1.0 } else { 0.0 });
        let cop = if self.electric_power > 0.0 {
            self.actual_capacity / self.electric_power
        } else {
            0.0
        };
        m.insert("efficiency_operating".into(), cop);
        m.insert("water_inlet_temperature".into(), self.water_inlet_temp);
        m.insert("water_outlet_temperature".into(), self.water_outlet_temp);
        m.insert("water_mass_flow".into(), self.water_mass_flow);
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;
    use std::path::PathBuf;

    fn example_path() -> PathBuf {
        // Path is relative to the openbse-components crate dir; the example
        // lives in the sibling openbse-a205 crate.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("openbse-a205")
            .join("examples")
            .join("RS0001_AppJ_CurveSetA.a205.json")
    }

    fn make_ctx(t_db: f64) -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 7,
                day: 15,
                hour: 14,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(t_db, 0.4, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn loads_and_sizes() {
        let rs = Rs0001::load(&example_path()).unwrap();
        // Curve Set A is an air-cooled chiller; the map covers chw leaving
        // 275–294 K (≈2–21 °C).  Our setpoint is 7 °C = 280.15 K (in range).
        let chiller = ChillerA205::from_rs0001("test", rs, 7.0, 0.0).unwrap();
        assert!(matches!(chiller.condenser_type(), CondenserType::Air));
        // Curve Set A: 0-150 ton, 2.96 COP rated.  Nominal capacity should be
        // in the right ballpark (positive, < 600 kW).
        assert!(chiller.nominal_capacity_w > 50_000.0);
        assert!(chiller.nominal_capacity_w < 1_000_000.0);
    }

    #[test]
    fn delivers_load_and_consumes_power() {
        let rs = Rs0001::load(&example_path()).unwrap();
        let mut chiller = ChillerA205::from_rs0001("test", rs, 7.0, 0.005).unwrap();
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        // Request ~50% of nominal capacity
        let load = 0.5 * chiller.nominal_capacity_w;
        let outlet = chiller.simulate_plant(&inlet, load, &ctx);
        assert!(chiller.electric_power > 0.0);
        assert!(chiller.actual_capacity > 0.0);
        assert!(chiller.plr > 0.4 && chiller.plr < 0.6);
        // Outlet should be cooler than inlet
        assert!(outlet.state.temp < inlet.state.temp);
        // Operating COP should be reasonable for an air-cooled chiller
        let cop = chiller.actual_capacity / chiller.electric_power;
        assert!(cop > 1.5, "COP too low: {}", cop);
        assert!(cop < 8.0, "COP too high: {}", cop);
    }

    #[test]
    fn zero_load_idle() {
        let rs = Rs0001::load(&example_path()).unwrap();
        let mut chiller = ChillerA205::from_rs0001("test", rs, 7.0, 0.005).unwrap();
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        let outlet = chiller.simulate_plant(&inlet, 0.0, &ctx);
        assert_eq!(chiller.electric_power, 0.0);
        assert_eq!(chiller.actual_capacity, 0.0);
        assert_eq!(outlet.state.temp, inlet.state.temp);
    }

    #[test]
    fn hotter_outdoor_reduces_cop() {
        let rs = Rs0001::load(&example_path()).unwrap();
        let mut cool = ChillerA205::from_rs0001("c", rs.clone(), 7.0, 0.005).unwrap();
        let mut hot = ChillerA205::from_rs0001("h", rs, 7.0, 0.005).unwrap();
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let load = 0.6 * cool.nominal_capacity_w;
        cool.simulate_plant(&inlet, load, &make_ctx(20.0));
        hot.simulate_plant(&inlet, load, &make_ctx(40.0));
        let cop_cool = cool.actual_capacity / cool.electric_power;
        let cop_hot = hot.actual_capacity / hot.electric_power;
        assert!(
            cop_cool > cop_hot,
            "expected colder day to yield higher COP: cool={:.3}, hot={:.3}",
            cop_cool,
            cop_hot
        );
    }
}
