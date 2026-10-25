//! Numerical-safety events in the envelope solve (#160).
//!
//! The solver keeps running when a state goes bad: an inside surface
//! temperature beyond ±[`SURFACE_TEMP_LIMIT`] is held at its previous
//! iteration's value, and a NaN zone or surface state is replaced by the
//! previous timestep's value in the BDF/CTF history. Those holds used to be
//! silent, so a broken model could run all year with surfaces stuck at
//! 199.98 °C and report "Errors: 0". Each hold is now recorded here, and the
//! CLI reports them in the `.err` file as severe errors.

use std::collections::BTreeMap;
use std::fmt;

/// Inside surface temperature limit [°C]: EnergyPlus `Building` "Surface
/// Temperature Upper Limit" default (`HeatBalanceSurfaceManager.cc`).
pub const SURFACE_TEMP_LIMIT: f64 = 200.0;

/// Past this multiple of [`SURFACE_TEMP_LIMIT`] the run stops, as in
/// EnergyPlus.
pub const SURFACE_TEMP_FATAL_FACTOR: f64 = 2.5;

/// The timestep an event happened in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepTime {
    pub month: u32,
    pub day: u32,
    /// Hour ending, 1–24.
    pub hour: u32,
    pub sub_hour: u32,
    pub timesteps_per_hour: u32,
    pub sizing: bool,
}

impl StepTime {
    pub fn from_ctx(ctx: &openbse_core::ports::SimulationContext) -> Self {
        let ts = &ctx.timestep;
        Self {
            month: ts.month,
            day: ts.day,
            hour: ts.hour,
            sub_hour: ts.sub_hour,
            timesteps_per_hour: ts.timesteps_per_hour,
            sizing: ctx.is_sizing,
        }
    }
}

impl fmt::Display for StepTime {
    /// End of the timestep as `MM/DD hh:mm`, as EnergyPlus prints it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tph = self.timesteps_per_hour.max(1);
        let minutes = (self.hour.saturating_sub(1)) * 60 + self.sub_hour * 60 / tph;
        write!(
            f,
            "{:02}/{:02} {:02}:{:02}",
            self.month,
            self.day,
            minutes / 60,
            minutes % 60
        )?;
        if self.sizing {
            write!(f, " (sizing)")?;
        }
        Ok(())
    }
}

/// Repeated holds of one state: how many timesteps, and the worst value.
#[derive(Debug, Clone, Default)]
pub struct HoldEvent {
    /// Timesteps with at least one hold.
    pub timesteps: u64,
    /// The value the solver produced and discarded, largest |value| (NaN
    /// counts as worse than any number).
    pub worst: f64,
    pub worst_time: StepTime,
    pub first_time: StepTime,
    last_time: Option<StepTime>,
}

impl HoldEvent {
    fn record(&mut self, value: f64, now: StepTime) {
        if self.last_time != Some(now) {
            if self.last_time.is_none() {
                self.first_time = now;
            }
            self.timesteps += 1;
            self.last_time = Some(now);
        }
        let worse = if self.timesteps == 1 && self.worst == 0.0 {
            true
        } else if value.is_nan() {
            !self.worst.is_nan()
        } else {
            !self.worst.is_nan() && value.abs() > self.worst.abs()
        };
        if worse {
            self.worst = value;
            self.worst_time = now;
        }
    }
}

/// Envelope numerical-safety events since the last [`Self::take`].
#[derive(Debug, Clone, Default)]
pub struct EnvelopeDiagnostics {
    /// The timestep being solved, set at the start of each solve.
    pub now: StepTime,
    /// Inside surface temperatures beyond ±[`SURFACE_TEMP_LIMIT`] or NaN,
    /// held at the previous iteration's value, by surface name.
    pub surface_temp_holds: BTreeMap<String, HoldEvent>,
    /// NaN zone or surface states replaced in the history, by state
    /// description (e.g. "zone 'Core' air temperature").
    pub nan_holds: BTreeMap<String, HoldEvent>,
    /// Set when a surface passes [`SURFACE_TEMP_FATAL_FACTOR`] × the limit;
    /// the CLI stops the run.
    pub fatal: Option<String>,
}

impl EnvelopeDiagnostics {
    /// Record a held inside surface temperature `value` [°C].
    pub fn surface_temp_held(&mut self, surface: &str, value: f64) {
        let now = self.now;
        self.surface_temp_holds
            .entry(surface.to_string())
            .or_default()
            .record(value, now);
        let fatal_limit = SURFACE_TEMP_LIMIT * SURFACE_TEMP_FATAL_FACTOR;
        if self.fatal.is_none() && value.abs() > fatal_limit {
            self.fatal = Some(format!(
                "Surface '{surface}' inside temperature reached {value:.1} °C at {now}, \
                 beyond {SURFACE_TEMP_FATAL_FACTOR} × the {SURFACE_TEMP_LIMIT} °C surface \
                 temperature limit; the heat balance has diverged"
            ));
        }
    }

    /// Record a NaN `state` replaced by its previous value.
    pub fn nan_held(&mut self, state: impl FnOnce() -> String) {
        let now = self.now;
        self.nan_holds
            .entry(state())
            .or_default()
            .record(f64::NAN, now);
    }

    /// Whether anything was recorded.
    pub fn is_empty(&self) -> bool {
        self.surface_temp_holds.is_empty() && self.nan_holds.is_empty() && self.fatal.is_none()
    }

    /// Hand over the events recorded so far and start afresh.
    pub fn take(&mut self) -> Self {
        let now = self.now;
        let taken = std::mem::take(self);
        self.now = now;
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u32, sub_hour: u32) -> StepTime {
        StepTime {
            month: 7,
            day: 21,
            hour,
            sub_hour,
            timesteps_per_hour: 4,
            sizing: false,
        }
    }

    #[test]
    fn holds_count_timesteps_not_iterations() {
        let mut d = EnvelopeDiagnostics {
            now: at(15, 2),
            ..Default::default()
        };
        d.surface_temp_held("Wall", 210.0);
        d.surface_temp_held("Wall", 230.0); // second iteration, same step
        d.now = at(15, 3);
        d.surface_temp_held("Wall", 220.0);
        let e = &d.surface_temp_holds["Wall"];
        assert_eq!(e.timesteps, 2);
        assert_eq!(e.worst, 230.0);
        assert_eq!(e.worst_time, at(15, 2));
        assert_eq!(e.first_time.to_string(), "07/21 14:30");
        assert!(d.fatal.is_none());
    }

    #[test]
    fn beyond_two_and_a_half_times_the_limit_is_fatal() {
        let mut d = EnvelopeDiagnostics::default();
        d.surface_temp_held("Roof", -480.0);
        assert!(d.fatal.is_none());
        d.surface_temp_held("Roof", 501.0);
        assert!(d.fatal.as_deref().unwrap().contains("'Roof'"));
    }

    #[test]
    fn nan_is_the_worst_value_and_take_resets() {
        let mut d = EnvelopeDiagnostics {
            now: at(1, 4),
            ..Default::default()
        };
        d.surface_temp_held("Floor", 250.0);
        d.surface_temp_held("Floor", f64::NAN);
        d.surface_temp_held("Floor", 300.0);
        assert!(d.surface_temp_holds["Floor"].worst.is_nan());
        d.nan_held(|| "zone 'Z' air temperature".into());
        let taken = d.take();
        assert!(d.is_empty());
        assert_eq!(d.now, at(1, 4));
        assert_eq!(taken.nan_holds.len(), 1);
        assert_eq!(at(24, 4).to_string(), "07/21 24:00");
    }
}
