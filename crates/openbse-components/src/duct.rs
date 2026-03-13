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

/// Duct component for modeling conduction losses and air leakage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Duct {
    pub name: String,
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
    ///   - "ground": use ground temperature model
    ///   - zone name: use that zone's air temperature
    pub ambient_zone: String,

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
            length,
            diameter,
            u_value,
            leakage_fraction,
            ambient_zone: ambient_zone.to_string(),
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

    fn simulate_air(
        &mut self,
        inlet: &AirPort,
        _ctx: &SimulationContext,
    ) -> AirPort {
        let m_dot = inlet.mass_flow;

        // No flow → no losses
        if m_dot < 1e-10 {
            self.conduction_loss = 0.0;
            self.leakage_flow = 0.0;
            return *inlet;
        }

        let cp = openbse_psychrometrics::cp_air_fn_w(inlet.state.w);

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
        let outlet_state = openbse_psychrometrics::MoistAirState::new(
            t_out,
            inlet.state.w,
            inlet.state.p_b,
        );

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
}
