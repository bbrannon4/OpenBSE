//! Named schedule system for time-varying inputs.
//!
//! Schedules define fractional multipliers (0.0–1.0) that vary by time of day
//! and day type (weekday, weekend/holiday). They are referenced by name from
//! internal gains, exhaust fans, outdoor air, etc.
//!
//! Two input formats are supported:
//!
//! **Explicit arrays** — 24-value vectors for each day type:
//! ```yaml
//! schedules:
//!   - name: Retail Occupancy
//!     weekday:  [0,0,0,0,0,0,0,0.1,0.5,0.9,1,1,0.8,1,1,1,1,1,0.8,0.5,0.2,0,0,0]
//!     weekend:  [0,0,0,0,0,0,0,0,0,0.3,0.5,0.7,0.7,0.7,0.7,0.5,0.3,0.1,0,0,0,0,0,0]
//!     holiday:  [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
//! ```
//!
//! **Compact strings** — EnergyPlus-inspired `"value until HH:MM"` syntax:
//! ```yaml
//! schedules:
//!   - name: Office Occupancy
//!     compact:
//!       weekday: "0 until 8:00, 1.0 until 18:00, 0.5 until 22:00, 0"
//!       weekend: "0 until 10:00, 0.5 until 14:00, 0"
//!       friday:  "0 until 8:00, 0.8 until 16:00, 0"
//!       holiday: "0"
//! ```

use serde::{Deserialize, Serialize};

/// Compact schedule strings for each day type.
///
/// Each string uses E+-inspired `"value until HH:MM"` syntax:
/// - Comma-separated segments: `"0 until 8:00, 1.0 until 18:00, 0"`
/// - `value until HH:MM`: sets value from previous boundary to HH:MM
/// - A bare `value` without `until`: fills remaining hours (must be last)
/// - `"0"` or `"1.0"`: constant all day
///
/// Day-type fields: `weekday` covers Mon–Fri, `weekend` covers Sat–Sun.
/// Individual days (`monday`..`friday`, `saturday`, `sunday`) override
/// the group they belong to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactScheduleInput {
    #[serde(default)]
    pub weekday: Option<String>,
    #[serde(default)]
    pub weekend: Option<String>,
    #[serde(default)]
    pub monday: Option<String>,
    #[serde(default)]
    pub tuesday: Option<String>,
    #[serde(default)]
    pub wednesday: Option<String>,
    #[serde(default)]
    pub thursday: Option<String>,
    #[serde(default)]
    pub friday: Option<String>,
    #[serde(default)]
    pub saturday: Option<String>,
    #[serde(default)]
    pub sunday: Option<String>,
    #[serde(default)]
    pub holiday: Option<String>,
}

/// A named schedule with hourly fractional values for different day types.
///
/// Supports two input modes:
/// 1. **Explicit**: 24-value `weekday`/`weekend`/etc. arrays
/// 2. **Compact**: `compact` struct with `"value until HH:MM"` strings
///
/// When `compact` is provided, it is resolved to explicit arrays at
/// construction time in `ScheduleManager::from_inputs()`.
///
/// Day-type priority: `saturday` > `weekend` > `weekday` for Saturdays,
/// `sunday` > `weekend` > `weekday` for Sundays, `holiday` > `sunday` for holidays.
/// Individual weekday overrides (`monday`..`friday`) take priority over `weekday`.
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
    /// Individual weekday overrides (Monday through Friday).
    /// When set, these take priority over the `weekday` array for that specific day.
    #[serde(default)]
    pub monday: Option<Vec<f64>>,
    #[serde(default)]
    pub tuesday: Option<Vec<f64>>,
    #[serde(default)]
    pub wednesday: Option<Vec<f64>>,
    #[serde(default)]
    pub thursday: Option<Vec<f64>>,
    #[serde(default)]
    pub friday: Option<Vec<f64>>,
    /// Compact schedule input (alternative to explicit arrays).
    /// When provided, resolved to explicit arrays during ScheduleManager construction.
    #[serde(default)]
    pub compact: Option<CompactScheduleInput>,
}

fn default_always_on() -> Vec<f64> {
    vec![1.0; 24]
}

/// Parse a compact schedule string into a 24-element hourly array.
///
/// Format: comma-separated segments of `value [until HH:MM]`
///
/// Examples:
/// - `"0"` → all zeros
/// - `"1.0"` → all ones
/// - `"0 until 8:00, 1.0 until 18:00, 0"` → 0 for hours 1-8, 1.0 for 9-18, 0 for 19-24
/// - `"0 until 8:00, 1.0 until 18:00, 0.5 until 22:00, 0"` → four segments
pub fn parse_compact_day(s: &str) -> Result<Vec<f64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Empty compact schedule string".to_string());
    }

    let mut values = vec![0.0_f64; 24];
    let segments: Vec<&str> = s.split(',').map(|seg| seg.trim()).collect();
    let mut current_hour = 0usize; // 0 = start of day (00:00)

    for (i, seg) in segments.iter().enumerate() {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }

        // Check if segment has "until HH:MM"
        let lower = seg.to_lowercase();
        if let Some(until_pos) = lower.find("until") {
            // Parse value before "until"
            let value_str = seg[..until_pos].trim();
            let value: f64 = value_str.parse()
                .map_err(|_| format!("Invalid value '{}' in compact schedule", value_str))?;

            // Parse time after "until"
            let time_str = seg[until_pos + 5..].trim().trim_matches(':');
            let time_parts: Vec<&str> = time_str.split(':').collect();
            let end_hour: usize = match time_parts.len() {
                1 => time_parts[0].parse()
                    .map_err(|_| format!("Invalid hour '{}' in compact schedule", time_parts[0]))?,
                2 => time_parts[0].parse()
                    .map_err(|_| format!("Invalid hour '{}' in compact schedule", time_parts[0]))?,
                _ => return Err(format!("Invalid time format '{}' in compact schedule", time_str)),
            };

            if end_hour > 24 {
                return Err(format!("Hour {} exceeds 24 in compact schedule", end_hour));
            }
            if end_hour <= current_hour {
                return Err(format!(
                    "Time {}:00 is not after previous boundary {}:00 in compact schedule",
                    end_hour, current_hour
                ));
            }

            // Fill hours from current_hour to end_hour
            for h in current_hour..end_hour.min(24) {
                values[h] = value;
            }
            current_hour = end_hour;
        } else {
            // Bare value — fills remaining hours (must be last segment or only segment)
            let value: f64 = seg.parse()
                .map_err(|_| format!("Invalid value '{}' in compact schedule", seg))?;

            if i < segments.len() - 1 {
                // Not the last segment — check if remaining segments are empty
                let remaining_non_empty = segments[i+1..].iter().any(|s| !s.trim().is_empty());
                if remaining_non_empty {
                    return Err(format!(
                        "Bare value '{}' must be the last segment in compact schedule", seg
                    ));
                }
            }

            // Fill remaining hours
            for h in current_hour..24 {
                values[h] = value;
            }
            current_hour = 24;
        }
    }

    // If we didn't reach hour 24, fill remaining with 0
    // (This happens if the string ends with an "until" segment that doesn't reach 24:00)
    // Actually, this is already handled — values start at 0.0

    Ok(values)
}

impl ScheduleInput {
    /// Resolve compact schedule strings into explicit arrays.
    ///
    /// Called during `ScheduleManager::from_inputs()`. If `compact` is provided,
    /// its strings are parsed and used to populate the explicit array fields.
    /// Compact values only override fields that are at their default values.
    pub fn resolve_compact(&mut self) -> Result<(), String> {
        let compact = match self.compact.take() {
            Some(c) => c,
            None => return Ok(()),
        };

        // Helper: parse and set if the compact field is provided
        let parse = |s: &Option<String>| -> Result<Option<Vec<f64>>, String> {
            match s {
                Some(ref text) => Ok(Some(parse_compact_day(text)?)),
                None => Ok(None),
            }
        };

        // Weekday (only override if still at the default "always on")
        if let Some(ref text) = compact.weekday {
            self.weekday = parse_compact_day(text)
                .map_err(|e| format!("Schedule '{}' compact weekday: {}", self.name, e))?;
        }
        if let Some(v) = parse(&compact.weekend).map_err(|e| format!("Schedule '{}' compact weekend: {}", self.name, e))? {
            self.weekend = Some(v);
        }
        if let Some(v) = parse(&compact.saturday).map_err(|e| format!("Schedule '{}' compact saturday: {}", self.name, e))? {
            self.saturday = Some(v);
        }
        if let Some(v) = parse(&compact.sunday).map_err(|e| format!("Schedule '{}' compact sunday: {}", self.name, e))? {
            self.sunday = Some(v);
        }
        if let Some(v) = parse(&compact.holiday).map_err(|e| format!("Schedule '{}' compact holiday: {}", self.name, e))? {
            self.holiday = Some(v);
        }
        if let Some(v) = parse(&compact.monday).map_err(|e| format!("Schedule '{}' compact monday: {}", self.name, e))? {
            self.monday = Some(v);
        }
        if let Some(v) = parse(&compact.tuesday).map_err(|e| format!("Schedule '{}' compact tuesday: {}", self.name, e))? {
            self.tuesday = Some(v);
        }
        if let Some(v) = parse(&compact.wednesday).map_err(|e| format!("Schedule '{}' compact wednesday: {}", self.name, e))? {
            self.wednesday = Some(v);
        }
        if let Some(v) = parse(&compact.thursday).map_err(|e| format!("Schedule '{}' compact thursday: {}", self.name, e))? {
            self.thursday = Some(v);
        }
        if let Some(v) = parse(&compact.friday).map_err(|e| format!("Schedule '{}' compact friday: {}", self.name, e))? {
            self.friday = Some(v);
        }

        Ok(())
    }

    /// Get the schedule fraction for a given hour (1-24) and day of week (1=Mon, 7=Sun).
    ///
    /// Hour is 1-indexed (1 = midnight to 1am, 24 = 11pm to midnight).
    /// Day of week: 1=Monday, 2=Tuesday, ..., 6=Saturday, 7=Sunday.
    ///
    /// Resolution priority:
    /// - Monday(1): monday → weekday
    /// - Tuesday(2): tuesday → weekday
    /// - Wednesday(3): wednesday → weekday
    /// - Thursday(4): thursday → weekday
    /// - Friday(5): friday → weekday
    /// - Saturday(6): saturday → weekend → weekday
    /// - Sunday(7): sunday → weekend → weekday
    /// - Holiday: holiday → sunday → weekend → weekday
    pub fn fraction(&self, hour: u32, day_of_week: u32) -> f64 {
        let idx = ((hour as usize).saturating_sub(1)).min(23);
        let values = match day_of_week {
            1 => self.monday.as_ref().unwrap_or(&self.weekday),
            2 => self.tuesday.as_ref().unwrap_or(&self.weekday),
            3 => self.wednesday.as_ref().unwrap_or(&self.weekday),
            4 => self.thursday.as_ref().unwrap_or(&self.weekday),
            5 => self.friday.as_ref().unwrap_or(&self.weekday),
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
            _ => &self.weekday,
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
            monday: None,
            tuesday: None,
            wednesday: None,
            thursday: None,
            friday: None,
            compact: None,
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
            monday: None,
            tuesday: None,
            wednesday: None,
            thursday: None,
            friday: None,
            compact: None,
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
        for mut input in inputs {
            // Resolve compact schedule strings to explicit arrays
            if input.compact.is_some() {
                if let Err(e) = input.resolve_compact() {
                    log::error!("Failed to resolve compact schedule '{}': {}", input.name, e);
                    continue;
                }
            }
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
            saturday: None, sunday: None, holiday: None,
            monday: None, tuesday: None, wednesday: None,
            thursday: None, friday: None, compact: None,
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
            monday: None, tuesday: None, wednesday: None,
            thursday: None, friday: None, compact: None,
        };
        assert_eq!(s.fraction(12, 1), 1.0);  // Monday → weekday
        assert_eq!(s.fraction(12, 5), 1.0);  // Friday → weekday
        assert_eq!(s.fraction(12, 6), 0.8);  // Saturday → saturday
        assert_eq!(s.fraction(12, 7), 0.3);  // Sunday → sunday
    }

    #[test]
    fn test_saturday_falls_back_to_weekend() {
        let s = ScheduleInput {
            name: "test".to_string(),
            weekday: vec![1.0; 24],
            weekend: Some(vec![0.5; 24]),
            saturday: Some(vec![0.8; 24]),
            sunday: None, holiday: None,
            monday: None, tuesday: None, wednesday: None,
            thursday: None, friday: None, compact: None,
        };
        assert_eq!(s.fraction(12, 6), 0.8);  // Saturday → saturday (specific)
        assert_eq!(s.fraction(12, 7), 0.5);  // Sunday → weekend (fallback)
    }

    #[test]
    fn test_individual_weekday_overrides() {
        let s = ScheduleInput {
            name: "test".to_string(),
            weekday: vec![1.0; 24],
            weekend: None, saturday: None, sunday: None, holiday: None,
            monday: Some(vec![0.1; 24]),
            tuesday: None,
            wednesday: Some(vec![0.3; 24]),
            thursday: None,
            friday: Some(vec![0.5; 24]),
            compact: None,
        };
        assert_eq!(s.fraction(12, 1), 0.1);  // Monday → monday override
        assert_eq!(s.fraction(12, 2), 1.0);  // Tuesday → weekday fallback
        assert_eq!(s.fraction(12, 3), 0.3);  // Wednesday → wednesday override
        assert_eq!(s.fraction(12, 4), 1.0);  // Thursday → weekday fallback
        assert_eq!(s.fraction(12, 5), 0.5);  // Friday → friday override
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
            saturday: None, sunday: None, holiday: None,
            monday: None, tuesday: None, wednesday: None,
            thursday: None, friday: None, compact: None,
        };

        assert_eq!(s.fraction(3, 2), 0.0);  // Weekday 2am-3am
        assert_eq!(s.fraction(11, 2), 1.0); // Weekday 10am-11am
        assert_eq!(s.fraction(12, 6), 0.7); // Weekend 11am-12pm
    }

    #[test]
    fn test_schedule_manager() {
        let inputs = vec![
            ScheduleInput {
                name: "occ".to_string(),
                weekday: vec![0.5; 24],
                weekend: Some(vec![0.25; 24]),
                saturday: None, sunday: None, holiday: None,
                monday: None, tuesday: None, wednesday: None,
                thursday: None, friday: None, compact: None,
            },
        ];
        let mgr = ScheduleManager::from_inputs(inputs);

        assert_eq!(mgr.fraction("occ", 12, 1), 0.5);     // known schedule
        assert_eq!(mgr.fraction("occ", 12, 6), 0.25);     // weekend
        assert_eq!(mgr.fraction("unknown", 12, 1), 1.0);  // unknown → always on
        assert_eq!(mgr.fraction("always_on", 12, 1), 1.0); // built-in
        assert_eq!(mgr.fraction("always_off", 12, 1), 0.0); // built-in
    }

    // ─── Compact schedule parsing tests ────────────────────────────────────

    #[test]
    fn test_parse_compact_constant() {
        let v = parse_compact_day("0").unwrap();
        assert_eq!(v, vec![0.0; 24]);

        let v = parse_compact_day("1.0").unwrap();
        assert_eq!(v, vec![1.0; 24]);

        let v = parse_compact_day("0.75").unwrap();
        assert_eq!(v, vec![0.75; 24]);
    }

    #[test]
    fn test_parse_compact_office_hours() {
        // 0 until 8:00, 1.0 until 18:00, 0
        let v = parse_compact_day("0 until 8:00, 1.0 until 18:00, 0").unwrap();
        assert_eq!(v[0..8], vec![0.0; 8]);     // hours 1-8 (00:00–08:00)
        assert_eq!(v[8..18], vec![1.0; 10]);   // hours 9-18 (08:00–18:00)
        assert_eq!(v[18..24], vec![0.0; 6]);   // hours 19-24 (18:00–24:00)
    }

    #[test]
    fn test_parse_compact_three_segments() {
        let v = parse_compact_day("0 until 8:00, 1.0 until 18:00, 0.5 until 22:00, 0").unwrap();
        assert_eq!(v[0..8], vec![0.0; 8]);
        assert_eq!(v[8..18], vec![1.0; 10]);
        assert_eq!(v[18..22], vec![0.5; 4]);
        assert_eq!(v[22..24], vec![0.0; 2]);
    }

    #[test]
    fn test_parse_compact_until_24() {
        let v = parse_compact_day("0.3 until 12:00, 0.7 until 24:00").unwrap();
        assert_eq!(v[0..12], vec![0.3; 12]);
        assert_eq!(v[12..24], vec![0.7; 12]);
    }

    #[test]
    fn test_parse_compact_errors() {
        assert!(parse_compact_day("").is_err());
        assert!(parse_compact_day("abc").is_err());
        assert!(parse_compact_day("0 until 25:00").is_err());
        assert!(parse_compact_day("0 until 8:00, 1.0 until 6:00").is_err()); // backwards
    }

    #[test]
    fn test_compact_schedule_resolve() {
        let mut s = ScheduleInput {
            name: "test".to_string(),
            weekday: vec![1.0; 24], // default
            weekend: None, saturday: None, sunday: None, holiday: None,
            monday: None, tuesday: None, wednesday: None,
            thursday: None, friday: None,
            compact: Some(CompactScheduleInput {
                weekday: Some("0 until 8:00, 1.0 until 18:00, 0".to_string()),
                weekend: Some("0".to_string()),
                friday: Some("0 until 8:00, 0.8 until 16:00, 0".to_string()),
                monday: None, tuesday: None, wednesday: None,
                thursday: None, saturday: None, sunday: None,
                holiday: None,
            }),
        };
        s.resolve_compact().unwrap();

        // Weekday: 0 until 8, 1.0 until 18, 0
        assert_eq!(s.fraction(5, 2), 0.0);   // Tuesday 4am-5am
        assert_eq!(s.fraction(12, 2), 1.0);  // Tuesday noon
        assert_eq!(s.fraction(20, 2), 0.0);  // Tuesday 7pm-8pm

        // Friday override: 0 until 8, 0.8 until 16, 0
        assert_eq!(s.fraction(5, 5), 0.0);   // Friday 4am-5am
        assert_eq!(s.fraction(12, 5), 0.8);  // Friday noon
        assert_eq!(s.fraction(17, 5), 0.0);  // Friday 4pm-5pm

        // Weekend: all 0
        assert_eq!(s.fraction(12, 6), 0.0);  // Saturday noon
        assert_eq!(s.fraction(12, 7), 0.0);  // Sunday noon
    }

    #[test]
    fn test_compact_schedule_manager_integration() {
        let inputs = vec![
            ScheduleInput {
                name: "Office".to_string(),
                weekday: vec![1.0; 24],
                weekend: None, saturday: None, sunday: None, holiday: None,
                monday: None, tuesday: None, wednesday: None,
                thursday: None, friday: None,
                compact: Some(CompactScheduleInput {
                    weekday: Some("0 until 8:00, 1.0 until 18:00, 0".to_string()),
                    weekend: Some("0".to_string()),
                    monday: None, tuesday: None, wednesday: None,
                    thursday: None, friday: None, saturday: None,
                    sunday: None, holiday: None,
                }),
            },
        ];
        let mgr = ScheduleManager::from_inputs(inputs);

        assert_eq!(mgr.fraction("Office", 5, 1), 0.0);   // Monday 4am
        assert_eq!(mgr.fraction("Office", 12, 3), 1.0);  // Wednesday noon
        assert_eq!(mgr.fraction("Office", 20, 1), 0.0);  // Monday 7pm
        assert_eq!(mgr.fraction("Office", 12, 6), 0.0);  // Saturday noon
    }

    // ─── Day of week tests ─────────────────────────────────────────────────

    #[test]
    fn test_day_of_week_monday_start() {
        assert_eq!(day_of_week(1, 1, 1), 1);  // Monday
        assert_eq!(day_of_week(1, 6, 1), 6);  // Saturday
        assert_eq!(day_of_week(1, 7, 1), 7);  // Sunday
        assert_eq!(day_of_week(1, 8, 1), 1);  // Monday again
    }

    #[test]
    fn test_day_of_week_sunday_start() {
        assert_eq!(day_of_week(1, 1, 7), 7);  // Sunday
        assert_eq!(day_of_week(1, 2, 7), 1);  // Monday
        assert_eq!(day_of_week(1, 7, 7), 6);  // Saturday
        assert_eq!(day_of_week(1, 8, 7), 7);  // Sunday again
    }
}
