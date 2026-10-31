//! Warmup convergence: repeat the run period's first week until each zone's
//! daily temperature extremes and peak loads stop changing.
//!
//! The test is E+'s `HeatBalanceManager::CheckWarmupConvergence` with its default
//! tolerances, applied to each day of the week against the same day of the
//! previous repeat. E+ repeats one day; a week also settles the weekday and
//! weekend schedules, and the same day compares like with like.

use openbse_envelope::zone::ZoneState;

/// Most repeats of the warmup week: 28 days, at least E+'s default 25-day limit.
pub const MAX_WARMUP_CYCLES: u32 = 4;
/// Days in one repeat.
pub const WARMUP_DAYS: usize = 7;
/// E+ `Building` "Temperature Convergence Tolerance Value" default [K].
const TEMP_TOL: f64 = 0.4;
/// E+ `Building` "Loads Convergence Tolerance Value" default (fraction).
const LOAD_TOL: f64 = 0.04;
/// E+ `MinLoad`: loads below this compare as this, so that small loads don't
/// fail on large percentage changes [W].
const MIN_LOAD: f64 = 100.0;

/// One zone's extremes over one day.
#[derive(Debug, Clone, Copy)]
struct DayStats {
    t_max: f64,
    t_min: f64,
    heat_peak: f64,
    cool_peak: f64,
}

impl Default for DayStats {
    fn default() -> Self {
        Self {
            t_max: f64::NEG_INFINITY,
            t_min: f64::INFINITY,
            heat_peak: 0.0,
            cool_peak: 0.0,
        }
    }
}

/// E+'s relative load comparison, with both loads floored at [`MIN_LOAD`].
fn load_settled(load: f64, prev: f64) -> bool {
    if load <= 1e-4 {
        return true;
    }
    let (a, b) = (load.max(MIN_LOAD), prev.max(MIN_LOAD));
    (a - b).abs() / a <= LOAD_TOL
}

/// What keeps a warmup repeat from counting as converged.
#[derive(Debug, Clone, PartialEq)]
pub struct WarmupMiss {
    pub zone: String,
    /// Day of the week's repeat, from 1.
    pub day: usize,
    pub detail: String,
}

/// Tracks the current and previous repeat's daily statistics per zone.
#[derive(Debug, Default)]
pub struct WarmupTracker {
    zones: Vec<String>,
    current: Vec<[DayStats; WARMUP_DAYS]>,
    previous: Option<Vec<[DayStats; WARMUP_DAYS]>>,
    cycles: u32,
}

impl WarmupTracker {
    pub fn new(zones: Vec<String>) -> Self {
        let n = zones.len();
        Self {
            zones,
            current: vec![[DayStats::default(); WARMUP_DAYS]; n],
            previous: None,
            cycles: 0,
        }
    }

    pub fn cycles(&self) -> u32 {
        self.cycles
    }

    /// Record one zone's state at the end of a timestep. `day` counts from 0.
    fn record(&mut self, zone: usize, day: usize, t: f64, heat: f64, cool: f64) {
        let s = &mut self.current[zone][day];
        s.t_max = s.t_max.max(t);
        s.t_min = s.t_min.min(t);
        s.heat_peak = s.heat_peak.max(heat);
        s.cool_peak = s.cool_peak.max(cool);
    }

    /// Record the converged timestep's zone temperatures and loads.
    pub fn record_zones(&mut self, day: usize, zones: &[ZoneState]) {
        for (i, z) in zones.iter().enumerate() {
            self.record(i, day, z.temp, z.ideal_heating_load, z.ideal_cooling_load);
        }
    }

    /// Finish a repeat. Returns what failed the comparison with the previous
    /// repeat: empty once converged. The first repeat has nothing to compare
    /// with, so it reports every zone.
    pub fn end_cycle(&mut self) -> Vec<WarmupMiss> {
        self.cycles += 1;
        let mut misses = Vec::new();
        match &self.previous {
            None => misses.extend(self.zones.iter().map(|z| WarmupMiss {
                zone: z.clone(),
                day: 1,
                detail: "first repeat".to_string(),
            })),
            Some(prev) => {
                for (zi, zone) in self.zones.iter().enumerate() {
                    for day in 0..WARMUP_DAYS {
                        let (a, b) = (self.current[zi][day], prev[zi][day]);
                        if let Some(detail) = day_miss(a, b) {
                            misses.push(WarmupMiss {
                                zone: zone.clone(),
                                day: day + 1,
                                detail,
                            });
                        }
                    }
                }
            }
        }
        let n = self.zones.len();
        self.previous = Some(std::mem::replace(
            &mut self.current,
            vec![[DayStats::default(); WARMUP_DAYS]; n],
        ));
        misses
    }
}

fn day_miss(a: DayStats, b: DayStats) -> Option<String> {
    if (a.t_max - b.t_max).abs() > TEMP_TOL {
        return Some(format!("daily max {:.2} vs {:.2} °C", a.t_max, b.t_max));
    }
    if (a.t_min - b.t_min).abs() > TEMP_TOL {
        return Some(format!("daily min {:.2} vs {:.2} °C", a.t_min, b.t_min));
    }
    if !load_settled(a.heat_peak, b.heat_peak) {
        return Some(format!(
            "peak heating load {:.0} vs {:.0} W",
            a.heat_peak, b.heat_peak
        ));
    }
    if !load_settled(a.cool_peak, b.cool_peak) {
        return Some(format!(
            "peak cooling load {:.0} vs {:.0} W",
            a.cool_peak, b.cool_peak
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker_after_week(t: f64, heat: f64, t_range: f64) -> WarmupTracker {
        let mut w = WarmupTracker::new(vec!["Z".to_string()]);
        for day in 0..WARMUP_DAYS {
            w.record(0, day, t, heat, 0.0);
            w.record(0, day, t + t_range, heat * 0.5, 0.0);
        }
        w
    }

    fn second_cycle(w: &mut WarmupTracker, t: f64, heat: f64, t_range: f64) -> Vec<WarmupMiss> {
        assert_eq!(w.end_cycle().len(), 1, "first repeat can't converge");
        for day in 0..WARMUP_DAYS {
            w.record(0, day, t, heat, 0.0);
            w.record(0, day, t + t_range, heat * 0.5, 0.0);
        }
        w.end_cycle()
    }

    #[test]
    fn identical_repeats_converge() {
        let mut w = tracker_after_week(20.0, 5000.0, 3.0);
        assert!(second_cycle(&mut w, 20.0, 5000.0, 3.0).is_empty());
        assert_eq!(w.cycles(), 2);
    }

    #[test]
    fn a_daily_extreme_fails_though_the_mean_matches() {
        // A coincidental match at the end of a week used to pass; a swing that
        // grew by 1 K must not.
        let mut w = tracker_after_week(20.0, 5000.0, 3.0);
        let misses = second_cycle(&mut w, 20.0, 5000.0, 4.0);
        assert_eq!(misses.len(), WARMUP_DAYS);
        assert!(misses[0].detail.contains("daily max"), "{misses:?}");
    }

    #[test]
    fn load_uses_the_relative_tolerance_with_a_floor() {
        let mut w = tracker_after_week(20.0, 5000.0, 0.0);
        assert!(
            !second_cycle(&mut w, 20.0, 5500.0, 0.0).is_empty(),
            "10% change"
        );
        let mut w = tracker_after_week(20.0, 5000.0, 0.0);
        assert!(
            second_cycle(&mut w, 20.0, 5100.0, 0.0).is_empty(),
            "2% change"
        );
        // Below E+'s 100 W floor: 20 → 80 W is settled.
        let mut w = tracker_after_week(20.0, 20.0, 0.0);
        assert!(
            second_cycle(&mut w, 20.0, 80.0, 0.0).is_empty(),
            "small loads"
        );
    }
}
