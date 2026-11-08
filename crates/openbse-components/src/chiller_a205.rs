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
//! axis that indexes loading stages (1 = minimum, N = full).  At the current
//! temperatures, the chiller meets the load exactly:
//!
//! - Below the minimum stage's capacity it cycles that stage, with run-time
//!   fraction `c / (1 - Cd (1 - c))` for capacity ratio `c` and the file's
//!   `cycling_degradation_coefficient` `Cd`.
//! - CONTINUOUS: between the minimum and full stage, it runs at the sequence
//!   whose capacity equals the load (bisection; capacity rises with sequence).
//! - DISCRETE: it cycles between the two stages that bracket the load, with
//!   power weighted by the time at each, as E+ multi-speed equipment does.
//! - Above full capacity it runs the full stage and delivers that.
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
        let cp_water = inlet.state.cp;
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
        let query = |seq: f64| {
            self.interp.query(&CoolingQuery {
                evap_volumetric_flow: self.file_evap_flow,
                evap_leaving_temp_k: c_to_k(self.chw_setpoint),
                condenser_temp_k: c_to_k(t_cond_c),
                condenser_air_rh: self.default_rh,
                condenser_liquid_flow: self.file_cond_flow,
                ambient_pressure_pa: self.default_pressure,
                compressor_sequence: seq,
            })
        };
        let (seq_min, seq_max) = self.interp.sequence_range;
        let (r_min, r_max) = match (query(seq_min), query(seq_max)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                log::warn!("ChillerA205 '{}' map query failed: {}", self.name, e);
                self.actual_capacity = 0.0;
                self.electric_power = 0.0;
                return *inlet;
            }
        };
        let cap_min = r_min.net_evaporator_capacity.max(1.0);
        let cap_max = r_max.net_evaporator_capacity.max(cap_min);
        let cd = self.rs0001.performance.cycling_degradation_coefficient;

        let (delivered, power, sequence, in_range) = if load >= cap_max {
            (cap_max, r_max.input_power, seq_max, r_max.in_range)
        } else if load <= cap_min {
            // Cycle the minimum stage.
            let c = load / cap_min;
            let plf = (1.0 - cd * (1.0 - c)).max(0.01);
            let rtf = (c / plf).min(1.0);
            (load, r_min.input_power * rtf, seq_min, r_min.in_range)
        } else {
            match self.rs0001.performance.compressor_speed_control_type {
                SpeedControl::Continuous => {
                    // Capacity rises with sequence: bisect for the load.
                    let (mut lo, mut hi) = (seq_min, seq_max);
                    let mut r = r_max;
                    for _ in 0..40 {
                        let mid = 0.5 * (lo + hi);
                        r = match query(mid) {
                            Ok(r) => r,
                            Err(_) => break,
                        };
                        if r.net_evaporator_capacity < load {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    let seq = 0.5 * (lo + hi);
                    let r = query(seq).unwrap_or(r);
                    // Bisection leaves the capacity a hair off the load;
                    // scale that bit so the cooling delivered is the load.
                    let power = r.input_power * load / r.net_evaporator_capacity.max(1.0);
                    (load, power, seq, r.in_range)
                }
                SpeedControl::Discrete => {
                    // Cycle between the two stages that bracket the load.
                    let mut low = r_min;
                    let mut low_seq = seq_min;
                    let mut result = (cap_max, r_max.input_power, seq_max, r_max.in_range);
                    let mut stage = seq_min + 1.0;
                    while stage <= seq_max + 1e-9 {
                        let high = query(stage).unwrap_or(r_max);
                        if high.net_evaporator_capacity >= load {
                            let span = (high.net_evaporator_capacity - low.net_evaporator_capacity)
                                .max(1e-9);
                            let f = ((load - low.net_evaporator_capacity) / span).clamp(0.0, 1.0);
                            result = (
                                load,
                                f * high.input_power + (1.0 - f) * low.input_power,
                                low_seq + f * (stage - low_seq),
                                high.in_range && low.in_range,
                            );
                            break;
                        }
                        low = high;
                        low_seq = stage;
                        stage += 1.0;
                    }
                    result
                }
            }
        };

        self.actual_capacity = delivered;
        self.electric_power = power;
        self.plr = (load / cap_max).min(1.0);
        self.sequence_number = sequence;
        self.in_range = in_range;

        // Water outlet
        let mass_flow = inlet.state.mass_flow.max(0.001);
        let delta_t = delivered / (mass_flow * cp_water);
        let t_outlet = (inlet.state.temp - delta_t).max(self.chw_setpoint - 2.0);

        self.water_inlet_temp = inlet.state.temp;
        self.water_outlet_temp = t_outlet;
        self.water_mass_flow = mass_flow;

        WaterPort::new(FluidState::water(t_outlet, mass_flow))
    }

    fn set_supply_setpoint(&mut self, setpoint: f64) {
        self.chw_setpoint = setpoint;
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("plr", self.plr);
        out("compressor_sequence", self.sequence_number);
        out("in_range", if self.in_range { 1.0 } else { 0.0 });
        let cop = if self.electric_power > 0.0 {
            self.actual_capacity / self.electric_power
        } else {
            0.0
        };
        out("efficiency_operating", cop);
        out("water_inlet_temperature", self.water_inlet_temp);
        out("water_outlet_temperature", self.water_outlet_temp);
        out("water_mass_flow", self.water_mass_flow);
        // Everything the chiller takes in leaves through its condenser.
        out(
            "condenser_heat_rate",
            self.actual_capacity + self.electric_power,
        );
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

    /// Map power and capacity at a sequence, at 35 °C outdoor and 7 °C leaving.
    fn map_at(chiller: &ChillerA205, seq: f64) -> (f64, f64) {
        let r = chiller
            .interp
            .query(&CoolingQuery {
                evap_volumetric_flow: chiller.file_evap_flow,
                evap_leaving_temp_k: c_to_k(7.0),
                condenser_temp_k: c_to_k(35.0),
                condenser_air_rh: chiller.default_rh,
                condenser_liquid_flow: chiller.file_cond_flow,
                ambient_pressure_pa: chiller.default_pressure,
                compressor_sequence: seq,
            })
            .unwrap();
        (r.net_evaporator_capacity, r.input_power)
    }

    /// The chiller delivers exactly the load, with the map's power: below the
    /// first stage it cycles that stage, so a small load costs the first
    /// stage's power in proportion (Cd = 0 in this file), not all of it.
    #[test]
    fn part_load_follows_the_map() {
        let rs = Rs0001::load(&example_path()).unwrap();
        let mut chiller = ChillerA205::from_rs0001("test", rs, 7.0, 0.0).unwrap();
        let ctx = make_ctx(35.0);
        let inlet = WaterPort::new(FluidState::water(12.0, 10.0));
        let (seq_min, seq_max) = chiller.interp.sequence_range;
        let (cap1, p1) = map_at(&chiller, seq_min);
        let (cap_n, p_n) = map_at(&chiller, seq_max);

        // A tenth of the first stage: a tenth of its power.
        chiller.simulate_plant(&inlet, 0.1 * cap1, &ctx);
        assert!((chiller.actual_capacity - 0.1 * cap1).abs() < 1e-6);
        assert!((chiller.electric_power - 0.1 * p1).abs() < 1e-6 * p1);

        // Between stages: the load exactly, at a power between the stages'.
        let load = 0.5 * (cap1 + cap_n);
        chiller.simulate_plant(&inlet, load, &ctx);
        assert!((chiller.actual_capacity - load).abs() < 1e-6);
        assert!(chiller.electric_power > p1 && chiller.electric_power < p_n);
        let (cap_s, p_s) = map_at(&chiller, chiller.sequence_number);
        assert!((cap_s - load).abs() < 1e-3 * load, "{cap_s} vs {load}");
        assert!((chiller.electric_power - p_s).abs() < 1e-3 * p_s);

        // Beyond full capacity: the full stage.
        chiller.simulate_plant(&inlet, 2.0 * cap_n, &ctx);
        assert!((chiller.actual_capacity - cap_n).abs() < 1e-6);
        assert!((chiller.electric_power - p_n).abs() < 1e-6);
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
