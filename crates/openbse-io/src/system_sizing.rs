//! Air-loop coil sizing at the design inlet state (E+ `Sizing:System`).
//!
//! Every loop type goes through the same two loads: a cooling coil takes the
//! mixed air down to the supply state as a total (enthalpy) load, and a heating
//! coil warms the mixed air to its outlet temperature. Mixed air comes from the
//! loop's outdoor-air flow, the design-day outdoor state and the zones' return
//! state at the hour of the peak.
//!
//! Reference: EnergyPlus `SimAirServingZones::UpdateSysSizing`,
//! `Autosizing/CoolingCapacitySizing.cc`, `Autosizing/HeatingCapacitySizing.cc`.

use openbse_psychrometrics as psych;
use std::collections::HashMap;

/// Latent heat for the zone moisture balance [J/kg], as in `zone.rs`.
const H_FG: f64 = 2_501_000.0;
/// Hottest return air the run-time return-heat model allows [°C].
pub const RETURN_AIR_TEMP_MAX: f64 = 60.0;

/// One zone's state in one design-day hour.
#[derive(Debug, Clone, Copy, Default)]
pub struct ZoneHour {
    /// Zone cooling airflow this hour, with the sizing factor [kg/s]; zero on heating days
    pub flow: f64,
    /// Zone air temperature [°C]
    pub temp: f64,
    /// Outdoor air entering the zone directly [kg/s]
    pub outdoor_inflow: f64,
    /// Latent gains [W]
    pub latent: f64,
    /// Lighting heat this zone's return air carries [W] (E+ `ZoneRetTempAtCoolPeak`)
    pub return_heat: f64,
}

impl ZoneHour {
    /// Return-air temperature for this flow [kg/s], capped as the run-time return is.
    fn return_temp(&self, flow: f64) -> f64 {
        if flow <= 1.0e-6 || self.return_heat <= 0.0 {
            return self.temp;
        }
        let rise = self.return_heat / (flow * psych::cp_air_fn_w(0.008));
        (self.temp + rise).min(RETURN_AIR_TEMP_MAX.max(self.temp))
    }
}

/// One hour of a design day.
#[derive(Debug, Clone, Default)]
pub struct SizingHour {
    pub hour: u32,
    /// Outdoor dry-bulb [°C]
    pub t_out: f64,
    /// Outdoor humidity ratio [kg/kg]
    pub w_out: f64,
    pub zones: HashMap<String, ZoneHour>,
}

/// One design day's hours, from the last repetition of the day.
#[derive(Debug, Clone, Default)]
pub struct SizingDay {
    pub name: String,
    pub hours: Vec<SizingHour>,
}

/// The loop's minimum outdoor air.
#[derive(Debug, Clone, Copy)]
pub enum OutdoorAir {
    /// A flow [kg/s], whose fraction grows as the system flow falls
    Flow(f64),
    /// A fixed fraction of the system flow
    Fraction(f64),
}

impl OutdoorAir {
    pub fn fraction_of(self, flow: f64) -> f64 {
        match self {
            OutdoorAir::Flow(oa) if flow > 0.0 => (oa / flow).min(1.0),
            OutdoorAir::Flow(_) => 0.0,
            OutdoorAir::Fraction(x) => x.clamp(0.0, 1.0),
        }
    }
}

/// Which hour's state a loop's cooling coil is sized at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoolingPeakBasis {
    /// The hour of the largest sensible coil load at that hour's system flow
    /// (E+ `Coincident`, `MixTempAtCoolPeak`)
    Coincident,
    /// The hour of the largest total coil load at the design flow, which the
    /// non-coincident flow must meet whenever it comes. E+ `NonCoincident` blends
    /// each zone's own peak hour instead, which sizes a high outdoor-air coil on
    /// mild air when a zone's load is flat and its peak falls at night.
    NonCoincident,
}

/// Air state at the coil inlet at the design peak.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoilInlet {
    pub t_db: f64,
    pub w: f64,
    /// Outdoor dry-bulb at that peak [°C], for condenser-side capacity curves
    pub t_outdoor: f64,
}

/// Quasi-steady zone humidity ratio with air supplied at the cooling design
/// humidity ratio. The ideal loads of the sizing run remove no moisture, so the
/// run's own zone humidity says nothing about the air a coil would return.
fn zone_humidity(z: &ZoneHour, w_out: f64, w_supply: f64) -> f64 {
    let inflow = z.flow + z.outdoor_inflow;
    if inflow <= 0.0 {
        return w_supply;
    }
    (z.flow * w_supply + z.outdoor_inflow * w_out + z.latent / H_FG) / inflow
}

/// Mix return and outdoor air, given `(flow, temperature, humidity)` for each
/// return stream and outdoor state `(t, w)`.
fn mix(returns: &[(f64, f64, f64)], outdoor: (f64, f64), x: f64) -> (f64, f64) {
    let total: f64 = returns.iter().map(|r| r.0).sum();
    if total <= 0.0 {
        return outdoor;
    }
    let t: f64 = returns.iter().map(|r| r.0 * r.1).sum::<f64>() / total;
    let w: f64 = returns.iter().map(|r| r.0 * r.2).sum::<f64>() / total;
    ((1.0 - x) * t + x * outdoor.0, (1.0 - x) * w + x * outdoor.1)
}

/// The coil inlet state at the loop's cooling peak. `None` without design days.
pub fn cooling_coil_inlet(
    days: &[SizingDay],
    zones: &[String],
    basis: CoolingPeakBasis,
    outdoor_air: OutdoorAir,
    design_flow: f64,
    t_supply: f64,
    w_supply: f64,
) -> Option<CoilInlet> {
    let mut best: Option<(f64, CoilInlet)> = None;
    for hour in days.iter().flat_map(|d| &d.hours) {
        let returns: Vec<_> = zones
            .iter()
            .map(|zone| {
                let z = hour.zones.get(zone).copied().unwrap_or_default();
                (
                    z.flow,
                    z.return_temp(z.flow),
                    zone_humidity(&z, hour.w_out, w_supply),
                )
            })
            .collect();
        let hour_flow: f64 = returns.iter().map(|r| r.0).sum();
        let flow = match basis {
            CoolingPeakBasis::Coincident => hour_flow,
            CoolingPeakBasis::NonCoincident => design_flow,
        };
        let x = outdoor_air.fraction_of(flow);
        let (t_db, w) = mix(&returns, (hour.t_out, hour.w_out), x);
        let inlet = CoilInlet {
            t_db,
            w,
            t_outdoor: hour.t_out,
        };
        let load = match basis {
            CoolingPeakBasis::Coincident => flow * (t_db - t_supply),
            CoolingPeakBasis::NonCoincident => cooling_coil_load(&inlet, flow, t_supply, w_supply),
        };
        if best.is_none_or(|(q, _)| load > q) {
            best = Some((load, inlet));
        }
    }
    best.map(|(_, inlet)| inlet)
}

/// Total cooling coil load [W] at the design flow, without fan heat: E+
/// `PeakCoilLoad = ρV(h_in − h_out)`, with the outlet at the design supply state
/// but never warmer or wetter than the inlet.
pub fn cooling_coil_load(inlet: &CoilInlet, mass_flow: f64, t_supply: f64, w_supply: f64) -> f64 {
    let h_in = psych::h_fn_tdb_w(inlet.t_db, inlet.w);
    let h_out = psych::h_fn_tdb_w(t_supply.min(inlet.t_db), w_supply.min(inlet.w));
    (mass_flow * (h_in - h_out)).max(0.0)
}

/// Heating coil load [W]: E+ `cp ρV (T_out − T_mix)` at the worst design-day
/// hour. The return is the zones' temperatures weighted by `zone_weights`.
pub fn heating_coil_load(
    days: &[SizingDay],
    zones: &[String],
    zone_weights: &HashMap<String, f64>,
    outdoor_air: OutdoorAir,
    mass_flow: f64,
    t_coil_out: f64,
) -> f64 {
    let x = outdoor_air.fraction_of(mass_flow);
    let mut peak = 0.0_f64;
    for hour in days.iter().flat_map(|d| &d.hours) {
        let returns: Vec<_> = zones
            .iter()
            .map(|z| {
                let weight = zone_weights.get(z).copied().unwrap_or(0.0);
                let temp = hour.zones.get(z).map_or(21.0, |s| s.return_temp(weight));
                (weight, temp, 0.0)
            })
            .collect();
        let (t_mix, _) = mix(&returns, (hour.t_out, 0.0), x);
        let cp = psych::cp_air_fn_w(hour.w_out);
        peak = peak.max(mass_flow * cp * (t_coil_out - t_mix));
    }
    peak
}

/// Rated capacity [W] of a DX coil of total load `load` [W]: the load over the
/// capacity modifier at the design state, then held inside the flow-per-capacity
/// range `(min, max)` [m³/s/W] of the coil model (E+ `CoolingCapacitySizer`).
pub fn dx_rated_capacity(load: f64, modifier: f64, volume_flow: f64, range: (f64, f64)) -> f64 {
    let capacity = load / modifier.max(0.1);
    if capacity <= 0.0 || volume_flow <= 0.0 {
        return capacity;
    }
    capacity.clamp(volume_flow / range.1, volume_flow / range.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(flow: f64, temp: f64) -> ZoneHour {
        ZoneHour {
            flow,
            temp,
            ..Default::default()
        }
    }

    fn hour(h: u32, t_out: f64, zones: &[(&str, ZoneHour)]) -> SizingHour {
        SizingHour {
            hour: h,
            t_out,
            w_out: 0.012,
            zones: zones.iter().map(|(n, z)| (n.to_string(), *z)).collect(),
        }
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    /// Hand calculation: 1 kg/s, 20% OA at 35 °C and 0.012, return at 24 °C and
    /// the supply humidity. Mixed air 26.2 °C, w = 0.0088.
    #[test]
    fn cooling_load_is_the_enthalpy_drop_from_mixed_air() {
        let day = SizingDay {
            name: "c".into(),
            hours: vec![hour(15, 35.0, &[("A", zone(1.0, 24.0))])],
        };
        let inlet = cooling_coil_inlet(
            &[day],
            &names(&["A"]),
            CoolingPeakBasis::Coincident,
            OutdoorAir::Fraction(0.2),
            1.0,
            13.0,
            0.008,
        )
        .unwrap();
        assert!((inlet.t_db - 26.2).abs() < 1e-9);
        assert!((inlet.w - 0.0088).abs() < 1e-9);
        let q = cooling_coil_load(&inlet, 1.0, 13.0, 0.008);
        let expected = psych::h_fn_tdb_w(26.2, 0.0088) - psych::h_fn_tdb_w(13.0, 0.008);
        assert!((q - expected).abs() < 1e-6);
        // Well above the sensible load: the latent part of the mixed air counts.
        assert!(q > 1.0 * 1005.0 * (26.2 - 13.0) * 1.05);
    }

    /// A dry inlet leaves the coil no wetter than it came in, and a coil never
    /// heats to reach the supply state.
    #[test]
    fn coil_outlet_is_capped_at_the_inlet_state() {
        let dry = CoilInlet {
            t_db: 20.0,
            w: 0.004,
            t_outdoor: 25.0,
        };
        let q = cooling_coil_load(&dry, 1.0, 13.0, 0.008);
        let sensible = psych::h_fn_tdb_w(20.0, 0.004) - psych::h_fn_tdb_w(13.0, 0.004);
        assert!((q - sensible).abs() < 1e-6);
        let cold = CoilInlet {
            t_db: 10.0,
            w: 0.004,
            t_outdoor: 10.0,
        };
        assert_eq!(cooling_coil_load(&cold, 1.0, 13.0, 0.008), 0.0);
    }

    /// The coincident state follows the system's flow; the non-coincident one the
    /// coil's own worst hour.
    #[test]
    fn peak_basis_picks_the_state_it_says() {
        let hours = vec![
            hour(10, 25.0, &[("A", zone(2.0, 24.0)), ("B", zone(0.5, 24.0))]),
            hour(16, 35.0, &[("A", zone(0.5, 24.0)), ("B", zone(1.0, 24.0))]),
        ];
        let days = [SizingDay {
            name: "c".into(),
            hours,
        }];
        let zones = names(&["A", "B"]);
        let oa = OutdoorAir::Flow(0.5);
        let inlet = |basis| cooling_coil_inlet(&days, &zones, basis, oa, 3.0, 13.0, 0.008).unwrap();
        let co = inlet(CoolingPeakBasis::Coincident);
        // Hour 10: 2.5 kg/s × (24.2 − 13) beats hour 16: 1.5 × (27.67 − 13).
        assert_eq!(co.t_outdoor, 25.0);
        assert!((co.t_db - (0.8 * 24.0 + 0.2 * 25.0)).abs() < 1e-9);
        let non = inlet(CoolingPeakBasis::NonCoincident);
        assert_eq!(non.t_outdoor, 35.0);
        let x = 0.5 / 3.0;
        assert!((non.t_db - ((1.0 - x) * 24.0 + x * 35.0)).abs() < 1e-9);
    }

    /// A flat zone load that peaks at night must not size a coil that takes mostly
    /// outdoor air on the mild night air (ER rooms in the Hospital prototype).
    #[test]
    fn high_outdoor_air_coil_sizes_on_the_hot_hour() {
        let days = [SizingDay {
            name: "c".into(),
            hours: vec![
                hour(24, 14.0, &[("A", zone(1.7, 24.0))]),
                hour(15, 33.0, &[("A", zone(1.7, 24.0))]),
            ],
        }];
        let inlet = cooling_coil_inlet(
            &days,
            &names(&["A"]),
            CoolingPeakBasis::NonCoincident,
            OutdoorAir::Fraction(0.8),
            1.7,
            12.8,
            0.008,
        )
        .unwrap();
        assert_eq!(inlet.t_outdoor, 33.0);
    }

    /// Latent gains and humid inflow raise the return humidity above supply.
    #[test]
    fn zone_humidity_balances_moisture() {
        let z = ZoneHour {
            flow: 1.0,
            temp: 24.0,
            outdoor_inflow: 0.1,
            latent: H_FG * 0.001,
            ..Default::default()
        };
        let w = zone_humidity(&z, 0.012, 0.008);
        // Moisture in = moisture out at the zone state.
        let inflow = 1.0 * 0.008 + 0.1 * 0.012 + 0.001;
        assert!((1.1 * w - inflow).abs() < 1e-12);
        assert!(w > 0.008);
    }

    /// Lighting heat in the return raises the coil inlet by Q/(ṁ cp), not the zone.
    #[test]
    fn return_air_heat_reaches_the_coil_inlet() {
        let z = ZoneHour {
            flow: 1.0,
            temp: 24.0,
            return_heat: 2000.0,
            ..Default::default()
        };
        let day = SizingDay {
            name: "c".into(),
            hours: vec![hour(15, 24.0, &[("A", z)])],
        };
        let inlet = cooling_coil_inlet(
            &[day],
            &names(&["A"]),
            CoolingPeakBasis::Coincident,
            OutdoorAir::Fraction(0.0),
            1.0,
            13.0,
            0.008,
        )
        .unwrap();
        let rise = 2000.0 / psych::cp_air_fn_w(0.008);
        assert!((inlet.t_db - (24.0 + rise)).abs() < 1e-9);
    }

    /// Hand calculation: 10% OA at −20 °C with returns at 21 °C, heated to 35 °C.
    #[test]
    fn heating_load_runs_from_mixed_air_to_coil_outlet() {
        let day = SizingDay {
            name: "h".into(),
            hours: vec![
                hour(1, -20.0, &[("A", zone(0.0, 21.0))]),
                hour(14, -5.0, &[("A", zone(0.0, 21.0))]),
            ],
        };
        let weights = HashMap::from([("A".to_string(), 1.0)]);
        let q = heating_coil_load(
            &[day],
            &names(&["A"]),
            &weights,
            OutdoorAir::Fraction(0.1),
            1.0,
            35.0,
        );
        let cp = psych::cp_air_fn_w(0.012);
        assert!((q - cp * (35.0 - (0.9 * 21.0 + 0.1 * -20.0))).abs() < 1e-6);
    }

    #[test]
    fn dx_capacity_stays_in_the_flow_per_capacity_range() {
        let range = (4.027e-5, 6.041e-5);
        // A modifier below one needs more rated capacity.
        assert!((dx_rated_capacity(20_000.0, 0.8, 1.2, range) - 25_000.0).abs() < 1e-6);
        // Too little capacity for the flow is raised, too much is lowered.
        assert!((dx_rated_capacity(1_000.0, 1.0, 1.2, range) - 1.2 / 6.041e-5).abs() < 1e-6);
        assert!((dx_rated_capacity(90_000.0, 1.0, 1.2, range) - 1.2 / 4.027e-5).abs() < 1e-6);
    }
}
