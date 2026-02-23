//! Named schedule system for time-varying inputs.
//!
//! Schedules define fractional multipliers (0.0–1.0) that vary by time of day
//! and day type (weekday, weekend/holiday). They are referenced by name from
//! internal gains, exhaust fans, outdoor air, etc.
//!
//! Example YAML:
//! ```yaml
//! schedules:
//!   - name: Retail Occupancy
//!     weekday:  [0,0,0,0,0,0,0,0.1,0.5,0.9,1.0,1.0,0.8,1.0,1.0,1.0,1.0,1.0,0.8,0.5,0.2,0,0,0]
//!     weekend:  [0,0,0,0,0,0,0,0,0,0.3,0.5,0.7,0.7,0.7,0.7,0.5,0.3,0.1,0,0,0,0,0,0]
//!     holiday:  [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
//! ```

use serde::{Deserialize, Serialize};

/// A named schedule with hourly fractional values for different day types.
///
/// Day-type priority: `saturday` > `weekend` > `weekday` for Saturdays,
/// `sunday` > `weekend` > `weekday` for Sundays, `holiday` > `sunday` for holidays.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleInput {
    pub name: String,
    /// Hourly fractions for weekdays (Mon–Fri), length 24. Index 0 = hour 1 (00:00–01:00).
    #[serde(default = "default_always_on")]
    pub weekday: Vec<f64>,
    /// Hourly fractions for weekends (Sat–Sun). Defaults to weekday if not specified.
    /// Used as fallback for Saturday and Sunday when specific profiles are not provided.
    #[serde(default)]
    pub weekend: Option<Vec<f64>>,
    /// Hourly fractions for Saturdays only. Falls back to `weekend`, then `weekday`.
    #[serde(default)]
    pub saturday: Option<Vec<f64>>,
    /// Hourly fractions for Sundays only. Falls back to `weekend`, then `weekday`.
    #[serde(default)]
    pub sunday: Option<Vec<f64>>,
    /// Hourly fractions for holidays. Falls back to `sunday`, then `weekend`, then `weekday`.
    #[serde(default)]
    pub holiday: Option<Vec<f64>>,
}

fn default_always_on() -> Vec<f64> {
    vec![1.0; 24]
}

impl ScheduleInput {
    /// Get the schedule fraction for a given hour (1-24) and day of week (1=Mon, 7=Sun).
    ///
    /// Hour is 1-indexed (1 = midnight to 1am, 24 = 11pm to midnight).
    /// Day of week: 1=Monday, 2=Tuesday, ..., 6=Saturday, 7=Sunday.
    pub fn fraction(&self, hour: u32, day_of_week: u32) -> f64 {
        let idx = ((hour as usize).saturating_sub(1)).min(23);
        let values = match day_of_week {
            6 => {
                // Saturday: try saturday → weekend → weekday
                self.saturday.as_ref()
                    .or(self.weekend.as_ref())
                    .unwrap_or(&self.weekday)
            }
            7 => {
                // Sunday: try sunday → weekend → weekday
                self.sunday.as_ref()
                    .or(self.weekend.as_ref())
                    .unwrap_or(&self.weekday)
            }
            _ => {
                // Weekday (Monday=1 through Friday=5)
                &self.weekday
            }
        };

        if idx < values.len() {
            values[idx].clamp(0.0, 1.0)
        } else if !values.is_empty() {
            values.last().copied().unwrap_or(1.0).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Create an "always on" schedule (fraction = 1.0 at all times).
    pub fn always_on(name: &str) -> Self {
        Self {
            name: name.to_string(),
            weekday: vec![1.0; 24],
            weekend: None,
            saturday: None,
            sunday: None,
            holiday: None,
        }
    }

    /// Create an "always off" schedule (fraction = 0.0 at all times).
    pub fn always_off(name: &str) -> Self {
        Self {
            name: name.to_string(),
            weekday: vec![0.0; 24],
            weekend: None,
            saturday: None,
            sunday: None,
            holiday: None,
        }
    }
}

/// A schedule lookup table built from parsed schedule inputs.
///
/// Provides O(1) lookup of schedule fractions by name + time.
#[derive(Debug, Clone, Default)]
pub struct ScheduleManager {
    schedules: std::collections::HashMap<String, ScheduleInput>,
}

impl ScheduleManager {
    pub fn new() -> Self {
        Self {
            schedules: std::collections::HashMap::new(),
        }
    }

    pub fn from_inputs(inputs: Vec<ScheduleInput>) -> Self {
        let mut mgr = Self::new();
        // Always add built-in schedules
        mgr.schedules.insert("always_on".to_string(), ScheduleInput::always_on("always_on"));
        mgr.schedules.insert("always_off".to_string(), ScheduleInput::always_off("always_off"));
        for input in inputs {
            mgr.schedules.insert(input.name.clone(), input);
        }
        mgr
    }

    /// Look up a schedule fraction. Returns 1.0 if schedule not found (fail-safe: always on).
    pub fn fraction(&self, name: &str, hour: u32, day_of_week: u32) -> f64 {
        match self.schedules.get(name) {
            Some(sched) => sched.fraction(hour, day_of_week),
            None => 1.0, // Unknown schedule defaults to always on
        }
    }
}

/// Calculate day of week from month/day, given the day of week for January 1.
///
/// `jan1_dow`: day of week for January 1 (1=Monday, ..., 7=Sunday).
/// Returns: 1=Monday, 2=Tuesday, ..., 6=Saturday, 7=Sunday.
///
/// Non-leap year assumed (365 days). For energy simulation purposes,
/// a consistent day-of-week pattern is sufficient.
pub fn day_of_week(month: u32, day: u32, jan1_dow: u32) -> u32 {
    let days_in_months: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut day_of_year = 0u32;
    for m in 0..((month - 1) as usize).min(11) {
        day_of_year += days_in_months[m];
    }
    day_of_year += day;
    // jan1_dow=1 means Jan 1 is Monday, jan1_dow=7 means Jan 1 is Sunday
    ((day_of_year - 1 + jan1_dow - 1) % 7) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_always_on() {
        let s = ScheduleInput::always_on("test");
        assert_eq!(s.fraction(1, 1), 1.0);  // Monday midnight
        assert_eq!(s.fraction(12, 3), 1.0); // Wednesday noon
        assert_eq!(s.fraction(24, 7), 1.0); // Sunday 11pm
    }

    #[test]
    fn test_always_off() {
        let s = ScheduleInput::always_off("test");
        assert_eq!(s.fraction(1, 1), 0.0);
        assert_eq!(s.fraction(12, 3), 0.0);
        assert_eq!(s.fraction(24, 7), 0.0);
    }

    #[test]
    fn test_weekday_weekend_split() {
        let s = ScheduleInput {
            name: "test".to_string(),
            weekday: vec![1.0; 24],
            weekend: Some(vec![0.5; 24]),
            saturday: None,
            sunday: None,
            holiday: None,
        };
        assert_eq!(s.fraction(12, 1), 1.0);  // Monday
        assert_eq!(s.fraction(12, 5), 1.0);  // Friday
        assert_eq!(s.fraction(12, 6), 0.5);  // Saturday
        assert_eq!(s.fraction(12, 7), 0.5);  // Sunday
    }

    #[test]
    fn test_saturday_sunday_distinct() {
        let s = ScheduleInput {
            name: "test".to_string(),
            weekday: vec![1.0; 24],
            weekend: None,
            saturday: Some(vec![0.8; 24]),
            sunday: Some(vec![0.3; 24]),
            holiday: None,
        };
        assert_eq!(s.fraction(12, 1), 1.0);  // Monday → weekday
        assert_eq!(s.fraction(12, 5), 1.0);  // Friday → weekday
        assert_eq!(s.fraction(12, 6), 0.8);  // Saturday → saturday
        assert_eq!(s.fraction(12, 7), 0.3);  // Sunday → sunday
    }

    #[test]
    fn test_saturday_falls_back_to_weekend() {
        // Saturday specified, no Sunday → Sunday falls back to weekend
        let s = ScheduleInput {
            name: "test".to_string(),
            weekday: vec![1.0; 24],
            weekend: Some(vec![0.5; 24]),
            saturday: Some(vec![0.8; 24]),
            sunday: None,
            holiday: None,
        };
        assert_eq!(s.fraction(12, 6), 0.8);  // Saturday → saturday (specific)
        assert_eq!(s.fraction(12, 7), 0.5);  // Sunday → weekend (fallback)
    }

    #[test]
    fn test_retail_occupancy_schedule() {
        let s = ScheduleInput {
            name: "Retail Occupancy".to_string(),
            weekday: vec![
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1,
                0.5, 0.9, 1.0, 1.0, 0.8, 1.0, 1.0, 1.0,
                1.0, 1.0, 0.8, 0.5, 0.2, 0.0, 0.0, 0.0,
            ],
            weekend: Some(vec![
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                0.0, 0.3, 0.5, 0.7, 0.7, 0.7, 0.7, 0.5,
                0.3, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            ]),
            saturday: None,
            sunday: None,
            holiday: None,
        };

        // Weekday early morning (hour 3 = 2am-3am)
        assert_eq!(s.fraction(3, 2), 0.0);
        // Weekday midday (hour 11 = 10am-11am)
        assert_eq!(s.fraction(11, 2), 1.0);
        // Weekend midday (hour 12 = 11am-12pm)
        assert_eq!(s.fraction(12, 6), 0.7);
    }

    #[test]
    fn test_schedule_manager() {
        let inputs = vec![
            ScheduleInput {
                name: "occ".to_string(),
                weekday: vec![0.5; 24],
                weekend: Some(vec![0.25; 24]),
                saturday: None,
                sunday: None,
                holiday: None,
            },
        ];
        let mgr = ScheduleManager::from_inputs(inputs);

        assert_eq!(mgr.fraction("occ", 12, 1), 0.5);     // known schedule
        assert_eq!(mgr.fraction("occ", 12, 6), 0.25);     // weekend
        assert_eq!(mgr.fraction("unknown", 12, 1), 1.0);  // unknown → always on
        assert_eq!(mgr.fraction("always_on", 12, 1), 1.0); // built-in
        assert_eq!(mgr.fraction("always_off", 12, 1), 0.0); // built-in
    }

    #[test]
    fn test_day_of_week_monday_start() {
        // Jan 1 = Monday (jan1_dow=1)
        assert_eq!(day_of_week(1, 1, 1), 1);  // Monday
        assert_eq!(day_of_week(1, 6, 1), 6);  // Saturday
        assert_eq!(day_of_week(1, 7, 1), 7);  // Sunday
        assert_eq!(day_of_week(1, 8, 1), 1);  // Monday again
    }

    #[test]
    fn test_day_of_week_sunday_start() {
        // Jan 1 = Sunday (jan1_dow=7) — matches most EPW files
        assert_eq!(day_of_week(1, 1, 7), 7);  // Sunday
        assert_eq!(day_of_week(1, 2, 7), 1);  // Monday
        assert_eq!(day_of_week(1, 7, 7), 6);  // Saturday
        assert_eq!(day_of_week(1, 8, 7), 7);  // Sunday again
    }
}
