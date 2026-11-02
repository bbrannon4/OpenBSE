//! Duct component model with conduction heat loss and air leakage.
//!
//! Models supply and return air ducts that may pass through unconditioned
//! spaces (basements, attics, crawlspaces). Two loss mechanisms:
//!
//! 1. **Conduction**: Heat transfer between duct air and surrounding ambient
//!    using effectiveness-NTU method:
//!      ε = 1 − exp(−UA / (ṁ·cp))
//!      T_out = T_in − ε·(T_in − T_ambient)
//!
//! 2. **Leakage**: Fraction of supply air lost to the surrounding space,
//!    reducing delivered mass flow:
//!      ṁ_out = ṁ_in · (1 − leakage_fraction)
//!
//! Reference: ASHRAE Handbook—Fundamentals, Chapter 21 "Duct Design"

use openbse_core::ports::*;
use serde::{Deserialize, Serialize};

fn default_ground_temperatures() -> Vec<f64> {
    vec![18.0; 12]
}

fn default_submeter() -> String {
    "General".to_string()
}

/// Duct component for modeling conduction losses and air leakage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Duct {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Duct length [m]
    pub length: f64,
    /// Duct hydraulic diameter [m]
    pub diameter: f64,
    /// Overall U-value of duct wall [W/(m²·K)]
    /// Includes insulation. Typical values:
    ///   - Uninsulated sheet metal: ~7.0
    ///   - R-4.2 (RSI-0.74) insulated: ~1.35
    ///   - R-6 (RSI-1.06) insulated: ~0.95
    ///   - R-8 (RSI-1.41) insulated: ~0.71
    pub u_value: f64,
    /// Fraction of air mass flow lost through leaks [0-1]
    /// ASHRAE default for typical residential: 0.04-0.08
    pub leakage_fraction: f64,
    /// Name of the zone surrounding the duct, or special values:
    ///   - "outdoor": use outdoor air temperature
    ///   - "ground": use the site's monthly ground temperatures
    ///   - zone name: use that zone's air temperature
    pub ambient_zone: String,
    /// Monthly ground temperatures [°C] for `ambient_zone: ground`
    /// (`simulation.ground_surface_temperatures`).
    #[serde(default = "default_ground_temperatures")]
    pub ground_temperatures: Vec<f64>,

    // ─── Runtime state (not serialized) ─────────────────────────────────
    /// Current ambient temperature around the duct [°C]
    #[serde(skip)]
    pub ambient_temp: f64,
    /// Conduction heat loss this timestep [W] (positive = heat lost from air)
    #[serde(skip)]
    pub conduction_loss: f64,
    /// Mass flow lost through leakage this timestep [kg/s]
    #[serde(skip)]
    pub leakage_flow: f64,
}

impl Duct {
    /// Create a new duct component.
    pub fn new(
        name: &str,
        length: f64,
        diameter: f64,
        u_value: f64,
        leakage_fraction: f64,
        ambient_zone: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            length,
            diameter,
            u_value,
            leakage_fraction,
            ambient_zone: ambient_zone.to_string(),
            ground_temperatures: default_ground_temperatures(),
            ambient_temp: 20.0,
            conduction_loss: 0.0,
            leakage_flow: 0.0,
        }
    }
}

impl AirComponent for Duct {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Duct
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        let m_dot = inlet.mass_flow;

        // No flow → no losses
        if m_dot < 1e-10 {
            self.conduction_loss = 0.0;
            self.leakage_flow = 0.0;
            return *inlet;
        }

        let cp = openbse_psychrometrics::cp_air_fn_w(inlet.state.w);

        // A zone or outdoor ambient is set from outside; the ground follows the
        // month, as the envelope's ground boundary does (#235).
        if self.ambient_zone == "ground" {
            let month = (ctx.timestep.month as usize).clamp(1, 12);
            self.ambient_temp =
                self.ground_temperatures[(month - 1) % self.ground_temperatures.len()];
        }

        // ── Conduction loss via effectiveness-NTU ────────────────────────
        // Surface area of cylindrical duct
        let area = std::f64::consts::PI * self.diameter * self.length;
        let ua = self.u_value * area; // [W/K]
        let ntu = ua / (m_dot * cp);
        let effectiveness = 1.0 - (-ntu).exp();

        let t_in = inlet.state.t_db;
        let t_out = t_in - effectiveness * (t_in - self.ambient_temp);
        self.conduction_loss = m_dot * cp * (t_in - t_out);

        // ── Leakage ─────────────────────────────────────────────────────
        self.leakage_flow = m_dot * self.leakage_fraction;
        let m_dot_out = m_dot - self.leakage_flow;

        // Build outlet state: temperature changed by conduction, humidity unchanged
        let outlet_state =
            openbse_psychrometrics::MoistAirState::new(t_out, inlet.state.w, inlet.state.p_b);

        AirPort::new(outlet_state, m_dot_out)
    }

    fn thermal_output(&self) -> f64 {
        // Negative = heat lost from the airstream
        -self.conduction_loss
    }

    fn set_ambient_temp(&mut self, temp: f64) {
        self.ambient_temp = temp;
    }

    fn ambient_zone(&self) -> Option<&str> {
        Some(&self.ambient_zone)
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("conduction_loss", self.conduction_loss);
        out("leakage_loss", self.leakage_flow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

    fn ctx(month: u32) -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month,
                day: 1,
                hour: 12,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    /// #235: a ground duct follows the site's month, not a fixed 18 °C.
    #[test]
    fn ground_ambient_uses_the_month_of_the_site_table() {
        let mut duct = Duct::new("d", 10.0, 0.3, 1.0, 0.0, "ground");
        duct.ground_temperatures = (1..=12).map(|m| m as f64).collect();
        let inlet = AirPort::new(MoistAirState::new(30.0, 0.008, 101_325.0), 0.5);
        duct.simulate_air(&inlet, &ctx(7));
        assert_eq!(duct.ambient_temp, 7.0);
        duct.simulate_air(&inlet, &ctx(12));
        assert_eq!(duct.ambient_temp, 12.0);
    }
}
