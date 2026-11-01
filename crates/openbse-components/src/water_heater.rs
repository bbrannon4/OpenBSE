//! Domestic hot water (DHW) storage-tank water heater component model.
//!
//! Models a mixed-tank water heater with deadband thermostat control,
//! standby shell losses, and draw-dependent energy delivery. This is a
//! standalone component (does not implement AirComponent or PlantComponent)
//! with its own `simulate` method called directly from the simulation loop.
//!
//! Physics match EnergyPlus `CalcWaterThermalTankMixed` (WaterThermalTanks.cc).
//! The tank balance
//!
//!   m_tank * Cp * dT/dt = Q_heat - m_draw * Cp * (T - T_mains) - UA * (T - T_ambient)
//!
//! is linear in T, so it is integrated in closed form between control events
//! (heater reaches setpoint, tank falls to the cut-in temperature). It stays
//! stable and conserves energy at any draw and timestep.
//!
//! On/off control: the heater fires at full output below setpoint - deadband
//! and stops at setpoint, within the step. Modulating control has no deadband:
//! full output below setpoint, then just enough to hold it.
//!
//! Heat pump water heaters follow EnergyPlus
//! Coil:WaterHeating:AirToWaterHeatPump:Pumped (WH-2 / #61): capacity and COP
//! are the rated values times curves of evaporator-inlet wet-bulb and tank
//! water temperature; the evaporator draws the condenser heat less the
//! compressor power from the surrounding air, split sensible/latent by the
//! DOE-2 bypass-factor method; below a minimum inlet air temperature the
//! compressor locks out and only the backup element heats the tank.
//!
//! Reference: EnergyPlus Engineering Reference, "Water Heaters"

use crate::performance_curve::{CurveType, PerformanceCurve};
use openbse_psychrometrics as psych;
use serde::{Deserialize, Serialize};

/// Barometric pressure for the evaporator-air psychrometrics [Pa].
const P_ATM: f64 = 101_325.0;

/// Heat pump rating point, the EnergyPlus defaults (the DOE test conditions):
/// evaporator inlet air 19.7 °C dry-bulb / 13.5 °C wet-bulb, tank water 57.5 °C.
const HP_RATED_INLET_DB: f64 = 19.7;
const HP_RATED_INLET_WB: f64 = 13.5;
const HP_RATED_WATER_TEMP: f64 = 57.5;

/// Evaporator airflow per watt of rated heating capacity [m³/s/W], the
/// EnergyPlus autocalculated value (31.25 cfm/MBH).
const HP_EVAP_FLOW_PER_W: f64 = 5.035e-5;

/// EnergyPlus `HeatPumpWaterHeater.idf` curves `HPWHHeatingCapFTemp` and
/// `HPWHHeatingCOPFTemp`: biquadratic in x = evaporator inlet wet-bulb [°C]
/// (0–40) and y = tank water temperature [°C] (20–90).
const HP_DEFAULT_CAP_COEFFS: [f64; 6] =
    [0.369827, 0.043341, -0.00023, 0.000466, 0.000026, -0.00027];
const HP_DEFAULT_COP_COEFFS: [f64; 6] =
    [1.19713, 0.077849, -0.0000016, -0.02675, 0.000296, -0.00112];

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

/// Water heater control type.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum WaterHeaterControl {
    /// On/off deadband thermostat (default for storage tanks).
    /// Burner fires at full rated capacity when tank temp drops below
    /// setpoint - deadband, turns off at setpoint.
    #[default]
    OnOff,
    /// Modulating control (matches E+ "Modulate" heater control type).
    /// Burner modulates output to exactly match the instantaneous load
    /// (draw + standby losses), capped at rated capacity.  Ideal for
    /// tankless / instantaneous water heaters.
    Modulate,
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
    /// Constant parasitic fuel/electricity consumption [W].
    ///
    /// This represents continuous fuel draw that does NOT heat the tank
    /// (e.g., pilot light, jacket heaters, controls). It is added to
    /// fuel_power/electric_power output regardless of burner on/off state.
    /// Matches the EnergyPlus Off-Cycle and On-Cycle Parasitic fields
    /// when both have the same value and zero heat fraction to tank.
    pub parasitic_power: f64,
    /// Heater control type (OnOff or Modulate). Default: OnOff.
    pub control_type: WaterHeaterControl,
    /// Humidity ratio of the air around the tank [kg/kg], the heat pump's
    /// evaporator inlet. Default: 50% RH at 20 °C.
    pub ambient_w: f64,
    /// Heat pump: heating-capacity modifier f(evaporator inlet wet-bulb [°C],
    /// tank water [°C]), 1.0 at the rating point.
    pub hp_capacity_curve: PerformanceCurve,
    /// Heat pump: COP modifier, same inputs and normalisation.
    pub hp_cop_curve: PerformanceCurve,
    /// Heat pump: evaporator sensible heat ratio at the rating point.
    pub hp_rated_shr: f64,
    /// Heat pump: the compressor is locked out below this evaporator inlet
    /// air dry-bulb [°C].
    pub hp_min_inlet_temp: f64,
    /// Heat pump: backup resistance element [W], the only heat source while
    /// the compressor is locked out.
    pub backup_capacity: f64,

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
    /// Tank shell loss to the surrounding air this step [W].
    #[serde(skip)]
    pub standby_loss: f64,
    /// Heat leaving in the draw, relative to mains, this step [W].
    #[serde(skip)]
    pub delivery_rate: f64,
    /// Sensible heat the heat pump's evaporator drew from the surrounding air [W].
    #[serde(skip)]
    pub evaporator_sensible: f64,
    /// Latent heat (condensed moisture) the evaporator drew from the air [W].
    #[serde(skip)]
    pub evaporator_latent: f64,
    /// Evaporator bypass factor, derived once from the rated SHR.
    #[serde(skip)]
    hp_bypass_factor: Option<f64>,
}

/// Most heating and floating intervals resolved in one step; generous, since
/// each pair needs a full swing across the deadband.
const MAX_TANK_INTERVALS: usize = 1000;

/// The mixed tank's linear heat balance over one step, solved in closed form:
/// `T(t) = T_eq(Q) + (T_0 - T_eq(Q)) exp(-k t)`, as E+ `CalcTankTemp`,
/// `CalcTimeNeeded` and `CalcTempIntegral`.
struct TankBalance {
    /// Tank water mass [kg].
    mass: f64,
    /// Water specific heat [J/(kg K)].
    cp: f64,
    /// Draw mass flow [kg/s].
    m_draw: f64,
    t_mains: f64,
    /// Shell loss coefficient [W/K].
    ua: f64,
    t_ambient: f64,
}

impl TankBalance {
    /// Total conductance to the mains and ambient [W/K].
    fn conductance(&self) -> f64 {
        self.m_draw * self.cp + self.ua
    }

    /// Time-constant inverse [1/s].
    fn k(&self) -> f64 {
        self.conductance() / (self.mass * self.cp)
    }

    /// Draw and shell loss at tank temperature `t` [W].
    fn load_at(&self, t: f64) -> f64 {
        self.m_draw * self.cp * (t - self.t_mains) + self.ua * (t - self.t_ambient)
    }

    /// Temperature the tank tends to with heat input `q` [degC].
    fn equilibrium(&self, q: f64) -> f64 {
        (q + self.m_draw * self.cp * self.t_mains + self.ua * self.t_ambient) / self.conductance()
    }

    fn rate(&self, t: f64, q: f64) -> f64 {
        (q - self.load_at(t)) / (self.mass * self.cp)
    }

    fn temperature_after(&self, t0: f64, q: f64, time: f64) -> f64 {
        if self.mass <= 0.0 {
            return if self.conductance() > 0.0 {
                self.equilibrium(q)
            } else {
                t0
            };
        }
        if self.conductance() <= 0.0 {
            return t0 + q * time / (self.mass * self.cp);
        }
        let t_eq = self.equilibrium(q);
        t_eq + (t0 - t_eq) * (-self.k() * time).exp()
    }

    /// Time to go from `t0` to `target`, or infinity if the tank never gets there.
    fn time_to(&self, t0: f64, target: f64, q: f64) -> f64 {
        if self.mass <= 0.0 {
            return if self.conductance() > 0.0 {
                0.0
            } else {
                f64::INFINITY
            };
        }
        let rate = self.rate(t0, q);
        if (target - t0) * rate <= 0.0 {
            return f64::INFINITY;
        }
        if self.conductance() <= 0.0 {
            return (target - t0) / rate;
        }
        let t_eq = self.equilibrium(q);
        let ratio = (target - t_eq) / (t0 - t_eq);
        if ratio <= 0.0 {
            f64::INFINITY
        } else {
            -ratio.ln() / self.k()
        }
    }

    /// Integral of T over `time`, from `t0` to `t1` [K s].
    fn temperature_integral(&self, t0: f64, t1: f64, q: f64, time: f64) -> f64 {
        if t0 == t1 {
            return t0 * time;
        }
        if self.conductance() <= 0.0 || self.mass <= 0.0 {
            return 0.5 * (t0 + t1) * time;
        }
        // From the balance itself: m Cp (t1 - t0) = q time - G (integral - T_eq(0) time).
        let t_eq0 = self.equilibrium(0.0);
        t_eq0 * time + (q * time - self.mass * self.cp * (t1 - t0)) / self.conductance()
    }
}

/// Heat available to the tank this step.
struct HeatSource {
    /// Heat to the tank at full output [W].
    q_max: f64,
    /// Energy input at full output [W].
    input_max: f64,
    /// Sensible fraction of the evaporator heat; `None` when no compressor runs.
    evaporator_shr: Option<f64>,
}

/// An EnergyPlus HPWH curve rescaled to 1.0 at the rating point, so the
/// heater's `efficiency` stays its rated COP.
fn normalised_hp_curve(name: &str, coefficients: [f64; 6]) -> PerformanceCurve {
    let curve = |coefficients: Vec<f64>| PerformanceCurve::Polynomial {
        name: name.to_string(),
        curve_type: CurveType::Biquadratic,
        coefficients,
        min_x: 0.0,
        max_x: 40.0,
        min_y: 20.0,
        max_y: 90.0,
        min_output: None,
        max_output: None,
    };
    let at_rating = curve(coefficients.to_vec()).evaluate(HP_RATED_INLET_WB, HP_RATED_WATER_TEMP);
    curve(coefficients.iter().map(|c| c / at_rating).collect())
}

/// Default heat pump capacity modifier (EnergyPlus `HPWHHeatingCapFTemp`).
fn default_hp_capacity_curve() -> PerformanceCurve {
    normalised_hp_curve("default_hpwh_cap_ft", HP_DEFAULT_CAP_COEFFS)
}

/// Default heat pump COP modifier (EnergyPlus `HPWHHeatingCOPFTemp`).
fn default_hp_cop_curve() -> PerformanceCurve {
    normalised_hp_curve("default_hpwh_cop_ft", HP_DEFAULT_COP_COEFFS)
}

/// Evaporator bypass factor from rated conditions (EnergyPlus `CalcCBF`): the
/// rated load and SHR fix the outlet state, and the line from inlet through
/// outlet meets the saturation curve at the apparatus dew point.
fn rated_bypass_factor(q_evap: f64, m_air: f64, shr: f64) -> f64 {
    const FALLBACK: f64 = 0.2;
    if q_evap <= 0.0 || m_air <= 0.0 {
        return FALLBACK;
    }
    let (t_in, w_in) = (
        HP_RATED_INLET_DB,
        psych::w_fn_tdb_twb_pb(HP_RATED_INLET_DB, HP_RATED_INLET_WB, P_ATM),
    );
    let h_in = psych::h_fn_tdb_w(t_in, w_in);
    let dh = q_evap / m_air;
    let w_out = psych::w_fn_tdb_h(t_in, h_in - (1.0 - shr.clamp(0.5, 1.0)) * dh);
    let h_out = h_in - dh;
    let t_out = psych::tdb_fn_h_w(h_out, w_out);
    let point = |s: f64| (t_in + s * (t_out - t_in), w_in + s * (w_out - w_in));
    let supersaturated = |s: f64| {
        let (t, w) = point(s);
        w >= psych::w_fn_tdb_rh_pb(t, 1.0, P_ATM)
    };
    let (mut lo, mut hi) = (1.0_f64, 2.0_f64);
    while !supersaturated(hi) {
        if hi > 1.0e3 {
            return FALLBACK;
        }
        (lo, hi) = (hi, 2.0 * hi);
    }
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if supersaturated(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let (t_adp, w_adp) = point(hi);
    let h_adp = psych::h_fn_tdb_w(t_adp, w_adp);
    ((h_out - h_adp) / (h_in - h_adp)).clamp(0.0, 0.9)
}

/// Sensible heat ratio of the evaporator at the current inlet air (EnergyPlus
/// `CalcDoe2DXCoil`): the bypass factor places the apparatus dew point, and a
/// dew point above the inlet humidity means a dry coil.
fn evaporator_shr(t_in: f64, w_in: f64, q_evap: f64, m_air: f64, bypass_factor: f64) -> f64 {
    if q_evap <= 0.0 || m_air <= 0.0 {
        return 1.0;
    }
    let h_in = psych::h_fn_tdb_w(t_in, w_in);
    let h_adp = h_in - q_evap / m_air / (1.0 - bypass_factor);
    let t_adp = psych::tsat_fn_h_pb(h_adp, P_ATM);
    let w_adp = psych::w_fn_tdb_h(t_adp, h_adp);
    if w_adp >= w_in {
        return 1.0;
    }
    ((psych::h_fn_tdb_w(t_in, w_adp) - h_adp) / (h_in - h_adp)).clamp(0.0, 1.0)
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
            parasitic_power: 0.0,
            control_type: WaterHeaterControl::OnOff,
            ambient_w: psych::w_fn_tdb_rh_pb(20.0, 0.5, P_ATM),
            hp_capacity_curve: default_hp_capacity_curve(),
            hp_cop_curve: default_hp_cop_curve(),
            // EnergyPlus defaults for the HPWH coil and compressor cut-out.
            hp_rated_shr: 0.85,
            hp_min_inlet_temp: 10.0,
            backup_capacity: 0.0,
            // Initialise tank at setpoint
            tank_temp: setpoint,
            heating_rate: 0.0,
            energy_input: 0.0,
            is_heating: false,
            standby_loss: 0.0,
            delivery_rate: 0.0,
            evaporator_sensible: 0.0,
            evaporator_latent: 0.0,
            hp_bypass_factor: None,
        }
    }

    /// Heat pump evaporator air mass flow [kg/s] (single-speed fan at the
    /// EnergyPlus autocalculated rated flow).
    fn evaporator_mass_flow(&self) -> f64 {
        let rated_heating = self.capacity * self.efficiency;
        HP_EVAP_FLOW_PER_W
            * rated_heating
            * psych::rho_air_fn_pb_tdb_w(P_ATM, self.ambient_temp, self.ambient_w)
    }

    /// The heat source for this step: the rated burner or element, or the heat
    /// pump at the current evaporator air and tank temperatures.
    fn heat_source(&mut self) -> HeatSource {
        if self.fuel_type != WaterHeaterFuel::HeatPump {
            return HeatSource {
                q_max: self.capacity * self.efficiency,
                input_max: self.capacity,
                evaporator_shr: None,
            };
        }
        if self.ambient_temp < self.hp_min_inlet_temp {
            return HeatSource {
                q_max: self.backup_capacity,
                input_max: self.backup_capacity,
                evaporator_shr: None,
            };
        }
        let t_wb = psych::twb_fn_tdb_w_pb(self.ambient_temp, self.ambient_w, P_ATM);
        let cap_mod = self
            .hp_capacity_curve
            .evaluate(t_wb, self.tank_temp)
            .max(0.0);
        let cop = (self.efficiency * self.hp_cop_curve.evaluate(t_wb, self.tank_temp)).max(1.0);
        let q_max = self.capacity * self.efficiency * cap_mod;

        let m_air = self.evaporator_mass_flow();
        let bypass_factor = *self.hp_bypass_factor.get_or_insert_with(|| {
            let rated_heating = self.capacity * self.efficiency;
            let rated_evap = rated_heating * (1.0 - 1.0 / self.efficiency.max(1.0));
            let rated_m_air = HP_EVAP_FLOW_PER_W
                * rated_heating
                * psych::rho_air_fn_pb_tdb_w(
                    P_ATM,
                    HP_RATED_INLET_DB,
                    psych::w_fn_tdb_twb_pb(HP_RATED_INLET_DB, HP_RATED_INLET_WB, P_ATM),
                );
            rated_bypass_factor(rated_evap, rated_m_air, self.hp_rated_shr)
        });
        let q_evap = q_max * (1.0 - 1.0 / cop);
        HeatSource {
            q_max,
            input_max: q_max / cop,
            evaporator_shr: Some(evaporator_shr(
                self.ambient_temp,
                self.ambient_w,
                q_evap,
                m_air,
                bypass_factor,
            )),
        }
    }

    /// Per-timestep simulation of the mixed-tank water heater.
    ///
    /// Walks the step from control event to control event as E+
    /// `CalcWaterThermalTankMixed` does. The heat pump's output is held at its
    /// start-of-step value: its curves vary little over one step.
    ///
    /// # Arguments
    /// * `draw_flow_liters_per_s` - Hot water draw rate [L/s]
    /// * `mains_temp`             - Cold (mains) water inlet temperature [degC]
    /// * `dt`                     - Timestep duration [s]
    pub fn simulate(&mut self, draw_flow_liters_per_s: f64, mains_temp: f64, dt: f64) {
        let source = self.heat_source();
        let tank = TankBalance {
            // E+ evaluates these at the tank temperature, and the draw's mass at
            // its fixed conversion temperature (`calcH2ODensity`).
            mass: self.tank_volume / 1000.0 * psych::water_rho(self.tank_temp),
            cp: psych::water_cp(self.tank_temp),
            m_draw: draw_flow_liters_per_s / 1000.0
                * psych::water_rho(psych::COLD_WATER_CONVERSION_TEMP),
            t_mains: mains_temp,
            ua: self.ua_standby,
            t_ambient: self.ambient_temp,
        };
        let (min_output, cut_in) = match self.control_type {
            // E+ sets a cycling heater's minimum capacity to its maximum: it
            // cannot hold the setpoint at part output, so it stops there. With
            // no deadband, cycling averages out to holding the setpoint.
            WaterHeaterControl::OnOff if self.deadband > 0.0 => {
                (source.q_max, self.setpoint_temp - self.deadband)
            }
            _ => (0.0, self.setpoint_temp),
        };

        let t_start = self.tank_temp;
        let mut t = t_start;
        let mut remaining = dt;
        let mut integral_t = 0.0; // integral of T over the step [K s]
        let mut heater_energy = 0.0; // heat added to the tank [J]
        let mut intervals = 0;

        while remaining > 0.0 {
            let mut holding = false;
            let (q, target) = if self.is_heating {
                if t > self.setpoint_temp {
                    self.is_heating = false;
                    continue;
                } else if t < self.setpoint_temp {
                    (source.q_max, Some(self.setpoint_temp))
                } else {
                    let q_hold = tank.load_at(t);
                    if q_hold <= 0.0 || q_hold < min_output {
                        self.is_heating = false;
                        continue;
                    }
                    holding = q_hold <= source.q_max;
                    (q_hold.min(source.q_max), None)
                }
            } else if t < cut_in || (t == cut_in && tank.load_at(t) > 0.0) {
                self.is_heating = true;
                continue;
            } else {
                (0.0, Some(cut_in))
            };

            // Past the budget the deadband is too narrow to resolve: finish the
            // step in the current state, which still closes energy.
            intervals += 1;
            let time_to_target = target
                .filter(|_| intervals < MAX_TANK_INTERVALS)
                .map_or(f64::INFINITY, |target| tank.time_to(t, target, q));
            let reached = time_to_target <= remaining;
            let step = time_to_target.min(remaining);
            let t_new = match target {
                Some(target) if reached => target,
                _ if holding => t,
                _ => tank.temperature_after(t, q, step),
            };
            integral_t += tank.temperature_integral(t, t_new, q, step);
            heater_energy += q * step;
            if reached && !self.is_heating {
                self.is_heating = true;
            }
            t = t_new;
            remaining -= step;
        }
        self.tank_temp = t;

        // Step means: E+ reports Quse and Qloss from the same temperature
        // integral, so the tank's energy closes exactly.
        let t_mean = if dt > 0.0 { integral_t / dt } else { t_start };
        let q_input = if dt > 0.0 { heater_energy / dt } else { 0.0 };
        let input_per_heat = if source.q_max > 0.0 {
            source.input_max / source.q_max
        } else {
            0.0
        };
        let energy_input = q_input * input_per_heat;

        self.heating_rate = q_input;
        self.delivery_rate = tank.m_draw * tank.cp * (t_mean - mains_temp);
        // Include constant parasitic draw (pilot light, controls, etc.)
        // Parasitic does NOT heat the tank — it is pure fuel waste.
        self.energy_input = energy_input + self.parasitic_power;
        self.standby_loss = self.ua_standby * (t_mean - self.ambient_temp);

        // The evaporator supplies the condenser heat the compressor does not.
        let q_evap = (q_input - energy_input).max(0.0);
        (self.evaporator_sensible, self.evaporator_latent) = match source.evaporator_shr {
            Some(shr) => (shr * q_evap, (1.0 - shr) * q_evap),
            None => (0.0, 0.0),
        };
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

    /// Heat capacity of a tank at temperature `t` [J/K].
    fn tank_capacity(litres: f64, t: f64) -> f64 {
        litres / 1000.0 * psych::water_rho(t) * psych::water_cp(t)
    }

    /// Heat capacity rate of a draw from a tank at temperature `t` [W/K].
    fn draw_capacity(litres_per_s: f64, t: f64) -> f64 {
        litres_per_s / 1000.0
            * psych::water_rho(psych::COLD_WATER_CONVERSION_TEMP)
            * psych::water_cp(t)
    }

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
        let expected_drop = (3.0 * 40.0) * 60.0 / tank_capacity(200.0, 60.0);
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
        assert!(
            wh2.electric_power() > 0.0,
            "Electric power should be >0 during recovery"
        );
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
        assert!(
            wh2.electric_power() > 0.0,
            "HPWH should report electric power"
        );
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
    fn test_within_step_cycling_no_overshoot() {
        // WH-1 (#61): on a coarse timestep, full-capacity firing must not drive
        // the tank far past the setpoint — the burner cycles off mid-step.
        let mut wh = WaterHeater::new(
            "Cycling Test",
            WaterHeaterFuel::Gas,
            200.0,
            11_720.0,
            0.80,
            60.0,
            2.0,
        );
        wh.ambient_temp = 20.0;
        wh.tank_temp = 58.0; // just below setpoint
        wh.is_heating = true; // already firing

        // One full HOUR timestep. Full firing would overshoot to ~98 °C.
        wh.simulate(0.0, 10.0, 3600.0);

        // Tank lands at/just below the setpoint, not far past it.
        assert!(
            wh.tank_temperature() <= 60.0 + 1e-6 && wh.tank_temperature() > 59.0,
            "tank should settle near setpoint, got {}",
            wh.tank_temperature()
        );
        // Element cycled off after reaching the setpoint.
        assert!(!wh.is_heating, "burner should cycle off at setpoint");
        // Averaged firing energy is far below full capacity (small on-fraction).
        assert!(
            wh.fuel_power() > 0.0 && wh.fuel_power() < 2000.0,
            "firing energy should reflect a small on-fraction, got {}",
            wh.fuel_power()
        );
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
        // Closed form: the shell loss rises as the tank warms, so the increase is
        // a little below the start-of-step estimate.
        let k = 2.0 / tank_capacity(200.0, 50.0);
        let t_eq = 20.0 + q_to_tank / 2.0;
        let expected = t_eq + (50.0 - t_eq) * (-k * dt).exp();
        assert!(
            (wh.tank_temperature() - expected).abs() < 1e-9,
            "T mismatch: actual={}, expected={expected}",
            wh.tank_temperature()
        );
        let euler_delta = (q_to_tank - q_loss) * dt / tank_capacity(200.0, 50.0);
        assert!((wh.tank_temperature() - 50.0 - euler_delta).abs() < 1e-3);
    }

    /// Energy stored in the tank equals heat in less draw and shell loss.
    fn assert_energy_closes(wh: &WaterHeater, t0: f64, dt: f64) {
        let stored = tank_capacity(wh.tank_volume, t0) * (wh.tank_temp - t0);
        let net = (wh.heating_rate - wh.delivery_rate - wh.standby_loss) * dt;
        assert!(
            (stored - net).abs() < 1e-5 * stored.abs().max(1e4),
            "stored {stored} J vs net {net} J"
        );
    }

    #[test]
    fn test_large_draw_stays_stable_and_closes_energy() {
        // E+ `CalcTankTemp` is stable where the explicit form was not: 1 L/s
        // from a 200 L tank for an hour is 18 tank volumes (#258).
        for control in [WaterHeaterControl::OnOff, WaterHeaterControl::Modulate] {
            let mut wh = WaterHeater::new("T", WaterHeaterFuel::Gas, 200.0, 5000.0, 0.8, 60.0, 2.0);
            wh.control_type = control;
            for _ in 0..24 {
                let t0 = wh.tank_temp;
                wh.simulate(1.0, 10.0, 3600.0);
                assert!(
                    (10.0..=60.0).contains(&wh.tank_temp),
                    "{control:?}: tank {}",
                    wh.tank_temp
                );
                assert_energy_closes(&wh, t0, 3600.0);
            }
            // Settles where the heater's output balances the draw.
            let c = draw_capacity(1.0, wh.tank_temp);
            let t_eq = (5000.0 * 0.8 + c * 10.0 + 2.0 * 20.0) / (c + 2.0);
            assert!((wh.tank_temp - t_eq).abs() < 0.01, "{}", wh.tank_temp);
        }
    }

    #[test]
    fn test_one_step_matches_many_small_steps() {
        // The closed form doesn't depend on the timestep, control events included,
        // apart from water properties being held at each step's start temperature.
        let run = |steps: usize| {
            let mut wh =
                WaterHeater::new("T", WaterHeaterFuel::Gas, 200.0, 10_000.0, 0.8, 60.0, 2.0);
            let mut heat = 0.0;
            let dt = 3600.0 / steps as f64;
            for _ in 0..steps {
                wh.simulate(0.05, 10.0, dt);
                heat += wh.energy_input * dt;
            }
            (wh.tank_temp, heat, wh.is_heating)
        };
        let (t1, e1, on1) = run(1);
        let (t2, e2, on2) = run(3600);
        assert!((t1 - t2).abs() < 0.05, "{t1} vs {t2}");
        assert!((e1 - e2).abs() < 2e-3 * e2.max(1.0), "{e1} vs {e2}");
        assert_eq!(on1, on2);
    }

    #[test]
    fn test_modulating_heater_recovers_and_holds_setpoint() {
        // Modulate used to match the load at the current temperature, so a tank
        // left below setpoint never recovered.
        let mut wh = WaterHeater::new("T", WaterHeaterFuel::Gas, 100.0, 20_000.0, 0.8, 60.0, 2.0);
        wh.control_type = WaterHeaterControl::Modulate;
        wh.tank_temp = 40.0;
        wh.simulate(0.0, 10.0, 3600.0);
        assert_eq!(wh.tank_temp, 60.0);
        // Then it holds the setpoint at part output, with no deadband droop.
        wh.simulate(0.02, 10.0, 600.0);
        assert_eq!(wh.tank_temp, 60.0);
        let load = draw_capacity(0.02, 60.0) * 50.0 + 2.0 * 40.0;
        assert!((wh.heating_rate - load).abs() < 1e-9);
        assert!((wh.energy_input - load / 0.8).abs() < 1e-9);
    }

    #[test]
    fn test_tank_below_mains_warms_from_the_draw() {
        let mut wh = WaterHeater::new("T", WaterHeaterFuel::Gas, 100.0, 0.0, 0.8, 60.0, 0.0);
        wh.tank_temp = 5.0;
        wh.simulate(0.1, 10.0, 600.0);
        assert!(wh.tank_temp > 5.0 && wh.tank_temp < 10.0);
        assert!(wh.delivery_rate < 0.0);
    }

    fn hpwh() -> WaterHeater {
        // 1.2 kW compressor input, rated COP 3.0 → 3.6 kW to the tank.
        let mut wh = WaterHeater::new(
            "HPWH",
            WaterHeaterFuel::HeatPump,
            200.0,
            1200.0,
            3.0,
            60.0,
            2.0,
        );
        wh.ambient_temp = HP_RATED_INLET_DB;
        wh.ambient_w = psych::w_fn_tdb_twb_pb(HP_RATED_INLET_DB, HP_RATED_INLET_WB, P_ATM);
        wh.tank_temp = HP_RATED_WATER_TEMP;
        wh
    }

    fn cop(wh: &mut WaterHeater) -> f64 {
        let s = wh.heat_source();
        s.q_max / s.input_max
    }

    #[test]
    fn test_heat_pump_cop_follows_air_and_tank_temperature() {
        // WH-2 (#61): rated COP and capacity at the rating point, COP falling as
        // the tank heats up and rising with warmer evaporator air.
        let mut wh = hpwh();
        let s = wh.heat_source();
        // (to the wet-bulb solver's round-trip precision)
        assert!((s.q_max / 3600.0 - 1.0).abs() < 1e-6, "{}", s.q_max);
        assert!((cop(&mut wh) / 3.0 - 1.0).abs() < 1e-6);

        wh.tank_temp = 40.0;
        let cop_cool_tank = cop(&mut wh);
        wh.tank_temp = 65.0;
        let cop_hot_tank = cop(&mut wh);
        assert!(
            cop_cool_tank > 3.0 && cop_hot_tank < 3.0,
            "{cop_cool_tank} {cop_hot_tank}"
        );

        wh.tank_temp = HP_RATED_WATER_TEMP;
        wh.ambient_temp = 27.0;
        wh.ambient_w = psych::w_fn_tdb_rh_pb(27.0, 0.5, P_ATM);
        assert!(cop(&mut wh) > 3.0);
    }

    #[test]
    fn test_heat_pump_locks_out_on_cold_air_and_uses_backup() {
        // Below the minimum inlet air temperature (10 °C, E+ default) only the
        // backup element heats the tank, at an efficiency of 1, and nothing is
        // drawn from the air.
        let mut wh = hpwh();
        wh.backup_capacity = 4500.0;
        wh.ambient_temp = 5.0;
        wh.ambient_w = psych::w_fn_tdb_rh_pb(5.0, 0.6, P_ATM);
        wh.tank_temp = 45.0;
        wh.simulate(0.0, 10.0, 60.0);
        assert!((wh.electric_power() - 4500.0).abs() < 1e-9);
        assert!((wh.heating_rate - 4500.0).abs() < 1e-9);
        assert_eq!((wh.evaporator_sensible, wh.evaporator_latent), (0.0, 0.0));
    }

    #[test]
    fn test_heat_pump_evaporator_cools_and_dries_the_air() {
        // Heat drawn from the air is the tank heat less the compressor power.
        // Humid air gives a wet coil (latent > 0); dry air a dry one.
        let mut wh = hpwh();
        wh.tank_temp = 45.0;
        wh.simulate(0.0, 10.0, 60.0);
        let q_evap = wh.heating_rate - wh.electric_power();
        assert!(q_evap > 0.0);
        assert!((wh.evaporator_sensible + wh.evaporator_latent - q_evap).abs() < 1e-6);
        assert!(
            wh.evaporator_latent > 0.0,
            "rated inlet air should condense"
        );
        let shr = wh.evaporator_sensible / q_evap;
        assert!(shr > 0.6 && shr < 1.0, "{shr}");

        let mut dry = hpwh();
        dry.tank_temp = 45.0;
        dry.ambient_w = psych::w_fn_tdb_rh_pb(HP_RATED_INLET_DB, 0.15, P_ATM);
        dry.simulate(0.0, 10.0, 60.0);
        assert_eq!(dry.evaporator_latent, 0.0);
        assert!(dry.evaporator_sensible > 0.0);
    }

    #[test]
    fn test_standby_loss_is_reported() {
        let mut wh = WaterHeater::new(
            "Tank",
            WaterHeaterFuel::Gas,
            200.0,
            10_000.0,
            0.8,
            60.0,
            2.0,
        );
        wh.ambient_temp = 15.0;
        wh.simulate(0.0, 10.0, 60.0);
        // The step mean: the tank barely cools in a minute.
        assert!((wh.standby_loss - 2.0 * (60.0 - 15.0)).abs() < 0.01);
    }
}
