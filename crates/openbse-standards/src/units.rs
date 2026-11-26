//! Exact unit conversions from a standard's printed units to SI.
//!
//! Data files keep each value exactly as the source table prints it, in the
//! table's own unit system. The engine works in SI, so values are converted on
//! load with factors built only from exact definitions:
//!
//! - 1 ft = 0.3048 m (international foot);
//! - 1 Btu (International Table) = 1055.05585262 J;
//! - 1 h = 3600 s;
//! - a temperature difference of 1 °F = 5/9 K.

use serde::{Deserialize, Serialize};

const FT: f64 = 0.3048;
const BTU_IT: f64 = 1055.05585262;
const HOUR: f64 = 3600.0;
const DEG_F: f64 = 5.0 / 9.0;

/// Btu/(h·ft²·°F) → W/(m²·K), ≈ 5.678263.
pub const U_IP_TO_SI: f64 = BTU_IT / HOUR / (FT * FT) / DEG_F;
/// Btu/(h·ft·°F) → W/(m·K), ≈ 1.730735.
pub const F_FACTOR_IP_TO_SI: f64 = BTU_IT / HOUR / FT / DEG_F;
/// W/ft² → W/m², ≈ 10.763910.
pub const LPD_IP_TO_SI: f64 = 1.0 / (FT * FT);
/// h·ft²·°F/Btu → m²·K/W, ≈ 0.176110.
pub const R_IP_TO_SI: f64 = 1.0 / U_IP_TO_SI;

/// Unit system a source table is printed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitSystem {
    /// Inch-pound edition.
    Ip,
    /// SI edition.
    Si,
}

/// Physical quantities that appear in standards tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    /// Lighting power density.
    LightingPowerDensity,
    /// Lighting power density per unit of space height.
    LpdPerHeight,
    /// Assembly U-factor (including air films) or C-factor.
    UFactor,
    /// Slab perimeter heat-loss coefficient (F-factor).
    FFactor,
    /// Thermal resistance (R-value).
    Resistance,
    /// Dimensionless (SHGC, VT, ratios, fractions).
    Dimensionless,
}

impl Quantity {
    /// Unit label for this quantity in a unit system.
    pub fn unit(self, system: UnitSystem) -> &'static str {
        use Quantity::*;
        match (self, system) {
            (LightingPowerDensity, UnitSystem::Ip) => "W/ft²",
            (LightingPowerDensity, UnitSystem::Si) => "W/m²",
            (LpdPerHeight, UnitSystem::Ip) => "W/ft² per ft",
            (LpdPerHeight, UnitSystem::Si) => "W/m² per m",
            (UFactor, UnitSystem::Ip) => "Btu/(h·ft²·°F)",
            (UFactor, UnitSystem::Si) => "W/(m²·K)",
            (FFactor, UnitSystem::Ip) => "Btu/(h·ft·°F)",
            (FFactor, UnitSystem::Si) => "W/(m·K)",
            (Resistance, UnitSystem::Ip) => "h·ft²·°F/Btu",
            (Resistance, UnitSystem::Si) => "m²·K/W",
            (Dimensionless, _) => "-",
        }
    }

    /// Convert a value printed in `system` to SI.
    pub fn to_si(self, value: f64, system: UnitSystem) -> f64 {
        if system == UnitSystem::Si {
            return value;
        }
        match self {
            Quantity::LightingPowerDensity => value * LPD_IP_TO_SI,
            Quantity::LpdPerHeight => value * LPD_IP_TO_SI / FT,
            Quantity::UFactor => value * U_IP_TO_SI,
            Quantity::FFactor => value * F_FACTOR_IP_TO_SI,
            Quantity::Resistance => value * R_IP_TO_SI,
            Quantity::Dimensionless => value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors_match_published_conversions() {
        // ASHRAE Handbook—Fundamentals, Ch. 38 conversion table (rounded values).
        assert!((U_IP_TO_SI - 5.678263).abs() < 1e-6);
        assert!((F_FACTOR_IP_TO_SI - 1.730735).abs() < 1e-6);
        assert!((LPD_IP_TO_SI - 10.763910).abs() < 1e-6);
        assert!((R_IP_TO_SI - 0.1761102).abs() < 1e-7);
        assert!((U_IP_TO_SI * R_IP_TO_SI - 1.0).abs() < 1e-15);
    }

    #[test]
    fn si_values_pass_through() {
        assert_eq!(Quantity::UFactor.to_si(0.35, UnitSystem::Si), 0.35);
        assert_eq!(Quantity::Dimensionless.to_si(0.4, UnitSystem::Ip), 0.4);
    }
}
