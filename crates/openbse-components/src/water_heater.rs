//! Domestic hot water (DHW) storage-tank water heater component model.
//!
//! Models a mixed-tank water heater with deadband thermostat control,
//! standby shell losses, and draw-dependent energy delivery. This is a
//! standalone component (does not implement AirComponent or PlantComponent)
//! with its own `simulate` method called directly from the simulation loop.
//!
//! Physics match EnergyPlus WaterHeaters.cc (mixed-tank energy balance):
//!
//!   Q_delivered = m_draw * Cp * (T_set - T_mains)     energy to heat draw water
//!   Q_loss     = UA_tank * (T_tank - T_ambient)        standby shell losses
//!   Q_needed   = Q_delivered + Q_loss
//!   Q_input    = min(Q_needed / efficiency, capacity)  limited by burner/element
//!   Tank temp:   dT = (Q_input * eff - Q_delivered - Q_loss) * dt / (m_tank * Cp)
//!
//! Deadband control: burner fires when T_tank < setpoint - deadband,
//!                   turns off when T_tank >= setpoint.
//!
//! Reference: EnergyPlus Engineering Reference, "Water Heaters"

use serde::{Deserialize, Serialize};

/// Specific heat capacity of water [J/(kg*K)].
const CP_WATER: f64 = 4186.0;

/// Density of water [kg/L] (approximation at typical DHW temperatures).
const RHO_WATER: f64 = 1.0;

/// Water heater fuel / energy source type.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WaterHeaterFuel {
    /// Natural gas burner
    Gas,
    /// Electric resistance element
    Electric,
    /// Heat pump water heater (higher effective efficiency expressed as COP)
    HeatPump,
}

/// Domestic hot water storage-tank water heater.
///
/// A simple mixed-tank model with deadband thermostat control. The tank is
/// assumed to be fully mixed (uniform temperature). Standby losses are
/// computed from a UA coefficient and the ambient-to-tank temperature
/// difference. Draw water is heated from mains temperature to the tank
/// setpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaterHeater {
    pub name: String,
    /// Fuel / energy source type.
    pub fuel_type: WaterHeaterFuel,
    /// Tank volume [L].
    pub tank_volume: f64,
    /// Burner or element input capacity [W].
    pub capacity: f64,
    /// Thermal efficiency [0-1] for gas/electric, COP for heat pump.
    pub efficiency: f64,
    /// Tank temperature setpoint [degC].
    pub setpoint_temp: f64,
    /// Deadband [degC] below setpoint at which burner fires.
    pub deadband: f64,
    /// Standby loss coefficient [W/K] (UA of tank shell).
    pub ua_standby: f64,
    /// Ambient temperature around the tank [degC].
    pub ambient_temp: f64,

    // ---- Runtime state (not serialised) ------------------------------------
    /// Current average tank temperature [degC].
    #[serde(skip)]
    pub tank_temp: f64,
    /// Current heating rate delivered to the tank [W].
    #[serde(skip)]
    pub heating_rate: f64,
    /// Current energy input rate [W] (fuel or electricity consumed by the heater).
    #[serde(skip)]
    pub energy_input: f64,
    /// Whether the burner / element is currently on (deadband hysteresis).
    #[serde(skip)]
    pub is_heating: bool,
}

impl WaterHeater {
    /// Create a new water heater.
    ///
    /// # Arguments
    /// * `name`        - Component name
    /// * `fuel_type`   - Gas, Electric, or HeatPump
    /// * `tank_volume` - Tank volume [L]
    /// * `capacity`    - Burner / element input capacity [W]
    /// * `efficiency`  - Thermal efficiency (0-1 for gas/electric, COP for HP)
    /// * `setpoint`    - Tank setpoint temperature [degC]
    /// * `ua_standby`  - Standby loss coefficient [W/K]
    pub fn new(
        name: &str,
        fuel_type: WaterHeaterFuel,
        tank_volume: f64,
        capacity: f64,
        efficiency: f64,
        setpoint: f64,
        ua_standby: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            fuel_type,
            tank_volume,
            capacity,
            efficiency,
            setpoint_temp: setpoint,
            deadband: 5.0,
            ua_standby,
            ambient_temp: 20.0,
            // Initialise tank at setpoint
            tank_temp: setpoint,
            heating_rate: 0.0,
            energy_input: 0.0,
            is_heating: false,
        }
    }

    /// Per-timestep simulation of the mixed-tank water heater.
    ///
    /// # Arguments
    /// * `draw_flow_liters_per_s` - Hot water draw rate [L/s]
    /// * `mains_temp`             - Cold (mains) water inlet temperature [degC]
    /// * `dt`                     - Timestep duration [s]
    pub fn simulate(&mut self, draw_flow_liters_per_s: f64, mains_temp: f64, dt: f64) {
        let m_tank = self.tank_volume * RHO_WATER; // tank water mass [kg]

        // --- Deadband thermostat control ------------------------------------
        if self.tank_temp < self.setpoint_temp - self.deadband {
            self.is_heating = true;
        } else if self.tank_temp >= self.setpoint_temp {
            self.is_heating = false;
        }
        // else: remain in current state (hysteresis)

        // --- Energy delivered to draw water ---------------------------------
        let m_draw = draw_flow_liters_per_s * RHO_WATER; // draw mass flow [kg/s]
        let q_delivered = m_draw * CP_WATER * (self.tank_temp - mains_temp).max(0.0);

        // --- Standby shell losses -------------------------------------------
        let q_loss = self.ua_standby * (self.tank_temp - self.ambient_temp);

        // --- Burner / element input -----------------------------------------
        // When the thermostat calls for heat the burner fires at full rated
        // input capacity.  The effective heat delivered to the tank is
        // capacity * efficiency.  This matches the EnergyPlus mixed-tank
        // approach where the burner runs at rated input whenever the
        // thermostat is calling for heat.
        let (q_input, energy_input) = if self.is_heating {
            let input = self.capacity; // full rated input [W]
            let q_to_tank = input * self.efficiency;
            (q_to_tank, input)
        } else {
            (0.0, 0.0)
        };

        // --- Tank temperature update ----------------------------------------
        // Energy balance on the mixed tank over the timestep:
        //   m_tank * Cp * dT = (Q_input_to_tank - Q_delivered - Q_loss) * dt
        if m_tank > 0.0 {
            let delta_t = (q_input - q_delivered - q_loss) * dt / (m_tank * CP_WATER);
            self.tank_temp += delta_t;
        }

        // Store runtime outputs
        self.heating_rate = q_input;
        self.energy_input = energy_input;
    }

    /// Electric power consumption [W].
    ///
    /// Returns the electric consumption for Electric and HeatPump types,
    /// and 0 for Gas.
    pub fn electric_power(&self) -> f64 {
        match self.fuel_type {
            WaterHeaterFuel::Electric | WaterHeaterFuel::HeatPump => self.energy_input,
            WaterHeaterFuel::Gas => 0.0,
        }
    }

    /// Fuel (gas) power consumption [W].
    ///
    /// Returns the gas consumption for Gas type, and 0 for Electric/HeatPump.
    pub fn fuel_power(&self) -> f64 {
        match self.fuel_type {
            WaterHeaterFuel::Gas => self.energy_input,
            _ => 0.0,
        }
    }

    /// Current average tank temperature [degC].
    pub fn tank_temperature(&self) -> f64 {
        self.tank_temp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for floating-point comparisons.
    const TOL: f64 = 0.1;

    // ---- Helpers -----------------------------------------------------------

    /// Run multiple timesteps and return final tank temperature.
    fn run_steps(wh: &mut WaterHeater, draw: f64, mains: f64, dt: f64, steps: usize) {
        for _ in 0..steps {
            wh.simulate(draw, mains, dt);
        }
    }

    // ---- Tests -------------------------------------------------------------

    #[test]
    fn test_gas_heater_maintains_setpoint_with_draw() {
        // A gas water heater with modest draw should keep the tank near
        // setpoint once steady state is reached.
        let mut wh = WaterHeater::new(
            "Gas WH",
            WaterHeaterFuel::Gas,
            200.0,    // 200 L tank
            11_720.0, // ~40,000 BTU/h gas input
            0.80,     // 80% thermal efficiency
            60.0,     // 60 degC setpoint
            2.0,      // UA 2 W/K
        );

        // Small draw: 0.05 L/s (~3 L/min, a single faucet)
        let draw = 0.05;
        let mains = 10.0;
        let dt = 60.0; // 1-minute timesteps

        // Run for 60 minutes of continuous draw
        run_steps(&mut wh, draw, mains, dt, 60);

        // Tank should stay within a reasonable band around setpoint.
        // With this capacity and draw the heater can keep up.
        assert!(
            wh.tank_temperature() > wh.setpoint_temp - wh.deadband - 2.0,
            "Tank temp {} dropped too far below setpoint",
            wh.tank_temperature()
        );

        // Gas consumption should be non-zero and electric should be zero
        assert!(wh.fuel_power() > 0.0);
        assert_eq!(wh.electric_power(), 0.0);
    }

    #[test]
    fn test_standby_losses_cool_tank() {
        // With no draw and the burner forced off (tank at setpoint so burner
        // is off), the tank should cool due to standby losses.
        let mut wh = WaterHeater::new(
            "Standby Test",
            WaterHeaterFuel::Electric,
            200.0,
            4500.0,
            1.0,
            60.0,
            5.0, // 5 W/K — moderate insulation
        );
        wh.ambient_temp = 20.0;

        let initial_temp = wh.tank_temperature();

        // Run 4 hours with no draw, 1-minute steps
        run_steps(&mut wh, 0.0, 10.0, 60.0, 240);

        // Tank should have cooled (but deadband means burner may kick in
        // partway through). At minimum the first drop should occur.
        // After losing heat for a while the tank will drop below setpoint - deadband
        // and the burner will fire. Let's just verify the tank doesn't exceed
        // the initial temperature and the heater eventually fires.
        assert!(
            wh.tank_temperature() <= initial_temp + TOL,
            "Tank should not heat above initial temp without draw"
        );

        // Let it run longer -- burner should have fired to recover
        run_steps(&mut wh, 0.0, 10.0, 60.0, 240);
        assert!(
            wh.tank_temperature() > wh.setpoint_temp - wh.deadband - 2.0,
            "Burner should recover tank from standby losses"
        );
    }

    #[test]
    fn test_recovery_from_cold_tank() {
        // Start the tank well below setpoint and verify it recovers.
        let mut wh = WaterHeater::new(
            "Recovery Test",
            WaterHeaterFuel::Gas,
            200.0,
            11_720.0,
            0.80,
            60.0,
            2.0,
        );
        // Force tank to cold temperature
        wh.tank_temp = 20.0;

        let dt = 60.0;
        // Run with no draw until recovered — should take well under 2 hours
        // for an 11.7 kW input on a 200 L tank.
        run_steps(&mut wh, 0.0, 10.0, dt, 120);

        assert!(
            wh.tank_temperature() > wh.setpoint_temp - wh.deadband,
            "Tank should recover to near setpoint; got {}",
            wh.tank_temperature()
        );
    }

    #[test]
    fn test_zero_draw_only_standby_losses() {
        // With zero draw the only load is standby losses. Verify Q_delivered
        // component is zero and tank behaviour is driven by shell losses.
        let mut wh = WaterHeater::new(
            "Zero Draw",
            WaterHeaterFuel::Electric,
            200.0,
            4500.0,
            1.0,
            60.0,
            3.0,
        );
        // Tank at setpoint, ambient 20 => loss = 3 * 40 = 120 W
        let initial_temp = wh.tank_temperature();

        // One timestep with zero draw
        wh.simulate(0.0, 10.0, 60.0);

        // Tank should cool slightly (burner is off because tank is at setpoint)
        assert!(
            wh.tank_temperature() < initial_temp,
            "Tank should cool from standby losses"
        );

        // Expected drop: dT = -Q_loss * dt / (m * Cp)
        //   = -(3.0 * 40.0) * 60.0 / (200.0 * 4186.0) = -0.00861 degC
        let expected_drop = (3.0 * 40.0) * 60.0 / (200.0 * CP_WATER);
        let actual_drop = initial_temp - wh.tank_temperature();
        assert!(
            (actual_drop - expected_drop).abs() < 0.001,
            "Drop {actual_drop} should match expected {expected_drop}"
        );
    }

    #[test]
    fn test_electric_heater_works() {
        let mut wh = WaterHeater::new(
            "Electric WH",
            WaterHeaterFuel::Electric,
            150.0,
            4500.0, // typical residential element
            1.0,    // 100% electric efficiency
            60.0,
            2.0,
        );
        wh.tank_temp = 40.0; // start cold

        // Run for 60 minutes, 1-minute steps, no draw
        run_steps(&mut wh, 0.0, 10.0, 60.0, 60);

        // Should have heated significantly
        assert!(
            wh.tank_temperature() > 50.0,
            "Electric heater should raise tank temp; got {}",
            wh.tank_temperature()
        );

        // Electric power should be non-zero, fuel should be zero
        // (may be zero on last step if at setpoint, so check mid-recovery)
        let mut wh2 = WaterHeater::new(
            "Elec Check",
            WaterHeaterFuel::Electric,
            150.0,
            4500.0,
            1.0,
            60.0,
            2.0,
        );
        wh2.tank_temp = 40.0;
        wh2.simulate(0.0, 10.0, 60.0);
        assert!(wh2.electric_power() > 0.0, "Electric power should be >0 during recovery");
        assert_eq!(wh2.fuel_power(), 0.0, "Fuel power should be 0 for electric");
    }

    #[test]
    fn test_heat_pump_heater_works() {
        let mut wh = WaterHeater::new(
            "HPWH",
            WaterHeaterFuel::HeatPump,
            200.0,
            1500.0, // 1.5 kW compressor input
            3.0,    // COP of 3.0
            60.0,
            2.0,
        );
        wh.tank_temp = 30.0; // cold start

        // Run for 2 hours, 1-minute steps, no draw
        run_steps(&mut wh, 0.0, 10.0, 60.0, 120);

        // With COP 3.0 and 1500 W input, effective heating = 4500 W.
        // Should heat 200 kg by ~40 K in about 62 minutes. After 120 min
        // should be at or near setpoint.
        assert!(
            wh.tank_temperature() > wh.setpoint_temp - wh.deadband,
            "HPWH should recover; got {}",
            wh.tank_temperature()
        );

        // Electric power reported, no fuel
        let mut wh2 = WaterHeater::new(
            "HPWH Check",
            WaterHeaterFuel::HeatPump,
            200.0,
            1500.0,
            3.0,
            60.0,
            2.0,
        );
        wh2.tank_temp = 30.0;
        wh2.simulate(0.0, 10.0, 60.0);
        assert!(wh2.electric_power() > 0.0, "HPWH should report electric power");
        assert_eq!(wh2.fuel_power(), 0.0, "HPWH should report zero fuel");
    }

    #[test]
    fn test_deadband_hysteresis() {
        // Verify the deadband prevents short-cycling:
        //   - Burner turns ON when tank_temp < setpoint - deadband
        //   - Burner stays ON until tank_temp >= setpoint
        //   - Burner stays OFF until tank_temp drops below setpoint - deadband
        let mut wh = WaterHeater::new(
            "Deadband Test",
            WaterHeaterFuel::Electric,
            200.0,
            4500.0,
            1.0,
            60.0,
            3.0,
        );
        wh.deadband = 5.0;

        // Start at setpoint -- burner should be off
        wh.tank_temp = 60.0;
        wh.is_heating = false;
        wh.simulate(0.0, 10.0, 60.0);
        assert!(!wh.is_heating, "Burner should be off at setpoint");

        // Drop tank to just above the deadband threshold (setpoint - deadband = 55)
        wh.tank_temp = 55.5;
        wh.is_heating = false;
        wh.simulate(0.0, 10.0, 60.0);
        assert!(
            !wh.is_heating,
            "Burner should stay off above setpoint - deadband"
        );

        // Drop tank below setpoint - deadband
        wh.tank_temp = 54.5;
        wh.is_heating = false;
        wh.simulate(0.0, 10.0, 60.0);
        assert!(
            wh.is_heating,
            "Burner should turn on below setpoint - deadband"
        );

        // Now set tank just below setpoint — burner should STAY on (hysteresis)
        wh.tank_temp = 59.0;
        // is_heating is already true from the previous step
        wh.simulate(0.0, 10.0, 60.0);
        assert!(
            wh.is_heating,
            "Burner should stay on between deadband and setpoint"
        );

        // At or above setpoint, burner turns off
        wh.tank_temp = 60.0;
        wh.simulate(0.0, 10.0, 60.0);
        assert!(!wh.is_heating, "Burner should turn off at setpoint");
    }

    #[test]
    fn test_energy_balance_single_step() {
        // Verify the energy balance arithmetic for one timestep.
        let mut wh = WaterHeater::new(
            "Balance Test",
            WaterHeaterFuel::Gas,
            200.0,
            11_720.0,
            0.80,
            60.0,
            2.0,
        );
        wh.tank_temp = 50.0; // below deadband threshold of 55
        wh.ambient_temp = 20.0;
        wh.is_heating = false;

        let draw = 0.0;
        let mains = 10.0;
        let dt = 60.0;

        wh.simulate(draw, mains, dt);

        // At 50 < 55 (setpoint - deadband), burner fires at full capacity
        assert!(wh.is_heating);

        // Q_delivered = 0 (no draw)
        // Q_loss = 2.0 * (50.0 - 20.0) = 60 W
        let q_loss = 2.0 * (50.0 - 20.0);
        // Burner fires at full rated capacity
        let input = 11_720.0_f64;
        assert!(
            (wh.energy_input - input).abs() < 0.01,
            "energy_input {} should equal capacity {}",
            wh.energy_input,
            input
        );

        // Q_to_tank = capacity * efficiency = 11720 * 0.80 = 9376 W
        let q_to_tank = input * 0.80;
        // dT = (9376 - 0 - 60) * 60 / (200 * 4186)
        let expected_delta = (q_to_tank - 0.0 - q_loss) * dt / (200.0 * CP_WATER);
        let actual_delta = wh.tank_temperature() - 50.0;
        assert!(
            (actual_delta - expected_delta).abs() < 1e-6,
            "dT mismatch: actual={actual_delta}, expected={expected_delta}"
        );
    }
}
