//! Fan driven by an ASHRAE Standard 205 RS0003 performance map.
//!
//! Replaces the analytical fan ([`crate::fan::Fan`]) when the user
//! supplies an `a205_file:` on a fan input.  The 205 map returns shaft
//! power and impeller speed for a given (flow, static pressure rise)
//! query; an optional motor / drive / VFD chain (from the file's
//! `assembly_components`) converts shaft power to grid-side electric
//! power.  When the file's assembly is empty, a configurable fallback
//! motor efficiency is used.
//!
//! ### Heat to airstream
//!
//! Standard 205 defines `heat_loss_fraction` as the fraction of the
//! motor/drive **losses** that escape to the surrounding space rather
//! than entering the airstream.  We follow that convention:
//!
//! ```text
//! heat_to_air = shaft_power + (1 - heat_loss_fraction) × (electric - shaft_power)
//! ```
//!
//! When `heat_loss_fraction == 1`, only the fan shaft work heats the
//! air (typical for an externally-mounted motor).  When 0, the entire
//! electric input heats the air (typical for an in-line plenum motor).

use openbse_a205::rs0003::{FanEfficiencyChain, FanInterpolator, FanQuery, Rs0003};
use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych};

const STANDARD_AIR_DENSITY: f64 = 1.204; // kg/m³ at 20 °C, sea level

fn default_submeter() -> String {
    "General".to_string()
}

pub struct FanA205 {
    pub name: String,
    pub tag: String,
    pub submeter: String,
    /// Design static pressure rise [Pa] — fed into the map for shaft-power lookup.
    pub design_pressure_rise: f64,
    /// Design air flow rate [m³/s] — used for autosizing.  When zero, the
    /// file's nominal flow is used.
    pub design_flow_rate: f64,
    /// Fallback motor efficiency when the file's `assembly_components`
    /// list is empty.  Ignored when a motor is present in the file.
    pub fallback_motor_efficiency: f64,

    rs0003: Rs0003,
    interp: FanInterpolator,
    chain: FanEfficiencyChain,

    // runtime state
    power: f64,
    shaft_power: f64,
    heat_to_air: f64,
    impeller_speed: f64,
    in_range: bool,
}

impl std::fmt::Debug for FanA205 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FanA205")
            .field("name", &self.name)
            .field("design_pressure_rise", &self.design_pressure_rise)
            .field("design_flow_rate", &self.design_flow_rate)
            .finish()
    }
}

impl FanA205 {
    pub fn from_rs0003(
        name: impl Into<String>,
        rs0003: Rs0003,
        design_pressure_rise: f64,
        design_flow_rate: f64,
        fallback_motor_efficiency: f64,
    ) -> Result<Self, openbse_a205::A205Error> {
        Self::from_rs0003_with_overrides(
            name,
            rs0003,
            design_pressure_rise,
            design_flow_rate,
            fallback_motor_efficiency,
            None,
            None,
            None,
        )
    }

    /// Same as [`from_rs0003`], but with optional user-supplied
    /// standalone overrides for motor / mechanical drive / VFD sub-models.
    /// Each override, when present, replaces the corresponding entry from
    /// the fan file's `assembly_components`.
    pub fn from_rs0003_with_overrides(
        name: impl Into<String>,
        rs0003: Rs0003,
        design_pressure_rise: f64,
        design_flow_rate: f64,
        fallback_motor_efficiency: f64,
        motor_override: Option<openbse_a205::rs0005::Rs0005>,
        drive_override: Option<openbse_a205::rs0007::Rs0007>,
        vfd_override: Option<openbse_a205::rs0006::Rs0006>,
    ) -> Result<Self, openbse_a205::A205Error> {
        let interp = FanInterpolator::new(&rs0003.performance.performance_map)?;
        let chain = FanEfficiencyChain::from_assembly_with_overrides(
            &rs0003,
            fallback_motor_efficiency,
            motor_override,
            drive_override,
            vfd_override,
        )?;
        let design_flow_rate = if design_flow_rate > 0.0 {
            design_flow_rate
        } else {
            rs0003.performance.nominal_standard_air_volumetric_flow_rate
        };
        Ok(Self {
            name: name.into(),
            tag: String::new(),
            submeter: default_submeter(),
            design_pressure_rise,
            design_flow_rate,
            fallback_motor_efficiency,
            rs0003,
            interp,
            chain,
            power: 0.0,
            shaft_power: 0.0,
            heat_to_air: 0.0,
            impeller_speed: 0.0,
            in_range: true,
        })
    }
}

impl AirComponent for FanA205 {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Fan
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        self.heat_to_air
    }

    fn design_air_flow_rate(&self) -> Option<f64> {
        if self.design_flow_rate > 0.0 {
            Some(self.design_flow_rate)
        } else {
            None
        }
    }

    fn set_design_air_flow_rate(&mut self, flow: f64) {
        self.design_flow_rate = flow;
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.power = 0.0;
            self.shaft_power = 0.0;
            self.heat_to_air = 0.0;
            self.impeller_speed = 0.0;
            return *inlet;
        }
        // Volumetric flow of standard air [m³/s]: mass / density_standard.
        let vol_std = inlet.mass_flow / STANDARD_AIR_DENSITY;
        let q = FanQuery {
            volumetric_flow_m3_s: vol_std,
            static_pressure_pa: self.design_pressure_rise,
        };
        let r = match self.interp.query(&q) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("FanA205 '{}' map query failed: {}", self.name, e);
                return *inlet;
            }
        };
        self.impeller_speed = r.impeller_speed_rev_s;
        self.in_range = r.in_range;
        self.shaft_power = r.shaft_power_w;

        // Compose motor / drive / VFD chain to recover grid electric input.
        self.power = self
            .chain
            .grid_electric_power(r.shaft_power_w, r.impeller_speed_rev_s);

        // Heat into airstream: shaft work always goes to air; some
        // fraction of motor / drive losses may bypass the airstream per
        // the file's `heat_loss_fraction`.
        let losses = (self.power - r.shaft_power_w).max(0.0);
        let heat_loss_frac = self.rs0003.performance.heat_loss_fraction.clamp(0.0, 1.0);
        self.heat_to_air = r.shaft_power_w + (1.0 - heat_loss_frac) * losses;

        // Apply fan heat to outlet enthalpy (E+ Fans.cc convention).
        let cp_air = psych::cp_air_fn_w(inlet.state.w);
        let dt = self.heat_to_air / (inlet.mass_flow * cp_air);
        let out_t = inlet.state.t_db + dt;
        AirPort::new(
            psych::MoistAirState::new(out_t, inlet.state.w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("shaft_power", self.shaft_power);
        out("impeller_speed", self.impeller_speed);
        out("heat_to_air", self.heat_to_air);
        out("in_range", if self.in_range { 1.0 } else { 0.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_a205::rs0003::Rs0003;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;
    use std::path::PathBuf;

    fn example_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("openbse-a205")
            .join("examples")
            .join("Fan-Continuous.RS0003.a205.json")
    }

    fn make_ctx() -> SimulationContext {
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
            outdoor_air: MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn loads_fan_and_reports_design_flow() {
        let rs = Rs0003::load(&example_path()).unwrap();
        let fan = FanA205::from_rs0003("test", rs, 750.0, 0.0, 0.9).unwrap();
        // Nominal flow from the file: 9.5 m³/s
        assert!(fan.design_air_flow_rate().unwrap() > 5.0);
    }

    #[test]
    fn consumes_power_and_warms_air() {
        let rs = Rs0003::load(&example_path()).unwrap();
        let mut fan = FanA205::from_rs0003("test", rs, 750.0, 0.0, 0.9).unwrap();
        // Inlet at ~30% of nominal flow (within the file's flow range).
        // Standard density 1.204 × 6 m³/s ≈ 7.2 kg/s.
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0), 7.2);
        let outlet = fan.simulate_air(&inlet, &make_ctx());
        assert!(fan.power > 0.0, "fan should consume power");
        assert!(fan.shaft_power > 0.0);
        // With the example file's heat_loss_fraction = 1 the airstream
        // gets only the shaft work (none of the motor losses).
        assert!(fan.heat_to_air >= fan.shaft_power * 0.99);
        // Some warming of the air (shaft work into air)
        assert!(outlet.state.t_db > inlet.state.t_db);
    }

    #[test]
    fn zero_flow_idle() {
        let rs = Rs0003::load(&example_path()).unwrap();
        let mut fan = FanA205::from_rs0003("test", rs, 750.0, 0.0, 0.9).unwrap();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0), 0.0);
        let outlet = fan.simulate_air(&inlet, &make_ctx());
        assert_eq!(fan.power, 0.0);
        assert_eq!(outlet.state.t_db, inlet.state.t_db);
    }
}
