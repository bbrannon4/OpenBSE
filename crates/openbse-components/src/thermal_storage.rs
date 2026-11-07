//! Thermal energy storage (TES) plant component.
//!
//! Supports chilled-water and ice storage with three control strategies:
//!   FullStorage:    charge off-peak, discharge during peak hours.
//!   LoadLeveling:   chiller runs at constant rate; TES fills the gap.
//!   DemandLimiting: discharge when building power > threshold.
//!
//! Standby loss: Q_loss = loss_ua × (T_ambient - T_storage).
//! State of charge evolves each timestep:
//!   stored_energy_wh += (charge_rate - discharge_rate - standby_loss) × dt / 3600.

use openbse_core::ports::*;
use openbse_psychrometrics::FluidState;
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}
fn default_tes_ua() -> f64 {
    5.0 // W/K
}
fn default_ice_charge_cop_factor() -> f64 {
    0.85
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TesType {
    ChilledWater,
    Ice,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TesControlStrategy {
    /// Charge off-peak, discharge during peak hours.
    FullStorage,
    /// Chiller runs at constant rate; TES supplements demand.
    LoadLeveling,
    /// Discharge to prevent building peak demand exceeding threshold [W].
    DemandLimiting { threshold_w: f64 },
}

/// Thermal energy storage plant component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalStorage {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    pub tes_type: TesType,
    /// Storage capacity [Wh]
    pub capacity_wh: f64,
    /// Maximum charge rate [W]
    pub max_charge_rate: f64,
    /// Maximum discharge rate [W]
    pub max_discharge_rate: f64,
    /// Standby loss UA [W/K] (default 5.0)
    #[serde(default = "default_tes_ua")]
    pub loss_ua: f64,
    /// COP penalty factor when making ice vs. normal chilling (default 0.85)
    #[serde(default = "default_ice_charge_cop_factor")]
    pub ice_charge_cop_factor: f64,
    /// Control strategy
    pub control_strategy: TesControlStrategy,
    /// Peak hours (1-24) when charging is suppressed and discharging preferred
    #[serde(default)]
    pub peak_hours: Vec<u32>,

    // ─── State ──────────────────────────────────────────────────────────────
    /// State of charge at the end of the current timestep [Wh]
    #[serde(skip)]
    pub stored_energy_wh: f64,
    /// State of charge at the start of the current timestep [Wh]: every HVAC
    /// iteration steps from it, so the step is counted once.
    #[serde(skip)]
    stored_at_step_start: f64,
    /// The timestep `stored_at_step_start` belongs to (month, day, hour, sub-hour).
    #[serde(skip)]
    step: Option<(u32, u32, u32, u32)>,
    /// Current charge rate [W] (positive = charging)
    #[serde(skip)]
    pub charge_rate: f64,
    /// Current discharge rate [W]
    #[serde(skip)]
    pub discharge_rate: f64,
    /// Electric power for TES controls/pumps [W]
    #[serde(skip)]
    pub power: f64,
    /// Net thermal output to loop [W] (positive = discharging = cooling provided)
    #[serde(skip)]
    pub net_thermal: f64,
}

impl ThermalStorage {
    pub fn new(
        name: &str,
        tes_type: TesType,
        capacity_wh: f64,
        max_charge_rate: f64,
        max_discharge_rate: f64,
        control_strategy: TesControlStrategy,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            tes_type,
            capacity_wh,
            max_charge_rate,
            max_discharge_rate,
            loss_ua: 5.0,
            ice_charge_cop_factor: 0.85,
            control_strategy,
            peak_hours: vec![],
            stored_energy_wh: 0.0,
            stored_at_step_start: 0.0,
            step: None,
            charge_rate: 0.0,
            discharge_rate: 0.0,
            power: 0.0,
            net_thermal: 0.0,
        }
    }

    fn is_peak_hour(&self, hour: u32) -> bool {
        self.peak_hours.contains(&hour)
    }

    /// On the first call of a new timestep, the state the last one ended in
    /// becomes the state this one starts from.
    fn begin_step(&mut self, ctx: &SimulationContext) {
        let t = &ctx.timestep;
        let key = (t.month, t.day, t.hour, t.sub_hour);
        if self.step != Some(key) {
            self.step = Some(key);
            self.stored_at_step_start = self.stored_energy_wh;
        }
    }

    fn storage_temp(&self) -> f64 {
        match self.tes_type {
            TesType::ChilledWater => 6.0,
            TesType::Ice => -5.0,
        }
    }
}

impl PlantComponent for ThermalStorage {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::ThermalStorage
    }

    fn rated_capacity(&self) -> f64 {
        self.max_discharge_rate
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        ctx: &SimulationContext,
    ) -> WaterPort {
        self.begin_step(ctx);
        let dt = ctx.timestep.dt; // seconds
        let hour = ctx.timestep.hour;
        let is_peak = self.is_peak_hour(hour);

        // Standby loss: ambient 20°C assumed; cooling loss warms the storage
        let t_storage = self.storage_temp();
        let standby_loss_w = self.loss_ua * (20.0 - t_storage); // always positive for CHW/ice
        let standby_loss_wh = (standby_loss_w * dt / 3600.0).min(self.stored_at_step_start);
        // Energy that can leave, and room that can fill, this step after the
        // standby loss, so the state of charge stays in range without clipping.
        let stored = self.stored_at_step_start - standby_loss_wh;
        let dt_h = dt / 3600.0;

        let cooling_load = load.abs(); // treat any load as cooling demand magnitude

        // Determine charge and discharge rates based on control strategy
        let (charge, discharge) = match self.control_strategy {
            TesControlStrategy::FullStorage => {
                if !is_peak && stored < self.capacity_wh {
                    // Off-peak: charge at max rate
                    let avail_cap = (self.capacity_wh - stored) / dt_h;
                    let c = self.max_charge_rate.min(avail_cap);
                    (c, 0.0)
                } else if is_peak && stored > 0.0 {
                    // Peak: discharge to meet load
                    let avail_energy = stored / dt_h;
                    let d = cooling_load.min(self.max_discharge_rate).min(avail_energy);
                    (0.0, d)
                } else {
                    (0.0, 0.0)
                }
            }
            TesControlStrategy::LoadLeveling => {
                // Discharge when demand exceeds a baseline; charge when below
                // Simplified: discharge = max(0, load - max_charge_rate); charge if slack
                if cooling_load > self.max_charge_rate && stored > 0.0 {
                    let need = cooling_load - self.max_charge_rate;
                    let avail = stored / dt_h;
                    let d = need.min(self.max_discharge_rate).min(avail);
                    (0.0, d)
                } else if cooling_load < self.max_charge_rate && stored < self.capacity_wh {
                    let slack = self.max_charge_rate - cooling_load;
                    let avail_cap = (self.capacity_wh - stored) / dt_h;
                    let c = slack.min(self.max_charge_rate).min(avail_cap);
                    (c, 0.0)
                } else {
                    (0.0, 0.0)
                }
            }
            TesControlStrategy::DemandLimiting { threshold_w } => {
                if cooling_load > threshold_w && stored > 0.0 {
                    let need = cooling_load - threshold_w;
                    let avail = stored / dt_h;
                    let d = need.min(self.max_discharge_rate).min(avail);
                    (0.0, d)
                } else {
                    (0.0, 0.0)
                }
            }
        };

        self.charge_rate = charge;
        self.discharge_rate = discharge;

        self.stored_energy_wh = stored + (charge - discharge) * dt_h;

        // Net thermal output: positive = provides cooling to loop
        self.net_thermal = discharge;
        // Small pump/controls power
        self.power = if charge > 0.0 || discharge > 0.0 {
            500.0
        } else {
            0.0
        };

        // Compute outlet conditions
        if discharge <= 0.0 || inlet.state.mass_flow <= 0.0 {
            return *inlet;
        }
        let cp = inlet.state.cp;
        let dt_fluid = discharge / (inlet.state.mass_flow * cp).max(1.0);
        let t_out = inlet.state.temp - dt_fluid; // cooling lowers temp
        WaterPort::new(FluidState {
            temp: t_out,
            mass_flow: inlet.state.mass_flow,
            cp,
        })
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        // Positive = provides cooling (negative load on the cooling loop)
        self.net_thermal
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("stored_energy_wh", self.stored_energy_wh);
        out("charge_rate_w", self.charge_rate);
        out("discharge_rate_w", self.discharge_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::{MoistAirState, STD_PRESSURE};

    fn make_ctx(hour: u32) -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 7,
                day: 15,
                hour,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(35.0, 0.30, STD_PRESSURE),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    fn make_inlet() -> WaterPort {
        WaterPort::new(FluidState::water(12.0, 2.0))
    }

    #[test]
    fn test_full_storage_charges_offpeak() {
        let mut tes = ThermalStorage::new(
            "TES1",
            TesType::ChilledWater,
            10000.0, // 10 kWh capacity
            5000.0,  // 5 kW max charge
            5000.0,  // 5 kW max discharge
            TesControlStrategy::FullStorage,
        );
        tes.peak_hours = vec![14, 15, 16, 17, 18]; // peak 14-18h
        tes.stored_energy_wh = 0.0;

        let ctx = make_ctx(2); // off-peak hour 2
        let inlet = make_inlet();
        let _out = tes.simulate_plant(&inlet, 0.0, &ctx);

        assert!(tes.charge_rate > 0.0, "should charge in off-peak hours");
        assert_eq!(tes.discharge_rate, 0.0, "should not discharge in off-peak");
        assert!(tes.stored_energy_wh > 0.0, "stored energy should increase");
    }

    #[test]
    fn test_full_storage_discharges_peak() {
        let mut tes = ThermalStorage::new(
            "TES2",
            TesType::ChilledWater,
            10000.0,
            5000.0,
            5000.0,
            TesControlStrategy::FullStorage,
        );
        tes.peak_hours = vec![14, 15, 16, 17, 18];
        tes.stored_energy_wh = 8000.0; // partially charged

        let ctx = make_ctx(15); // peak hour
        let inlet = make_inlet();
        let _out = tes.simulate_plant(&inlet, 3000.0, &ctx);

        assert_eq!(tes.charge_rate, 0.0, "should not charge during peak");
        assert!(tes.discharge_rate > 0.0, "should discharge during peak");
        assert!(
            tes.stored_energy_wh < 8000.0,
            "stored energy should decrease"
        );
    }

    #[test]
    fn test_standby_loss() {
        let mut tes = ThermalStorage::new(
            "TES3",
            TesType::ChilledWater,
            10000.0,
            5000.0,
            5000.0,
            TesControlStrategy::FullStorage,
        );
        // Mark all hours as peak so no charging occurs; zero load → no discharging either
        tes.peak_hours = (1..=24).collect();
        tes.stored_energy_wh = 5000.0;

        let ctx = make_ctx(12);
        let inlet = make_inlet();
        let _out = tes.simulate_plant(&inlet, 0.0, &ctx);

        // standby_loss = 5.0 W/K × (20 - 6) = 70 W → 70 Wh loss in 1 hour
        assert!(
            tes.stored_energy_wh < 5000.0,
            "stored energy should decrease due to standby loss"
        );
    }

    /// HVAC iterations repeat a timestep: the store steps once from its
    /// start-of-step state, and that state closes on charge, discharge and loss.
    #[test]
    fn test_iterations_step_the_state_once_and_close() {
        let mut tes = ThermalStorage::new(
            "TES",
            TesType::Ice,
            100_000.0,
            5000.0,
            8000.0,
            TesControlStrategy::FullStorage,
        );
        tes.peak_hours = vec![15];
        tes.stored_energy_wh = 50_000.0;
        let inlet = make_inlet();
        for (hour, load) in [(2, 0.0), (15, 6000.0), (15, 6000.0), (16, 0.0)] {
            let start = if tes.step == Some((7, 15, hour, 1)) {
                tes.stored_at_step_start
            } else {
                tes.stored_energy_wh
            };
            for _ in 0..4 {
                tes.simulate_plant(&inlet, load, &make_ctx(hour));
            }
            let loss = tes.loss_ua * (20.0 - tes.storage_temp());
            let expected = start + tes.charge_rate - tes.discharge_rate - loss;
            assert!(
                (tes.stored_energy_wh - expected).abs() < 1e-9,
                "hour {hour}: {} vs {expected}",
                tes.stored_energy_wh
            );
        }
    }

    #[test]
    fn test_capacity_clamped() {
        let mut tes = ThermalStorage::new(
            "TES4",
            TesType::ChilledWater,
            1000.0, // 1 kWh capacity
            5000.0,
            5000.0,
            TesControlStrategy::FullStorage,
        );
        tes.peak_hours = vec![14, 15];
        tes.stored_energy_wh = 999.0; // nearly full

        let ctx = make_ctx(2); // off-peak: will try to charge
        let inlet = make_inlet();
        let _out = tes.simulate_plant(&inlet, 0.0, &ctx);

        assert!(
            tes.stored_energy_wh <= tes.capacity_wh,
            "stored energy should never exceed capacity"
        );
    }

    #[test]
    fn test_no_discharge_below_zero() {
        let mut tes = ThermalStorage::new(
            "TES5",
            TesType::ChilledWater,
            1000.0,
            5000.0,
            5000.0,
            TesControlStrategy::FullStorage,
        );
        tes.peak_hours = vec![14, 15];
        tes.stored_energy_wh = 0.0; // empty

        let ctx = make_ctx(15); // peak: wants to discharge but nothing stored
        let inlet = make_inlet();
        let _out = tes.simulate_plant(&inlet, 3000.0, &ctx);

        assert_eq!(tes.discharge_rate, 0.0, "cannot discharge when empty");
        assert!(tes.stored_energy_wh >= 0.0, "stored energy never negative");
    }
}
