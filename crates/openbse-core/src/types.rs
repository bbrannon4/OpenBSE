//! Common types used throughout the simulation engine.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Unique identifier for a component in the simulation graph.
pub type ComponentId = usize;

/// Unique identifier for a node (internal connection point) in the graph.
pub type NodeId = usize;

/// Simulation timestep information.
#[derive(Debug, Clone, Copy)]
pub struct TimeStep {
    /// Month [1-12]
    pub month: u32,
    /// Day of month [1-31]
    pub day: u32,
    /// Hour of day [1-24]
    pub hour: u32,
    /// Timestep within the hour [1..n]
    pub sub_hour: u32,
    /// Number of timesteps per hour
    pub timesteps_per_hour: u32,
    /// Simulation time in seconds from start of year
    pub sim_time_s: f64,
    /// Duration of this timestep [seconds]
    pub dt: f64,
}

impl TimeStep {
    /// Day of year (1-based).
    pub fn day_of_year(&self) -> u32 {
        let days_in_months = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut doy: u32 = 0;
        for m in 0..(self.month - 1) as usize {
            doy += days_in_months[m];
        }
        doy + self.day
    }

    /// Fractional hour (e.g. 14.5 for 2:30 PM).
    pub fn fractional_hour(&self) -> f64 {
        self.hour as f64
            - 1.0
            + (self.sub_hour as f64 - 0.5) / self.timesteps_per_hour as f64
    }
}

/// What kind of simulation day is this?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DayType {
    DesignDay,
    WeatherDay,
    SizingDay,
}

/// Autosizing sentinel value used internally by component models.
/// YAML inputs accept the `autosize` string, which maps to this constant.
pub const AUTOSIZE: f64 = -99999.0;

/// Check if a value is marked for autosizing.
pub fn is_autosize(val: f64) -> bool {
    (val - AUTOSIZE).abs() < 1.0
}

/// A value that can be either a numeric f64 or the string "autosize".
///
/// In YAML, users write either a number or the literal string `autosize`:
/// ```yaml
/// capacity: autosize
/// capacity: 10000.0
/// ```
///
/// Internally, `Autosize` maps to the sentinel `AUTOSIZE` (-99999.0) which
/// component code checks via `is_autosize()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutosizeValue {
    /// A concrete numeric value.
    Value(f64),
    /// Marked for autosizing — the engine will calculate this.
    Autosize,
}

impl AutosizeValue {
    /// Get the f64 value. Returns `AUTOSIZE` sentinel for `Autosize` variant.
    pub fn to_f64(self) -> f64 {
        match self {
            AutosizeValue::Value(v) => v,
            AutosizeValue::Autosize => AUTOSIZE,
        }
    }

    /// Check if this value is marked for autosizing.
    pub fn is_autosize(self) -> bool {
        matches!(self, AutosizeValue::Autosize)
    }
}

impl Default for AutosizeValue {
    fn default() -> Self {
        AutosizeValue::Value(0.0)
    }
}

impl From<f64> for AutosizeValue {
    fn from(v: f64) -> Self {
        if is_autosize(v) {
            AutosizeValue::Autosize
        } else {
            AutosizeValue::Value(v)
        }
    }
}

impl Serialize for AutosizeValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AutosizeValue::Value(v) => serializer.serialize_f64(*v),
            AutosizeValue::Autosize => serializer.serialize_str("autosize"),
        }
    }
}

impl<'de> Deserialize<'de> for AutosizeValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{self, Visitor};
        use std::fmt;

        struct AutosizeVisitor;

        impl<'de> Visitor<'de> for AutosizeVisitor {
            type Value = AutosizeValue;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a number or the string 'autosize'")
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<AutosizeValue, E> {
                // Interpret -99999 sentinel as autosize
                if is_autosize(v) {
                    Ok(AutosizeValue::Autosize)
                } else {
                    Ok(AutosizeValue::Value(v))
                }
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<AutosizeValue, E> {
                self.visit_f64(v as f64)
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<AutosizeValue, E> {
                self.visit_f64(v as f64)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<AutosizeValue, E> {
                if v.eq_ignore_ascii_case("autosize") {
                    Ok(AutosizeValue::Autosize)
                } else {
                    // Try parsing as a number (e.g. "10000.0" as a string)
                    v.parse::<f64>()
                        .map(AutosizeValue::Value)
                        .map_err(|_| de::Error::invalid_value(
                            de::Unexpected::Str(v),
                            &"a number or 'autosize'",
                        ))
                }
            }
        }

        deserializer.deserialize_any(AutosizeVisitor)
    }
}
