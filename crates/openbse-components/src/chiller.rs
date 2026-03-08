//! Electric chiller component model.
//!
//! Models an electric chiller using the E+ Chiller:Electric:EIR approach:
//!   Power = AvailCoolCap × (1/RefCOP) × EIRFT × EIRFPLR × CyclingRatio
//!
//! Supports both air-cooled and water-cooled chillers. When performance curves
//! (CAPFT, EIRFT, EIRFPLR) are provided, they are used directly. Otherwise,
//! simplified linear fallbacks are used for air-cooled units.
//!
//! For water-cooled chillers without a condenser water loop, the condenser
//! entering water temperature is estimated from outdoor wet-bulb + tower_approach.
//!
//! Reference: EnergyPlus Engineering Reference, "Chiller:Electric:EIR"

use crate::performance_curve::PerformanceCurve;
use openbse_core::ports::*;
use openbse_psychrometrics::FluidState;
use serde::{Deserialize, Serialize};

/// Electric chiller (air-cooled or water-cooled).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirCooledChiller {
    pub name: String,
    pub rated_capacity: f64,
    pub rated_cop: f64,
    pub chw_setpoint: f64,
    pub design_chw_flow: f64,
    #[serde(default = "default_min_plr")]
    pub min_plr: f64,

    /// Whether this is a water-cooled chiller. If true, condenser temperature
    /// is determined by condenser_entering_temp (if set) or wet-bulb + tower_approach.
    #[serde(default)]
    pub water_cooled: bool,
    /// For water-cooled: fixed condenser entering water temperature [°C].
    /// If set, this overrides the wet-bulb + tower_approach calculation.
    /// Matches E+ SetpointManager:Scheduled on the condenser loop.
    #[serde(default)]
    pub condenser_entering_temp: Option<f64>,
    /// For water-cooled (fallback): offset from outdoor wet-bulb to condenser
    /// entering water temperature [°C]. Default 5.56°C (10°F).
    /// Only used when condenser_entering_temp is None.
    #[serde(default = "default_tower_approach")]
    pub tower_approach: f64,

    /// CAPFT: Capacity modifier as f(T_chw_leaving, T_condenser_entering).
    /// Biquadratic: c1 + c2*x + c3*x² + c4*y + c5*y² + c6*x*y
    #[serde(skip)]
    pub capft_curve: Option<PerformanceCurve>,
    /// EIRFT: EIR modifier as f(T_chw_leaving, T_condenser_entering).
    /// Biquadratic: c1 + c2*x + c3*x² + c4*y + c5*y² + c6*x*y
    #[serde(skip)]
    pub eirft_curve: Option<PerformanceCurve>,
    /// EIRFPLR: EIR modifier as f(PLR).
    /// Quadratic: c1 + c2*x + c3*x²
    #[serde(skip)]
    pub eirfplr_curve: Option<PerformanceCurve>,

    #[serde(skip)]
    pub actual_capacity: f64,
    #[serde(skip)]
    pub actual_cop: f64,
    #[serde(skip)]
    pub electric_power: f64,
    #[serde(skip)]
    pub plr: f64,
}

fn default_min_plr() -> f64 { 0.25 }
fn default_tower_approach() -> f64 { 5.56 }

impl AirCooledChiller {
    pub fn new(name: &str, rated_capacity: f64, rated_cop: f64, chw_setpoint: f64, design_chw_flow: f64) -> Self {
        Self {
            name: name.to_string(), rated_capacity, rated_cop, chw_setpoint,
            design_chw_flow, min_plr: 0.25,
            water_cooled: false,
            condenser_entering_temp: None,
            tower_approach: 5.56,
            capft_curve: None,
            eirft_curve: None,
            eirfplr_curve: None,
            actual_capacity: 0.0, actual_cop: 0.0, electric_power: 0.0, plr: 0.0,
        }
    }

    /// Get the condenser-side temperature for curve evaluation.
    ///
    /// - Air-cooled: outdoor dry-bulb temperature
    /// - Water-cooled with setpoint + approach: max(setpoint, T_wb + approach).
    ///   The cooling tower tries to maintain the setpoint, but can't cool below
    ///   T_wb + approach due to physics.
    /// - Water-cooled with setpoint only: fixed condenser_entering_temp
    /// - Water-cooled with approach only: T_wb + approach
    fn condenser_temp(&self, ctx: &SimulationContext) -> f64 {
        if self.water_cooled {
            let t_wb = ctx.outdoor_air.t_wb();
            match (self.condenser_entering_temp, self.tower_approach) {
                (Some(setpoint), approach) if approach > 0.0 => {
                    // Tower maintains setpoint when possible, but can't cool
                    // below T_wb + approach (physical limit)
                    setpoint.max(t_wb + approach)
                }
                (Some(setpoint), _) => setpoint,
                (None, approach) => t_wb + approach,
            }
        } else {
            ctx.outdoor_air.t_db
        }
    }

    /// CAPFT: capacity modifier as function of temperatures.
    ///
    /// If a capft_curve is set, evaluates the biquadratic with:
    ///   x = leaving CHW temp (chw_setpoint), y = condenser entering temp.
    /// Otherwise uses simplified linear correction for air-cooled.
    fn capacity_modifier(&self, t_condenser: f64) -> f64 {
        if let Some(ref curve) = self.capft_curve {
            curve.evaluate(self.chw_setpoint, t_condenser)
        } else {
            // Simplified linear fallback (air-cooled only)
            (1.0 - 0.015 * (t_condenser - 29.4)).clamp(0.5, 1.1)
        }
    }

    /// EIRFT: EIR modifier as function of temperatures.
    ///
    /// If an eirft_curve is set, evaluates the biquadratic with:
    ///   x = leaving CHW temp, y = condenser entering temp.
    /// Otherwise uses simplified linear correction for air-cooled.
    fn eir_modifier_temp(&self, t_condenser: f64) -> f64 {
        if let Some(ref curve) = self.eirft_curve {
            curve.evaluate(self.chw_setpoint, t_condenser)
        } else {
            // Simplified linear fallback: higher temp → worse EIR
            // At reference (29.4°C), EIRFT = 1.0
            let cop_factor = (1.0 - 0.02 * (t_condenser - 29.4)).clamp(0.4, 1.1);
            if cop_factor > 0.0 { 1.0 / cop_factor } else { 1.0 }
        }
    }

    /// EIRFPLR: EIR modifier as function of part load ratio.
    ///
    /// If an eirfplr_curve is set, evaluates the quadratic curve.
    /// Otherwise uses a simplified linear curve (EIR increases with PLR).
    fn eir_modifier_plr(&self, plr: f64) -> f64 {
        let p = plr.clamp(self.min_plr, 1.0);
        if let Some(ref curve) = self.eirfplr_curve {
            curve.evaluate_1d(p).max(0.01)
        } else {
            // Simplified quadratic fallback (constant EIR at all PLR)
            // This is intentionally flat so that without curves, the chiller
            // uses rated COP at all load levels (no part-load advantage).
            1.0
        }
    }
}

impl PlantComponent for AirCooledChiller {
    fn name(&self) -> &str { &self.name }
    fn simulate_plant(&mut self, inlet: &WaterPort, load: f64, ctx: &SimulationContext) -> WaterPort {
        let cp_water = 4186.0;
        if load <= 0.0 {
            self.actual_capacity = 0.0; self.actual_cop = 0.0; self.electric_power = 0.0; self.plr = 0.0;
            return *inlet;
        }

        let t_condenser = self.condenser_temp(ctx);

        // CAPFT: capacity at current temperatures
        let capft = self.capacity_modifier(t_condenser);
        let available_cap = self.rated_capacity * capft;
        let raw_plr = (load / available_cap).min(1.0);

        // E+-style ON/OFF cycling below min_plr:
        // When raw PLR < min_plr, the chiller cycles ON at min_plr and OFF,
        // with cycling_ratio = raw_plr / min_plr. Time-averaged power and
        // capacity are multiplied by cycling_ratio.
        let (operating_plr, cycling_ratio) = if raw_plr < self.min_plr {
            (self.min_plr, raw_plr / self.min_plr)
        } else {
            (raw_plr, 1.0)
        };

        self.plr = raw_plr;

        // E+ formulation:
        //   Power = AvailCoolCap × (1/RefCOP) × EIRFT × EIRFPLR × CyclingRatio
        let eirft = self.eir_modifier_temp(t_condenser);
        let eirfplr = self.eir_modifier_plr(operating_plr);

        let ref_eir = if self.rated_cop > 0.0 { 1.0 / self.rated_cop } else { 1.0 };
        let instantaneous_power = available_cap * ref_eir * eirft * eirfplr;
        self.electric_power = instantaneous_power * cycling_ratio;

        // Capacity delivered
        let instantaneous_cap = operating_plr * available_cap;
        self.actual_capacity = instantaneous_cap * cycling_ratio;

        // Effective COP for reporting
        self.actual_cop = if self.electric_power > 0.0 {
            self.actual_capacity / self.electric_power
        } else {
            self.rated_cop
        };

        let mass_flow = inlet.state.mass_flow.max(0.001);
        let delta_t = self.actual_capacity / (mass_flow * cp_water);
        let t_outlet = (inlet.state.temp - delta_t).max(self.chw_setpoint - 2.0);
        WaterPort::new(FluidState::water(t_outlet, mass_flow))
    }
    fn design_water_flow_rate(&self) -> Option<f64> {
        if self.design_chw_flow <= 0.0 { None } else { Some(self.design_chw_flow) }
    }
    fn power_consumption(&self) -> f64 { self.electric_power }
    fn thermal_output(&self) -> f64 { self.actual_capacity }
    fn nominal_capacity(&self) -> Option<f64> { Some(self.rated_capacity) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;
    use crate::performance_curve::{PerformanceCurve, CurveType};

    fn make_ctx(t_outdoor: f64) -> SimulationContext {
        SimulationContext {
            timestep: TimeStep { month: 7, day: 15, hour: 14, sub_hour: 1, timesteps_per_hour: 1, sim_time_s: 0.0, dt: 3600.0 },
            outdoor_air: MoistAirState::from_tdb_rh(t_outdoor, 0.4, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    /// Create an E+ IDF-style water-cooled chiller with actual ASHRAE 90.1-2022 curves.
    fn make_water_cooled_chiller(capacity: f64, cop: f64) -> AirCooledChiller {
        let mut chiller = AirCooledChiller::new("WC Chiller", capacity, cop, 6.67, 0.005);
        chiller.water_cooled = true;
        chiller.tower_approach = 5.56;
        chiller.min_plr = 0.25;
        // CAPFT from E+ IDF: ASHRAE901_AppJ_wtr_AB_cent_gt1055kW cap-f-t
        chiller.capft_curve = Some(PerformanceCurve {
            name: "capft".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![
                0.988289399105446, 0.031127718217927, -0.00154986412921254,
                -0.00334864255828258, -0.000146694443912943, 0.000502974958539996,
            ],
            min_x: 4.0, max_x: 16.0, min_y: 12.8, max_y: 40.0,
            min_output: None, max_output: None,
        });
        // EIRFT from E+ IDF: ASHRAE901_AppJ_wtr_AB_cent_gt1055kW eir-f-t
        chiller.eirft_curve = Some(PerformanceCurve {
            name: "eirft".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![
                0.563967192309392, -0.034330681975216, 0.00101506219660673,
                0.0339405925147554, -0.000431970984586658, -0.0000252410848639146,
            ],
            min_x: 4.0, max_x: 16.0, min_y: 12.8, max_y: 40.0,
            min_output: None, max_output: None,
        });
        // EIRFPLR from E+ IDF: ASHRAE901_AppJ_wtr_AB_cent_gt1055kW eir-f-plr
        chiller.eirfplr_curve = Some(PerformanceCurve {
            name: "eirfplr".to_string(),
            curve_type: CurveType::Quadratic,
            coefficients: vec![0.309752375539755, 0.153649268551135, 0.536462254009109],
            min_x: 0.0, max_x: 1.0, min_y: 0.0, max_y: 0.0,
            min_output: None, max_output: None,
        });
        chiller
    }

    #[test]
    fn test_chiller_rated_conditions() {
        let mut chiller = AirCooledChiller::new("Test Chiller", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        let _outlet = chiller.simulate_plant(&inlet, 100_000.0, &ctx);
        assert!(chiller.actual_cop > 2.0, "COP at rated: {}", chiller.actual_cop);
        assert!(chiller.electric_power > 0.0);
        assert_relative_eq!(chiller.plr, 1.0, epsilon = 0.01);
    }

    #[test]
    fn test_chiller_zero_load() {
        let mut chiller = AirCooledChiller::new("Test Chiller", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(35.0);
        let _outlet = chiller.simulate_plant(&inlet, 0.0, &ctx);
        assert_eq!(chiller.electric_power, 0.0);
        assert_eq!(chiller.plr, 0.0);
    }

    #[test]
    fn test_chiller_hot_outdoor_reduces_cop() {
        let mut chiller_cool = AirCooledChiller::new("C1", 100_000.0, 3.0, 7.0, 0.005);
        let mut chiller_hot = AirCooledChiller::new("C2", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        chiller_cool.simulate_plant(&inlet, 80_000.0, &make_ctx(25.0));
        chiller_hot.simulate_plant(&inlet, 80_000.0, &make_ctx(40.0));
        assert!(chiller_cool.actual_cop > chiller_hot.actual_cop,
            "COP at 25C ({:.2}) should be > COP at 40C ({:.2})", chiller_cool.actual_cop, chiller_hot.actual_cop);
    }

    #[test]
    fn test_chiller_part_load() {
        let mut chiller = AirCooledChiller::new("Test Chiller", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        let _out = chiller.simulate_plant(&inlet, 50_000.0, &ctx);
        assert!((chiller.plr - 0.5).abs() < 0.05, "PLR should be ~0.5, got {}", chiller.plr);
        assert!(chiller.electric_power > 0.0);
    }

    #[test]
    fn test_chiller_cycling_below_min_plr() {
        let mut chiller = AirCooledChiller::new("Cycling", 100_000.0, 6.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        // Load = 10% of capacity, below min_plr (25%)
        let _out = chiller.simulate_plant(&inlet, 10_000.0, &ctx);
        // Cycling ratio = 0.1/0.25 = 0.4
        assert!(chiller.plr < chiller.min_plr, "PLR ({}) should be < min_plr ({})", chiller.plr, chiller.min_plr);
        // Actual capacity should be close to load (time-averaged)
        assert!((chiller.actual_capacity - 10_000.0).abs() < 1000.0,
            "Actual cap ({:.0}) should be close to load (10000)", chiller.actual_capacity);
    }

    #[test]
    fn test_water_cooled_chiller_with_curves() {
        // Water-cooled chiller with actual E+ IDF curves
        let mut chiller = make_water_cooled_chiller(1_570_096.0, 6.11);
        let inlet = WaterPort::new(FluidState::water(12.0, 50.0));
        // At reference condenser temp: ODB ≈ 42°C → WB ≈ 24°C → cond = 29.56°C ≈ ref
        let ctx = make_ctx(30.0);
        let _out = chiller.simulate_plant(&inlet, 1_000_000.0, &ctx);
        assert!(chiller.actual_cop > 4.0, "Water-cooled COP should be > 4, got {:.2}", chiller.actual_cop);
        assert!(chiller.electric_power > 0.0);
    }

    #[test]
    fn test_water_cooled_chiller_curves_at_reference() {
        // Verify CAPFT ≈ 1.0 and EIRFT ≈ 1.0 at reference conditions
        let chiller = make_water_cooled_chiller(1_000_000.0, 6.11);
        // Reference: CHW leaving = 6.67°C, condenser entering = 29.44°C
        let capft = chiller.capacity_modifier(29.44);
        let eirft = chiller.eir_modifier_temp(29.44);
        assert!((capft - 1.0).abs() < 0.05, "CAPFT at reference should be ~1.0, got {:.4}", capft);
        assert!((eirft - 1.0).abs() < 0.05, "EIRFT at reference should be ~1.0, got {:.4}", eirft);
    }

    #[test]
    fn test_eirfplr_curve_at_full_load() {
        let chiller = make_water_cooled_chiller(1_000_000.0, 6.11);
        let eirfplr = chiller.eir_modifier_plr(1.0);
        // At PLR=1.0: 0.3098 + 0.1536 + 0.5365 ≈ 1.0
        assert!((eirfplr - 1.0).abs() < 0.01, "EIRFPLR at PLR=1.0 should be ~1.0, got {:.4}", eirfplr);
    }
}
