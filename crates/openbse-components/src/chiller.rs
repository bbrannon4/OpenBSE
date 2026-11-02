//! Electric chiller component model.
//!
//! Models an electric chiller using the E+ Chiller:Electric:EIR approach:
//!   Power = AvailCoolCap × (1/RefCOP) × EIRFT × EIRFPLR × CyclingRatio
//!
//! Supports both air-cooled and water-cooled chillers. When performance curves
//! (CAPFT, EIRFT, EIRFPLR) are provided, they are used directly. Otherwise,
//! simplified linear fallbacks are used for CAPFT and EIRFT (air-cooled), and
//! EIRFPLR = PLR (constant COP across part load).
//!
//! For water-cooled chillers without a condenser water loop, the condenser
//! entering water temperature is estimated from outdoor wet-bulb + tower_approach.
//!
//! `ChillerModel::ReformulatedEir` follows E+ Chiller:Electric:ReformulatedEIR
//! instead: CAPFT and EIRFT take the *leaving* condenser water temperature, and
//! EIRFPLR is f(leaving condenser temperature, PLR). That temperature depends
//! on the chiller's own power through the condenser heat balance, so it is
//! solved by fixed-point iteration (E+ uses regula falsi on the same residual).
//!
//! An EIR chiller can also recover condenser heat to another plant loop
//! (`ChillerHeatRecovery`), following E+'s leaving-setpoint heat recovery: the
//! recovered share heats the heat-recovery water up to its setpoint and the
//! rest goes to the condenser. The curves then see a heat-weighted blend of
//! the two sinks' entering temperatures.
//!
//! Reference: EnergyPlus Engineering Reference, "Chiller:Electric:EIR" and
//! "Chiller:Electric:ReformulatedEIR"

use crate::performance_curve::PerformanceCurve;
use openbse_core::ports::*;
use openbse_core::types::is_autosize;
use openbse_psychrometrics::{
    water_cp, water_rho, FluidState, COLD_WATER_CONVERSION_TEMP, HOT_WATER_CONVERSION_TEMP,
};
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

/// Which E+ chiller formulation the curves follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChillerModel {
    /// Chiller:Electric:EIR — curves of the entering condenser temperature.
    #[default]
    Eir,
    /// Chiller:Electric:ReformulatedEIR — curves of the leaving condenser
    /// water temperature; EIRFPLR is f(T_cond_leaving, PLR). Water-cooled only.
    ReformulatedEir,
}

/// Condenser heat recovery on an EIR chiller (E+ Chiller:Electric:EIR with a
/// heat recovery leaving temperature setpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChillerHeatRecovery {
    /// Heat-recovery water leaving temperature setpoint [°C].
    pub leaving_setpoint: f64,
    /// Heat-recovery water flow [m³/s]. 0 sizes it as E+ does: the design
    /// condenser flow × `capacity_fraction`.
    pub design_flow: f64,
    /// Share of full-load condenser heat that can be recovered (E+
    /// "Condenser Heat Recovery Relative Capacity Fraction").
    pub capacity_fraction: f64,
    /// Heat recovery stops while the entering heat-recovery water is above
    /// this temperature [°C].
    pub inlet_high_limit: Option<f64>,
}

/// Electric chiller (air-cooled or water-cooled).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirCooledChiller {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    pub rated_capacity: f64,
    pub rated_cop: f64,
    pub chw_setpoint: f64,
    pub design_chw_flow: f64,
    #[serde(default = "default_min_plr")]
    pub min_plr: f64,
    /// E+ Minimum Unloading Ratio. Between `min_plr` and this ratio the
    /// chiller false-loads (hot-gas bypass): it runs at this ratio and
    /// rejects the excess capacity to the condenser. `None` = `min_plr`.
    #[serde(default)]
    pub min_unloading_ratio: Option<f64>,

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

    /// Curve formulation (E+ Chiller:Electric:EIR or ReformulatedEIR).
    #[serde(default)]
    pub model: ChillerModel,
    /// Reformulated EIR: design condenser water flow [m³/s], held constant
    /// while the chiller runs (E+ ConstantFlow). 0 sizes it as E+ does:
    /// capacity × (1 + 1/COP) / (ρ·cp·condenser_design_delta_t).
    #[serde(default)]
    pub design_condenser_flow: f64,
    /// Condenser loop design temperature difference [K] used to size
    /// `design_condenser_flow`.
    #[serde(default = "default_condenser_design_delta_t")]
    pub condenser_design_delta_t: f64,
    /// Condenser heat recovery (EIR model only).
    #[serde(default)]
    pub heat_recovery: Option<ChillerHeatRecovery>,

    /// CAPFT: Capacity modifier as f(T_chw_leaving, T_condenser), with the
    /// entering (EIR) or leaving (reformulated EIR) condenser temperature.
    /// Biquadratic: c1 + c2*x + c3*x² + c4*y + c5*y² + c6*x*y
    #[serde(skip)]
    pub capft_curve: Option<PerformanceCurve>,
    /// EIRFT: EIR modifier as f(T_chw_leaving, T_condenser), with the
    /// entering (EIR) or leaving (reformulated EIR) condenser temperature.
    /// Biquadratic: c1 + c2*x + c3*x² + c4*y + c5*y² + c6*x*y
    #[serde(skip)]
    pub eirft_curve: Option<PerformanceCurve>,
    /// EIRFPLR: EIR modifier as f(PLR) (quadratic), or as
    /// f(T_condenser_leaving, PLR) (bicubic) on the reformulated EIR model.
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
    #[serde(skip)]
    pub water_inlet_temp: f64,
    #[serde(skip)]
    pub water_outlet_temp: f64,
    #[serde(skip)]
    pub water_mass_flow: f64,
    #[serde(skip)]
    pub condenser_inlet_temp: f64,
    #[serde(skip)]
    pub condenser_outlet_temp: f64,
    /// Heat-recovery water entering temperature [°C], set by the plant
    /// solver from the receiving loop.
    #[serde(skip)]
    pub heat_recovery_inlet_temp: f64,
    /// Most heat [W] the receiving loop can take this timestep.
    #[serde(skip)]
    pub heat_recovery_max: f64,
    /// Heat recovered this timestep [W].
    #[serde(skip)]
    pub heat_recovered: f64,
    #[serde(skip)]
    pub heat_recovery_outlet_temp: f64,
}

fn default_min_plr() -> f64 {
    0.25
}
fn default_tower_approach() -> f64 {
    5.56
}
fn default_condenser_design_delta_t() -> f64 {
    5.56
}

/// Time-averaged performance at one curve temperature.
struct Operation {
    raw_plr: f64,
    /// Electric power [W].
    power: f64,
    /// Cooling delivered [W].
    capacity: f64,
    /// Capacity run beyond the load below the min unloading ratio [W]; it
    /// is rejected with the condenser heat.
    false_load: f64,
}

impl Operation {
    /// Heat leaving through the condenser and heat recovery [W], with all
    /// compressor heat rejected there (E+ default).
    fn heat_rejected(&self) -> f64 {
        self.power + self.capacity + self.false_load
    }
}

impl AirCooledChiller {
    pub fn new(
        name: &str,
        rated_capacity: f64,
        rated_cop: f64,
        chw_setpoint: f64,
        design_chw_flow: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            rated_capacity,
            rated_cop,
            chw_setpoint,
            design_chw_flow,
            min_plr: 0.25,
            min_unloading_ratio: None,
            water_cooled: false,
            condenser_entering_temp: None,
            tower_approach: 5.56,
            model: ChillerModel::Eir,
            design_condenser_flow: 0.0,
            condenser_design_delta_t: default_condenser_design_delta_t(),
            heat_recovery: None,
            capft_curve: None,
            eirft_curve: None,
            eirfplr_curve: None,
            actual_capacity: 0.0,
            actual_cop: 0.0,
            electric_power: 0.0,
            plr: 0.0,
            water_inlet_temp: 0.0,
            water_outlet_temp: 0.0,
            water_mass_flow: 0.0,
            condenser_inlet_temp: 0.0,
            condenser_outlet_temp: 0.0,
            heat_recovery_inlet_temp: 0.0,
            heat_recovery_max: f64::INFINITY,
            heat_recovered: 0.0,
            heat_recovery_outlet_temp: 0.0,
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
            if cop_factor > 0.0 {
                1.0 / cop_factor
            } else {
                1.0
            }
        }
    }

    /// EIRFPLR: EIR modifier as function of part load ratio.
    ///
    /// If an eirfplr_curve is set, evaluates it: at PLR for the EIR model, and
    /// at (leaving condenser temperature `t_condenser`, PLR) for a two-variable
    /// curve on the reformulated model. Otherwise EIRFPLR = PLR.
    ///
    /// E+ Chiller:Electric:EIR requires this curve and has no default. The
    /// fallback keeps the chiller at its rated COP (adjusted only by EIRFT) at
    /// every part load, with no part-load advantage or penalty: power scales
    /// with PLR because EIRFPLR multiplies the *available* capacity, not the
    /// load. 99 of the 111 air-cooled chillers in E+'s AirCooledChiller.idf
    /// dataset use exactly this curve. Below min_plr the chiller cycles at
    /// min_plr, so its time-averaged COP stays at rated there too.
    fn eir_modifier_plr(&self, plr: f64, t_condenser: f64) -> f64 {
        let p = plr.clamp(self.min_plr, 1.0);
        match &self.eirfplr_curve {
            Some(curve)
                if self.model == ChillerModel::ReformulatedEir && curve.dimensions() == 2 =>
            {
                curve.evaluate(t_condenser, p).max(0.01)
            }
            Some(curve) => curve.evaluate_1d(p).max(0.01),
            None => p,
        }
    }

    /// Condenser water mass flow [kg/s] for the reformulated model.
    fn condenser_mass_flow(&self) -> f64 {
        if self.design_condenser_flow > 0.0 {
            self.design_condenser_flow * water_rho(COLD_WATER_CONVERSION_TEMP)
        } else if self.rated_cop > 0.0 && self.condenser_design_delta_t > 0.0 {
            self.rated_capacity * (1.0 + 1.0 / self.rated_cop)
                / (water_cp(COLD_WATER_CONVERSION_TEMP) * self.condenser_design_delta_t)
        } else {
            0.0
        }
    }

    /// Performance with the curves evaluated at condenser temperature
    /// `t_curve`: entering (EIR) or leaving (reformulated EIR).
    ///
    /// E+ formulation:
    ///   Power = AvailCoolCap × (1/RefCOP) × EIRFT × EIRFPLR × CyclingRatio
    fn operate(&self, load: f64, t_curve: f64) -> Operation {
        // CAPFT: capacity at current temperatures
        let available_cap = self.rated_capacity * self.capacity_modifier(t_curve);
        let raw_plr = (load / available_cap).min(1.0);

        // E+-style ON/OFF cycling below min_plr:
        // When raw PLR < min_plr, the chiller cycles ON at min_plr and OFF,
        // with cycling_ratio = raw_plr / min_plr. Time-averaged power and
        // capacity are multiplied by cycling_ratio.
        let cycling_ratio = if raw_plr < self.min_plr {
            raw_plr / self.min_plr
        } else {
            1.0
        };
        // Below the min unloading ratio the compressor runs at that ratio
        // and false-loads (E+ ChillerFalseLoadRate).
        let operating_plr = raw_plr.max(self.min_unloading_ratio());

        let eirft = self.eir_modifier_temp(t_curve);
        let eirfplr = self.eir_modifier_plr(operating_plr, t_curve);
        let ref_eir = if self.rated_cop > 0.0 {
            1.0 / self.rated_cop
        } else {
            1.0
        };
        let capacity = raw_plr * available_cap;
        Operation {
            raw_plr,
            power: available_cap * ref_eir * eirft * eirfplr * cycling_ratio,
            capacity,
            false_load: (operating_plr * available_cap * cycling_ratio - capacity).max(0.0),
        }
    }

    fn min_unloading_ratio(&self) -> f64 {
        self.min_unloading_ratio
            .unwrap_or(self.min_plr)
            .max(self.min_plr)
    }

    /// Heat-recovery water mass flow [kg/s].
    fn heat_recovery_mass_flow(&self, hr: &ChillerHeatRecovery) -> f64 {
        if hr.design_flow > 0.0 {
            hr.design_flow * water_rho(HOT_WATER_CONVERSION_TEMP)
        } else {
            self.condenser_mass_flow() * hr.capacity_fraction
        }
    }

    /// EIR chiller with heat recovery: E+'s leaving-setpoint algorithm.
    ///
    /// Of the heat rejected (power + cooling + false load), the heat-recovery
    /// water takes up to what heats it to its setpoint, capped at
    /// capacity_fraction × full-load rejection and at what the receiving
    /// loop can use this timestep; the condenser takes the rest. CAPFT and
    /// EIRFT see the heat-weighted blend of the two entering temperatures.
    ///
    /// E+ blends with the split from its previous iteration. Here the blend
    /// and the split are solved together, which E+'s iterations converge to.
    /// Returns the operation, the heat recovered [W] and the blended
    /// temperature.
    fn operate_with_heat_recovery(
        &self,
        load: f64,
        t_cond_in: f64,
        hr: &ChillerHeatRecovery,
    ) -> (Operation, f64) {
        let t_hr_in = self.heat_recovery_inlet_temp;
        let to_setpoint = (self.heat_recovery_mass_flow(hr)
            * water_cp(t_hr_in)
            * (hr.leaving_setpoint - t_hr_in))
            .max(0.0);
        let full_load_rejection = if self.rated_cop > 0.0 {
            self.rated_capacity * (1.0 + 1.0 / self.rated_cop)
        } else {
            self.rated_capacity
        };
        let blocked = hr.inlet_high_limit.is_some_and(|limit| t_hr_in > limit);
        let recoverable = if blocked {
            0.0
        } else {
            to_setpoint
                .min(hr.capacity_fraction * full_load_rejection)
                .min(self.heat_recovery_max.max(0.0))
        };

        let mut t_curve = if recoverable > 0.0 {
            t_hr_in
        } else {
            t_cond_in
        };
        let mut op = self.operate(load, t_curve);
        for _ in 0..50 {
            let rejected = op.heat_rejected();
            if rejected <= 0.0 {
                break;
            }
            let recovered = rejected.min(recoverable);
            let t_new = (recovered * t_hr_in + (rejected - recovered) * t_cond_in) / rejected;
            let converged = (t_new - t_curve).abs() < 1e-9;
            t_curve = t_new;
            op = self.operate(load, t_curve);
            if converged {
                break;
            }
        }
        let recovered = op.heat_rejected().min(recoverable);
        (op, recovered)
    }

    /// Reformulated EIR: solve the leaving condenser temperature from
    ///   T_out = T_in + (Power + Q_evap + false load)(T_out) / (m_cond·cp)
    /// with all compressor heat rejected to the condenser (E+ default).
    /// The residual's slope is small (a few kW/K of power against a
    /// condenser m·cp of order 100 kW/K), so fixed-point iteration converges
    /// in a few steps.
    fn operate_reformulated(&self, load: f64, t_cond_in: f64) -> (Operation, f64) {
        let m_cp = self.condenser_mass_flow() * water_cp(t_cond_in);
        let mut t_out = t_cond_in;
        let mut op = self.operate(load, t_out);
        if m_cp <= 0.0 {
            return (op, t_out);
        }
        for _ in 0..50 {
            let t_new = t_cond_in + op.heat_rejected() / m_cp;
            let converged = (t_new - t_out).abs() < 1e-6;
            t_out = t_new;
            op = self.operate(load, t_out);
            if converged {
                break;
            }
        }
        (op, t_out)
    }
}

impl PlantComponent for AirCooledChiller {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Chiller
    }

    fn rated_capacity(&self) -> f64 {
        self.rated_capacity
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        ctx: &SimulationContext,
    ) -> WaterPort {
        let cp_water = inlet.state.cp;
        let t_cond_in = self.condenser_temp(ctx);
        self.condenser_inlet_temp = t_cond_in;
        if load <= 0.0 {
            self.actual_capacity = 0.0;
            self.actual_cop = 0.0;
            self.electric_power = 0.0;
            self.plr = 0.0;
            self.water_inlet_temp = inlet.state.temp;
            self.water_outlet_temp = inlet.state.temp;
            self.water_mass_flow = inlet.state.mass_flow;
            self.condenser_outlet_temp = t_cond_in;
            self.heat_recovered = 0.0;
            self.heat_recovery_outlet_temp = self.heat_recovery_inlet_temp;
            return *inlet;
        }

        let (op, t_cond_out, recovered) = match (self.model, self.heat_recovery.as_ref()) {
            (ChillerModel::Eir, Some(hr)) => {
                let (op, recovered) = self.operate_with_heat_recovery(load, t_cond_in, hr);
                (op, t_cond_in, recovered)
            }
            (ChillerModel::Eir, None) => (self.operate(load, t_cond_in), t_cond_in, 0.0),
            (ChillerModel::ReformulatedEir, _) => {
                let (op, t_out) = self.operate_reformulated(load, t_cond_in);
                (op, t_out, 0.0)
            }
        };
        self.condenser_outlet_temp = t_cond_out;
        self.heat_recovered = recovered;
        self.heat_recovery_outlet_temp = match self.heat_recovery.as_ref() {
            Some(hr) if recovered > 0.0 => {
                self.heat_recovery_inlet_temp
                    + recovered
                        / (self.heat_recovery_mass_flow(hr)
                            * water_cp(self.heat_recovery_inlet_temp))
            }
            _ => self.heat_recovery_inlet_temp,
        };
        self.plr = op.raw_plr;
        self.electric_power = op.power;
        self.actual_capacity = op.capacity;

        // Effective COP for reporting
        self.actual_cop = if self.electric_power > 0.0 {
            self.actual_capacity / self.electric_power
        } else {
            self.rated_cop
        };

        let mass_flow = inlet.state.mass_flow.max(0.001);
        let delta_t = self.actual_capacity / (mass_flow * cp_water);
        let t_outlet = (inlet.state.temp - delta_t).max(self.chw_setpoint - 2.0);

        // Store water conditions for detailed outputs
        self.water_inlet_temp = inlet.state.temp;
        self.water_outlet_temp = t_outlet;
        self.water_mass_flow = mass_flow;

        WaterPort::new(FluidState::water(t_outlet, mass_flow))
    }
    fn design_water_flow_rate(&self) -> Option<f64> {
        if self.design_chw_flow <= 0.0 {
            None
        } else {
            Some(self.design_chw_flow)
        }
    }
    fn power_consumption(&self) -> f64 {
        self.electric_power
    }
    fn thermal_output(&self) -> f64 {
        self.actual_capacity
    }
    fn nominal_capacity(&self) -> Option<f64> {
        if is_autosize(self.rated_capacity) {
            None
        } else {
            Some(self.rated_capacity)
        }
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.rated_capacity = cap;
    }

    fn unsized_fields(&self) -> Vec<&'static str> {
        openbse_core::types::autosize_fields([("capacity", self.rated_capacity)])
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("plr", self.plr);
        out("efficiency_operating", self.actual_cop);
        out("water_inlet_temperature", self.water_inlet_temp);
        out("water_outlet_temperature", self.water_outlet_temp);
        out("water_mass_flow", self.water_mass_flow);
        if self.model == ChillerModel::ReformulatedEir {
            out("condenser_inlet_temperature", self.condenser_inlet_temp);
            out("condenser_outlet_temperature", self.condenser_outlet_temp);
        }
        if self.heat_recovery.is_some() {
            out("heat_recovery_rate", self.heat_recovered);
            out(
                "heat_recovery_inlet_temperature",
                self.heat_recovery_inlet_temp,
            );
            out(
                "heat_recovery_outlet_temperature",
                self.heat_recovery_outlet_temp,
            );
        }
    }

    fn set_heat_recovery_conditions(&mut self, inlet_temp: f64, max_heat: f64) {
        self.heat_recovery_inlet_temp = inlet_temp;
        self.heat_recovery_max = max_heat;
    }

    fn heat_recovered(&self) -> f64 {
        self.heat_recovered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::performance_curve::{CurveType, PerformanceCurve};
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep, AUTOSIZE};
    use openbse_psychrometrics::MoistAirState;

    fn make_ctx(t_outdoor: f64) -> SimulationContext {
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
        chiller.capft_curve = Some(PerformanceCurve::Polynomial {
            name: "capft".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![
                0.988289399105446,
                0.031127718217927,
                -0.00154986412921254,
                -0.00334864255828258,
                -0.000146694443912943,
                0.000502974958539996,
            ],
            min_x: 4.0,
            max_x: 16.0,
            min_y: 12.8,
            max_y: 40.0,
            min_output: None,
            max_output: None,
        });
        // EIRFT from E+ IDF: ASHRAE901_AppJ_wtr_AB_cent_gt1055kW eir-f-t
        chiller.eirft_curve = Some(PerformanceCurve::Polynomial {
            name: "eirft".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![
                0.563967192309392,
                -0.034330681975216,
                0.00101506219660673,
                0.0339405925147554,
                -0.000431970984586658,
                -0.0000252410848639146,
            ],
            min_x: 4.0,
            max_x: 16.0,
            min_y: 12.8,
            max_y: 40.0,
            min_output: None,
            max_output: None,
        });
        // EIRFPLR from E+ IDF: ASHRAE901_AppJ_wtr_AB_cent_gt1055kW eir-f-plr
        chiller.eirfplr_curve = Some(PerformanceCurve::Polynomial {
            name: "eirfplr".to_string(),
            curve_type: CurveType::Quadratic,
            coefficients: vec![0.309752375539755, 0.153649268551135, 0.536462254009109],
            min_x: 0.0,
            max_x: 1.0,
            min_y: 0.0,
            max_y: 0.0,
            min_output: None,
            max_output: None,
        });
        chiller
    }

    /// DOE prototype hospital chiller as E+ Chiller:Electric:ReformulatedEIR
    /// (`ASHRAE901_Hospital_STD2019_Denver.idf`, CoolSys1 Chiller1), held at
    /// leaving CHW temperature `chw` with condenser water entering at `t_in`.
    fn make_reformulated_chiller(chw: f64, t_in: f64) -> AirCooledChiller {
        let mut chiller = AirCooledChiller::new("CoolSys1 Chiller1", 684_947.265, 5.33, chw, 0.02);
        chiller.model = ChillerModel::ReformulatedEir;
        chiller.water_cooled = true;
        chiller.condenser_entering_temp = Some(t_in);
        chiller.tower_approach = 0.0;
        chiller.min_plr = 0.1;
        // E+ autosized condenser flow, as reported in the run.
        chiller.design_condenser_flow = 34.587555 / water_rho(COLD_WATER_CONVERSION_TEMP);
        let biquadratic = |name: &str, c: [f64; 6]| PerformanceCurve::Polynomial {
            name: name.to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: c.to_vec(),
            min_x: 5.0,
            max_x: 10.0,
            min_y: 17.78,
            max_y: 46.11,
            min_output: None,
            max_output: None,
        };
        chiller.capft_curve = Some(biquadratic(
            "WC_PD_GT150_2010_PA_CAPFT",
            [
                0.9061150, 0.0292277, -0.0003647, -0.0009709, -0.0000905, 0.0002527,
            ],
        ));
        chiller.eirft_curve = Some(biquadratic(
            "WC_PD_GT150_2010_PA_EIRFT",
            [
                0.3773115, -0.0229032, 0.0016504, 0.0118156, 0.0004345, -0.0010132,
            ],
        ));
        chiller.eirfplr_curve = Some(PerformanceCurve::Polynomial {
            name: "WC_PD_150to300_2010_PA_EIRFPLR".to_string(),
            curve_type: CurveType::Bicubic,
            coefficients: vec![
                -1.360532e-01,
                8.642703e-03,
                3.855583e-06,
                1.024034,
                6.047444e-02,
                -8.947860e-03,
                0.0,
                5.706602e-02,
                0.0,
                0.0,
            ],
            min_x: 31.13,
            max_x: 36.52,
            min_y: 0.19,
            max_y: 1.03,
            min_output: None,
            max_output: None,
        });
        chiller
    }

    #[test]
    fn test_reformulated_eir_matches_eplus_timesteps() {
        // EnergyPlus 25.2, ASHRAE901_Hospital_STD2019_Denver.idf on Boulder
        // weather, CoolSys1 Chiller1 timestep outputs. Columns: evaporator
        // load [W], leaving CHW [°C], entering and leaving condenser water
        // [°C], electric power [W], relative tolerance on power. Rows span
        // on/off cycling (cycling ratio 0.02 and 0.50), the region where the
        // EIRFPLR curve holds its 0.19 PLR limit, and part to full load.
        //
        // E+ evaluates CAPFT at the CHW setpoint and EIRFT at the actual
        // leaving temperature, while OpenBSE uses the setpoint for both. They
        // agree on every row but the third, where the water left 0.1 % of
        // CAPFT away from its setpoint.
        let cases = [
            (
                1_568.399634,
                8.889294,
                21.031263,
                21.045287,
                459.395768,
                1e-4,
            ),
            (
                38_466.547313,
                8.87267,
                20.985974,
                21.330806,
                11_395.112864,
                1e-4,
            ),
            (
                90_733.537957,
                8.265474,
                21.488994,
                22.280009,
                23_639.304269,
                1.5e-3,
            ),
            (
                358_781.225507,
                6.7,
                22.067144,
                24.891405,
                49_555.966728,
                1e-4,
            ),
            (
                473_980.721478,
                6.7,
                22.578186,
                26.313823,
                66_098.534927,
                1e-4,
            ),
            (
                684_947.265,
                6.700157,
                22.34162,
                27.764882,
                99_138.08006,
                1e-4,
            ),
        ];
        let inlet = WaterPort::new(FluidState::water(12.0, 30.0));
        for (load, chw, t_in, t_out, power, tol) in cases {
            let mut chiller = make_reformulated_chiller(chw, t_in);
            chiller.simulate_plant(&inlet, load, &make_ctx(25.0));
            assert_relative_eq!(chiller.actual_capacity, load, max_relative = 1e-9);
            assert_relative_eq!(chiller.electric_power, power, max_relative = tol);
            assert_relative_eq!(chiller.condenser_outlet_temp, t_out, epsilon = 1e-3);
        }
    }

    #[test]
    fn test_reformulated_eir_closes_condenser_heat_balance() {
        let mut chiller = make_reformulated_chiller(6.7, 22.0);
        let inlet = WaterPort::new(FluidState::water(12.0, 30.0));
        chiller.simulate_plant(&inlet, 400_000.0, &make_ctx(25.0));
        let m_cp = chiller.condenser_mass_flow() * water_cp(chiller.condenser_inlet_temp);
        let q_cond = chiller.electric_power + chiller.actual_capacity;
        assert_relative_eq!(
            chiller.condenser_outlet_temp,
            22.0 + q_cond / m_cp,
            epsilon = 1e-5
        );
    }

    #[test]
    fn test_reformulated_eir_sizes_condenser_flow_as_eplus() {
        // E+: capacity × (1 + 1/COP) / (ΔT·cp·ρ) in m³/s, i.e. a mass flow of
        // capacity × (1 + 1/COP) / (ΔT·cp).
        let mut chiller = make_reformulated_chiller(6.7, 22.0);
        chiller.design_condenser_flow = 0.0;
        let expected =
            684_947.265 * (1.0 + 1.0 / 5.33) / (water_cp(COLD_WATER_CONVERSION_TEMP) * 5.56);
        assert_relative_eq!(
            chiller.condenser_mass_flow(),
            expected,
            max_relative = 1e-12
        );
    }

    /// DOE prototype hospital heat-recovery chiller (E+ Chiller:Electric:EIR,
    /// `ASHRAE901_Hospital_STD2022_Denver.idf`), holding 6.70 °C with tower
    /// water entering at `t_cond_in` and heat-recovery water at `t_hr_in`.
    fn make_heat_recovery_chiller(t_cond_in: f64, t_hr_in: f64) -> AirCooledChiller {
        let mut chiller =
            AirCooledChiller::new("Heat Recovery Chiller", 93_781.5151, 2.84025, 6.7, 0.0041);
        chiller.water_cooled = true;
        chiller.condenser_entering_temp = Some(t_cond_in);
        chiller.tower_approach = 0.0;
        chiller.min_plr = 0.1;
        chiller.min_unloading_ratio = Some(0.2);
        let biquadratic = |name: &str, c: [f64; 6]| PerformanceCurve::Polynomial {
            name: name.to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: c.to_vec(),
            min_x: 0.0,
            max_x: 999.0,
            min_y: 0.0,
            max_y: 999.0,
            min_output: None,
            max_output: None,
        };
        chiller.capft_curve = Some(biquadratic(
            "Lp2LpChlr_CAP_FT",
            [
                1.087789472,
                0.0404749,
                0.000308567,
                -0.003035172,
                -9.93726e-05,
                -0.000313643,
            ],
        ));
        chiller.eirft_curve = Some(biquadratic(
            "Lp2LpChlr_EIR_FT",
            [
                0.457251364,
                -0.007255161,
                0.000255435,
                0.00659246,
                0.000238374,
                -0.000294649,
            ],
        ));
        chiller.eirfplr_curve = Some(PerformanceCurve::Polynomial {
            name: "Lp2LpChlr_PLR".to_string(),
            curve_type: CurveType::Quadratic,
            coefficients: vec![0.02297712, 0.90150452, 0.07551826],
            min_x: 0.0,
            max_x: 1.0,
            min_y: 0.0,
            max_y: 0.0,
            min_output: None,
            max_output: None,
        });
        chiller.heat_recovery = Some(ChillerHeatRecovery {
            leaving_setpoint: 48.89,
            // E+ autosized heat-recovery flow, as reported in the run.
            design_flow: 5.46102 / water_rho(HOT_WATER_CONVERSION_TEMP),
            capacity_fraction: 1.0,
            inlet_high_limit: None,
        });
        chiller.set_heat_recovery_conditions(t_hr_in, f64::INFINITY);
        chiller
    }

    #[test]
    fn test_heat_recovery_matches_eplus_timesteps() {
        // EnergyPlus 22.1, ASHRAE901_Hospital_STD2022_Denver.idf on Boulder
        // weather, Heat Recovery Chiller timestep outputs. Columns:
        // evaporator load [W], entering tower and heat-recovery water [°C],
        // electric power [W], heat recovered [W], leaving heat-recovery
        // water [°C], relative tolerance on power.
        //  1. all condenser heat recovered (Jan 2 10:50);
        //  2. recovery capped by the 48.89 °C setpoint, rest to the tower
        //     (Jan 2 13:50). E+ blends the sink temperatures with the split
        //     from its previous iteration; the converged blend here draws
        //     0.16 % more power;
        //  3. PLR 0.12, below the 0.2 min unloading ratio: 10.4 kW of false
        //     load is recovered with the rest (Jan 2 09:30).
        let cases = [
            (
                43_037.544848,
                21.064519,
                31.894178,
                12_127.913869,
                55_165.458717,
                34.310848,
                1e-5,
            ),
            (
                84_306.834544,
                20.668688,
                44.162184,
                31_015.329209,
                107_922.143139,
                48.89,
                2e-3,
            ),
            (
                11_623.907073,
                21.048156,
                25.613282,
                5_565.920793,
                27_624.566953,
                26.823703,
                1e-5,
            ),
        ];
        let inlet = WaterPort::new(FluidState::water(12.0, 4.1));
        for (load, t_cond_in, t_hr_in, power, recovered, t_hr_out, tol) in cases {
            let mut chiller = make_heat_recovery_chiller(t_cond_in, t_hr_in);
            chiller.simulate_plant(&inlet, load, &make_ctx(10.0));
            assert_relative_eq!(chiller.actual_capacity, load, max_relative = 1e-9);
            assert_relative_eq!(chiller.electric_power, power, max_relative = tol);
            assert_relative_eq!(chiller.heat_recovered, recovered, max_relative = 1e-5);
            assert_relative_eq!(chiller.heat_recovery_outlet_temp, t_hr_out, epsilon = 1e-3);
        }
    }

    #[test]
    fn test_heat_recovery_limited_by_receiving_loop() {
        // A heating loop that can take only 20 kW: the rest goes to the tower,
        // and the curves see a sink temperature between the two inlets.
        let mut capped = make_heat_recovery_chiller(21.0, 38.0);
        capped.set_heat_recovery_conditions(38.0, 20_000.0);
        let mut free = make_heat_recovery_chiller(21.0, 38.0);
        let mut none = make_heat_recovery_chiller(21.0, 38.0);
        none.heat_recovery = None;
        let inlet = WaterPort::new(FluidState::water(12.0, 4.1));
        for c in [&mut capped, &mut free, &mut none] {
            c.simulate_plant(&inlet, 60_000.0, &make_ctx(10.0));
        }
        assert_relative_eq!(capped.heat_recovered, 20_000.0, max_relative = 1e-12);
        assert_relative_eq!(none.heat_recovered, 0.0);
        // Recovering into 38 °C water costs more power than rejecting at 21 °C.
        assert!(none.electric_power < capped.electric_power);
        assert!(capped.electric_power < free.electric_power);
    }

    #[test]
    fn test_min_unloading_ratio_false_loads_to_condenser() {
        // PLR 0.12 between min PLR 0.1 and min unloading 0.2: E+ runs the
        // compressor at 0.2 without cycling, so power matches PLR 0.2 and the
        // condenser rejects the extra 0.08 of capacity.
        let mut chiller = make_reformulated_chiller(6.7, 22.0);
        chiller.min_unloading_ratio = Some(0.2);
        let avail = chiller.rated_capacity * chiller.capacity_modifier(22.0);
        let op_low = chiller.operate(0.12 * avail, 22.0);
        let op_unload = chiller.operate(0.2 * avail, 22.0);
        assert_relative_eq!(op_low.power, op_unload.power, max_relative = 1e-12);
        assert_relative_eq!(op_low.false_load, 0.08 * avail, max_relative = 1e-9);
        assert_relative_eq!(op_unload.false_load, 0.0);
    }

    #[test]
    fn test_chiller_rated_conditions() {
        let mut chiller = AirCooledChiller::new("Test Chiller", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        let _outlet = chiller.simulate_plant(&inlet, 100_000.0, &ctx);
        assert!(
            chiller.actual_cop > 2.0,
            "COP at rated: {}",
            chiller.actual_cop
        );
        assert!(chiller.electric_power > 0.0);
        assert_relative_eq!(chiller.plr, 1.0, epsilon = 0.01);
    }

    #[test]
    fn test_autosized_chiller_takes_its_sized_capacity() {
        // The autosize sentinel used to be run as a −99,999 W capacity,
        // giving PLR ≈ −5 and a quarter of the rated COP.
        let mut chiller = AirCooledChiller::new("Autosized", AUTOSIZE, 3.0, 7.0, 0.005);
        assert_eq!(chiller.nominal_capacity(), None);
        chiller.set_nominal_capacity(100_000.0);
        assert_eq!(chiller.nominal_capacity(), Some(100_000.0));

        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        chiller.simulate_plant(&inlet, 50_000.0, &make_ctx(29.4));
        assert_relative_eq!(chiller.plr, 0.5, epsilon = 1e-9);
        assert_relative_eq!(chiller.actual_capacity, 50_000.0, epsilon = 1e-6);
        chiller.simulate_plant(&inlet, 100_000.0, &make_ctx(29.4));
        assert_relative_eq!(chiller.actual_cop, 3.0, epsilon = 1e-9);
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
        assert!(
            chiller_cool.actual_cop > chiller_hot.actual_cop,
            "COP at 25C ({:.2}) should be > COP at 40C ({:.2})",
            chiller_cool.actual_cop,
            chiller_hot.actual_cop
        );
    }

    #[test]
    fn test_chiller_part_load() {
        let mut chiller = AirCooledChiller::new("Test Chiller", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        let _out = chiller.simulate_plant(&inlet, 50_000.0, &ctx);
        assert!(
            (chiller.plr - 0.5).abs() < 0.05,
            "PLR should be ~0.5, got {}",
            chiller.plr
        );
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
        assert!(
            chiller.plr < chiller.min_plr,
            "PLR ({}) should be < min_plr ({})",
            chiller.plr,
            chiller.min_plr
        );
        // Actual capacity should be close to load (time-averaged)
        assert!(
            (chiller.actual_capacity - 10_000.0).abs() < 1000.0,
            "Actual cap ({:.0}) should be close to load (10000)",
            chiller.actual_capacity
        );
    }

    #[test]
    fn test_chiller_without_eirfplr_curve_keeps_rated_cop_at_part_load() {
        // A flat EIRFPLR = 1.0 fallback used to draw full-load power at any
        // load, so COP fell to max(PLR, min_plr) × rated COP (1.75 at PLR 0.5).
        // At the 29.4 °C reference, CAPFT = EIRFT = 1 and COP is rated.
        let mut chiller = AirCooledChiller::new("No Curves", 100_000.0, 3.5, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        // 0.1 is below min_plr (0.25) and exercises on/off cycling.
        for plr in [0.1, 0.2, 0.25, 0.5, 0.75, 1.0] {
            let load = plr * 100_000.0;
            chiller.simulate_plant(&inlet, load, &ctx);
            assert_relative_eq!(chiller.actual_capacity, load, epsilon = 1e-6);
            assert_relative_eq!(chiller.electric_power, load / 3.5, epsilon = 1e-6);
            assert_relative_eq!(chiller.actual_cop, 3.5, epsilon = 1e-9);
        }
    }

    #[test]
    fn test_water_cooled_chiller_with_curves() {
        // Water-cooled chiller with actual E+ IDF curves
        let mut chiller = make_water_cooled_chiller(1_570_096.0, 6.11);
        let inlet = WaterPort::new(FluidState::water(12.0, 50.0));
        // At reference condenser temp: ODB ≈ 42°C → WB ≈ 24°C → cond = 29.56°C ≈ ref
        let ctx = make_ctx(30.0);
        let _out = chiller.simulate_plant(&inlet, 1_000_000.0, &ctx);
        assert!(
            chiller.actual_cop > 4.0,
            "Water-cooled COP should be > 4, got {:.2}",
            chiller.actual_cop
        );
        assert!(chiller.electric_power > 0.0);
    }

    #[test]
    fn test_water_cooled_chiller_curves_at_reference() {
        // Verify CAPFT ≈ 1.0 and EIRFT ≈ 1.0 at reference conditions
        let chiller = make_water_cooled_chiller(1_000_000.0, 6.11);
        // Reference: CHW leaving = 6.67°C, condenser entering = 29.44°C
        let capft = chiller.capacity_modifier(29.44);
        let eirft = chiller.eir_modifier_temp(29.44);
        assert!(
            (capft - 1.0).abs() < 0.05,
            "CAPFT at reference should be ~1.0, got {:.4}",
            capft
        );
        assert!(
            (eirft - 1.0).abs() < 0.05,
            "EIRFT at reference should be ~1.0, got {:.4}",
            eirft
        );
    }

    #[test]
    fn test_eirfplr_curve_at_full_load() {
        let chiller = make_water_cooled_chiller(1_000_000.0, 6.11);
        let eirfplr = chiller.eir_modifier_plr(1.0, 29.44);
        // At PLR=1.0: 0.3098 + 0.1536 + 0.5365 ≈ 1.0
        assert!(
            (eirfplr - 1.0).abs() < 0.01,
            "EIRFPLR at PLR=1.0 should be ~1.0, got {:.4}",
            eirfplr
        );
    }
}
