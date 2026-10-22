//! Spatial comfort grid recorder and `<stem>_comfort.json` writer (#136).
//!
//! The grid geometry and physics live in `openbse_envelope::comfort_grid`.
//! This module picks the zones, feeds the grids hourly means from the running
//! simulation, keeps per-point annual statistics, and writes what a viewer
//! needs to recompute any hour itself (view factors, hourly surface
//! temperatures, window beam and sun direction) instead of storing every
//! point for every hour.
//!
//! **Physics limit:** zone air is well mixed, so air temperature and humidity
//! are uniform within a zone. All spatial variation comes from mean radiant
//! temperature and direct sun on the occupant.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use openbse_core::types::TimeStep;
use openbse_envelope::comfort_grid::{
    window_beam_irradiance, GridSettings, GridSurface, OccupantModel, PmvBand, Sensation,
    ZoneAirState, ZoneGrid,
};
use openbse_envelope::geometry::{Point3D, Vec3};
use openbse_envelope::heat_balance::BuildingEnvelope;
use openbse_envelope::internal_gains::InternalGainInput;
use openbse_envelope::{solar, SurfaceType, ZoneComfortConfig};
use openbse_psychrometrics::comfort::solar_mrt_correction;
use rayon::prelude::*;
use serde::Serialize;

use crate::input::ComfortGridInput;

const FORMAT: &str = "openbse-comfort-grid";
const FORMAT_VERSION: u32 = 1;
const PHYSICS_NOTE: &str = "Zone air is well mixed (one air node per zone), so air \
    temperature and humidity are uniform within each zone. Spatial variation comes only \
    from mean radiant temperature and direct sun on the occupant; this is not a CFD \
    air-temperature field.";

/// Matches the zone-level comfort outputs.
const BODY_EMISSIVITY: f64 = 0.95;
const PSYCH_PRESSURE: f64 = 101_325.0;

/// An hour counts as "in direct sun" only when the sun raises the occupant's
/// MRT by at least this much [K]. Beam through a shade's openness is real but
/// faint, and counting it would make a shaded seat look sunlit.
const SUN_HOUR_MIN_DELTA_MRT: f64 = 1.0;

/// Below this coverage a zone's enclosure has real gaps worth a warning.
const LOW_COVERAGE: f64 = 0.95;

struct GriddedZone {
    env_index: usize,
    name: String,
    grid: ZoneGrid,
    /// Recorder surface slot for each grid column.
    slots: Vec<usize>,
    comfort: ZoneComfortConfig,
    people: Vec<(f64, Option<String>)>,

    air_temperature: Vec<f32>,
    relative_humidity: Vec<f32>,
    occupancy: Vec<f32>,

    occupied_hours: u32,
    comfortable: Vec<u32>,
    cold: Vec<u32>,
    hot: Vec<u32>,
    sun: Vec<u32>,
    operative_sum: Vec<f64>,
    operative_max: Vec<f32>,

    acc_air: f64,
    acc_w: f64,
    acc_occupancy: f64,
}

/// Collects comfort-grid data during the annual simulation.
pub struct ComfortGridRecorder {
    settings: GridSettings,
    pmv_limit: f64,
    zones: Vec<GriddedZone>,
    skipped: Vec<(String, String)>,

    /// Envelope surface index per slot (surfaces of gridded zones).
    slot_surfaces: Vec<usize>,
    slot_is_window: Vec<bool>,
    /// Windows with a physical shade layer (#144).
    slot_has_shade: Vec<bool>,
    surface_temps: Vec<Vec<f32>>,
    window_beam: Vec<Vec<f32>>,
    shade_deployed: Vec<Vec<f32>>,
    acc_temp: Vec<f64>,
    acc_beam: Vec<f64>,
    acc_deployed: Vec<f64>,
    acc_steps: u32,

    sun: [Vec<f32>; 3],
    start_hour_of_year: Option<u32>,
}

impl ComfortGridRecorder {
    /// Builds the grids for every eligible zone. Zones that cannot be gridded
    /// are skipped with a warning and listed in the output.
    pub fn new(input: &ComfortGridInput, env: &BuildingEnvelope) -> Self {
        let settings = GridSettings {
            spacing: input.spacing,
            wall_offset: input.wall_offset,
            height: input.height,
            occupant: input.occupant,
        };
        let mut skipped = Vec::new();

        let wanted: Vec<usize> = if input.zones.is_empty() {
            (0..env.zones.len()).collect()
        } else {
            input
                .zones
                .iter()
                .filter_map(|name| {
                    let idx = env.zone_index.get(name).copied();
                    if idx.is_none() {
                        skipped.push((name.clone(), "zone not found".to_string()));
                    }
                    idx
                })
                .collect()
        };

        let mut slot_of: HashMap<usize, usize> = HashMap::new();
        let mut slot_surfaces = Vec::new();
        let mut zones = Vec::new();
        for zi in wanted {
            let zone = &env.zones[zi];
            match grid_zone(env, zi, &settings) {
                Ok((grid, members)) => {
                    let min_cov = grid.min_coverage();
                    if min_cov < LOW_COVERAGE {
                        log::warn!(
                            "Comfort grid: zone '{}' surfaces cover only {:.0}% of the view at \
                             some points (missing walls or gaps); the rest is treated as the \
                             average of the surfaces seen.",
                            zone.input.name,
                            min_cov * 100.0
                        );
                    }
                    let slots = members
                        .iter()
                        .map(|&si| {
                            *slot_of.entry(si).or_insert_with(|| {
                                slot_surfaces.push(si);
                                slot_surfaces.len() - 1
                            })
                        })
                        .collect();
                    let n = grid.points.len();
                    zones.push(GriddedZone {
                        env_index: zi,
                        name: zone.input.name.clone(),
                        grid,
                        slots,
                        comfort: zone.input.comfort.clone().unwrap_or_default(),
                        people: people_of(&zone.input.internal_gains),
                        air_temperature: Vec::new(),
                        relative_humidity: Vec::new(),
                        occupancy: Vec::new(),
                        occupied_hours: 0,
                        comfortable: vec![0; n],
                        cold: vec![0; n],
                        hot: vec![0; n],
                        sun: vec![0; n],
                        operative_sum: vec![0.0; n],
                        operative_max: vec![f32::NEG_INFINITY; n],
                        acc_air: 0.0,
                        acc_w: 0.0,
                        acc_occupancy: 0.0,
                    });
                }
                Err(reason) => {
                    // Only worth a warning when the user named the zone; by
                    // default, zones without full geometry are expected.
                    if input.zones.is_empty() {
                        log::info!(
                            "Comfort grid: skipping zone '{}': {reason}",
                            zone.input.name
                        );
                    } else {
                        log::warn!(
                            "Comfort grid: skipping zone '{}': {reason}",
                            zone.input.name
                        );
                    }
                    skipped.push((zone.input.name.clone(), reason));
                }
            }
        }

        let points: usize = zones.iter().map(|z| z.grid.points.len()).sum();
        log::info!(
            "Comfort grid: {} zone(s), {} points, {} skipped",
            zones.len(),
            points,
            skipped.len()
        );

        let n_slots = slot_surfaces.len();
        let slot_is_window = slot_surfaces
            .iter()
            .map(|&si| env.surfaces[si].is_window)
            .collect();
        let slot_has_shade = slot_surfaces
            .iter()
            .map(|&si| shade_configured(env, si))
            .collect();
        Self {
            settings,
            pmv_limit: input.pmv_limit,
            zones,
            skipped,
            slot_surfaces,
            slot_is_window,
            slot_has_shade,
            surface_temps: vec![Vec::new(); n_slots],
            window_beam: vec![Vec::new(); n_slots],
            shade_deployed: vec![Vec::new(); n_slots],
            acc_temp: vec![0.0; n_slots],
            acc_beam: vec![0.0; n_slots],
            acc_deployed: vec![0.0; n_slots],
            acc_steps: 0,
            sun: [Vec::new(), Vec::new(), Vec::new()],
            start_hour_of_year: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    /// Adds one converged timestep. The last timestep of each hour closes the
    /// hour and updates the per-point statistics.
    pub fn record_timestep(&mut self, env: &BuildingEnvelope, ts: &TimeStep) {
        if self.zones.is_empty() {
            return;
        }
        let doy = ts.day_of_year();
        if self.start_hour_of_year.is_none() {
            self.start_hour_of_year = Some((doy - 1) * 24 + ts.hour - 1);
        }

        for (slot, &si) in self.slot_surfaces.iter().enumerate() {
            let s = &env.surfaces[si];
            // The face the room sees: the shade when one is down (#144).
            self.acc_temp[slot] += s.temp_inside;
            if s.blind_deployed && s.shade.is_some() {
                self.acc_deployed[slot] += 1.0;
            }
            if self.slot_is_window[slot] {
                self.acc_beam[slot] +=
                    window_beam_irradiance(s.transmitted_solar_beam, s.net_area, s.cos_incidence);
            }
        }
        let dow = openbse_envelope::schedule::day_of_week(ts.month, ts.day, env.jan1_dow);
        for z in &mut self.zones {
            let state = &env.zones[z.env_index];
            z.acc_air += state.temp;
            z.acc_w += state.humidity_ratio;
            z.acc_occupancy += occupancy_fraction(&z.people, env, ts.hour, dow, doy);
        }
        self.acc_steps += 1;

        if ts.sub_hour >= ts.timesteps_per_hour {
            self.finish_hour(env, doy, ts.hour);
        }
    }

    fn finish_hour(&mut self, env: &BuildingEnvelope, doy: u32, hour: u32) {
        let n = self.acc_steps.max(1) as f64;
        let temps: Vec<f64> = self.acc_temp.iter().map(|t| t / n).collect();
        let beams: Vec<f64> = self.acc_beam.iter().map(|b| b / n).collect();
        for slot in 0..self.slot_surfaces.len() {
            self.surface_temps[slot].push(round(temps[slot], 2));
            if self.slot_is_window[slot] {
                self.window_beam[slot].push(round(beams[slot], 1));
            }
            if self.slot_has_shade[slot] {
                self.shade_deployed[slot].push(round(self.acc_deployed[slot] / n, 2));
            }
        }

        // The patch is placed with the sun at mid-hour, consistent with the
        // hourly-mean beam it is lit by.
        let solar_hour =
            solar::local_solar_hour(hour as f64 - 0.5, env.time_zone, env.longitude, doy);
        let pos = solar::solar_position(doy, solar_hour, env.latitude);
        let toward_sun = if pos.altitude > 0.0 {
            solar::sun_direction_vector(&pos).scale(-1.0)
        } else {
            Vec3::new(0.0, 0.0, 0.0)
        };
        self.sun[0].push(round(toward_sun.x, 4));
        self.sun[1].push(round(toward_sun.y, 4));
        self.sun[2].push(round(toward_sun.z, 4));

        let pmv_limit = self.pmv_limit;
        self.zones.par_iter_mut().for_each(|z| {
            let t_air = z.acc_air / n;
            let w = z.acc_w / n;
            let occupancy = z.acc_occupancy / n;
            let rh = openbse_psychrometrics::rh_fn_tdb_w_pb(t_air, w, PSYCH_PRESSURE);
            z.air_temperature.push(round(t_air, 2));
            z.relative_humidity.push(round(rh, 3));
            z.occupancy.push(round(occupancy, 3));
            z.acc_air = 0.0;
            z.acc_w = 0.0;
            z.acc_occupancy = 0.0;

            if occupancy <= 0.0 {
                return;
            }
            z.occupied_hours += 1;
            let zone_temps: Vec<f64> = z.slots.iter().map(|&s| temps[s]).collect();
            let zone_beam: Vec<f64> = z.slots.iter().map(|&s| beams[s]).collect();
            let any_beam = pos.altitude > 0.0 && zone_beam.iter().any(|&b| b > 0.0);
            let air = ZoneAirState {
                temperature: t_air,
                relative_humidity: rh,
            };
            let band = PmvBand::new(air, &z.comfort, pmv_limit);
            for i in 0..z.grid.points.len() {
                let mrt = z.grid.mrt(i, &zone_temps);
                let beam = if any_beam {
                    z.grid.beam_irradiance(i, toward_sun, &zone_beam)
                } else {
                    0.0
                };
                let delta = if beam > 0.0 {
                    solar_mrt_correction(
                        beam,
                        pos.altitude,
                        mrt,
                        z.comfort.solar_absorptivity,
                        BODY_EMISSIVITY,
                    )
                } else {
                    0.0
                };
                if delta >= SUN_HOUR_MIN_DELTA_MRT {
                    z.sun[i] += 1;
                }
                let mrt_eff = mrt + delta;
                let operative =
                    openbse_psychrometrics::comfort::operative_temperature(t_air, mrt_eff);
                z.operative_sum[i] += operative;
                z.operative_max[i] = z.operative_max[i].max(operative as f32);
                match band.classify(mrt_eff) {
                    Sensation::Cold => z.cold[i] += 1,
                    Sensation::Comfortable => z.comfortable[i] += 1,
                    Sensation::Hot => z.hot[i] += 1,
                }
            }
        });

        self.acc_temp.iter_mut().for_each(|v| *v = 0.0);
        self.acc_beam.iter_mut().for_each(|v| *v = 0.0);
        self.acc_deployed.iter_mut().for_each(|v| *v = 0.0);
        self.acc_steps = 0;
    }

    /// Writes `<stem>_comfort.json`.
    pub fn write(&self, path: &Path, env: &BuildingEnvelope) -> std::io::Result<()> {
        let slot_of: HashMap<usize, usize> = self
            .slot_surfaces
            .iter()
            .enumerate()
            .map(|(slot, &si)| (si, slot))
            .collect();

        // Every surface with vertices is written, so a viewer can draw the
        // whole building around the gridded zones.
        let mut out_index: HashMap<usize, usize> = HashMap::new();
        let mut surfaces = Vec::new();
        for (si, s) in env.surfaces.iter().enumerate() {
            let Some(verts) = s.input.vertices.as_ref().filter(|v| v.len() >= 3) else {
                continue;
            };
            let slot = slot_of.get(&si).copied();
            out_index.insert(si, surfaces.len());
            surfaces.push(SurfaceOut {
                name: &s.input.name,
                zone: &s.input.zone,
                kind: surface_kind(s.input.surface_type),
                vertices: verts.iter().map(point).collect(),
                inside_temperature: slot.map(|k| self.surface_temps[k].as_slice()),
                beam_irradiance: slot
                    .filter(|&k| self.slot_is_window[k])
                    .map(|k| self.window_beam[k].as_slice()),
                shade_deployed: slot
                    .filter(|&k| self.slot_has_shade[k])
                    .map(|k| self.shade_deployed[k].as_slice()),
            });
        }

        let zones = self
            .zones
            .iter()
            .map(|z| {
                let n = z.grid.points.len();
                let n_s = z.grid.n_surfaces();
                let mut view_factors = Vec::with_capacity(n * n_s);
                for i in 0..n {
                    view_factors.extend(z.grid.view_factors(i).iter().map(|&f| round(f, 5)));
                }
                let occupied = z.occupied_hours.max(1) as f64;
                let has_stats = z.occupied_hours > 0;
                ZoneOut {
                    name: &z.name,
                    comfort: ComfortOut {
                        metabolic_rate: z.comfort.metabolic_rate,
                        clothing: z.comfort.clothing,
                        air_velocity: z.comfort.air_velocity,
                        solar_absorptivity: z.comfort.solar_absorptivity,
                    },
                    coverage_min: round(z.grid.min_coverage(), 3),
                    points: z.grid.points.iter().map(point).collect(),
                    surfaces: z
                        .slots
                        .iter()
                        .map(|&slot| out_index[&self.slot_surfaces[slot]])
                        .collect(),
                    view_factors,
                    air_temperature: &z.air_temperature,
                    relative_humidity: &z.relative_humidity,
                    occupancy: &z.occupancy,
                    aggregates: AggregatesOut {
                        occupied_hours: z.occupied_hours,
                        comfortable_hours: &z.comfortable,
                        cold_hours: &z.cold,
                        hot_hours: &z.hot,
                        sun_hours: &z.sun,
                        mean_operative_temperature: z
                            .operative_sum
                            .iter()
                            .map(|&s| {
                                if has_stats {
                                    round(s / occupied, 2)
                                } else {
                                    f32::NAN
                                }
                            })
                            .collect(),
                        max_operative_temperature: z
                            .operative_max
                            .iter()
                            .map(|&m| {
                                if has_stats {
                                    round(m as f64, 2)
                                } else {
                                    f32::NAN
                                }
                            })
                            .collect(),
                    },
                }
            })
            .collect();

        let out = Output {
            format: FORMAT,
            version: FORMAT_VERSION,
            note: PHYSICS_NOTE,
            settings: SettingsOut {
                spacing: self.settings.spacing,
                wall_offset: self.settings.wall_offset,
                height: self
                    .settings
                    .height
                    .unwrap_or_else(|| self.settings.occupant.default_height()),
                occupant: self.settings.occupant,
                pmv_limit: self.pmv_limit,
            },
            time: TimeOut {
                start_hour_of_year: self.start_hour_of_year.unwrap_or(0),
                hours: self.sun[0].len(),
                resolution: "hourly mean",
            },
            sun: SunOut {
                x: &self.sun[0],
                y: &self.sun[1],
                z: &self.sun[2],
            },
            surfaces,
            zones,
            skipped_zones: self
                .skipped
                .iter()
                .map(|(name, reason)| SkippedOut { name, reason })
                .collect(),
        };

        let file = std::fs::File::create(path)?;
        let mut w = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut w, &out).map_err(std::io::Error::other)?;
        w.flush()
    }
}

/// Grid for one zone, with the envelope surface index of each grid column.
fn grid_zone(
    env: &BuildingEnvelope,
    zone_index: usize,
    settings: &GridSettings,
) -> Result<(ZoneGrid, Vec<usize>), String> {
    let zone = &env.zones[zone_index];
    // Internal mass is generated without geometry; it adds thermal mass to
    // the zone but is not something an occupant sees at a known place.
    let internal_mass: HashSet<String> = (1..=zone.input.internal_mass.len())
        .map(|i| format!("{} IntMass {}", zone.input.name, i))
        .collect();
    let members: Vec<usize> = env
        .surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| s.input.zone == zone.input.name && !internal_mass.contains(&s.input.name))
        .map(|(i, _)| i)
        .collect();

    let missing = members
        .iter()
        .filter(|&&i| {
            env.surfaces[i]
                .input
                .vertices
                .as_ref()
                .is_none_or(|v| v.len() < 3)
        })
        .count();
    if missing > 0 {
        return Err(format!("{missing} surface(s) have no vertices"));
    }

    let position: HashMap<&str, usize> = members
        .iter()
        .enumerate()
        .map(|(k, &i)| (env.surfaces[i].input.name.as_str(), k))
        .collect();
    let grid_surfaces: Vec<GridSurface> = members
        .iter()
        .map(|&i| {
            let s = &env.surfaces[i].input;
            GridSurface {
                vertices: s.vertices.clone().unwrap_or_default(),
                surface_type: s.surface_type,
                host: s
                    .parent_surface
                    .as_deref()
                    .and_then(|p| position.get(p).copied()),
            }
        })
        .collect();
    let grid = ZoneGrid::build(&grid_surfaces, settings).map_err(|e| e.to_string())?;
    Ok((grid, members))
}

/// Whether a window's construction has a physical shade layer. Known before
/// the first timestep, unlike `SurfaceState::shade`, which the blind control
/// fills in lazily.
fn shade_configured(env: &BuildingEnvelope, si: usize) -> bool {
    let s = &env.surfaces[si];
    s.is_window
        && env
            .window_constructions
            .get(&s.input.construction)
            .and_then(|wc| wc.shading_control.as_ref())
            .is_some_and(|c| c.shade.is_some())
}

fn people_of(gains: &[InternalGainInput]) -> Vec<(f64, Option<String>)> {
    gains
        .iter()
        .filter_map(|g| match g {
            InternalGainInput::People {
                count, schedule, ..
            } => Some((*count, schedule.clone())),
            _ => None,
        })
        .collect()
}

/// Share of the zone's design occupants present [0–1]; 0 for a zone with no
/// people, which then has no occupied hours.
fn occupancy_fraction(
    people: &[(f64, Option<String>)],
    env: &BuildingEnvelope,
    hour: u32,
    dow: u32,
    doy: u32,
) -> f64 {
    let design: f64 = people.iter().map(|(c, _)| c).sum();
    if design <= 0.0 {
        return 0.0;
    }
    let present: f64 = people
        .iter()
        .map(|(count, schedule)| {
            let frac = schedule
                .as_deref()
                .map(|name| env.schedule_manager.fraction_doy(name, hour, dow, doy))
                .unwrap_or(1.0);
            count * frac
        })
        .sum();
    present / design
}

fn round(x: f64, decimals: i32) -> f32 {
    let f = 10f64.powi(decimals);
    ((x * f).round() / f) as f32
}

fn point(p: &Point3D) -> [f32; 3] {
    [round(p.x, 3), round(p.y, 3), round(p.z, 3)]
}

fn surface_kind(t: SurfaceType) -> &'static str {
    match t {
        SurfaceType::Wall => "wall",
        SurfaceType::Floor => "floor",
        SurfaceType::Roof => "roof",
        SurfaceType::Ceiling => "ceiling",
        SurfaceType::Window => "window",
    }
}

#[derive(Serialize)]
struct Output<'a> {
    format: &'static str,
    version: u32,
    note: &'static str,
    settings: SettingsOut,
    time: TimeOut,
    /// Unit vector toward the sun at mid-hour; zero when the sun is down.
    sun: SunOut<'a>,
    surfaces: Vec<SurfaceOut<'a>>,
    zones: Vec<ZoneOut<'a>>,
    skipped_zones: Vec<SkippedOut<'a>>,
}

#[derive(Serialize)]
struct SettingsOut {
    spacing: f64,
    wall_offset: f64,
    height: f64,
    occupant: OccupantModel,
    pmv_limit: f64,
}

#[derive(Serialize)]
struct TimeOut {
    start_hour_of_year: u32,
    hours: usize,
    resolution: &'static str,
}

#[derive(Serialize)]
struct SunOut<'a> {
    x: &'a [f32],
    y: &'a [f32],
    z: &'a [f32],
}

#[derive(Serialize)]
struct SurfaceOut<'a> {
    name: &'a str,
    zone: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    vertices: Vec<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inside_temperature: Option<&'a [f32]>,
    /// Transmitted beam irradiance normal to the beam [W/m²], windows only.
    #[serde(skip_serializing_if = "Option::is_none")]
    beam_irradiance: Option<&'a [f32]>,
    /// Hourly fraction of the time a shade layer was down (#144); only for
    /// windows that have one. `inside_temperature` is the shade's while down.
    #[serde(skip_serializing_if = "Option::is_none")]
    shade_deployed: Option<&'a [f32]>,
}

#[derive(Serialize)]
struct ComfortOut {
    metabolic_rate: f64,
    clothing: f64,
    air_velocity: f64,
    solar_absorptivity: f64,
}

#[derive(Serialize)]
struct ZoneOut<'a> {
    name: &'a str,
    comfort: ComfortOut,
    coverage_min: f32,
    points: Vec<[f32; 3]>,
    /// Index into `surfaces` for each view-factor column.
    surfaces: Vec<usize>,
    /// Row-major `[point][surface]`.
    view_factors: Vec<f32>,
    air_temperature: &'a [f32],
    relative_humidity: &'a [f32],
    occupancy: &'a [f32],
    aggregates: AggregatesOut<'a>,
}

#[derive(Serialize)]
struct AggregatesOut<'a> {
    occupied_hours: u32,
    comfortable_hours: &'a [u32],
    cold_hours: &'a [u32],
    hot_hours: &'a [u32],
    sun_hours: &'a [u32],
    mean_operative_temperature: Vec<f32>,
    max_operative_temperature: Vec<f32>,
}

#[derive(Serialize)]
struct SkippedOut<'a> {
    name: &'a str,
    reason: &'a str,
}
