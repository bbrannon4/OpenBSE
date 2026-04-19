//! Heat balance solver — orchestrates surface and zone heat balance.
//!
//! Per-timestep algorithm:
//! 1. Calculate solar position and incident solar on each surface
//! 2. Compute internal gains and infiltration per zone
//! 3. Apply HVAC conditions from controls (or use ideal loads)
//! 4. Iterate surface ↔ zone coupling (5 iterations):
//!    a. Outside surface heat balance (solar + convection + conduction)
//!    b. CTF conduction flux from outside to inside
//!    c. Inside surface heat balance (conduction + convection + radiation + solar)
//!    d. Zone air heat balance (with ideal loads if configured)
//! 5. Update CTF histories and zone previous temperatures
//! 6. Return zone temps, humidity, and loads

use openbse_core::ports::{EnvelopeResults, EnvelopeSolver, SimulationContext, ZoneHvacConditions};
use openbse_psychrometrics as psych;
use openbse_weather::WeatherHour;
use std::collections::HashMap;

use crate::convection;
use crate::ctf::{apply_ctf, calculate_ctf, calculate_ctf_simple, CtfCoefficients, CtfHistory};
use crate::geometry;
use crate::ground_temp::GroundTempModel;
use crate::infiltration;
use crate::internal_gains;
use crate::material::{Construction, Material, Roughness, SimpleConstruction, WindowConstruction};
use crate::schedule::{day_of_week, ScheduleManager};
use crate::shading;
use crate::solar;
use crate::surface::{BoundaryCondition, SurfaceInput, SurfaceState, SurfaceType};
use crate::zone::{InteriorSolarDistribution, ZoneInput, ZoneState};

/// The building envelope heat balance solver.
#[derive(Debug, Clone)]
pub struct BuildingEnvelope {
    pub zones: Vec<ZoneState>,
    pub surfaces: Vec<SurfaceState>,
    pub ctf_coefficients: Vec<Option<CtfCoefficients>>,
    pub ctf_histories: Vec<Option<CtfHistory>>,
    pub materials: HashMap<String, Material>,
    pub constructions: HashMap<String, Construction>,
    pub window_constructions: HashMap<String, WindowConstruction>,
    pub simple_constructions: HashMap<String, SimpleConstruction>,
    /// F-factor ground floor constructions.
    pub f_factor_constructions: HashMap<String, crate::material::FFactorConstruction>,
    pub zone_index: HashMap<String, usize>,
    /// Ground temperature model (Kusuda-Achenbach)
    pub ground_temp_model: Option<GroundTempModel>,
    /// Schedule manager for time-varying internal gains, exhaust fans, etc.
    pub schedule_manager: ScheduleManager,
    /// Site latitude [degrees]
    pub latitude: f64,
    /// Site longitude [degrees]
    pub longitude: f64,
    /// Site time zone [hours from GMT]
    pub time_zone: f64,
    /// Ground reflectance [0-1]
    pub ground_reflectance: f64,
    /// Day of week for January 1 (1=Mon .. 7=Sun). From weather file.
    pub jan1_dow: u32,
    /// Timestep duration [s]
    pub dt: f64,
    pub initialized: bool,
    /// Solar shading calculation mode (basic = no shadows, detailed = polygon clipping).
    pub shading_calculation: shading::ShadingCalculation,
    /// Resolved shading polygons for shadow calculations.
    /// Includes explicit shading surfaces, auto-generated overhangs/fins,
    /// and building self-shading surfaces (all outdoor surfaces with vertices).
    pub shading_polygons: Vec<shading::ShadingPolygon>,
    /// Site terrain for wind profile calculations.
    /// Determines wind profile exponent and boundary layer height.
    pub terrain: convection::Terrain,
    /// Site elevation above sea level [m].
    /// Used for air mass correction in Perez sky model (matching E+).
    pub elevation: f64,
    /// Per-zone interior view factors for radiation exchange.
    /// `Some` for zones where vertex-based box geometry is available;
    /// `None` for zones that fall back to area-weighted MRT.
    pub zone_view_factors: Vec<Option<ZoneViewFactors>>,
    /// Interzone surface pairs: `interzone_pairs[i] = Some(j)` means surface i
    /// and surface j are opposite faces of the same interzone wall.
    /// Matches EnergyPlus behaviour where `T_outside[i] = T_inside[j]` and vice versa,
    /// so the CTF cross-coupling term Y[0] correctly connects the two zones
    /// through a single wall thickness.
    pub interzone_pairs: Vec<Option<usize>>,
    /// Gross wall and window areas by cardinal direction (N, E, S, W).
    /// Used for window-to-wall ratio reporting.
    pub envelope_areas: crate::geometry::EnvelopeAreas,
    /// Infiltration interaction mode: how infiltration combines with exhaust.
    pub infiltration_interaction: crate::zone_loads::InfiltrationInteraction,
    /// Multizone airflow network (None when AFN is disabled).
    pub airflow_network: Option<crate::airflow_network::AirflowNetwork>,
    /// Solar distribution method for interior beam solar radiation.
    /// FullExterior: all beam to floor.  FullInteriorAndExterior: geometric projection.
    pub solar_distribution_method: SolarDistributionMethod,
    /// Holiday dates as `(month, day)` pairs.  When the current simulation date
    /// matches any entry, schedule lookups use day_of_week = 8 (Holiday).
    pub holiday_set: std::collections::HashSet<(u32, u32)>,
    /// Per-surface CTF inside/outside heat flux [W/m²] from the last solve.
    /// Used by update_bdf_history() to shift CTF history once after HVAC convergence.
    ctf_q_last_inside: Vec<f64>,
    ctf_q_last_outside: Vec<f64>,
    /// Precomputed sunlit fractions for each surface at each timestep.
    /// When populated, avoids expensive per-timestep polygon clipping.
    /// Indexed by global timestep index → per-surface sunlit fraction.
    pub solar_cache: Option<SolarCache>,
}

/// Precomputed solar data for all annual timesteps.
///
/// Caches per-surface sunlit fractions (the expensive polygon clipping
/// computation) for the entire year. Solar position and Perez decomposition
/// remain computed inline since they're cheap arithmetic.
///
/// Analogous to IES-VE's Suncast preprocessor: compute all shadow geometry
/// upfront, then look up during the thermal simulation.
///
/// Supports disk persistence: saves to a `.solar` file next to the YAML input
/// and reloads on subsequent runs if the geometry fingerprint matches.
/// This avoids recomputing shadow geometry when only schedules, setpoints,
/// or HVAC parameters change between runs.
///
/// Memory footprint: ~N_surfaces × N_timesteps × 8 bytes.
/// For 50 surfaces × 52,560 timesteps = ~20 MB.
#[derive(Debug, Clone)]
pub struct SolarCache {
    /// Sunlit fractions indexed by [timestep_index][surface_index].
    /// Each inner Vec has length = number of surfaces.
    pub sunlit_fractions: Vec<Vec<f64>>,
    /// Number of timesteps per hour used when building the cache.
    pub timesteps_per_hour: u32,
    /// Start hour index in the annual simulation.
    pub start_hour: usize,
    /// Geometry fingerprint: hash of all inputs that affect shadow calculations.
    /// If this doesn't match, the cache is stale and must be recomputed.
    pub fingerprint: u64,
}

impl SolarCache {
    /// Magic bytes identifying a solar cache file.
    const MAGIC: &[u8; 8] = b"OBSE_SOL";
    /// File format version.
    const VERSION: u32 = 1;

    /// Compute a geometry fingerprint from all inputs that affect shadow calcs.
    ///
    /// Changes to any of these invalidate the cache:
    /// - Surface vertices and boundary conditions
    /// - Shading polygon geometry
    /// - Site latitude, longitude, timezone
    /// - Timestep configuration (start, end, resolution)
    pub fn compute_fingerprint(
        surfaces: &[SurfaceState],
        shading_polygons: &[shading::ShadingPolygon],
        latitude: f64,
        longitude: f64,
        time_zone: f64,
        timesteps_per_hour: u32,
        start_hour: usize,
        end_hour: usize,
    ) -> u64 {
        // Simple FNV-1a hash — fast, no external deps, good distribution
        let mut hash: u64 = 0xcbf29ce484222325;
        let mix = |h: &mut u64, bytes: &[u8]| {
            for &b in bytes {
                *h ^= b as u64;
                *h = h.wrapping_mul(0x100000001b3);
            }
        };

        // Hash surface geometry
        for surf in surfaces {
            if let Some(ref verts) = surf.input.vertices {
                for v in verts {
                    mix(&mut hash, &v.x.to_le_bytes());
                    mix(&mut hash, &v.y.to_le_bytes());
                    mix(&mut hash, &v.z.to_le_bytes());
                }
            }
            mix(&mut hash, &surf.input.area.to_le_bytes());
            mix(&mut hash, &surf.input.azimuth.to_le_bytes());
            mix(&mut hash, &surf.input.tilt.to_le_bytes());
            let is_outdoor = if surf.input.boundary == BoundaryCondition::Outdoor {
                1u8
            } else {
                0u8
            };
            mix(&mut hash, &[is_outdoor]);
        }

        // Hash shading polygons
        for sp in shading_polygons {
            for v in &sp.vertices {
                mix(&mut hash, &v.x.to_le_bytes());
                mix(&mut hash, &v.y.to_le_bytes());
                mix(&mut hash, &v.z.to_le_bytes());
            }
        }

        // Hash site parameters
        mix(&mut hash, &latitude.to_le_bytes());
        mix(&mut hash, &longitude.to_le_bytes());
        mix(&mut hash, &time_zone.to_le_bytes());
        mix(&mut hash, &timesteps_per_hour.to_le_bytes());
        mix(&mut hash, &(start_hour as u64).to_le_bytes());
        mix(&mut hash, &(end_hour as u64).to_le_bytes());

        hash
    }

    /// Save the solar cache to a binary file.
    ///
    /// Format: MAGIC(8) | VERSION(4) | fingerprint(8) | timesteps_per_hour(4) |
    ///         start_hour(8) | n_timesteps(8) | n_surfaces(8) |
    ///         sunlit_fractions[n_timesteps * n_surfaces](f64 each)
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        use std::io::Write;
        let mut file = std::fs::File::create(path)
            .map_err(|e| format!("Failed to create solar cache file: {e}"))?;
        let mut buf = std::io::BufWriter::new(&mut file);

        buf.write_all(Self::MAGIC).map_err(|e| e.to_string())?;
        buf.write_all(&Self::VERSION.to_le_bytes())
            .map_err(|e| e.to_string())?;
        buf.write_all(&self.fingerprint.to_le_bytes())
            .map_err(|e| e.to_string())?;
        buf.write_all(&self.timesteps_per_hour.to_le_bytes())
            .map_err(|e| e.to_string())?;
        buf.write_all(&(self.start_hour as u64).to_le_bytes())
            .map_err(|e| e.to_string())?;

        let n_timesteps = self.sunlit_fractions.len() as u64;
        let n_surfaces = if n_timesteps > 0 {
            self.sunlit_fractions[0].len() as u64
        } else {
            0
        };
        buf.write_all(&n_timesteps.to_le_bytes())
            .map_err(|e| e.to_string())?;
        buf.write_all(&n_surfaces.to_le_bytes())
            .map_err(|e| e.to_string())?;

        for step in &self.sunlit_fractions {
            for &val in step {
                buf.write_all(&val.to_le_bytes())
                    .map_err(|e| e.to_string())?;
            }
        }

        buf.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Load a solar cache from a binary file, validating the fingerprint.
    ///
    /// Returns `None` if the file doesn't exist, is corrupted, or has a
    /// different fingerprint (geometry has changed).
    pub fn load_from_file(path: &std::path::Path, expected_fingerprint: u64) -> Option<Self> {
        use std::io::Read;
        let mut file = std::fs::File::open(path).ok()?;
        let mut buf = std::io::BufReader::new(&mut file);

        let mut magic = [0u8; 8];
        buf.read_exact(&mut magic).ok()?;
        if &magic != Self::MAGIC {
            return None;
        }

        let mut version_bytes = [0u8; 4];
        buf.read_exact(&mut version_bytes).ok()?;
        let version = u32::from_le_bytes(version_bytes);
        if version != Self::VERSION {
            return None;
        }

        let mut fp_bytes = [0u8; 8];
        buf.read_exact(&mut fp_bytes).ok()?;
        let fingerprint = u64::from_le_bytes(fp_bytes);
        if fingerprint != expected_fingerprint {
            log::info!("Solar cache fingerprint mismatch (geometry changed), will recompute");
            return None;
        }

        let mut u32_buf = [0u8; 4];
        buf.read_exact(&mut u32_buf).ok()?;
        let timesteps_per_hour = u32::from_le_bytes(u32_buf);

        let mut u64_buf = [0u8; 8];
        buf.read_exact(&mut u64_buf).ok()?;
        let start_hour = u64::from_le_bytes(u64_buf) as usize;

        buf.read_exact(&mut u64_buf).ok()?;
        let n_timesteps = u64::from_le_bytes(u64_buf) as usize;

        buf.read_exact(&mut u64_buf).ok()?;
        let n_surfaces = u64::from_le_bytes(u64_buf) as usize;

        let mut sunlit_fractions = Vec::with_capacity(n_timesteps);
        let mut f64_buf = [0u8; 8];
        for _ in 0..n_timesteps {
            let mut step = Vec::with_capacity(n_surfaces);
            for _ in 0..n_surfaces {
                buf.read_exact(&mut f64_buf).ok()?;
                step.push(f64::from_le_bytes(f64_buf));
            }
            sunlit_fractions.push(step);
        }

        Some(SolarCache {
            sunlit_fractions,
            timesteps_per_hour,
            start_hour,
            fingerprint,
        })
    }
}

/// Interior view factor data for a zone modeled as a rectangular box.
///
/// Uses analytical view factor formulas for parallel and perpendicular
/// rectangles to compute face-to-face radiation exchange factors.
/// Per-surface view factors are derived using uniform irradiation
/// assumption: F(surf_i → surf_j) = F(face_i → face_j) × A_j / A_face_j.
///
/// This replaces the area-weighted MRT approximation with proper geometric
/// view factors, matching the physics of EnergyPlus ScriptF for box zones.
#[derive(Debug, Clone)]
pub struct ZoneViewFactors {
    /// 6×6 face-to-face view factor matrix.
    /// Indices: 0=floor, 1=ceiling, 2=south, 3=north, 4=east, 5=west.
    pub face_vf: [[f64; 6]; 6],
    /// Total area of each face [m²] (sum of all surfaces on that face).
    pub face_area: [f64; 6],
}

/// Solar distribution method for interior beam solar radiation.
///
/// Matches EnergyPlus `Building` → `Solar Distribution` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolarDistributionMethod {
    /// All beam solar falls on the floor (E+ "FullExterior").
    /// This is the default and most common E+ setting.
    FullExterior,
    /// Beam solar is geometrically projected onto interior surfaces
    /// (E+ "FullInteriorAndExterior").
    FullInteriorAndExterior,
}

impl Default for SolarDistributionMethod {
    fn default() -> Self {
        SolarDistributionMethod::FullExterior
    }
}

/// Sealed air gap conductance using ISO 15099 model.
///
/// Computes the total gap heat transfer coefficient [W/(m²·K)] from convection
/// and radiation through a sealed gas gap between two glass panes.
///
/// This matches the EnergyPlus `WindowManager.cc` implementation which uses
/// ISO 15099 gas property correlations and Nusselt number formulas for sealed
/// glazing gaps.
///
/// # Arguments
/// * `gap_width` — Gap width [m] (e.g. 0.012 for 12mm air gap)
/// * `t_mean_c` — Mean gap temperature [°C]
/// * `delta_t` — Absolute temperature difference across gap [°C]
/// * `tilt_deg` — Surface tilt [degrees from horizontal: 0=face-up, 90=vertical]
/// * `emissivity` — Glass emissivity of gap-facing surfaces (same for both faces
///   for standard clear glass; for low-e, pass the effective emissivity)
///
/// # Returns
/// Total gap conductance h_gap = h_conv + h_rad [W/(m²·K)]
///
/// # Reference
/// ISO 15099:2003 Section 5.3.3.3 and Table C.1.
/// EnergyPlus WindowManager.cc `NusseltNumber()` and `CalcWindowHeatBalance()`.
fn sealed_air_gap_conductance(
    gap_width: f64,
    t_mean_c: f64,
    delta_t: f64,
    tilt_deg: f64,
    emissivity: f64,
) -> f64 {
    const SIGMA: f64 = 5.6704e-8;
    const G: f64 = 9.81;

    let t_k = t_mean_c + 273.15;
    let dt = delta_t.abs().max(0.01); // Avoid zero ΔT
    let s = gap_width.max(0.001); // Gap width [m]

    // ── ISO 15099 Table C.1: Dry air properties ──────────────────────
    let k_air = 2.873e-3 + 7.76e-5 * t_k; // [W/(m·K)]
    let mu = 3.723e-6 + 4.94e-8 * t_k; // [Pa·s]
    let rho = 101325.0 / (287.05 * t_k); // [kg/m³]
    let cp = 1002.737 + 1.2324e-2 * t_k; // [J/(kg·K)]

    // Derived transport properties
    let nu = mu / rho; // kinematic viscosity [m²/s]
    let alpha = k_air / (rho * cp); // thermal diffusivity [m²/s]
    let beta = 1.0 / t_k; // thermal expansion [1/K]

    // ── Rayleigh number ──────────────────────────────────────────────
    let ra = (G * beta * dt * s.powi(3) / (nu * alpha)).max(0.0);

    // ── Nusselt number (E+ WindowManager.cc NusseltNumber()) ─────────
    //
    // For tilt ≥ 60° (vertical and near-vertical):
    //   Ra > 2e5:  Nu = 0.073 × Ra^(1/3)
    //   Ra > 1e4:  Nu = 0.028154 × Ra^0.4134
    //   Ra ≤ 1e4:  Nu = 1 + 1.7596678e-10 × Ra^2.2984755  (ISO 15099 Eq A1.3.1)
    //
    // For 0° < tilt < 60° (inclined layers): Hollands et al. (1976)
    //
    // For tilt = 0° (horizontal, heated below): Nu = 1 + 1.44*(1-1708/Ra)+ ...
    let nu_gap = if tilt_deg >= 60.0 {
        // Vertical / near-vertical
        if ra > 2.0e5 {
            0.073 * ra.powf(1.0 / 3.0)
        } else if ra > 1.0e4 {
            0.028154 * ra.powf(0.4134)
        } else {
            1.0 + 1.7596678e-10 * ra.powf(2.2984755)
        }
    } else if tilt_deg > 0.0 {
        // Inclined layer: Hollands et al. (1976) correlation
        // Nu = 1 + 1.44*(1 - 1708·sin(1.8·tilt)^1.6 / Ra)·max(0, 1-1708/Ra)
        //     + max(0, (Ra·cos(tilt)/5830)^(1/3) - 1)
        let tilt_rad = tilt_deg.to_radians();
        let cos_t = tilt_rad.cos().max(0.001);
        let sin_18t = (1.8 * tilt_rad).sin();

        let term1 = if ra > 0.01 {
            let ratio = 1708.0 / ra;
            let sin_factor = 1708.0 * sin_18t.powf(1.6) / ra;
            1.44 * (1.0 - sin_factor) * (1.0 - ratio).max(0.0)
        } else {
            0.0
        };

        let term2 = if ra * cos_t > 0.01 {
            ((ra * cos_t / 5830.0).powf(1.0 / 3.0) - 1.0).max(0.0)
        } else {
            0.0
        };

        1.0 + term1 + term2
    } else {
        // Horizontal (face up, heated below): Hollands with tilt=0
        let term1 = if ra > 1708.0 {
            1.44 * (1.0 - 1708.0 / ra)
        } else {
            0.0
        };
        let term2 = if ra > 5830.0 {
            (ra / 5830.0).powf(1.0 / 3.0) - 1.0
        } else {
            0.0
        };
        1.0 + term1 + term2
    }
    .max(1.0); // Nusselt number ≥ 1 (pure conduction minimum)

    // ── Gap convection ───────────────────────────────────────────────
    let h_conv = nu_gap * k_air / s;

    // ── Gap radiation (linearized between two parallel pane surfaces) ─
    // h_rad = 4·σ·T_mean³ / (1/ε₁ + 1/ε₂ - 1)
    // For standard clear glass: ε₁ = ε₂ = 0.84
    let eps = emissivity.clamp(0.01, 1.0);
    let denom = 2.0 / eps - 1.0; // = 1/ε + 1/ε - 1 for symmetric emissivity
    let h_rad = 4.0 * SIGMA * t_k.powi(3) / denom;

    h_conv + h_rad
}

impl BuildingEnvelope {
    /// Create from input data.
    pub fn from_input(
        materials: Vec<Material>,
        constructions: Vec<Construction>,
        window_constructions: Vec<WindowConstruction>,
        zones: Vec<ZoneInput>,
        surfaces: Vec<SurfaceInput>,
        latitude: f64,
        longitude: f64,
        time_zone: f64,
    ) -> Self {
        Self::from_input_full(
            materials,
            constructions,
            window_constructions,
            vec![],
            vec![],
            zones,
            surfaces,
            latitude,
            longitude,
            time_zone,
            0.0,
            SolarDistributionMethod::default(),
        )
    }

    /// Create from input data with simple constructions.
    pub fn from_input_full(
        materials: Vec<Material>,
        constructions: Vec<Construction>,
        window_constructions: Vec<WindowConstruction>,
        simple_constructions: Vec<SimpleConstruction>,
        f_factor_constructions: Vec<crate::material::FFactorConstruction>,
        zones: Vec<ZoneInput>,
        mut surfaces: Vec<SurfaceInput>,
        latitude: f64,
        longitude: f64,
        time_zone: f64,
        elevation: f64,
        solar_distribution_method: SolarDistributionMethod,
    ) -> Self {
        let material_map: HashMap<String, Material> =
            materials.into_iter().map(|m| (m.name.clone(), m)).collect();
        let construction_map: HashMap<String, Construction> = constructions
            .into_iter()
            .map(|c| (c.name.clone(), c))
            .collect();
        let window_map: HashMap<String, WindowConstruction> = window_constructions
            .into_iter()
            .map(|w| (w.name.clone(), w))
            .collect();
        let simple_map: HashMap<String, SimpleConstruction> = simple_constructions
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect();
        let f_factor_map: HashMap<String, crate::material::FFactorConstruction> =
            f_factor_constructions
                .into_iter()
                .map(|f| (f.name.clone(), f))
                .collect();

        let initial_temp = 21.0;

        // Resolve geometry from vertices (if present)
        for surf in &mut surfaces {
            surf.resolve_geometry();
        }

        // Build zone states
        let mut zone_states: Vec<ZoneState> = Vec::new();
        let mut zone_index: HashMap<String, usize> = HashMap::new();
        for (i, z) in zones.into_iter().enumerate() {
            zone_index.insert(z.name.clone(), i);
            zone_states.push(ZoneState::new(z, initial_temp));
        }

        // Auto-calculate zone floor area and volume from surface vertices.
        //
        // Floor area: sum of floor-type surface areas (Newell's method — correct
        // for arbitrary planar polygons including non-convex shapes).
        //
        // Volume: floor_area × height, where height = max(z) - min(z) across all
        // zone surface vertices.  This is far more robust than the divergence
        // theorem, which requires a fully closed polyhedron (all 6 faces present
        // with consistent outward normals).  Most zone geometries have missing
        // interior walls, so the divergence theorem produces wrong results.
        for zone in &mut zone_states {
            let zone_surfaces: Vec<&SurfaceInput> = surfaces
                .iter()
                .filter(|s| s.zone == zone.input.name)
                .collect();

            if zone_surfaces.is_empty() {
                continue;
            }

            // ── Collect all vertex z-coordinates to determine zone height ──
            let all_verts: Vec<&Vec<geometry::Point3D>> = zone_surfaces
                .iter()
                .filter(|s| s.surface_type != SurfaceType::Window)
                .filter_map(|s| s.vertices.as_ref())
                .filter(|v| v.len() >= 3)
                .collect();

            let z_min = all_verts
                .iter()
                .flat_map(|vs| vs.iter().map(|v| v.z))
                .fold(f64::INFINITY, f64::min);
            let z_max = all_verts
                .iter()
                .flat_map(|vs| vs.iter().map(|v| v.z))
                .fold(f64::NEG_INFINITY, f64::max);
            let zone_height = if z_max > z_min { z_max - z_min } else { 0.0 };

            // ── Auto-calculate floor area (from floor-type surfaces) ──
            if zone.input.floor_area <= 0.0 {
                let floor_verts: Vec<Vec<geometry::Point3D>> = zone_surfaces
                    .iter()
                    .filter(|s| s.surface_type == SurfaceType::Floor)
                    .filter_map(|s| s.vertices.clone())
                    .collect();
                if !floor_verts.is_empty() {
                    let refs: Vec<&[geometry::Point3D]> =
                        floor_verts.iter().map(|v| v.as_slice()).collect();
                    let area = geometry::zone_floor_area(&refs);
                    if area > 0.0 {
                        zone.input.floor_area = area;
                        log::info!(
                            "Auto-calculated zone '{}' floor area: {:.1} m²",
                            zone.input.name,
                            area
                        );
                    }
                }
            }

            // ── Auto-calculate volume ──
            if zone.input.volume <= 0.0 {
                // Primary method: floor_area × height (works for any zone with
                // a floor surface and identifiable height, even with missing walls)
                if zone.input.floor_area > 0.0 && zone_height > 0.0 {
                    zone.input.volume = zone.input.floor_area * zone_height;
                    log::info!("Auto-calculated zone '{}' volume: {:.1} m³ (floor_area {:.1} × height {:.2})",
                        zone.input.name, zone.input.volume, zone.input.floor_area, zone_height);
                } else if !all_verts.is_empty() {
                    // Fallback: divergence theorem (only reliable for closed polyhedra)
                    let vert_sets: Vec<Vec<geometry::Point3D>> = zone_surfaces
                        .iter()
                        .filter(|s| s.surface_type != SurfaceType::Window)
                        .filter_map(|s| s.vertices.clone())
                        .collect();
                    let refs: Vec<&[geometry::Point3D]> =
                        vert_sets.iter().map(|v| v.as_slice()).collect();
                    let vol = geometry::zone_volume_from_surfaces(&refs);
                    if vol > 0.0 {
                        zone.input.volume = vol;
                        log::warn!(
                            "Zone '{}': using divergence-theorem volume ({:.1} m³) — \
                            no floor surfaces found for floor_area × height method. \
                            Result may be inaccurate if zone is not a closed polyhedron.",
                            zone.input.name,
                            vol
                        );
                    }
                }

                if zone.input.volume <= 0.0 {
                    log::warn!(
                        "Zone '{}': could not auto-calculate volume (no surfaces with vertices)",
                        zone.input.name
                    );
                }
            }
        }

        // ── Generate synthetic adiabatic surfaces for internal mass ──────
        //
        // EnergyPlus `InternalMass` objects represent furniture, contents, and
        // internal partitions that add thermal capacitance to a zone.  They are
        // modeled as adiabatic surfaces: both sides face the same zone, so
        // T_outside = T_inside at every timestep.  CTF coefficients capture the
        // thermal lag through the mass.
        //
        // Without internal mass the zone air responds almost instantly to solar
        // gains and outdoor temperature swings, producing HVAC loads 2-7× too
        // high compared to EnergyPlus.
        for zone in &zone_states {
            for (im_idx, im) in zone.input.internal_mass.iter().enumerate() {
                let name = format!("{} IntMass {}", zone.input.name, im_idx + 1);
                log::info!(
                    "Zone '{}': adding internal mass surface '{}' — {:.1} m², construction '{}'",
                    zone.input.name,
                    name,
                    im.area,
                    im.construction,
                );
                let surf = SurfaceInput {
                    name,
                    zone: zone.input.name.clone(),
                    surface_type: SurfaceType::Wall, // arbitrary; adiabatic ignores orientation
                    construction: im.construction.clone(),
                    area: im.area,
                    azimuth: 0.0,
                    tilt: 90.0,
                    boundary: BoundaryCondition::Adiabatic,
                    parent_surface: None,
                    vertices: None,
                    shading: None,
                    sun_exposure: false,
                    wind_exposure: false,
                    exposed_perimeter: None,
                    airflow: None,
                    height: None,
                    width: None,
                };
                surfaces.push(surf);
            }
        }

        // Auto-calculate solar distribution if not specified
        for zone in &mut zone_states {
            if zone.input.solar_distribution.is_none() {
                let zone_surfaces: Vec<&SurfaceInput> = surfaces
                    .iter()
                    .filter(|s| s.zone == zone.input.name)
                    .filter(|s| s.surface_type != SurfaceType::Window)
                    .collect();

                if !zone_surfaces.is_empty() {
                    let mut floor_area = 0.0_f64;
                    let mut wall_area = 0.0_f64;
                    let mut ceiling_area = 0.0_f64;

                    for s in &zone_surfaces {
                        // Use net area (area minus windows) if available via parent subtraction,
                        // otherwise use the surface's declared area
                        let area = s.area;
                        match s.surface_type {
                            SurfaceType::Floor => floor_area += area,
                            SurfaceType::Wall => wall_area += area,
                            SurfaceType::Roof | SurfaceType::Ceiling => ceiling_area += area,
                            _ => {}
                        }
                    }

                    let total_area = floor_area + wall_area + ceiling_area;
                    if total_area > 0.0 {
                        let dist = InteriorSolarDistribution {
                            floor_fraction: floor_area / total_area,
                            wall_fraction: wall_area / total_area,
                            ceiling_fraction: ceiling_area / total_area,
                        };
                        log::info!(
                            "Auto-calculated solar distribution for zone '{}': \
                             floor={:.1}%, wall={:.1}%, ceiling={:.1}%",
                            zone.input.name,
                            dist.floor_fraction * 100.0,
                            dist.wall_fraction * 100.0,
                            dist.ceiling_fraction * 100.0,
                        );
                        zone.input.solar_distribution = Some(dist);
                    }
                }
            }
        }

        // Build surface states
        let mut surface_states: Vec<SurfaceState> = Vec::new();
        for surf_input in &surfaces {
            let is_window = surf_input.surface_type == SurfaceType::Window;

            // Window solar transmittance ratio (τ_sol / SHGC)
            let win_solar_trans_ratio = if is_window {
                let wc = window_map.get(&surf_input.construction);
                wc.map(|w| w.solar_transmittance_ratio()).unwrap_or(1.0)
            } else {
                1.0
            };

            // Window-specific absorptance properties (used below)
            let (win_solar_absorptance, win_inside_fraction) = if is_window {
                let wc = window_map.get(&surf_input.construction);
                let (abs, frac) = wc
                    .map(|w| (w.effective_solar_absorptance(), w.inside_absorbed_fraction))
                    .unwrap_or((0.06, 0.5));
                (abs, frac)
            } else {
                (0.0, 0.0)
            };

            // Pre-compute glass angular parameters (kd, N_i, n_eff) for Fresnel SHGC model
            let (glass_kd, glass_ni, glass_n) = if is_window {
                let wc = window_map.get(&surf_input.construction);
                wc.map(|w| {
                    crate::solar::compute_glass_angular_params(
                        w.shgc,
                        w.pane_solar_transmittance,
                        w.pane_solar_reflectance,
                    )
                })
                .unwrap_or((0.0, 0.0, 1.526))
            } else {
                (0.0, 0.0, 1.526)
            };

            // Build E+ SimpleGlazingSystem angular model for windows without
            // per-pane optical properties (Method 3 in compute_glass_angular_params).
            // When per-pane properties are available, the Fresnel model is used instead.
            let sgs_model = if is_window && glass_kd < 1e-12 && glass_ni < 1e-12 {
                let wc = window_map.get(&surf_input.construction);
                wc.map(|w| solar::SgsAngularModel::new(w.shgc, w.u_factor))
            } else {
                None
            };

            // Hemispherical diffuse back-transmittance for window cavity back-out
            let win_diffuse_back_tau = if is_window && glass_kd > 1e-12 {
                // First-principles Fresnel model: integrate τ_system(θ) over hemisphere
                crate::solar::diffuse_hemispherical_transmittance(glass_n, glass_kd)
            } else if is_window {
                // SGS or fallback: approximate from SHGC × ratio × diffuse modifier
                let wc = window_map.get(&surf_input.construction);
                let (s, r) = wc
                    .map(|w| (w.shgc, w.solar_transmittance_ratio()))
                    .unwrap_or((0.5, 0.85));
                s * r * 0.87 // τ_sys_normal × hemispherical modifier ≈ 0.87
            } else {
                0.0
            };

            // Hemispherical diffuse back-absorptance: α = 1 - τ - ρ
            let win_diffuse_back_abs = if is_window && glass_kd > 1e-12 {
                let rho = crate::solar::diffuse_hemispherical_reflectance(glass_n, glass_kd);
                (1.0 - win_diffuse_back_tau - rho).max(0.0)
            } else if is_window {
                // Approximate: total hemispherical absorptance from energy balance
                // For SGS windows, use a typical clear double-pane value
                0.18
            } else {
                0.0
            };

            let (
                solar_abs_out,
                thermal_abs_out,
                thermal_abs_in,
                solar_abs_in,
                roughness,
                u_factor,
                shgc,
            ) = if is_window {
                let wc = window_map.get(&surf_input.construction);
                let (u, s) = wc.map(|w| (w.u_factor, w.shgc)).unwrap_or((3.0, 0.7));
                (0.0, 0.0, 0.0, 0.0, Roughness::VerySmooth, u, s)
            } else if let Some(c) = construction_map.get(&surf_input.construction) {
                // Layered construction
                let outside_mat = c.outside_material().and_then(|name| material_map.get(name));
                let inside_mat = c.inside_material().and_then(|name| material_map.get(name));

                let (sa_out, ta_out, rough) = outside_mat
                    .map(|m| (m.solar_absorptance, m.thermal_absorptance, m.roughness))
                    .unwrap_or((0.7, 0.9, Roughness::MediumRough));
                let (sa_in, ta_in) = inside_mat
                    .map(|m| (m.solar_absorptance, m.thermal_absorptance))
                    .unwrap_or((0.7, 0.9));

                let u = c.u_factor(&material_map);

                (sa_out, ta_out, ta_in, sa_in, rough, u, 0.0)
            } else if let Some(sc) = simple_map.get(&surf_input.construction) {
                // Simple construction — same thermal absorptance both sides
                (
                    sc.solar_absorptance,
                    sc.thermal_absorptance,
                    sc.thermal_absorptance,
                    sc.solar_absorptance,
                    sc.roughness,
                    sc.u_factor,
                    0.0,
                )
            } else if let Some(fc) = f_factor_map.get(&surf_input.construction) {
                // F-factor ground floor construction.
                // Compute effective U = F × P / A for this surface.
                let perimeter = surf_input.exposed_perimeter.unwrap_or_else(|| {
                        log::warn!(
                            "Surface '{}' uses F-factor construction '{}' but has no exposed_perimeter; \
                             defaulting to sqrt(area)*2",
                            surf_input.name, surf_input.construction
                        );
                        // Rough estimate: square floor
                        surf_input.area.sqrt() * 2.0
                    });
                let u_eff = if surf_input.area > 0.0 {
                    fc.f_factor * perimeter / surf_input.area
                } else {
                    0.5 // fallback
                };
                log::info!(
                        "F-factor floor '{}': F={:.3} W/(m·K), P={:.2}m, A={:.2}m² → U_eff={:.4} W/(m²·K)",
                        surf_input.name, fc.f_factor, perimeter, surf_input.area, u_eff
                    );
                (
                    fc.solar_absorptance,
                    fc.thermal_absorptance,
                    fc.thermal_absorptance,
                    fc.solar_absorptance,
                    Roughness::Rough,
                    u_eff,
                    0.0,
                )
            } else {
                (0.7, 0.9, 0.9, 0.7, Roughness::MediumRough, 5.0, 0.0)
            };

            let tilt_rad = surf_input.tilt.to_radians();

            // Compute centroid height above ground for wind speed profile.
            // If vertices are available, use vertex centroid z-coordinate.
            // Otherwise, estimate from zone geometry and surface type.
            let centroid_height = if let Some(ref verts) = surf_input.vertices {
                if !verts.is_empty() {
                    let z_avg: f64 = verts.iter().map(|v| v.z).sum::<f64>() / verts.len() as f64;
                    z_avg.max(0.1)
                } else {
                    1.0
                }
            } else {
                // Estimate from zone dimensions
                let zone_name = &surf_input.zone;
                let zone_input = zone_states.iter().find(|z| z.input.name == *zone_name);
                if let Some(zone) = zone_input {
                    let floor_area = zone.input.floor_area;
                    let volume = zone.input.volume;
                    let zone_height = if floor_area > 0.0 {
                        volume / floor_area
                    } else {
                        3.0
                    };
                    match surf_input.surface_type {
                        SurfaceType::Floor => 0.1,                               // At ground level
                        SurfaceType::Wall => zone_height / 2.0,                  // Midpoint of wall
                        SurfaceType::Roof | SurfaceType::Ceiling => zone_height, // At top
                        SurfaceType::Window => zone_height / 2.0, // Midpoint of parent wall
                    }
                } else {
                    2.0 // Fallback
                }
            };

            // For windows, determine glass-only conductance (u_glass).
            //
            // Two approaches:
            //
            // 1. **First-principles (ISO 15099)** — when pane/gap properties are
            //    provided. Computes gap conductance from sealed-gas model at an
            //    initial estimate of gap temperatures, then updates dynamically
            //    during simulation. Matches EnergyPlus layer-by-layer approach.
            //
            // 2. **NFRC film stripping** (fallback) — decomposes the NFRC rated
            //    U-factor by removing standard film coefficients:
            //      h_i_std = 8.29 W/(m²·K), h_e_std = 26.0 W/(m²·K)
            //      u_glass = 1 / (1/U - 1/h_e_std - 1/h_i_std)
            //    This is less accurate because the NFRC gap conductance (at NFRC
            //    test conditions) doesn't match runtime conditions — the gap
            //    Rayleigh number and radiation coefficient change with temperature.

            // Extract gap properties and glass emissivity from window construction
            let (
                win_gap_width,
                win_pane_thickness,
                win_pane_conductivity,
                win_gap_emissivity,
                win_glass_emissivity,
            ) = if is_window {
                let wc = window_map.get(&surf_input.construction);
                let glass_eps = wc.and_then(|w| w.glass_emissivity).unwrap_or(0.84);
                let gap_props = wc
                    .and_then(|w| {
                        // Require gap_width + pane_conductivity + pane_thickness for first-principles
                        match (w.gap_width, w.pane_conductivity, w.pane_thickness) {
                            (Some(gw), Some(pk), Some(pt)) if gw > 0.0 => {
                                Some((gw, pt, pk, glass_eps))
                            }
                            _ => None,
                        }
                    })
                    .unwrap_or((0.0, 0.0, 1.0, glass_eps));
                (
                    gap_props.0,
                    gap_props.1,
                    gap_props.2,
                    gap_props.3,
                    glass_eps,
                )
            } else {
                (0.0, 0.0, 1.0, 0.84, 0.84)
            };

            // Always compute the NFRC film-stripped u_glass as the rated value.
            // This serves as the upper-bound cap for the dynamic gap model.
            let u_glass_rated = if is_window && u_factor > 0.0 {
                let r_overall = 1.0 / u_factor;
                let r_films = 1.0 / 26.0 + 1.0 / 8.29; // ~0.159 m²K/W (NFRC standard)
                let r_glass = (r_overall - r_films).max(0.01);
                1.0 / r_glass
            } else {
                u_factor
            };

            let u_glass = if is_window && win_gap_width > 0.0 {
                // First-principles: compute u_glass from pane + gap properties.
                // Initial estimate at ~0°C mean gap temperature, ~15°C ΔT across gap.
                let h_gap_init = sealed_air_gap_conductance(
                    win_gap_width,
                    0.0,
                    15.0,
                    surf_input.tilt,
                    win_gap_emissivity,
                );
                let r_gap = 1.0 / h_gap_init;
                let r_pane = win_pane_thickness / win_pane_conductivity;
                let r_glass = 2.0 * r_pane + r_gap; // double-pane: outer + gap + inner
                                                    // Cap at 110% of rated value: the gap model can improve upon
                                                    // the NFRC rating (lower U in winter) but should not degrade
                                                    // too far beyond it (higher U at extreme temperatures where
                                                    // h_rad ∝ T³). The 10% margin allows modest temperature-
                                                    // dependent increases at moderate summer conditions while
                                                    // still preventing runaway at free-float extremes (60°C+).
                (1.0 / r_glass).min(u_glass_rated * 1.05)
            } else {
                u_glass_rated
            };

            let state = SurfaceState {
                input: surf_input.clone(),
                is_window,
                net_area: surf_input.area,
                temp_outside: initial_temp,
                temp_inside: initial_temp,
                q_conv_inside: 0.0,
                h_conv_inside: 3.076,
                h_conv_outside: 10.0,
                incident_solar: 0.0,
                absorbed_solar_outside: 0.0,
                transmitted_solar: 0.0,
                transmitted_solar_beam: 0.0,
                transmitted_solar_diffuse: 0.0,
                absorbed_solar_inside_window: 0.0,
                window_solar_absorptance: win_solar_absorptance,
                window_inside_absorbed_fraction: win_inside_fraction,
                solar_absorptance_outside: solar_abs_out,
                thermal_absorptance_outside: thermal_abs_out,
                thermal_absorptance_inside: thermal_abs_in,
                solar_absorptance_inside: solar_abs_in,
                roughness,
                u_factor,
                u_glass,
                u_glass_rated,
                shgc,
                solar_transmittance_ratio: win_solar_trans_ratio,
                glass_kd,
                glass_ni,
                glass_n,
                sgs_model,
                gap_width: win_gap_width,
                pane_thickness: win_pane_thickness,
                pane_conductivity: win_pane_conductivity,
                gap_emissivity: win_gap_emissivity,
                glass_emissivity: win_glass_emissivity,
                diffuse_back_transmittance: win_diffuse_back_tau,
                diffuse_back_absorptance: win_diffuse_back_abs,
                cos_tilt: tilt_rad.cos(),
                sin_tilt: tilt_rad.sin(),
                centroid_height,
                diffuse_sky_shading_ratio: 1.0, // Updated by compute_diffuse_shading_ratios()
                diffuse_horizon_shading_ratio: 1.0, // Updated by compute_diffuse_shading_ratios()
                box_face: None,                 // Set after zone VF computation below
                f_factor_ground_temps: {
                    // Store F-factor ground temperatures on the surface state
                    // so the heat balance uses FCfactorMethod temps instead of
                    // building-level BuildingSurface temps.
                    if let Some(fc) = f_factor_map.get(&surf_input.construction) {
                        if let Some(ref temps) = fc.ground_temperatures {
                            if temps.len() == 12 {
                                let mut arr = [0.0_f64; 12];
                                arr.copy_from_slice(temps);
                                Some(arr)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                },
                q_cond_inside: 0.0,
                q_cond_outside: 0.0,
                q_rad_inside: 0.0,
                h_rad_inside: 5.0,
            };
            surface_states.push(state);
        }

        // Assign surfaces to zones
        for (surf_idx, surf) in surface_states.iter().enumerate() {
            if let Some(&zi) = zone_index.get(&surf.input.zone) {
                zone_states[zi].surface_indices.push(surf_idx);
            }
        }

        // Compute zone centroid height (area-weighted average of surface centroid heights).
        // EnergyPlus uses the zone centroid Z for infiltration wind speed correction.
        for zone in &mut zone_states {
            let mut sum_az = 0.0_f64;
            let mut sum_a = 0.0_f64;
            for &si in &zone.surface_indices {
                let area = surface_states[si].input.area;
                let ch = surface_states[si].centroid_height;
                sum_az += area * ch;
                sum_a += area;
            }
            zone.centroid_height = if sum_a > 0.0 { sum_az / sum_a } else { 1.5 };
        }

        // Subtract window areas from parent surfaces
        let window_parents: Vec<(String, f64)> = surface_states
            .iter()
            .filter(|s| s.is_window)
            .filter_map(|s| {
                s.input
                    .parent_surface
                    .as_ref()
                    .map(|p| (p.clone(), s.input.area))
            })
            .collect();
        for (parent_name, window_area) in &window_parents {
            if let Some(parent) = surface_states
                .iter_mut()
                .find(|s| s.input.name == *parent_name)
            {
                parent.net_area = (parent.net_area - window_area).max(0.0);
            }
        }

        let n_surfaces = surface_states.len();

        // ── Compute interior view factors for box-shaped zones ────────────
        //
        // For each zone, attempt to derive box dimensions (L, W, H) from the
        // bounding box of surface vertices, classify each surface into one of
        // 6 box faces, and compute the 6×6 face-to-face VF matrix.
        //
        // Zones without vertex data fall back to area-weighted MRT.
        let mut zone_view_factors: Vec<Option<ZoneViewFactors>> = vec![None; zone_states.len()];

        for (zi, zone) in zone_states.iter().enumerate() {
            // Collect all vertices in this zone (opaque + window)
            let zone_surfs: Vec<usize> = zone.surface_indices.clone();
            let mut all_verts: Vec<geometry::Point3D> = Vec::new();
            for &si in &zone_surfs {
                if let Some(ref verts) = surface_states[si].input.vertices {
                    all_verts.extend_from_slice(verts);
                }
            }
            if all_verts.len() < 8 {
                continue; // Need at least a box (8 corners)
            }

            // Bounding box → box dimensions
            let x_min = all_verts.iter().map(|v| v.x).fold(f64::INFINITY, f64::min);
            let x_max = all_verts
                .iter()
                .map(|v| v.x)
                .fold(f64::NEG_INFINITY, f64::max);
            let y_min = all_verts.iter().map(|v| v.y).fold(f64::INFINITY, f64::min);
            let y_max = all_verts
                .iter()
                .map(|v| v.y)
                .fold(f64::NEG_INFINITY, f64::max);
            let z_min = all_verts.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
            let z_max = all_verts
                .iter()
                .map(|v| v.z)
                .fold(f64::NEG_INFINITY, f64::max);

            let box_l = (x_max - x_min).max(0.1); // Length (east-west)
            let box_w = (y_max - y_min).max(0.1); // Width (north-south)
            let box_h = (z_max - z_min).max(0.1); // Height

            // Classify each surface into a box face
            let mut face_area = [0.0_f64; 6];
            let mut all_classified = true;

            for &si in &zone_surfs {
                let s = &surface_states[si];
                if let Some(face) = geometry::classify_box_face(s.input.tilt, s.input.azimuth) {
                    // Use gross area for windows, net area for opaque
                    let area = if s.is_window {
                        s.input.area
                    } else {
                        s.net_area
                    };
                    face_area[face] += area;
                } else {
                    all_classified = false;
                    break;
                }
            }

            if !all_classified {
                log::warn!("Zone '{}': not all surfaces classified into box faces, using area-weighted MRT", zone.input.name);
                continue;
            }

            // Compute 6×6 VF matrix from box dimensions
            let face_vf = geometry::compute_box_view_factors(box_l, box_w, box_h);

            // Set box_face on each surface in this zone
            for &si in &zone_surfs {
                let s = &surface_states[si];
                if let Some(face) = geometry::classify_box_face(s.input.tilt, s.input.azimuth) {
                    surface_states[si].box_face = Some(face);
                }
            }

            log::info!(
                "Zone '{}': view factors computed for {:.1}×{:.1}×{:.1}m box \
                 (F_floor→ceiling={:.4}, F_floor→south={:.4})",
                zone.input.name,
                box_l,
                box_w,
                box_h,
                face_vf[geometry::FACE_FLOOR][geometry::FACE_CEILING],
                face_vf[geometry::FACE_FLOOR][geometry::FACE_SOUTH],
            );

            zone_view_factors[zi] = Some(ZoneViewFactors { face_vf, face_area });
        }

        // ── Build interzone surface pairs ─────────────────────────────
        //
        // For each surface with BoundaryCondition::Zone("other"), find the
        // matching surface in "other" zone that points back to this surface's
        // zone.  Matches EnergyPlus behaviour: the outside temperature of
        // each interzone surface is set to the paired surface's inside
        // temperature, so the CTF cross-coupling term Y[0] correctly
        // connects the two zones through a single wall thickness.
        let mut interzone_pairs: Vec<Option<usize>> = vec![None; n_surfaces];
        for i in 0..n_surfaces {
            if let BoundaryCondition::Zone(ref other_zone) = surface_states[i].input.boundary {
                let my_zone = &surface_states[i].input.zone;
                for j in 0..n_surfaces {
                    if j == i {
                        continue;
                    }
                    if surface_states[j].input.zone == *other_zone {
                        if let BoundaryCondition::Zone(ref back_zone) =
                            surface_states[j].input.boundary
                        {
                            if back_zone == my_zone {
                                interzone_pairs[i] = Some(j);
                                log::info!(
                                    "Interzone pair: '{}' (zone {}) <-> '{}' (zone {})",
                                    surface_states[i].input.name,
                                    my_zone,
                                    surface_states[j].input.name,
                                    other_zone,
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }

        // ── Compute envelope areas by cardinal direction ──────────────
        let mut envelope_areas = crate::geometry::EnvelopeAreas::default();
        for ss in &surface_states {
            // Only count exterior (outdoor boundary) walls and windows
            if ss.input.boundary != BoundaryCondition::Outdoor {
                continue;
            }
            let dir = crate::geometry::azimuth_to_cardinal(ss.input.azimuth);
            match ss.input.surface_type {
                SurfaceType::Wall => {
                    // Use gross area (before window subtraction)
                    envelope_areas.add_wall(dir, ss.input.area);
                }
                SurfaceType::Window => {
                    envelope_areas.add_window(dir, ss.input.area);
                }
                _ => {} // floors, roofs, ceilings don't contribute to WWR
            }
        }

        BuildingEnvelope {
            zones: zone_states,
            surfaces: surface_states,
            ctf_coefficients: vec![None; n_surfaces],
            ctf_histories: vec![None; n_surfaces],
            materials: material_map,
            constructions: construction_map,
            window_constructions: window_map,
            simple_constructions: simple_map,
            f_factor_constructions: f_factor_map,
            zone_index,
            ground_temp_model: None,
            schedule_manager: ScheduleManager::new(),
            latitude,
            longitude,
            time_zone,
            ground_reflectance: 0.2,
            jan1_dow: 1, // Default: Monday; overridden by weather file
            dt: 3600.0,
            initialized: false,
            shading_calculation: shading::ShadingCalculation::Basic,
            shading_polygons: Vec::new(),
            terrain: convection::Terrain::default(),
            elevation,
            zone_view_factors,
            interzone_pairs,
            envelope_areas,
            infiltration_interaction: crate::zone_loads::InfiltrationInteraction::Basic,
            airflow_network: None, // built later via build_airflow_network()
            solar_distribution_method,
            ctf_q_last_inside: vec![0.0; n_surfaces],
            ctf_q_last_outside: vec![0.0; n_surfaces],
            solar_cache: None,
            holiday_set: std::collections::HashSet::new(),
        }
    }

    /// Resolve shading surfaces from explicit definitions, window overhang/fin
    /// definitions, and building self-shading (all outdoor surfaces with vertices).
    ///
    /// Called after `from_input_full` to populate `self.shading_polygons`.
    pub fn resolve_shading(
        &mut self,
        surface_inputs: &[SurfaceInput],
        shading_surface_inputs: &[shading::ShadingSurfaceInput],
    ) {
        let mut polygons = Vec::new();

        // 1. Explicit shading surfaces from model input
        polygons.extend(shading::resolve_shading_surfaces(shading_surface_inputs));

        // 2. Auto-generated overhangs/fins from window shading definitions
        for surf in surface_inputs {
            if surf.surface_type == SurfaceType::Window {
                if let Some(ref shade_input) = surf.shading {
                    if let Some(ref verts) = surf.vertices {
                        if verts.len() >= 4 {
                            // Find the parent wall's outward normal
                            let wall_outward = if let Some(ref parent_name) = surf.parent_surface {
                                // Look up parent wall vertices
                                surface_inputs
                                    .iter()
                                    .find(|s| s.name == *parent_name)
                                    .and_then(|s| s.vertices.as_ref())
                                    .map(|v| geometry::newell_normal(v).normalize())
                                    .unwrap_or_else(|| {
                                        // Fallback: use window normal (windows are coplanar with wall)
                                        geometry::newell_normal(verts).normalize()
                                    })
                            } else {
                                geometry::newell_normal(verts).normalize()
                            };

                            // Generate overhang
                            if let Some(ref ovh) = shade_input.overhang {
                                let ovh_verts =
                                    shading::generate_overhang_vertices(verts, ovh, &wall_outward);
                                if let Some(sp) = shading::surface_to_shading_polygon(
                                    &format!("ovh:{}", surf.name),
                                    &ovh_verts,
                                ) {
                                    polygons.push(sp);
                                }
                            }

                            // Generate left fin
                            if let Some(ref fin) = shade_input.left_fin {
                                let fin_verts = shading::generate_fin_vertices(
                                    verts,
                                    fin,
                                    &wall_outward,
                                    shading::FinSide::Left,
                                );
                                if let Some(sp) = shading::surface_to_shading_polygon(
                                    &format!("lfin:{}", surf.name),
                                    &fin_verts,
                                ) {
                                    polygons.push(sp);
                                }
                            }

                            // Generate right fin
                            if let Some(ref fin) = shade_input.right_fin {
                                let fin_verts = shading::generate_fin_vertices(
                                    verts,
                                    fin,
                                    &wall_outward,
                                    shading::FinSide::Right,
                                );
                                if let Some(sp) = shading::surface_to_shading_polygon(
                                    &format!("rfin:{}", surf.name),
                                    &fin_verts,
                                ) {
                                    polygons.push(sp);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3. Self-shading: all outdoor surfaces with vertices are potential casters
        for surf in surface_inputs {
            if surf.boundary == BoundaryCondition::Outdoor {
                if let Some(ref verts) = surf.vertices {
                    if let Some(sp) = shading::surface_to_shading_polygon(&surf.name, verts) {
                        polygons.push(sp);
                    }
                }
            }
        }

        if !polygons.is_empty() {
            log::info!("Shading: {} casting polygons registered ({} explicit, rest from building geometry + overhangs/fins)",
                polygons.len(),
                shading_surface_inputs.len());
        }
        self.shading_polygons = polygons;
    }

    /// Build the multizone airflow network from building geometry.
    ///
    /// Should be called after `from_input_full()` when an airflow network
    /// config is provided in the simulation settings. The network auto-generates
    /// crack flow paths from exterior and interzone surfaces.
    pub fn build_airflow_network(&mut self, config: &crate::airflow_network::AirflowNetworkConfig) {
        if !config.enabled {
            return;
        }
        // Collect per-surface airflow overrides
        let overrides: Vec<Option<crate::airflow_network::SurfaceAirflowOverride>> = self
            .surfaces
            .iter()
            .map(|s| s.input.airflow.clone())
            .collect();

        let network = crate::airflow_network::build_network(
            &self.zones,
            &self.surfaces,
            config,
            &self.zone_index,
            &self.interzone_pairs,
            &overrides,
            &self.envelope_areas,
        );
        log::info!(
            "Airflow network: {} zone nodes, {} flow paths, side_ratio={:.2}",
            network.zone_to_node.len(),
            network.paths.len(),
            network.side_ratio,
        );
        self.airflow_network = Some(network);
    }

    /// Precompute sunlit fractions for every annual timestep (Suncast-style).
    ///
    /// Computes the expensive polygon-clipping sunlit fractions for all surfaces
    /// at every simulation timestep upfront, using Rayon for parallelism.
    /// During the annual simulation, cached values are looked up instead of
    /// recomputed, eliminating the main per-timestep bottleneck.
    ///
    /// Only meaningful when `shading_calculation == Detailed` and shading
    /// polygons are present. Otherwise, all sunlit fractions are 1.0 and
    /// caching provides no benefit.
    ///
    /// # Arguments
    /// * `weather_hours` - Full year weather data (needed for hour count)
    /// * `timesteps_per_hour` - Sub-hourly resolution
    /// * `start_hour` - First weather hour index (0-based)
    /// * `end_hour` - Last weather hour index (exclusive)
    pub fn precompute_solar(
        &mut self,
        weather_hours: &[openbse_weather::WeatherHour],
        timesteps_per_hour: u32,
        start_hour: usize,
        end_hour: usize,
        cache_path: Option<&std::path::Path>,
    ) {
        use rayon::prelude::*;

        // Only precompute if detailed shading is enabled with geometry
        if self.shading_calculation != shading::ShadingCalculation::Detailed
            || self.shading_polygons.is_empty()
        {
            log::info!("Solar precompute: skipped (shading mode is Basic or no shading polygons)");
            return;
        }

        let n_surfaces = self.surfaces.len();
        let n_hours = end_hour.min(weather_hours.len()) - start_hour;
        let total_steps = n_hours * timesteps_per_hour as usize;

        // Compute geometry fingerprint for cache validation
        let fingerprint = SolarCache::compute_fingerprint(
            &self.surfaces,
            &self.shading_polygons,
            self.latitude,
            self.longitude,
            self.time_zone,
            timesteps_per_hour,
            start_hour,
            end_hour,
        );

        // Try loading from disk cache
        if let Some(path) = cache_path {
            if let Some(cached) = SolarCache::load_from_file(path, fingerprint) {
                let cached_steps = cached.sunlit_fractions.len();
                let cached_surfs = if cached_steps > 0 {
                    cached.sunlit_fractions[0].len()
                } else {
                    0
                };
                if cached_steps == total_steps && cached_surfs == n_surfaces {
                    log::info!(
                        "Solar precompute: loaded from cache '{}' ({} timesteps, fingerprint {:016x})",
                        path.display(), cached_steps, fingerprint,
                    );
                    self.solar_cache = Some(cached);
                    return;
                }
                log::info!(
                    "Solar cache size mismatch (expected {}x{}, got {}x{}), will recompute",
                    total_steps,
                    n_surfaces,
                    cached_steps,
                    cached_surfs
                );
            }
        }

        log::info!(
            "Solar precompute: computing sunlit fractions for {} timesteps × {} surfaces ({:.1} MB)...",
            total_steps, n_surfaces,
            (total_steps * n_surfaces * 8) as f64 / 1_048_576.0,
        );

        // Collect immutable surface data needed for sunlit fraction computation.
        // This must be done before the parallel closure to avoid borrowing &self.
        struct SurfaceGeo {
            boundary_is_outdoor: bool,
            vertices: Option<Vec<crate::geometry::Point3D>>,
        }
        let surface_geo: Vec<SurfaceGeo> = self
            .surfaces
            .iter()
            .map(|s| SurfaceGeo {
                boundary_is_outdoor: s.input.boundary == BoundaryCondition::Outdoor,
                vertices: s.input.vertices.clone(),
            })
            .collect();

        // Surface names for self-shading exclusion
        let surface_names: Vec<String> = self
            .surfaces
            .iter()
            .map(|s| format!("self:{}", s.input.name))
            .collect();

        // Clone shading polygons for thread-safe access
        let shading_polys = self.shading_polygons.clone();
        let latitude = self.latitude;
        let time_zone = self.time_zone;
        let longitude = self.longitude;

        // Compute sunlit fractions for all timesteps in parallel
        let sunlit_fractions: Vec<Vec<f64>> = (0..total_steps)
            .into_par_iter()
            .map(|step| {
                let hour_idx = start_hour + step / timesteps_per_hour as usize;
                let sub = (step % timesteps_per_hour as usize) as u32 + 1;

                // Compute timestep's fractional hour (matching TimeStep::fractional_hour)
                let wh = &weather_hours[hour_idx];
                let hour_1based = (hour_idx % 24) as u32 + 1;
                let fractional_hour =
                    hour_1based as f64 - 1.0 + (sub as f64 - 0.5) / timesteps_per_hour as f64;

                // Day of year from weather data
                let doy = {
                    let days_in_months = [31u32, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
                    let mut d = 0u32;
                    for m in 0..(wh.month.saturating_sub(1)) as usize {
                        d += days_in_months[m.min(11)];
                    }
                    d + wh.day
                };

                let eot = solar::equation_of_time(doy);
                let solar_hour = fractional_hour + (time_zone - longitude / 15.0) + eot;
                let sol_pos = solar::solar_position(doy, solar_hour, latitude);

                if !sol_pos.is_sunup || sol_pos.altitude <= 0.0 {
                    // Night: all surfaces fully sunlit (doesn't matter, no solar)
                    return vec![1.0; n_surfaces];
                }

                let sun_dir = solar::sun_direction_vector(&sol_pos);

                // Compute sunlit fraction for each surface
                surface_geo
                    .iter()
                    .enumerate()
                    .map(|(si, geo)| {
                        if !geo.boundary_is_outdoor {
                            return 1.0;
                        }
                        if let Some(ref verts) = geo.vertices {
                            if verts.len() >= 3 {
                                let normal = crate::geometry::newell_normal(verts).normalize();
                                let casters: Vec<&shading::ShadingPolygon> = shading_polys
                                    .iter()
                                    .filter(|sp| sp.name != surface_names[si])
                                    .collect();
                                return shading::calculate_sunlit_fraction(
                                    verts, &normal, &casters, &sun_dir,
                                );
                            }
                        }
                        1.0
                    })
                    .collect()
            })
            .collect();

        log::info!(
            "Solar precompute: done ({} timesteps computed)",
            sunlit_fractions.len()
        );

        let cache = SolarCache {
            sunlit_fractions,
            timesteps_per_hour,
            start_hour,
            fingerprint,
        };

        // Save to disk for future runs
        if let Some(path) = cache_path {
            match cache.save_to_file(path) {
                Ok(()) => {
                    let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                    log::info!(
                        "Solar precompute: saved cache to '{}' ({:.1} MB, fingerprint {:016x})",
                        path.display(),
                        size_bytes as f64 / 1_048_576.0,
                        fingerprint,
                    );
                }
                Err(e) => {
                    log::warn!("Solar precompute: failed to save cache: {e}");
                }
            }
        }

        self.solar_cache = Some(cache);
    }

    /// Compute diffuse sky shading ratios for all outdoor surfaces.
    ///
    /// Uses hemisphere sampling (144 sky patches, matching EnergyPlus
    /// SkyDifSolarShading()) to determine what fraction of isotropic sky
    /// diffuse radiation reaches each surface after obstruction by ALL
    /// shading polygons (overhangs, fins, detached shading, AND building
    /// self-shading geometry). This matches E+'s approach where building
    /// geometry is always included in the DifShdgRatioIsoSky calculation.
    ///
    /// Must be called AFTER `resolve_shading()` populates `shading_polygons`.
    pub fn compute_diffuse_shading_ratios(&mut self) {
        if self.shading_polygons.is_empty()
            || self.shading_calculation != shading::ShadingCalculation::Detailed
        {
            return;
        }

        for si in 0..self.surfaces.len() {
            if self.surfaces[si].input.boundary != BoundaryCondition::Outdoor {
                continue;
            }
            if let Some(ref verts) = self.surfaces[si].input.vertices {
                if verts.len() >= 3 {
                    let normal = geometry::newell_normal(verts).normalize();
                    // Use ALL shading polygons except this surface itself
                    let self_name = &self.surfaces[si].input.name;
                    let casters: Vec<&shading::ShadingPolygon> = self
                        .shading_polygons
                        .iter()
                        .filter(|sp| sp.name != *self_name)
                        .collect();

                    let ratio =
                        shading::compute_diffuse_sky_shading_ratio(verts, &normal, &casters);
                    let horiz_ratio =
                        shading::compute_diffuse_horizon_shading_ratio(verts, &normal, &casters);

                    if ratio < 0.999 || horiz_ratio < 0.999 {
                        log::info!(
                            "Diffuse shading for '{}': sky={:.3} ({:.1}% blocked), horiz={:.3} ({:.1}% blocked)",
                            self.surfaces[si].input.name,
                            ratio, (1.0 - ratio) * 100.0,
                            horiz_ratio, (1.0 - horiz_ratio) * 100.0,
                        );
                    }
                    self.surfaces[si].diffuse_sky_shading_ratio = ratio;
                    self.surfaces[si].diffuse_horizon_shading_ratio = horiz_ratio;
                }
            }
        }
    }

    /// Compute CTF coefficients for all opaque surfaces.
    fn compute_all_ctf(&mut self) {
        for i in 0..self.surfaces.len() {
            if self.surfaces[i].is_window {
                continue;
            }
            let construction_name = &self.surfaces[i].input.construction;

            // Try layered construction first
            if let Some(construction) = self.constructions.get(construction_name).cloned() {
                let resolved_layers = construction.resolve_layers(&self.materials);
                if !resolved_layers.is_empty() {
                    let ctf = calculate_ctf(&resolved_layers, self.dt);
                    // Temporary: log CTF coefficients for comparison with E+
                    log::info!(
                        "CTF for '{}' (construction '{}'): terms={}, Z[0]={:.6}, Y[0]={:.6e}, X[0]={:.6}",
                        self.surfaces[i].input.name, construction_name,
                        ctf.num_terms, ctf.z[0], ctf.y[0], ctf.x[0]
                    );
                    if self.surfaces[i].input.name.contains("Wall South 1F")
                        || self.surfaces[i].input.name.contains("Attic Floor")
                    {
                        for j in 0..ctf.num_terms {
                            let phi_j = if j < ctf.phi.len() { ctf.phi[j] } else { 0.0 };
                            log::info!(
                                "  CTF[{}]: X={:.6e}, Y={:.6e}, Z={:.6e}, Phi={:.6e}",
                                j,
                                ctf.x[j],
                                ctf.y[j],
                                ctf.z[j],
                                phi_j
                            );
                        }
                    }
                    let history = CtfHistory::new(ctf.num_terms.max(1), 21.0);
                    self.ctf_coefficients[i] = Some(ctf);
                    self.ctf_histories[i] = Some(history);
                    continue;
                }
            }

            // Try simple construction
            if let Some(sc) = self.simple_constructions.get(construction_name).cloned() {
                let ctf = calculate_ctf_simple(
                    sc.u_factor,
                    sc.thermal_capacity,
                    self.dt,
                    sc.mass_outside,
                    sc.mass_conductivity,
                    sc.mass_density,
                );
                let history = CtfHistory::new(ctf.num_terms.max(1), 21.0);
                self.ctf_coefficients[i] = Some(ctf);
                self.ctf_histories[i] = Some(history);
                continue;
            }

            // Try F-factor ground floor construction
            if let Some(fc) = self.f_factor_constructions.get(construction_name).cloned() {
                // Effective U = F × P / A (already computed and stored in surface u_factor)
                let u_eff = self.surfaces[i].u_factor;
                let ctf =
                    calculate_ctf_simple(u_eff, fc.thermal_capacity, self.dt, false, None, None);
                let history = CtfHistory::new(ctf.num_terms.max(1), 21.0);
                self.ctf_coefficients[i] = Some(ctf);
                self.ctf_histories[i] = Some(history);
            }
        }
    }

    /// Check if any zone has an ideal loads system configured.
    pub fn has_ideal_loads(&self) -> bool {
        self.zones.iter().any(|z| z.input.ideal_loads.is_some())
    }
}

impl EnvelopeSolver for BuildingEnvelope {
    fn initialize(&mut self, dt: f64) -> Result<(), String> {
        self.dt = dt;
        self.compute_all_ctf();
        self.initialized = true;
        Ok(())
    }

    fn solve_timestep(
        &mut self,
        ctx: &SimulationContext,
        weather: &WeatherHour,
        hvac: &ZoneHvacConditions,
    ) -> EnvelopeResults {
        let t_outdoor = weather.dry_bulb;
        let wind_speed_met = weather.wind_speed;
        let wind_direction = weather.wind_direction;
        let dt = ctx.timestep.dt;
        let p_b = weather.pressure;
        let hour = ctx.timestep.hour;

        // 1. Solar position
        let doy = ctx.timestep.day_of_year();
        let eot = solar::equation_of_time(doy);
        let solar_hour =
            ctx.timestep.fractional_hour() + (self.time_zone - self.longitude / 15.0) + eot;
        let sol_pos = solar::solar_position(doy, solar_hour, self.latitude);
        let sun_dir = solar::sun_direction_vector(&sol_pos);

        // 1b. Sky temperature for longwave radiation exchange
        // σ = 5.6704e-8 W/(m²·K⁴) (Stefan-Boltzmann constant)
        const SIGMA: f64 = 5.6704e-8;
        //
        // Use Clark & Allen (1978) clear-sky emissivity model with opaque sky
        // cover correction. This matches EnergyPlus's default Sky Temperature
        // Model (eplusout.eio: "Site:WeatherStation, ..., Clark and Allen") and
        // is more robust than direct EPW horizontal IR, which can produce
        // unreasonably cold sky temperatures in some TMYx files.
        //
        // Clear-sky emissivity: ε_clear = 0.787 + 0.764 * ln(T_dp_K / 273)
        // Cloud correction: ε_sky = ε_clear * (1 + 0.0224*N - 0.0035*N² + 0.00028*N³)
        //   where N = opaque sky cover in tenths [0-10]
        //
        // T_sky = (ε_sky)^0.25 * T_air_K - 273.15
        //
        // Reference: Clark & Allen (1978), Walton (1983), EnergyPlus Engineering Ref.
        let t_sky = {
            let t_dp_k = (weather.dew_point + 273.15).max(200.0);
            let t_db_k = (t_outdoor + 273.15).max(200.0);
            let ln_ratio = (t_dp_k / 273.0).ln();
            let eps_clear = (0.787 + 0.764 * ln_ratio).clamp(0.3, 1.0);

            // Cloud cover correction (N in tenths, 0=clear, 10=overcast)
            let n = weather.opaque_sky_cover.clamp(0.0, 10.0);
            let cloud_factor = 1.0 + 0.0224 * n - 0.0035 * n * n + 0.00028 * n * n * n;
            let eps_sky = (eps_clear * cloud_factor).clamp(0.3, 1.0);

            // T_sky = eps_sky^0.25 * T_air (in Kelvin)
            eps_sky.powf(0.25) * t_db_k - 273.15
        };

        // 2. Apply HVAC conditions from external controls (non-ideal-loads mode)
        for zone in &mut self.zones {
            if zone.input.ideal_loads.is_none() {
                // Reset supply air to zero each timestep. If the HVAC loop is off
                // (e.g., availability schedule), no entry will be in the hashmap
                // and the zone should see zero supply flow (HVAC off = no air delivered).
                zone.supply_air_temp = t_outdoor;
                zone.supply_air_mass_flow = 0.0;
                // Only apply external HVAC when zone doesn't use ideal loads
                if let Some(&t_sup) = hvac.supply_temps.get(&zone.input.name) {
                    zone.supply_air_temp = t_sup;
                }
                if let Some(&m_sup) = hvac.supply_mass_flows.get(&zone.input.name) {
                    zone.supply_air_mass_flow = m_sup;
                }
            } else {
                // Ideal loads: zero out supply air (HVAC is handled as direct Q)
                zone.supply_air_temp = t_outdoor;
                zone.supply_air_mass_flow = 0.0;
            }
        }

        // 3. Internal gains — mode depends on sizing context
        //
        // During normal simulation: use schedules (time-varying fractions).
        // During sizing: behavior is controlled by the design day's
        // `internal_gains` mode (Off / Full / Scheduled / FullWhenOccupied).
        use openbse_core::ports::SizingInternalGains;
        let mut dow = day_of_week(ctx.timestep.month, ctx.timestep.day, self.jan1_dow);
        if self
            .holiday_set
            .contains(&(ctx.timestep.month, ctx.timestep.day))
        {
            dow = 8; // Holiday day-type
        }
        for zone in &mut self.zones {
            let gains = if ctx.is_sizing {
                match ctx.sizing_internal_gains {
                    SizingInternalGains::Off => {
                        // Zero internal gains (heating design days: conservative).
                        internal_gains::ResolvedGain::default()
                    }
                    SizingInternalGains::Full => {
                        // 100% design-level gains at all hours (cooling DDs: conservative).
                        // Passing None for schedule_mgr → fraction = 1.0 for all gains.
                        internal_gains::resolve_gains_scheduled(
                            &zone.input.internal_gains,
                            None,
                            hour,
                            dow,
                        )
                    }
                    SizingInternalGains::Scheduled => {
                        // Follow normal schedules, same as annual simulation.
                        internal_gains::resolve_gains_scheduled(
                            &zone.input.internal_gains,
                            Some(&self.schedule_manager),
                            hour,
                            dow,
                        )
                    }
                    SizingInternalGains::FullWhenOccupied => {
                        // Check scheduled gains to determine if this hour is "occupied".
                        // If any gain is active (schedule > 0), use full design gains.
                        // If unoccupied (all schedules = 0), use zero gains.
                        let scheduled = internal_gains::resolve_gains_scheduled(
                            &zone.input.internal_gains,
                            Some(&self.schedule_manager),
                            hour,
                            dow,
                        );
                        if scheduled.total > 0.0 {
                            internal_gains::resolve_gains_scheduled(
                                &zone.input.internal_gains,
                                None,
                                hour,
                                dow,
                            )
                        } else {
                            internal_gains::ResolvedGain::default()
                        }
                    }
                }
            } else {
                // Normal simulation: always use schedules
                internal_gains::resolve_gains_scheduled(
                    &zone.input.internal_gains,
                    Some(&self.schedule_manager),
                    hour,
                    dow,
                )
            };
            zone.q_internal_conv = gains.convective;
            zone.q_internal_rad = gains.radiative;
            zone.lighting_power = gains.lighting_power;
            zone.equipment_power = gains.equipment_power;
            zone.people_heat = gains.people_heat;
            zone.people_latent = gains.people_latent;
            zone.equipment_latent = gains.equipment_latent;
            zone.lighting_gain_to_zone = gains.lighting_gain_to_zone;
            zone.equipment_sensible_gain_to_zone = gains.equipment_sensible_gain_to_zone;

            // Add radiant HVAC panel gains (fin-tube radiators, chilled ceilings).
            // Distributed across surfaces by area × absorptance, same as internal gains.
            if let Some(&panel_rad) = hvac.radiant_gains.get(&zone.input.name) {
                zone.q_internal_rad += panel_rad;
            }
        }

        // 4. Infiltration + scheduled ventilation + exhaust + outdoor air
        let w_outdoor = psych::w_fn_tdb_rh_pb(t_outdoor, weather.rel_humidity / 100.0, p_b);
        let rho_outdoor = psych::rho_air_fn_pb_tdb_w(p_b, t_outdoor, w_outdoor);

        // If airflow network is active, solve the pressure network for infiltration.
        // This replaces the per-zone Design Flow Rate infiltration calculation.
        let afn_active = self.airflow_network.is_some();
        if let Some(ref mut afn) = self.airflow_network {
            // Update zone temperatures and densities in network nodes
            for (zi, zone) in self.zones.iter().enumerate() {
                let node = &mut afn.nodes[afn.zone_to_node[zi]];
                node.temperature = zone.temp + 273.15;
                node.density = psych::rho_air_fn_pb_tdb_w(p_b, zone.temp, 0.008);
            }
            // Update exhaust fan fixed-flow paths from current scheduled rates
            // (exhaust will be computed below, so we use previous timestep value for now)
            crate::airflow_network::solve_pressures(
                afn,
                wind_speed_met,
                wind_direction,
                t_outdoor,
                rho_outdoor,
                self.terrain,
            );
            // Write AFN outdoor mass flow results to zone state
            for (zi, zone) in self.zones.iter_mut().enumerate() {
                zone.infiltration_mass_flow = afn.zone_outdoor_mass_flow[zi];
            }
        }

        for zone in &mut self.zones {
            if !afn_active {
                // Design Flow Rate infiltration model (unchanged when AFN is off)
                let wind_speed_local = convection::wind_speed_at_height(
                    wind_speed_met,
                    zone.centroid_height,
                    convection::DEFAULT_WEATHER_WIND_MOD_COEFF,
                    self.terrain.wind_exp(),
                    self.terrain.wind_bl_height(),
                );

                zone.infiltration_mass_flow = 0.0;
                for infil in &zone.input.infiltration {
                    let sched_mult = match &infil.schedule {
                        Some(name) => self.schedule_manager.fraction(name, hour, dow),
                        None => 1.0,
                    };
                    zone.infiltration_mass_flow += infiltration::calc_infiltration_mass_flow(
                        infil,
                        zone.input.volume,
                        zone.temp,
                        t_outdoor,
                        wind_speed_local,
                        rho_outdoor,
                    ) * sched_mult;
                }
            }

            // Scheduled ventilation (e.g., night ventilation for Case 650)
            let vent_flow = zone.input.scheduled_ventilation_flow(
                hour,
                zone.input.volume,
                zone.temp,
                t_outdoor,
            );
            zone.ventilation_mass_flow = vent_flow * rho_outdoor;

            // Exhaust fan (removes air from zone — schedule-aware)
            // Power uses same physics as Fan component (fan.rs):
            //   Power = MassFlow × PressureRise / (TotalEff × ρ_air)
            //   ShaftPower = MotorEff × Power
            //   HeatToAir = ShaftPower + (Power - ShaftPower) × MotorInAirFrac
            //   HeatToZone = Power - HeatToAir  (motor heat NOT in airstream stays in zone)
            if let Some(ref exhaust) = zone.input.exhaust_fan {
                let exhaust_frac = match &exhaust.schedule {
                    Some(name) => self.schedule_manager.fraction(name, hour, dow),
                    None => 1.0,
                };
                let vol_flow = exhaust.flow_rate * exhaust_frac;
                let mass_flow = vol_flow * rho_outdoor;
                zone.exhaust_mass_flow = mass_flow;

                // Fan power (same formula as fan.rs lines 110-114)
                if mass_flow > 0.0 && exhaust.pressure_rise > 0.0 {
                    let power = (mass_flow * exhaust.pressure_rise
                        / (exhaust.total_efficiency * rho_outdoor))
                        .max(0.0);
                    let shaft_power = exhaust.motor_efficiency * power;
                    let heat_to_air =
                        shaft_power + (power - shaft_power) * exhaust.motor_in_airstream_fraction;
                    // Heat in airstream exits with exhaust (lost from zone).
                    // Heat NOT in airstream stays in zone as convective gain.
                    zone.exhaust_fan_power = power;
                    zone.exhaust_fan_heat_to_zone = power - heat_to_air;
                } else {
                    zone.exhaust_fan_power = 0.0;
                    zone.exhaust_fan_heat_to_zone = 0.0;
                }
            } else {
                zone.exhaust_mass_flow = 0.0;
                zone.exhaust_fan_power = 0.0;
                zone.exhaust_fan_heat_to_zone = 0.0;
            }

            // Natural ventilation (wind + stack driven through operable openings)
            //
            // EnergyPlus ZoneVentilation:WindandStackOpenArea model:
            //   V = sqrt(V_wind² + V_stack²)
            // where:
            //   V_wind  = Cw × A_opening × v_wind
            //   V_stack = Cd × A_opening × sqrt(2·g·ΔH·|Tz-To|/(Tz+273.15))
            //
            // Opening effectiveness Cw depends on windward/leeward orientation:
            //   Cw = 0.55 if windward (wind hits the opening face)
            //   Cw = 0.30 if leeward  (wind is behind the opening)
            //
            // Natural ventilation is disabled during sizing (design day) runs.
            if let Some(ref nv) = zone.input.natural_ventilation.clone() {
                // Schedule check (disabled during design days)
                let sched_frac = if ctx.is_sizing {
                    0.0
                } else {
                    match &nv.schedule {
                        Some(name) => self.schedule_manager.fraction(name, hour, dow),
                        None => 1.0,
                    }
                };

                // Temperature and wind speed conditions
                let t_zone = zone.temp;
                let temp_ok = t_zone >= nv.min_indoor_temp
                    && t_zone <= nv.max_indoor_temp
                    && t_outdoor >= nv.min_outdoor_temp
                    && t_outdoor <= nv.max_outdoor_temp
                    && wind_speed_met <= nv.max_wind_speed;

                if sched_frac > 0.0 && temp_ok {
                    // Wind-driven component
                    // Determine windward/leeward: the opening is windward if the
                    // angle between wind direction and the outward normal of the
                    // opening is < 90°.
                    let angle_diff = {
                        let mut d = (wind_direction - nv.effective_angle).abs() % 360.0;
                        if d > 180.0 {
                            d = 360.0 - d;
                        }
                        d
                    };
                    let cw = if angle_diff <= 90.0 { 0.55 } else { 0.30 };
                    let v_wind = cw * nv.opening_area * sched_frac * wind_speed_met;

                    // Stack-driven component
                    let dt_abs = (t_zone - t_outdoor).abs();
                    let t_zone_k = t_zone + 273.15;
                    let v_stack = if nv.height_difference > 0.0 && dt_abs > 0.01 {
                        let cd = nv.discharge_coefficient;
                        cd * nv.opening_area
                            * sched_frac
                            * (2.0 * 9.81 * nv.height_difference * dt_abs / t_zone_k).sqrt()
                    } else {
                        0.0
                    };

                    // Combined flow (root-sum-of-squares)
                    let v_total = (v_wind * v_wind + v_stack * v_stack).sqrt();

                    zone.nat_vent_flow = v_total;
                    zone.nat_vent_mass_flow = v_total * rho_outdoor;
                    zone.nat_vent_active = true;
                    zone.nat_vent_off_timesteps = 0;
                } else {
                    zone.nat_vent_flow = 0.0;
                    zone.nat_vent_mass_flow = 0.0;
                    zone.nat_vent_active = false;
                    // Increment off-timestep counter (saturate to avoid overflow)
                    if zone.nat_vent_off_timesteps < u32::MAX {
                        zone.nat_vent_off_timesteps = zone.nat_vent_off_timesteps.saturating_add(1);
                    }
                }
            } else {
                zone.nat_vent_flow = 0.0;
                zone.nat_vent_mass_flow = 0.0;
                zone.nat_vent_active = false;
            }

            // ASHRAE 62.1 outdoor air (calculated from people count + floor area + absolute + ACH)
            if let Some(ref oa) = zone.input.outdoor_air {
                let people_count: f64 = zone
                    .input
                    .internal_gains
                    .iter()
                    .filter_map(|g| match g {
                        internal_gains::InternalGainInput::People {
                            count, schedule, ..
                        } => {
                            let frac = match (schedule, Some(&self.schedule_manager)) {
                                (Some(name), Some(mgr)) => mgr.fraction(name, hour, dow),
                                _ => 1.0,
                            };
                            Some(count * frac)
                        }
                        _ => None,
                    })
                    .sum();

                // Supply OA from all four specification methods
                let person_flow = oa.per_person * people_count;
                let area_flow = oa.per_area * zone.input.floor_area;
                let abs_flow = oa.absolute;
                let ach_flow = oa.ach * zone.input.volume / 3600.0;
                let oa_flow = match oa.oa_method {
                    crate::zone_loads::OaMethod::Sum => {
                        person_flow + area_flow + abs_flow + ach_flow
                    }
                    crate::zone_loads::OaMethod::Maximum => {
                        person_flow.max(area_flow).max(abs_flow).max(ach_flow)
                    }
                };
                zone.outdoor_air_mass_flow = oa_flow * rho_outdoor;

                // Exhaust flow from ventilation requirements
                // (supplements the explicit exhaust_fan flow via ASHRAE combined model)
                let ex_people = oa.exhaust_per_person * people_count;
                let ex_area = oa.exhaust_per_area * zone.input.floor_area;
                let ex_abs = oa.exhaust_absolute;
                let ex_ach = oa.exhaust_ach * zone.input.volume / 3600.0;
                let exhaust_from_oa = match oa.exhaust_method {
                    crate::zone_loads::OaMethod::Sum => ex_people + ex_area + ex_abs + ex_ach,
                    crate::zone_loads::OaMethod::Maximum => {
                        ex_people.max(ex_area).max(ex_abs).max(ex_ach)
                    }
                };
                // Add exhaust requirement to exhaust mass flow (supplements fan exhaust).
                // Take max of fan exhaust and requirement exhaust — they represent
                // the same physical air removal, not additive sources.
                let exhaust_req_mass_flow = exhaust_from_oa * rho_outdoor;
                if exhaust_req_mass_flow > zone.exhaust_mass_flow {
                    zone.exhaust_mass_flow = exhaust_req_mass_flow;
                }
            } else {
                zone.outdoor_air_mass_flow = 0.0;
            }
        }

        // 5. Incident solar on each surface
        //
        // When shading_calculation == Detailed, compute sunlit fractions for all
        // outdoor surfaces using the Sutherland-Hodgman polygon clipping algorithm.
        // The sunlit fraction reduces the beam (direct) component only; diffuse
        // radiation is unaffected. When shading_calculation == Basic (default),
        // all surfaces are treated as fully sunlit (sunlit_fraction = 1.0).

        // 5a. Sunlit fractions: use precomputed cache if available, else compute inline
        let sunlit_fractions: Vec<f64> = if let Some(ref cache) = self.solar_cache {
            // Look up from Suncast-style precomputed cache
            let hour_idx = ((doy - 1) as usize) * 24 + (hour as usize - 1);
            let step_idx = (hour_idx - cache.start_hour) * cache.timesteps_per_hour as usize
                + (ctx.timestep.sub_hour as usize - 1);
            if step_idx < cache.sunlit_fractions.len() {
                cache.sunlit_fractions[step_idx].clone()
            } else {
                vec![1.0; self.surfaces.len()]
            }
        } else if self.shading_calculation == shading::ShadingCalculation::Detailed
            && sol_pos.altitude > 0.0
            && !self.shading_polygons.is_empty()
        {
            // Inline computation (no cache — design days, sizing, or basic mode)
            self.surfaces
                .iter()
                .map(|surface| {
                    if surface.input.boundary != BoundaryCondition::Outdoor {
                        return 1.0;
                    }
                    if let Some(ref verts) = surface.input.vertices {
                        if verts.len() >= 3 {
                            let normal = geometry::newell_normal(verts).normalize();
                            let self_name = format!("self:{}", surface.input.name);
                            let casters: Vec<&shading::ShadingPolygon> = self
                                .shading_polygons
                                .iter()
                                .filter(|sp| sp.name != self_name)
                                .collect();
                            return shading::calculate_sunlit_fraction(
                                verts, &normal, &casters, &sun_dir,
                            );
                        }
                    }
                    1.0
                })
                .collect()
        } else {
            vec![1.0; self.surfaces.len()]
        };

        // 5b. Apply solar radiation with sunlit fractions
        for (si, surface) in self.surfaces.iter_mut().enumerate() {
            let sunlit = sunlit_fractions[si];
            if surface.input.boundary == BoundaryCondition::Outdoor && surface.input.sun_exposure {
                // Always split into beam/diffuse components to support shading
                let components = solar::incident_solar_components(
                    weather.direct_normal_rad,
                    weather.diffuse_horiz_rad,
                    weather.global_horiz_rad,
                    &sol_pos,
                    surface.input.azimuth,
                    surface.input.tilt,
                    self.ground_reflectance,
                    doy,
                    self.elevation,
                );
                // Anisotropic diffuse shading (HD total with Perez F1 decomposition):
                //   beam × SunlitFrac (directional beam, blocked by overhangs/fins)
                //   circumsolar × SunlitFrac (directional diffuse, same shading as beam)
                //   isotropic × DifShdgRatioIsoSky (sky dome view factor reduction)
                //   horizon × DifShdgRatioHoriz (horizon band obstruction)
                //
                // IMPORTANT: Circumsolar stays in the diffuse transmittance path for windows.
                // This preserves validated base-case results: for unshaded surfaces (skyR=1,
                // sunlit=1), the total diffuse = iso + cs + hz = HD_total regardless of the
                // F1 decomposition, giving identical hemispherical-modifier transmission.
                // Beam: only DNI × SunlitFrac (directional, blocked by overhangs/fins)
                let shaded_beam = components.beam * sunlit;
                // Diffuse shading configuration:
                //   - Circumsolar: shaded by sunlit fraction (directional, like beam)
                //   - Isotropic sky: shaded by DifShdgRatioIsoSky for windows only
                //     (geometric view factor reduction from overhangs/fins)
                //   - Horizon: passes through unshaded (overhangs block upper sky,
                //     not low-altitude horizon band)
                //
                // The geometric sky_ratio is floored at 0.75 (max 25% blocking) because
                // our model does not account for inter-reflections between shading
                // surfaces. For complex configurations (overhang + fins), reflected
                // diffuse radiation bouncing between shading devices and the wall
                // recovers a portion of the blocked sky radiation. The 25% cap is a
                // conservative limit that prevents over-shading E/W facades while
                // correctly applying moderate diffuse shading to south overhangs.
                let (sky_ratio, horiz_ratio) = if surface.is_window {
                    // Windows: 25% cap to account for inter-reflections not modeled
                    (surface.diffuse_sky_shading_ratio.max(0.75), 1.0)
                } else {
                    // Opaque surfaces: apply full DifShdgRatio from building
                    // self-shading (e.g., attached garage, perpendicular walls),
                    // matching E+ FullExterior behavior.
                    (
                        surface.diffuse_sky_shading_ratio,
                        surface.diffuse_horizon_shading_ratio,
                    )
                };
                let shaded_sky_diffuse = components.sky_diffuse * sky_ratio
                    + components.circumsolar * sunlit
                    + components.horizon * horiz_ratio;
                let diffuse_total = shaded_sky_diffuse + components.ground_diffuse;
                let effective_incident = (shaded_beam + diffuse_total).max(0.0);

                if surface.is_window {
                    // Windows: beam uses angular SHGC modifier,
                    //          all diffuse (incl. cs) uses hemispherical SHGC modifier
                    //          (matches E+ SkyDiffuse × DiffTrans treatment)
                    // Store incident in [W] (not W/m²) so it matches transmitted_solar
                    // units for the summary report diagnostic ratio.
                    surface.incident_solar = effective_incident * surface.net_area;

                    if let Some(ref sgs) = surface.sgs_model {
                        // E+ SimpleGlazingSystem angular model (LBNL-2804E curves).
                        //
                        // Compute Tsol(θ) and SHGC(θ) directly at beam angle,
                        // matching E+'s TransAndReflAtPhi.  The Tsol/SHGC ratio
                        // varies with angle so a constant split is incorrect.
                        let (tsol_beam, shgc_beam) = solar::sgs_angular_properties(
                            components.cos_aoi,
                            sgs.tsol,
                            sgs.rsol,
                            sgs.ni,
                            &sgs.trans_curve,
                            &sgs.refl_curve,
                        );

                        // Beam: angle-dependent Tsol and SHGC
                        let beam_transmitted =
                            (tsol_beam * surface.net_area * shaded_beam).max(0.0);
                        let beam_total_shgc = (shgc_beam * surface.net_area * shaded_beam).max(0.0);

                        // Diffuse: precomputed hemispherical Tsol and SHGC fractions
                        let diff_transmitted =
                            (sgs.tsol * sgs.diff_tsol_frac * surface.net_area * diffuse_total)
                                .max(0.0);
                        let diff_total_shgc =
                            (surface.shgc * sgs.diff_shgc_frac * surface.net_area * diffuse_total)
                                .max(0.0);

                        surface.transmitted_solar = beam_transmitted + diff_transmitted;
                        surface.transmitted_solar_beam = beam_transmitted;
                        surface.transmitted_solar_diffuse = diff_transmitted;

                        let total_shgc_gain = beam_total_shgc + diff_total_shgc;
                        // Update ratio dynamically for downstream absorbed-inward calc
                        if total_shgc_gain > 1e-6 {
                            surface.solar_transmittance_ratio =
                                surface.transmitted_solar / total_shgc_gain;
                        }
                    } else {
                        // Fresnel model (per-pane optical properties available) or
                        // polynomial fallback for legacy paths.
                        let (beam_shgc, diff_shgc) = solar::window_transmitted_solar_split(
                            surface.shgc,
                            surface.net_area,
                            shaded_beam,
                            diffuse_total,
                            components.cos_aoi,
                            surface.glass_kd,
                            surface.glass_ni,
                            surface.glass_n,
                            surface.u_factor,
                        );
                        let total_shgc_gain = beam_shgc + diff_shgc;

                        let ratio = surface.solar_transmittance_ratio;
                        surface.transmitted_solar = total_shgc_gain * ratio;
                        surface.transmitted_solar_beam = beam_shgc * ratio;
                        surface.transmitted_solar_diffuse = diff_shgc * ratio;
                    }

                    // Total SHGC gain (transmitted + absorbed-inward combined).
                    let total_shgc_gain =
                        surface.transmitted_solar / surface.solar_transmittance_ratio.max(0.001);

                    // Split into transmitted (τ_sol) and absorbed-inward (N_in × α_sol).
                    //
                    // SHGC = τ_sol + N_in × α_sol. EnergyPlus routes τ_sol to interior
                    // surfaces and heats the glass with the absorbed portion. The ratio
                    // τ_sol/SHGC is pre-computed from the E+ SimpleGlazingSystem
                    // correlations (or from per-pane optical properties when available).
                    //
                    // The absorbed-inward portion warms the glass (reducing conduction
                    // loss through the window), which is the primary effect this split
                    // captures. Any residual not delivered to the zone through the glass
                    // warming is added directly via q_window_absorbed.
                    // Absorbed-inward solar: heats the glass, reducing conduction loss.
                    // This value is added to the glass temperature equation as a source
                    // term. After the glass heat balance, any portion that does not
                    // reach the zone through the glass (due to lumped-model limitations)
                    // is kept here for direct injection via q_window_absorbed.
                    surface.absorbed_solar_inside_window =
                        total_shgc_gain - surface.transmitted_solar;
                    surface.absorbed_solar_outside = 0.0;
                } else {
                    // Opaque surfaces: beam reduced by sunlit fraction
                    surface.incident_solar = effective_incident;
                    surface.absorbed_solar_outside =
                        surface.solar_absorptance_outside * effective_incident;
                    surface.transmitted_solar = 0.0;
                    surface.transmitted_solar_beam = 0.0;
                    surface.transmitted_solar_diffuse = 0.0;
                    surface.absorbed_solar_inside_window = 0.0;
                }
            } else {
                surface.incident_solar = 0.0;
                surface.transmitted_solar = 0.0;
                surface.transmitted_solar_beam = 0.0;
                surface.transmitted_solar_diffuse = 0.0;
                surface.absorbed_solar_outside = 0.0;
                surface.absorbed_solar_inside_window = 0.0;
            }
        }

        // 5c. Interior solar distribution — EnergyPlus-style geometric beam + VMULT diffuse
        //
        // For each zone with vertex data, project beam solar through windows
        // geometrically onto interior surfaces, then redistribute reflected beam
        // and diffuse solar uniformly via VMULT = 1/SUM(A_i × alpha_i).
        //
        // This replaces the fixed-fraction approach (64.2% floor, 19.1% wall, 16.7% ceiling)
        // which under-weights floor absorption of beam solar in high-mass cases.
        let mut geometric_solar_to_surface: Vec<f64> = vec![0.0; self.surfaces.len()];
        let mut has_geometric_distribution: Vec<bool> = vec![false; self.zones.len()];

        for (zi, zone) in self.zones.iter().enumerate() {
            // Always apply geometric solar distribution to surfaces.
            // FullExterior (default): beam → floor, diffuse via VMULT — works
            // without zone-level solar_distribution fractions.  Without this,
            // zones without explicit solar_distribution get ALL transmitted
            // solar dumped directly to zone air, causing 3× faster temperature
            // changes during deadband and excessive HVAC cycling.

            // FullInteriorAndExterior requires vertex data for geometric projection.
            // FullExterior only needs floor surfaces by type (no vertices needed).
            if self.solar_distribution_method == SolarDistributionMethod::FullInteriorAndExterior {
                let all_have_vertices = zone.surface_indices.iter().all(|&si| {
                    self.surfaces[si].is_window
                        || self.surfaces[si]
                            .input
                            .vertices
                            .as_ref()
                            .map_or(false, |v| v.len() >= 3)
                });
                if !all_have_vertices {
                    continue; // Fall back to fixed-fraction for this zone
                }
            }

            // Collect window beam and diffuse totals
            let total_beam: f64 = zone
                .surface_indices
                .iter()
                .filter(|&&si| self.surfaces[si].is_window)
                .map(|&si| self.surfaces[si].transmitted_solar_beam)
                .sum();
            let total_diffuse: f64 = zone
                .surface_indices
                .iter()
                .filter(|&&si| self.surfaces[si].is_window)
                .map(|&si| self.surfaces[si].transmitted_solar_diffuse)
                .sum();

            if total_beam + total_diffuse < 0.01 {
                has_geometric_distribution[zi] = true;
                continue; // No solar to distribute
            }

            // --- Phase 1: Beam distribution ---
            //
            // FullExterior (default, matches E+): ALL beam solar falls on the floor.
            //   The floor's high thermal mass absorbs and slowly releases the energy,
            //   preventing rapid zone temperature swings.  Reflected beam joins diffuse pool.
            //
            // FullInteriorAndExterior: Geometric projection of beam through windows onto
            //   all interior surfaces (walls, floor, ceiling) using Sutherland-Hodgman clipping.
            //   More physically accurate but beam on low-mass walls convects to air faster.
            let mut beam_landing: Vec<f64> = vec![0.0; self.surfaces.len()];

            if total_beam > 0.01 && sol_pos.is_sunup {
                match self.solar_distribution_method {
                    SolarDistributionMethod::FullExterior => {
                        // All beam solar goes to floor surfaces, weighted by area.
                        // This matches E+ "FullExterior" / "FullInteriorAndExteriorWithReflections"
                        // default behavior where beam is assumed to strike the floor.
                        let total_floor_area: f64 = zone
                            .surface_indices
                            .iter()
                            .filter(|&&si| {
                                !self.surfaces[si].is_window
                                    && self.surfaces[si].input.surface_type == SurfaceType::Floor
                            })
                            .map(|&si| self.surfaces[si].net_area)
                            .sum();

                        if total_floor_area > 0.0 {
                            for &si in &zone.surface_indices {
                                if !self.surfaces[si].is_window
                                    && self.surfaces[si].input.surface_type == SurfaceType::Floor
                                {
                                    let frac = self.surfaces[si].net_area / total_floor_area;
                                    beam_landing[si] = total_beam * frac;
                                }
                            }
                        }
                        // If no floor surfaces, beam_landing stays zero → all beam
                        // joins diffuse pool in Phase 2 (reflected_beam = total_beam).
                    }
                    SolarDistributionMethod::FullInteriorAndExterior => {
                        // Geometric projection: project window beam onto all interior surfaces.
                        for &wi in &zone.surface_indices {
                            if !self.surfaces[wi].is_window {
                                continue;
                            }
                            let beam_w = self.surfaces[wi].transmitted_solar_beam;
                            if beam_w < 0.01 {
                                continue;
                            }

                            let win_verts = match self.surfaces[wi].input.vertices.as_ref() {
                                Some(v) if v.len() >= 3 => v,
                                _ => continue,
                            };
                            let win_normal = geometry::newell_normal(win_verts).normalize();
                            let win_area = geometry::polygon_area(win_verts);
                            if win_area < 1e-6 {
                                continue;
                            }

                            let cos_sun_win = sun_dir.dot(&win_normal);
                            if cos_sun_win.abs() < 1e-6 {
                                continue;
                            }

                            let parent_wall_name = self.surfaces[wi].input.parent_surface.clone();
                            let beam_flux = beam_w / win_area;

                            for &ri in &zone.surface_indices {
                                if ri == wi || self.surfaces[ri].is_window {
                                    continue;
                                }
                                if let Some(ref parent) = parent_wall_name {
                                    if self.surfaces[ri].input.name == *parent {
                                        continue;
                                    }
                                }

                                let recv_verts = match self.surfaces[ri].input.vertices.as_ref() {
                                    Some(v) if v.len() >= 3 => v,
                                    _ => continue,
                                };
                                let recv_normal = geometry::newell_normal(recv_verts).normalize();

                                if sun_dir.dot(&recv_normal) <= 0.0 {
                                    continue;
                                }

                                let projected = match shading::project_polygon_onto_plane(
                                    win_verts,
                                    &recv_verts[0],
                                    &recv_normal,
                                    &sun_dir,
                                ) {
                                    Some(p) => p,
                                    None => continue,
                                };

                                let (origin, u_axis, v_axis) =
                                    shading::build_local_coords(recv_verts, &recv_normal);
                                let proj_2d: Vec<shading::Point2D> = projected
                                    .iter()
                                    .map(|v| shading::to_local_2d(v, &origin, &u_axis, &v_axis))
                                    .collect();
                                let recv_2d: Vec<shading::Point2D> = recv_verts
                                    .iter()
                                    .map(|v| shading::to_local_2d(v, &origin, &u_axis, &v_axis))
                                    .collect();

                                let clipped = shading::sutherland_hodgman_clip(&proj_2d, &recv_2d);
                                if clipped.len() < 3 {
                                    continue;
                                }

                                let overlap_area_recv = shading::polygon_area_2d(&clipped);
                                if overlap_area_recv < 1e-8 {
                                    continue;
                                }

                                let cos_recv = sun_dir.dot(&recv_normal).abs();
                                let cos_win = cos_sun_win.abs();
                                let overlap_area_win = if cos_win > 1e-6 {
                                    overlap_area_recv * cos_recv / cos_win
                                } else {
                                    0.0
                                };

                                beam_landing[ri] += beam_flux * overlap_area_win;
                            }
                        }
                    }
                }
            }

            // --- Phase 2: Compute absorbed beam and reflected beam ---
            let mut total_beam_absorbed = 0.0_f64;
            for &si in &zone.surface_indices {
                if self.surfaces[si].is_window || beam_landing[si] < 1e-10 {
                    continue;
                }
                let absorbed = self.surfaces[si].solar_absorptance_inside * beam_landing[si];
                total_beam_absorbed += absorbed;
            }
            // Any beam that wasn't absorbed joins the diffuse pool
            let reflected_beam = (total_beam - total_beam_absorbed).max(0.0);

            // --- Phase 3: VMULT diffuse distribution with cavity back-out ---
            // Diffuse pool = transmitted diffuse + reflected beam
            let diffuse_pool_raw = total_diffuse + reflected_beam;

            // VMULT = 1 / SUM(A_i × alpha_i) for all opaque surfaces in zone
            let sum_a_alpha: f64 = zone
                .surface_indices
                .iter()
                .filter(|&&si| !self.surfaces[si].is_window)
                .map(|&si| self.surfaces[si].net_area * self.surfaces[si].solar_absorptance_inside)
                .sum();

            // Cavity back-out correction: interior-reflected diffuse solar that
            // bounces off surfaces and exits back through windows. Modeled as an
            // infinite geometric series of bounces, where each bounce either:
            //   (a) is absorbed by an opaque surface (fraction ∝ A_i × α_i)
            //   (b) transmits back out through a window (fraction ∝ A_win × τ_back)
            //   (c) is absorbed by window glass and flows outward (∝ A_win × α_back × (1 - N_i))
            //   (d) is absorbed by window glass and flows inward (∝ A_win × α_back × N_i) — stays in zone
            //   (e) is reflected and continues bouncing
            // The effective window loss per bounce includes both transmission and
            // the outward-flowing portion of glass absorptance:
            //   loss_win = Σ(A_win × (τ_back + α_back × (1 - N_i)))
            // F_cavity = Σ(A_i×α_i) / (Σ(A_i×α_i) + loss_win)
            let sum_win_loss: f64 = zone
                .surface_indices
                .iter()
                .filter(|&&si| self.surfaces[si].is_window)
                .map(|&si| {
                    let s = &self.surfaces[si];
                    let tau_back = s.diffuse_back_transmittance;
                    let abs_back = s.diffuse_back_absorptance;
                    let ni = s.glass_ni;
                    // Outward loss = transmitted + absorbed-outward
                    s.net_area * (tau_back + abs_back * (1.0 - ni))
                })
                .sum();
            let cavity_denom = sum_a_alpha + sum_win_loss;
            let f_cavity = if cavity_denom > 0.0 {
                sum_a_alpha / cavity_denom
            } else {
                1.0
            };
            let diffuse_pool = diffuse_pool_raw * f_cavity;

            let vmult = if sum_a_alpha > 0.0 {
                1.0 / sum_a_alpha
            } else {
                0.0
            };

            // --- Phase 4: Total solar to each surface ---
            for &si in &zone.surface_indices {
                if self.surfaces[si].is_window {
                    continue;
                }
                let beam_absorbed = self.surfaces[si].solar_absorptance_inside * beam_landing[si];
                let diffuse_absorbed = self.surfaces[si].solar_absorptance_inside
                    * self.surfaces[si].net_area
                    * diffuse_pool
                    * vmult;
                geometric_solar_to_surface[si] = beam_absorbed + diffuse_absorbed;
            }

            has_geometric_distribution[zi] = true;
        }

        // 6. Surface ↔ zone coupling iteration
        //
        // Track the actual CTF conduction fluxes for history update.
        // These are the pure conduction q values from apply_ctf(), NOT the
        // surface-to-zone convective fluxes (which include radiative gains).
        let n_surf = self.surfaces.len();
        let mut ctf_q_inside: Vec<f64> = vec![0.0; n_surf];
        let mut ctf_q_outside: Vec<f64> = vec![0.0; n_surf];

        // Save original window absorbed-inward solar values before the iteration
        // loop. These are set by the solar calculation (step 5) and must NOT be
        // modified during iteration — each iteration should see the same solar
        // source. Without this, the residual correction in the glass heat balance
        // would cascade: iteration N overwrites absorbed_solar_inside_window with
        // the residual, then iteration N+1 reads the residual as the "original"
        // value, causing exponential decay toward zero.
        let original_window_absorbed: Vec<f64> = self
            .surfaces
            .iter()
            .map(|s| s.absorbed_solar_inside_window)
            .collect();

        let max_iterations = 5;
        for _iter in 0..max_iterations {
            // Restore original absorbed-inward solar for windows
            for si in 0..self.surfaces.len() {
                if self.surfaces[si].is_window {
                    self.surfaces[si].absorbed_solar_inside_window = original_window_absorbed[si];
                }
            }
            // 6a. Outside surface temperatures
            // For outdoor opaque surfaces, the exterior surface energy balance is:
            //   q_solar_abs + h_conv*(T_air - T_s) + h_rad_sky*(T_sky - T_s)
            //     + h_rad_gnd*(T_gnd - T_s) + q_CTF_outside = 0
            //
            // where q_CTF_outside = X[0]*T_s_out - Y[0]*T_s_in + history_terms
            //
            // Solving for T_s_out:
            //   T_s = (h_conv*T_air + h_rad_sky*T_sky + h_rad_gnd*T_gnd + q_solar
            //          + Y[0]*T_s_in - flux_history_outside)
            //         / (h_conv + h_rad_sky + h_rad_gnd + X[0])
            //
            // This couples the exterior surface to the building interior through
            // the CTF conduction term, preventing the surface from sitting at
            // the outdoor air temperature regardless of interior conditions.
            for si in 0..self.surfaces.len() {
                match &self.surfaces[si].input.boundary.clone() {
                    BoundaryCondition::Outdoor => {
                        if self.surfaces[si].input.wind_exposure {
                            // Wind speed at surface centroid height (E+ DataSurfaces.cc:635-660)
                            let wind_at_surface = convection::wind_speed_at_height(
                                wind_speed_met,
                                self.surfaces[si].centroid_height,
                                convection::DEFAULT_WEATHER_WIND_MOD_COEFF,
                                self.terrain.wind_exp(),
                                self.terrain.wind_bl_height(),
                            );
                            self.surfaces[si].h_conv_outside = convection::exterior_convection_full(
                                self.surfaces[si].temp_outside,
                                t_outdoor,
                                wind_at_surface,
                                self.surfaces[si].input.tilt,
                                self.surfaces[si].roughness,
                                wind_direction,
                                self.surfaces[si].input.azimuth,
                            );
                        } else {
                            // NoWind: natural convection only (matches E+ NoWind surface property).
                            // Uses exterior natural convection (cosTilt NOT negated), unlike
                            // interior convection which negates cosTilt. This gives correct
                            // stability for exterior surfaces (e.g., floor exterior face-down
                            // is stable when warm, not unstable).
                            self.surfaces[si].h_conv_outside =
                                convection::exterior_natural_convection(
                                    self.surfaces[si].temp_outside,
                                    t_outdoor,
                                    self.surfaces[si].input.tilt,
                                );
                        }
                        if !self.surfaces[si].is_window {
                            let h_conv = self.surfaces[si].h_conv_outside;
                            let eps = self.surfaces[si].thermal_absorptance_outside;

                            // In E+, NoSun only suppresses solar absorption (handled
                            // separately at line ~1569). Longwave radiation exchange
                            // always occurs for outdoor-exposed surfaces based on view
                            // factors from tilt. The sun_exposure flag does NOT affect
                            // longwave radiation exchange (ASHRAE 140 floors use
                            // Outdoors+NoSun+NoWind and still exchange longwave).
                            let suppress_ext_radiation = false;

                            // E+ view factors (ConvectionCoefficients.cc)
                            let f_sky = if suppress_ext_radiation {
                                0.0
                            } else {
                                (1.0 + self.surfaces[si].cos_tilt) / 2.0
                            };
                            let f_gnd = if suppress_ext_radiation {
                                0.0
                            } else {
                                1.0 - f_sky
                            };

                            // E+ SurfAirSkyRadSplit (SurfaceGeometry.cc line 327):
                            // Splits sky-hemisphere radiation between actual sky (cold)
                            // and atmosphere (at air temperature).
                            let air_sky_rad_split = if suppress_ext_radiation {
                                0.0
                            } else {
                                (0.5 * (1.0 + self.surfaces[si].cos_tilt)).sqrt()
                            };

                            let t_s_k = (self.surfaces[si].temp_outside + 273.15).max(200.0);
                            let t_sky_k = (t_sky + 273.15).max(200.0);
                            let t_air_k = (t_outdoor + 273.15).max(200.0);
                            let t_gnd_k = t_air_k; // E+ uses outdoor dry bulb for ground

                            // Exact linearization matching E+ (ConvectionCoefficients.cc lines 662-676):
                            //   h = σ·ε·F·(T_s⁴ - T_ref⁴)/(T_s - T_ref)
                            // Falls back to 4·σ·ε·F·T³ when ΔT is small
                            let exact_h_rad = |t1: f64, t2: f64, f_view: f64| -> f64 {
                                let dt = (t1 - t2).abs();
                                if dt > 0.1 {
                                    SIGMA * eps * f_view * (t1.powi(4) - t2.powi(4)).abs() / dt
                                } else {
                                    4.0 * SIGMA * eps * f_view * ((t1 + t2) / 2.0).powi(3)
                                }
                            };

                            // E+ three exterior radiation coefficients:
                            // HSky: radiation to sky dome (at sky temperature)
                            let h_sky = exact_h_rad(t_s_k, t_sky_k, f_sky * air_sky_rad_split);
                            // HAir: radiation to atmosphere (at air temperature)
                            let h_air =
                                exact_h_rad(t_s_k, t_air_k, f_sky * (1.0 - air_sky_rad_split));
                            // HGround: radiation to ground (at ground/air temperature)
                            let h_gnd = exact_h_rad(t_s_k, t_gnd_k, f_gnd);

                            // CTF conduction coupling: include X[0] and Y[0] terms
                            // from the outside CTF equation to couple exterior surface
                            // to interior through the wall assembly.
                            let (ctf_x0, ctf_y0, ctf_flux_hist) =
                                if let (Some(ctf), Some(history)) = (
                                    self.ctf_coefficients[si].as_ref(),
                                    self.ctf_histories[si].as_ref(),
                                ) {
                                    // Flux history: Σ(Φ·q_out_old) + higher-order X,Y terms
                                    let mut flux_hist = 0.0_f64;
                                    for j in 0..ctf.phi.len() {
                                        if j < history.q_outside.len() {
                                            flux_hist += ctf.phi[j] * history.q_outside[j];
                                        }
                                    }
                                    for j in 1..ctf.x.len() {
                                        let idx = j - 1;
                                        if idx < history.t_outside.len() {
                                            flux_hist += ctf.x[j] * history.t_outside[idx];
                                        }
                                        if idx < history.t_inside.len() {
                                            flux_hist -= ctf.y[j] * history.t_inside[idx];
                                        }
                                    }
                                    (ctf.x[0], ctf.y[0], flux_hist)
                                } else {
                                    (0.0, 0.0, 0.0)
                                };

                            // E+ outside surface equation (HeatBalanceSurfaceManager.cc line 9573-9580):
                            // T_out = (-CTFConstOutPart + q_solar + (h_conv+h_air)*T_ext
                            //         + h_sky*T_sky + h_gnd*T_gnd + CTFCross[0]*T_in)
                            //       / (CTFOutside[0] + h_conv + h_air + h_sky + h_gnd)
                            let h_total = h_conv + h_air + h_sky + h_gnd + ctf_x0;

                            self.surfaces[si].temp_outside = ((h_conv + h_air) * t_outdoor
                                + h_sky * t_sky
                                + h_gnd * t_outdoor
                                + self.surfaces[si].absorbed_solar_outside
                                + ctf_y0 * self.surfaces[si].temp_inside
                                - ctf_flux_hist)
                                / h_total.max(1.0);
                        }
                    }
                    BoundaryCondition::Adiabatic => {
                        self.surfaces[si].temp_outside = self.surfaces[si].temp_inside;
                    }
                    BoundaryCondition::Ground => {
                        // F-factor ground floors use FCfactorMethod temps;
                        // standard ground floors use building-level ground temp model.
                        self.surfaces[si].temp_outside =
                            if let Some(ref temps) = self.surfaces[si].f_factor_ground_temps {
                                // E+ holds FCfactorMethod temps constant for
                                // the entire month (step function, no interpolation).
                                GroundTempModel::monthly_step_static(temps, ctx.timestep.month)
                            } else if let Some(ref gt) = self.ground_temp_model {
                                gt.temperature(doy as f64)
                            } else {
                                10.0
                            };
                    }
                    BoundaryCondition::Zone(other_zone) => {
                        // E+ approach: set outside temperature to paired surface's
                        // inside temperature, NOT the zone air temp.  This ensures
                        // the CTF cross-coupling term Y[0] correctly couples the
                        // two zone-interior surfaces through a single wall thickness.
                        // (HeatBalanceSurfaceManager.cc: TH(SurfNum,1,1) = TH(ExtSurfNum,1,2))
                        if let Some(paired) = self.interzone_pairs[si] {
                            self.surfaces[si].temp_outside = self.surfaces[paired].temp_inside;
                        } else if self.surfaces[si].input.zone == *other_zone {
                            // Self-referencing: surface references its own zone.
                            // E+ sets T_outside = T_inside for self-referencing surfaces
                            // (HeatBalanceSurfaceManager.cc: TH(SurfNum,1,1) = TH(SurfNum,1,2)).
                            // This creates a symmetric boundary that gives the slab its full
                            // thermal mass without the artificial reservoir effect of using
                            // zone air temperature.
                            self.surfaces[si].temp_outside = self.surfaces[si].temp_inside;
                        } else {
                            // Fallback: use zone air temp (no matched pair)
                            if let Some(&zi) = self.zone_index.get(other_zone) {
                                self.surfaces[si].temp_outside = self.zones[zi].temp;
                            }
                        }
                    }
                }
            }

            // 6b-6c. Inside surface heat balance with iteration loop
            //
            // Matches EnergyPlus HeatBalanceSurfaceManager.cc lines 7903-8534.
            //
            // E+ iterates the inside surface heat balance until convergence:
            //   1. Save old inside temps (SurfTempInsOld)
            //   2. Compute net LW radiation exchange (CalcInteriorRadExchange)
            //   3. Solve T_inside for each surface
            //   4. Check convergence: max|T_new - T_old| ≤ 0.002°C
            //
            // E+ equation (standard, no source/sink):
            //   T_i = (TempTerm + IterDampConst*T_old + CTFCross[0]*T_out)
            //       / (CTFInside[0] + HConvIn + IterDampConst)
            //   where TempTerm includes q_lw_net as a flux (NOT linearized h_rad).
            //
            // Our approach: linearize LW radiation as h_rad*(T_mrt - T_i), which
            // puts h_rad*T_mrt in numerator and h_rad in denominator. We still
            // iterate to update h_rad and MRT with current surface temps.
            // IterDampConst aids convergence of the iteration.
            //
            // Reference: DataHeatBalSurface.hh line 69:
            //   Real64 constexpr IterDampConst(5.0);
            const ITER_DAMP_CONST: f64 = 5.0;
            const MAX_INSIDE_SURF_ITER: usize = 500;
            const CONVERGENCE_TOLERANCE: f64 = 0.002; // °C, matches E+ MaxAllowedDelTemp

            // Collect zone temps (mutable: predictor sets t_zone_vec for ideal
            // loads zones so surfaces see the correct zone temp during iteration)
            let mut t_zone_vec: Vec<f64> = self.zones.iter().map(|z| z.temp).collect();
            // Predicted HVAC mode per zone is stored on zone.ideal_pred_mode
            // (locked across HVAC iterations within a physical timestep).

            // Handle windows first (not part of iteration)
            //
            // Windows use dynamic film coefficients (matching EnergyPlus approach).
            // The overall U-factor is decomposed into glass conductance by removing
            // NFRC standard films. Dynamic exterior AND interior films are then
            // applied at runtime based on actual conditions.
            //
            // Exterior radiation: Window exterior face exchanges LW radiation with
            // sky, atmosphere, and ground — identical to opaque surfaces. This
            // matches E+ which treats window exterior LW exactly like opaque walls.
            // Using E+ SurfAirSkyRadSplit to separate sky hemisphere into actual
            // sky (at T_sky) and atmosphere (at T_air).
            //
            // Interior: The window interior surface exchanges heat via:
            //   - TARP natural convection to zone air (h_conv)
            //   - Linearized radiation to zone MRT (h_rad)
            // The glass temperature T_glass is solved from a 3-node balance:
            //   u_ext·(T_ext_eff - T_glass) = h_conv·(T_glass - T_zone) + h_rad·(T_glass - T_mrt)
            //
            // This properly separates convective (to zone air) and radiative (to
            // surfaces via MRT) heat transfer paths, matching E+'s approach where
            // window interior surface temperature is explicitly solved and
            // participates in the ScriptF radiation exchange.
            //
            // The convective portion enters the zone air balance through sum_ha/sum_hat
            // (since temp_inside = T_glass). The radiative portion is captured through
            // the cold T_glass lowering the MRT for opaque surfaces, which then
            // exchange more heat with zone air.

            // Compute zone MRT from OPAQUE surface temperatures for window interior
            // radiation. Windows are excluded because their temp_inside hasn't been
            // computed yet. This uses temperatures from the previous iteration/timestep.
            //
            // Two approaches depending on whether view factors are available:
            //
            // 1. View-factor MRT (zones with vertex geometry): precompute per-face
            //    ε-weighted temperature averages, then each window gets its MRT from
            //    the VF matrix based on which face it's on. This gives the floor a
            //    much larger influence on south windows than the ceiling (~0.32 vs
            //    ~0.10), matching reality and improving heating accuracy.
            //
            // 2. Area-weighted MRT (fallback): single zone MRT from ε·A weighting.
            let mut zone_mrt_for_windows = vec![21.0_f64; self.zones.len()];

            // Per-face ε-weighted data for VF zones (used for both windows and opaque MRT)
            let mut vf_face_ea: Vec<Option<[f64; 6]>> = vec![None; self.zones.len()];
            let mut vf_face_eat: Vec<Option<[f64; 6]>> = vec![None; self.zones.len()];

            for (zi, zone) in self.zones.iter().enumerate() {
                let mut sum_ea = 0.0_f64;
                let mut sum_eat = 0.0_f64;

                // Always compute area-weighted totals (used as fallback and
                // for zones without view factors).
                // Include ALL surfaces (opaque + windows) so that MRT accounts
                // for cold window temperatures. Windows use ε=0.84 (standard
                // glass emissivity), matching the value used in the window solve.
                for &si in &zone.surface_indices {
                    let s = &self.surfaces[si];
                    let eps = if s.is_window {
                        s.glass_emissivity
                    } else {
                        s.thermal_absorptance_inside
                    };
                    let ea = eps * s.net_area;
                    sum_ea += ea;
                    sum_eat += ea * s.temp_inside;
                }
                if sum_ea > 0.01 {
                    zone_mrt_for_windows[zi] = sum_eat / sum_ea;
                } else {
                    zone_mrt_for_windows[zi] = t_zone_vec.get(zi).copied().unwrap_or(21.0);
                }

                // For VF zones: compute per-face ε·(A/A_face) sums.
                // Include ALL surfaces (opaque + windows) so that the VF-weighted
                // MRT properly accounts for cold window temperatures on each face.
                if let Some(ref zvf) = self.zone_view_factors[zi] {
                    let mut fea = [0.0_f64; 6];
                    let mut feat = [0.0_f64; 6];
                    for &si in &zone.surface_indices {
                        let s = &self.surfaces[si];
                        let eps = if s.is_window {
                            s.glass_emissivity
                        } else {
                            s.thermal_absorptance_inside
                        };
                        if let Some(face) = s.box_face {
                            let face_total = zvf.face_area[face].max(0.01);
                            let w = eps * s.net_area / face_total;
                            fea[face] += w;
                            feat[face] += w * s.temp_inside;
                        }
                    }
                    vf_face_ea[zi] = Some(fea);
                    vf_face_eat[zi] = Some(feat);
                }
            }

            // Window LW radiation is now handled through the unified face-averaged
            // MRT (windows included in fea/feat), so no separate redistribution needed.

            for i in 0..self.surfaces.len() {
                if self.surfaces[i].is_window {
                    let zi = self
                        .zone_index
                        .get(&self.surfaces[i].input.zone)
                        .copied()
                        .unwrap_or(0);
                    let t_z = t_zone_vec.get(zi).copied().unwrap_or(21.0);

                    // Per-window MRT: use view-factor weighting if available,
                    // otherwise fall back to area-weighted zone MRT.
                    let t_mrt =
                        if let (Some(face_i), Some(ref zvf), Some(ref fea), Some(ref feat)) = (
                            self.surfaces[i].box_face,
                            self.zone_view_factors.get(zi).and_then(|v| v.as_ref()),
                            vf_face_ea.get(zi).and_then(|v| v.as_ref()),
                            vf_face_eat.get(zi).and_then(|v| v.as_ref()),
                        ) {
                            // VF-weighted MRT for this window:
                            // T_mrt = Σ_{k≠face_i} F(face_i→k)·feat[k]
                            //       / Σ_{k≠face_i} F(face_i→k)·fea[k]
                            let mut num = 0.0_f64;
                            let mut den = 0.0_f64;
                            for k in 0..6 {
                                if k == face_i {
                                    continue;
                                }
                                let f = zvf.face_vf[face_i][k];
                                num += f * feat[k];
                                den += f * fea[k];
                            }
                            if den > 1.0e-10 {
                                num / den
                            } else {
                                t_z
                            }
                        } else {
                            zone_mrt_for_windows.get(zi).copied().unwrap_or(t_z)
                        };

                    // Dynamic exterior combined coefficient: h_conv (already computed
                    // in the exterior loop above) + exterior longwave radiation split
                    // between sky and ground, matching E+ and the opaque surface model.
                    //
                    // Previously, window exterior radiation used T_outdoor as the
                    // reference temperature for all radiation. But vertical windows
                    // see the cold sky (F_sky=0.5 for vertical), and sky depression
                    // in Boulder winter is ~16°C. Using T_outdoor overestimates the
                    // effective exterior temperature by 2-3°C, reducing window heat
                    // loss by ~600-900 kWh/year.
                    let h_conv_out = self.surfaces[i].h_conv_outside;
                    let eps_glass = self.surfaces[i].glass_emissivity;
                    let t_ext_surf_est = self.surfaces[i].temp_outside;
                    let cos_tilt = self.surfaces[i].cos_tilt;

                    // View factors to sky and ground (same as opaque surfaces)
                    let f_sky = (1.0 + cos_tilt) / 2.0;
                    let f_gnd = 1.0 - f_sky;

                    // E+ SurfAirSkyRadSplit: fraction of sky hemisphere that is
                    // actual sky (cold) vs atmosphere (at air temperature)
                    let air_sky_rad_split = (0.5 * (1.0 + cos_tilt)).sqrt();

                    let t_s_k = (t_ext_surf_est + 273.15).max(200.0);
                    let t_sky_k = (t_sky + 273.15).max(200.0);
                    let t_air_k = (t_outdoor + 273.15).max(200.0);

                    // Linearized radiation coefficients for each environment
                    let h_rad_base = |t1: f64, t2: f64, f_view: f64| -> f64 {
                        let dt = (t1 - t2).abs();
                        if dt > 0.1 {
                            SIGMA * eps_glass * f_view * (t1.powi(4) - t2.powi(4)).abs() / dt
                        } else {
                            4.0 * SIGMA * eps_glass * f_view * ((t1 + t2) / 2.0).powi(3)
                        }
                    };

                    let h_sky = h_rad_base(t_s_k, t_sky_k, f_sky * air_sky_rad_split);
                    let h_air = h_rad_base(t_s_k, t_air_k, f_sky * (1.0 - air_sky_rad_split));
                    let h_gnd = h_rad_base(t_s_k, t_air_k, f_gnd);
                    let h_rad_total = h_sky + h_air + h_gnd;
                    let h_e = h_conv_out + h_rad_total;

                    // Effective exterior driving temperature: conductance-weighted
                    // average of sky, air, and ground temperatures
                    let t_ext_eff = if h_e > 1.0e-10 {
                        (h_conv_out * t_outdoor
                            + h_sky * t_sky
                            + h_air * t_outdoor
                            + h_gnd * t_outdoor)
                            / h_e
                    } else {
                        t_outdoor
                    };

                    let mut u_glass = self.surfaces[i].u_glass;
                    let tilt = self.surfaces[i].input.tilt;

                    // Combined outside-film + glass conductance
                    let mut u_e_glass = 1.0 / (1.0 / h_e + 1.0 / u_glass);

                    // Whether this window uses first-principles gap thermal model
                    let has_gap_model = self.surfaces[i].gap_width > 0.0;

                    // Absorbed-inward solar source for the glass heat balance [W/m²].
                    //
                    // The absorbed-inward solar (N_in × α × I) warms the glass interior
                    // surface, reducing the temperature difference and hence the net
                    // conduction loss through the window. This is the primary mechanism
                    // by which the SHGC → τ_sol split affects heating energy.
                    let q_solar_abs_per_area = if self.surfaces[i].net_area > 0.0 {
                        self.surfaces[i].absorbed_solar_inside_window / self.surfaces[i].net_area
                    } else {
                        0.0
                    };

                    // Iteratively solve glass temperature using 3-node balance:
                    //   u_e_glass·(T_ext_eff - T_glass) = h_conv·(T_glass - T_zone)
                    //       + h_rad·(T_glass - T_mrt) - q_solar_abs
                    //
                    // The exterior effective temperature accounts for sky radiation
                    // (T_sky << T_air for vertical windows with F_sky=0.5), pulling
                    // the glass exterior colder and increasing conduction loss.
                    // Interior radiation references the zone MRT from opaque surfaces.
                    let mut h_conv_in: f64 = 3.0;
                    let mut h_rad_in: f64 = 5.0;
                    let mut t_glass: f64 = (t_ext_eff + t_z) / 2.0;
                    for _ in 0..5 {
                        t_glass = (u_e_glass * t_ext_eff
                            + h_conv_in * t_z
                            + h_rad_in * t_mrt
                            + q_solar_abs_per_area)
                            / (u_e_glass + h_conv_in + h_rad_in);

                        // Dynamic gap thermal model (ISO 15099): recompute u_glass
                        // from current pane temperatures each iteration. This accounts
                        // for temperature-dependent gap convection (Ra varies with T)
                        // and radiation (h_rad ∝ T³). Matches EnergyPlus layer-by-layer
                        // window model.
                        if has_gap_model {
                            let t_ext = self.surfaces[i].temp_outside;
                            let t_mean_gap = (t_ext + t_glass) / 2.0;
                            let dt_gap = (t_glass - t_ext).abs().max(0.1);
                            let h_gap = sealed_air_gap_conductance(
                                self.surfaces[i].gap_width,
                                t_mean_gap,
                                dt_gap,
                                tilt,
                                self.surfaces[i].gap_emissivity,
                            );
                            let r_pane = self.surfaces[i].pane_thickness
                                / self.surfaces[i].pane_conductivity;
                            u_glass = 1.0 / (2.0 * r_pane + 1.0 / h_gap);
                            // Cap at 110% of NFRC-rated value: the gap model can
                            // improve upon the rating (lower U in winter when gas
                            // is cooler) but should not degrade too far beyond it.
                            // At extreme temperatures, gap radiation (h_rad ∝ T³)
                            // can push u_glass well above the rated value, causing
                            // excessive heat loss during free-float peak conditions.
                            // The 10% margin allows natural variation at moderate
                            // temperatures while still capping extremes.
                            u_glass = u_glass.min(self.surfaces[i].u_glass_rated * 1.05);
                            u_e_glass = 1.0 / (1.0 / h_e + 1.0 / u_glass);
                        }

                        // Interior natural convection — ISO 15099 for windows
                        let win_area = self.surfaces[i].input.area.max(0.01);
                        let win_h = self.surfaces[i]
                            .input
                            .height
                            .unwrap_or_else(|| win_area.sqrt());
                        let win_w = self.surfaces[i]
                            .input
                            .width
                            .unwrap_or_else(|| win_area.sqrt());
                        h_conv_in =
                            convection::window_interior_convection(t_glass, t_z, win_h, win_w);

                        // Interior radiation: linearized between glass and zone MRT
                        // (previously used T_zone, which over-estimated radiation to glass
                        // since T_zone > T_mrt by 4-5°C during heating)
                        let t_mean_in_k = ((t_glass + t_mrt) / 2.0 + 273.15).max(200.0);
                        h_rad_in = 4.0 * eps_glass * SIGMA * t_mean_in_k.powi(3);

                        // Enforce minimum total interior film
                        if h_conv_in + h_rad_in < 2.0 {
                            h_conv_in = h_conv_in.max(0.5);
                            h_rad_in = h_rad_in.max(1.5);
                        }
                    }

                    // Store dynamically computed u_glass for diagnostics and next timestep
                    if has_gap_model {
                        self.surfaces[i].u_glass = u_glass;
                    }

                    // Interior heat flow decomposition:
                    //
                    // In EnergyPlus, window interior convection enters the zone air
                    // balance through sum_ha/sum_hat (h_conv_inside, temp_inside =
                    // T_glass), and LW radiation is handled by the ScriptF network.
                    //
                    // Since we use simplified area-weighted MRT instead of ScriptF,
                    // routing radiation purely through MRT depression of opaque
                    // surfaces doesn't capture enough of the window radiative loss
                    // (area-weighted MRT dampens the effect relative to proper view
                    // factors). Instead:
                    //
                    //   Convective: h_conv_in → sum_ha/sum_hat (T_glass)
                    //   Radiative:  h_rad_in·(T_mrt − T_glass) → q_conv_inside
                    //               → q_window_cond (direct zone air injection)
                    //
                    // To prevent double-counting, windows are EXCLUDED from the
                    // per-surface MRT used by opaque surfaces (see zone_sum_ea/eat
                    // below). This way the radiative path is counted exactly once.

                    self.surfaces[i].temp_inside = t_glass;

                    // Window interior convection coefficient (TARP natural convection).
                    // This enters sum_ha and sum_hat in the zone air balance, coupling
                    // zone air to the cold window glass via convection.
                    self.surfaces[i].h_conv_inside = h_conv_in;

                    // Residual correction for absorbed-inward solar.
                    //
                    // The lumped glass model distributes the absorbed solar source
                    // between inward (h_conv+h_rad) and outward (u_e_glass) paths.
                    // The fraction N_lumped = (h_conv+h_rad)/(total) reaches the
                    // zone through glass warming. The remainder (1-N_lumped) flows
                    // outward — but since absorbed_inward IS the inward fraction
                    // by definition (from SHGC = τ_sol + N_in×α), any outward loss
                    // is a lumped-model artifact. The residual correction adds back
                    // this lost portion directly to zone air.
                    let absorbed_inward_total = self.surfaces[i].absorbed_solar_inside_window;
                    if absorbed_inward_total > 0.0 {
                        let h_total = u_e_glass + h_conv_in + h_rad_in;
                        let n_lumped = if h_total > 1.0e-10 {
                            (h_conv_in + h_rad_in) / h_total
                        } else {
                            1.0
                        };
                        let delivered_through_glass = n_lumped * absorbed_inward_total;
                        let residual = (absorbed_inward_total - delivered_through_glass).max(0.0);
                        self.surfaces[i].absorbed_solar_inside_window = residual;
                    }

                    // Window interior LW radiation.
                    //
                    // Windows now participate in the unified face-averaged MRT
                    // (included in fea/feat). Opaque surfaces naturally see cold
                    // window temperatures through VF-weighted MRT, and windows
                    // see warm opaque temperatures — matching E+'s ScriptF
                    // radiation exchange network.
                    //
                    // The window's q_conv_inside remains zero — the convective
                    // component enters via sum_ha/sum_hat (h_conv_inside, T_glass).
                    let q_rad_interior = h_rad_in * (t_mrt - t_glass);
                    self.surfaces[i].h_rad_inside = h_rad_in;
                    self.surfaces[i].q_rad_inside = q_rad_interior;
                    self.surfaces[i].q_conv_inside = 0.0;

                    // Exterior surface temperature from glass energy balance.
                    //
                    // The full balance at the glass node is:
                    //   u_e·(T_ext_eff − T_glass) + h_conv·(Tz − Tg)
                    //     + h_rad·(Tmrt − Tg) + q_solar_abs = 0
                    //
                    // So: u_glass·(T_ext_surf − T_glass)
                    //       = −[h_conv·(Tz−Tg) + h_rad·(Tmrt−Tg) + q_solar_abs]
                    //
                    // Previously q_solar_abs was omitted, causing T_ext_surf to be
                    // wildly wrong during solar hours (e.g. +54°C vs +5°C). This
                    // corrupted the dynamic gap model (which uses T_ext_surf to
                    // compute gap ΔT), inflating u_glass and making the glass
                    // ~4°C too cold during peak solar.
                    let q_interior =
                        h_conv_in * (t_z - t_glass) + q_rad_interior + q_solar_abs_per_area;
                    let t_ext_surf = t_glass - q_interior / u_glass;
                    self.surfaces[i].temp_outside = t_ext_surf;

                    // q_cond_inside for windows is set post-convergence in the
                    // diagnostic section using converged zone temp and h values.
                }
            }

            // Pre-compute CTF history terms and radiative gains (constant per timestep)
            struct SurfPrecomp {
                zi: usize,
                ctf_const_in: f64,
                ctf_z0: f64,
                ctf_y0: f64,
                q_rad_flux: f64,
                is_adiabatic: bool,
            }
            let mut precomp: Vec<Option<SurfPrecomp>> = Vec::with_capacity(self.surfaces.len());

            for i in 0..self.surfaces.len() {
                if self.surfaces[i].is_window {
                    precomp.push(None);
                    continue;
                }

                let zi = self
                    .zone_index
                    .get(&self.surfaces[i].input.zone)
                    .copied()
                    .unwrap_or(0);

                if let (Some(ctf), Some(history)) = (
                    self.ctf_coefficients[i].as_ref(),
                    self.ctf_histories[i].as_ref(),
                ) {
                    // CTFConstInPart: history terms (j ≥ 1)
                    let mut ctf_const_in = 0.0_f64;
                    for j in 0..ctf.phi.len() {
                        if j < history.q_inside.len() {
                            let term = ctf.phi[j] * history.q_inside[j];
                            if term.is_finite() {
                                ctf_const_in += term;
                            }
                        }
                    }
                    for j in 1..ctf.y.len() {
                        let idx = j - 1;
                        if idx < history.t_outside.len() {
                            let term = ctf.y[j] * history.t_outside[idx];
                            if term.is_finite() {
                                ctf_const_in += term;
                            }
                        }
                        if idx < history.t_inside.len() {
                            let term = ctf.z[j] * history.t_inside[idx];
                            if term.is_finite() {
                                ctf_const_in -= term;
                            }
                        }
                    }

                    // Radiative gains (internal + solar distribution)
                    let zone_rad_gain = if zi < self.zones.len() {
                        self.zones[zi].q_internal_rad
                    } else {
                        0.0
                    };

                    // Solar distribution: use geometric beam + VMULT if available,
                    // otherwise fall back to fixed fractions
                    let q_solar_to_surface = if zi < has_geometric_distribution.len()
                        && has_geometric_distribution[zi]
                    {
                        geometric_solar_to_surface[i]
                    } else if zi < self.zones.len() {
                        let zone = &self.zones[zi];
                        if let Some(ref dist) = zone.input.solar_distribution {
                            let q_sol_total: f64 = zone
                                .surface_indices
                                .iter()
                                .filter(|&&si| self.surfaces[si].is_window)
                                .map(|&si| self.surfaces[si].transmitted_solar)
                                .sum();

                            let surface_type = self.surfaces[i].input.surface_type;
                            let type_fraction = match surface_type {
                                SurfaceType::Floor => dist.floor_fraction,
                                SurfaceType::Wall => dist.wall_fraction,
                                SurfaceType::Roof | SurfaceType::Ceiling => dist.ceiling_fraction,
                                SurfaceType::Window => 0.0,
                            };

                            let same_type_area: f64 = zone
                                .surface_indices
                                .iter()
                                .filter(|&&si| !self.surfaces[si].is_window)
                                .filter(|&&si| {
                                    let st = self.surfaces[si].input.surface_type;
                                    match surface_type {
                                        SurfaceType::Floor => st == SurfaceType::Floor,
                                        SurfaceType::Wall => st == SurfaceType::Wall,
                                        SurfaceType::Roof | SurfaceType::Ceiling => {
                                            st == SurfaceType::Roof || st == SurfaceType::Ceiling
                                        }
                                        _ => false,
                                    }
                                })
                                .map(|&si| self.surfaces[si].net_area)
                                .sum();

                            if same_type_area > 0.0 {
                                q_sol_total * type_fraction * self.surfaces[i].net_area
                                    / same_type_area
                            } else {
                                0.0
                            }
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };

                    let zone_total_area: f64 = if zi < self.zones.len() {
                        self.zones[zi]
                            .surface_indices
                            .iter()
                            .map(|&si| self.surfaces[si].net_area)
                            .sum()
                    } else {
                        1.0
                    };

                    let q_rad_to_surface = if zone_total_area > 0.0 {
                        zone_rad_gain * self.surfaces[i].net_area / zone_total_area
                    } else {
                        0.0
                    };

                    // Window LW radiation is now handled through the unified
                    // face-averaged MRT — no separate redistribution needed.

                    let q_total_rad = q_rad_to_surface + q_solar_to_surface;
                    let q_rad_flux = q_total_rad / self.surfaces[i].net_area.max(0.01);

                    let is_adiabatic = matches!(
                        self.surfaces[i].input.boundary,
                        BoundaryCondition::Adiabatic
                    );

                    precomp.push(Some(SurfPrecomp {
                        zi,
                        ctf_const_in,
                        ctf_z0: ctf.z[0],
                        ctf_y0: ctf.y[0],
                        q_rad_flux,
                        is_adiabatic,
                    }));
                } else {
                    precomp.push(None);
                }
            }

            // ── Precompute VF face data once before inner iteration ────────
            //
            // For VF zones, compute per-face ε-weighted temperature averages
            // ONCE using surface temps at the start of the inner iteration.
            // These remain fixed during the inner loop for numerical stability,
            // matching how EnergyPlus fixes ScriptF exchange coefficients per
            // timestep.
            //
            // ALL surfaces (opaque + windows) participate, matching E+'s ScriptF
            // where every surface exchanges radiation with every other surface.
            // Windows use ε=0.84 (standard glass emissivity).
            //
            // fea[face] = Σ_{j on face} ε_j × A_j / A_face
            // feat[face] = Σ_{j on face} ε_j × A_j / A_face × T_j
            //
            // The area-weighted fallback (zone_sum_ea/eat) is still recomputed
            // each inner iteration because its broader averaging provides
            // sufficient self-damping.
            let mut opaque_face_ea: Vec<Option<[f64; 6]>> = vec![None; self.zones.len()];
            let mut opaque_face_eat: Vec<Option<[f64; 6]>> = vec![None; self.zones.len()];

            for (zi, zone) in self.zones.iter().enumerate() {
                if let Some(ref zvf) = self.zone_view_factors[zi] {
                    let mut fea = [0.0_f64; 6];
                    let mut feat = [0.0_f64; 6];
                    for &si in &zone.surface_indices {
                        let s = &self.surfaces[si];
                        let eps = if s.is_window {
                            s.glass_emissivity
                        } else {
                            s.thermal_absorptance_inside
                        };
                        if let Some(face) = s.box_face {
                            let face_total = zvf.face_area[face].max(0.01);
                            let w = eps * s.net_area / face_total;
                            fea[face] += w;
                            feat[face] += w * s.temp_inside;
                        }
                    }
                    opaque_face_ea[zi] = Some(fea);
                    opaque_face_eat[zi] = Some(feat);
                }
            }

            // ── Ideal loads predictor: determine HVAC mode BEFORE surface
            //    iteration ─────────────────────────────────────────────────
            //
            // E+-style predictor-corrector: the predictor uses the PREVIOUS
            // timestep's surface state to estimate whether the zone needs
            // heating, cooling, or can float in the deadband.  This mode is
            // LOCKED for the corrector to prevent oscillation / NaN.
            //
            // IMPORTANT: t_zone_vec is NOT overwritten here.  It stays at
            // zone.temp (the previous timestep value, set at line 2194).
            // In E+, CalcHeatBalanceInsideSurf runs BEFORE PredictSystemLoads
            // — surfaces always iterate with the previous timestep's zone
            // temp, not the predicted setpoint.  Overwriting t_zone_vec with
            // the setpoint creates a circular dependency: surfaces see 27°C
            // (cooling) → converge to state consistent with 27°C → corrector
            // confirms cooling.  This biased cooling by ~+200 kWh/yr for
            // ASHRAE 140 Case 900.
            for zi in 0..self.zones.len() {
                // Compute predictor result in a block to avoid borrow conflicts
                let pred_result: Option<(i8, f64)> = {
                    let zone = &self.zones[zi];
                    if zone.input.ideal_loads.is_none() {
                        None
                    } else if zone.ideal_pred_mode_locked {
                        // Reuse locked mode — still need to set t_zone_vec
                        let pred_mode = zone.ideal_pred_mode;
                        let (heat_sp, cool_sp) = zone.input.active_setpoints(hour);
                        let t_zone = match pred_mode {
                            1 => heat_sp,
                            -1 => cool_sp,
                            _ => {
                                // Re-estimate free-float for t_zone_vec using BDF3
                                let cp_air = psych::cp_air_fn_w(zone.humidity_ratio);
                                let rho_air =
                                    psych::rho_air_fn_pb_tdb_w(p_b, zone.temp, zone.humidity_ratio);
                                let (dt_eff_pred, t_prev_eff_pred) =
                                    crate::zone::backward_diff_effective(
                                        zone.temp_order,
                                        dt,
                                        zone.temp_prev,
                                        zone.temp_prev2,
                                        zone.temp_prev3,
                                    );
                                let cap_pred = rho_air * zone.input.volume * cp_air / dt_eff_pred;
                                let mut sum_ha_pred = 0.0_f64;
                                let mut sum_hat_pred = 0.0_f64;
                                for &si in &zone.surface_indices {
                                    let s = &self.surfaces[si];
                                    let ha = s.h_conv_inside * s.net_area;
                                    sum_ha_pred += ha;
                                    sum_hat_pred += ha * s.temp_inside;
                                }
                                let total_outdoor = match self.infiltration_interaction {
                                    crate::zone_loads::InfiltrationInteraction::Basic => {
                                        zone.infiltration_mass_flow
                                            + zone.ventilation_mass_flow
                                            + zone.outdoor_air_mass_flow
                                            + zone.nat_vent_mass_flow
                                    }
                                    crate::zone_loads::InfiltrationInteraction::AshraeCombined => {
                                        let unbal = (zone.exhaust_mass_flow
                                            - zone.outdoor_air_mass_flow)
                                            .max(0.0);
                                        (zone.infiltration_mass_flow.powi(2) + unbal.powi(2)).sqrt()
                                            + zone.ventilation_mass_flow
                                            + zone.outdoor_air_mass_flow
                                            + zone.nat_vent_mass_flow
                                    }
                                };
                                let mcpi_pred = total_outdoor * cp_air;
                                let q_solar_trans: f64 = zone
                                    .surface_indices
                                    .iter()
                                    .filter(|&&si| self.surfaces[si].is_window)
                                    .map(|&si| self.surfaces[si].transmitted_solar)
                                    .sum();
                                let q_solar_to_air = if zi < has_geometric_distribution.len()
                                    && has_geometric_distribution[zi]
                                {
                                    0.0
                                } else if let Some(ref dist) = zone.input.solar_distribution {
                                    let to_surf = dist.floor_fraction
                                        + dist.wall_fraction
                                        + dist.ceiling_fraction;
                                    q_solar_trans * (1.0 - to_surf).max(0.0)
                                } else {
                                    q_solar_trans
                                };
                                let q_window_cond: f64 = zone
                                    .surface_indices
                                    .iter()
                                    .filter(|&&si| self.surfaces[si].is_window)
                                    .map(|&si| {
                                        self.surfaces[si].q_conv_inside * self.surfaces[si].net_area
                                    })
                                    .sum();
                                let q_window_absorbed: f64 = zone
                                    .surface_indices
                                    .iter()
                                    .filter(|&&si| self.surfaces[si].is_window)
                                    .map(|&si| self.surfaces[si].absorbed_solar_inside_window)
                                    .sum();
                                let q_conv_pred = zone.q_internal_conv
                                    + zone.exhaust_fan_heat_to_zone
                                    + q_solar_to_air
                                    + q_window_cond
                                    + q_window_absorbed;
                                let denom_pred = sum_ha_pred + mcpi_pred + cap_pred;
                                if denom_pred > 1e-10 {
                                    (sum_hat_pred
                                        + mcpi_pred * t_outdoor
                                        + q_conv_pred
                                        + cap_pred * t_prev_eff_pred)
                                        / denom_pred
                                } else {
                                    zone.temp_prev
                                }
                            }
                        };
                        Some((pred_mode, t_zone))
                    } else {
                        let cp_air = psych::cp_air_fn_w(zone.humidity_ratio);
                        let rho_air =
                            psych::rho_air_fn_pb_tdb_w(p_b, zone.temp, zone.humidity_ratio);

                        let mut sum_ha_pred = 0.0_f64;
                        let mut sum_hat_pred = 0.0_f64;
                        for &si in &zone.surface_indices {
                            let s = &self.surfaces[si];
                            let ha = s.h_conv_inside * s.net_area;
                            sum_ha_pred += ha;
                            sum_hat_pred += ha * s.temp_inside;
                        }

                        let total_outdoor = match self.infiltration_interaction {
                            crate::zone_loads::InfiltrationInteraction::Basic => {
                                zone.infiltration_mass_flow
                                    + zone.ventilation_mass_flow
                                    + zone.outdoor_air_mass_flow
                                    + zone.nat_vent_mass_flow
                            }
                            crate::zone_loads::InfiltrationInteraction::AshraeCombined => {
                                let unbal =
                                    (zone.exhaust_mass_flow - zone.outdoor_air_mass_flow).max(0.0);
                                (zone.infiltration_mass_flow.powi(2) + unbal.powi(2)).sqrt()
                                    + zone.ventilation_mass_flow
                                    + zone.outdoor_air_mass_flow
                                    + zone.nat_vent_mass_flow
                            }
                        };
                        let mcpi_pred = total_outdoor * cp_air;

                        let q_solar_trans: f64 = zone
                            .surface_indices
                            .iter()
                            .filter(|&&si| self.surfaces[si].is_window)
                            .map(|&si| self.surfaces[si].transmitted_solar)
                            .sum();
                        let q_solar_to_air = if zi < has_geometric_distribution.len()
                            && has_geometric_distribution[zi]
                        {
                            0.0
                        } else if let Some(ref dist) = zone.input.solar_distribution {
                            let to_surf =
                                dist.floor_fraction + dist.wall_fraction + dist.ceiling_fraction;
                            q_solar_trans * (1.0 - to_surf).max(0.0)
                        } else {
                            q_solar_trans
                        };
                        let q_window_cond: f64 = zone
                            .surface_indices
                            .iter()
                            .filter(|&&si| self.surfaces[si].is_window)
                            .map(|&si| self.surfaces[si].q_conv_inside * self.surfaces[si].net_area)
                            .sum();
                        let q_window_absorbed: f64 = zone
                            .surface_indices
                            .iter()
                            .filter(|&&si| self.surfaces[si].is_window)
                            .map(|&si| self.surfaces[si].absorbed_solar_inside_window)
                            .sum();
                        let q_conv_pred = zone.q_internal_conv
                            + zone.exhaust_fan_heat_to_zone
                            + q_solar_to_air
                            + q_window_cond
                            + q_window_absorbed;

                        // Use the same BDF order as the corrector so predictor
                        // and corrector agree on mode. E+ uses BDF3 in both
                        // PredictSystemLoads and CalcZoneAirTempSetPoints.
                        // A BDF1 predictor with BDF3 corrector causes mode
                        // disagreements that amplify oscillations → NaN.
                        let (dt_eff_pred, t_prev_eff_pred) = crate::zone::backward_diff_effective(
                            zone.temp_order,
                            dt,
                            zone.temp_prev,
                            zone.temp_prev2,
                            zone.temp_prev3,
                        );
                        let cap_pred = rho_air * zone.input.volume * cp_air / dt_eff_pred;
                        let denom_pred = sum_ha_pred + mcpi_pred + cap_pred;
                        let t_free_pred = if denom_pred > 1e-10 {
                            (sum_hat_pred
                                + mcpi_pred * t_outdoor
                                + q_conv_pred
                                + cap_pred * t_prev_eff_pred)
                                / denom_pred
                        } else {
                            zone.temp_prev
                        };

                        let (heat_sp, cool_sp) = zone.input.active_setpoints(hour);
                        let il = zone.input.ideal_loads.as_ref().unwrap();
                        let mut pred_mode = if t_free_pred < heat_sp {
                            1_i8
                        } else if t_free_pred > cool_sp {
                            -1_i8
                        } else {
                            0_i8
                        };

                        // If the predicted mode has zero capacity, the system
                        // can't actually heat/cool → zone will free-float.
                        // Use deadband so surfaces iterate with the correct
                        // (free-float) zone temp, not the unreachable setpoint.
                        // Critical for night ventilation cases (e.g. ASHRAE 140
                        // Case 650: heating_capacity=0 at night, zone must
                        // free-float below the heating setpoint).
                        if pred_mode == 1 && il.heating_capacity <= 0.0 {
                            pred_mode = 0;
                        } else if pred_mode == -1 && il.cooling_capacity <= 0.0 {
                            pred_mode = 0;
                        }

                        let t_zone = match pred_mode {
                            1 => heat_sp,
                            -1 => cool_sp,
                            _ => t_free_pred,
                        };
                        Some((pred_mode, t_zone))
                    }
                }; // end immutable borrow of self.zones[zi]

                if let Some((pred_mode, _t_zone)) = pred_result {
                    // Lock the predicted mode for the corrector
                    if !self.zones[zi].ideal_pred_mode_locked {
                        self.zones[zi].ideal_pred_mode = pred_mode;
                        self.zones[zi].ideal_pred_mode_locked = true;
                    }
                    // t_zone_vec[zi] intentionally NOT overwritten.
                    // Surfaces iterate with zone.temp (previous timestep),
                    // matching E+'s call order where surface HB precedes
                    // the zone predictor.
                }
            }

            // --- Inside surface iteration loop ---
            for _iter in 0..MAX_INSIDE_SURF_ITER {
                // Save old temps for convergence check and damping
                let t_inside_old: Vec<f64> = self.surfaces.iter().map(|s| s.temp_inside).collect();

                // Area-weighted totals (recomputed each iteration for self-exclusion)
                let mut zone_sum_ea: Vec<f64> = vec![0.0; self.zones.len()];
                let mut zone_sum_eat: Vec<f64> = vec![0.0; self.zones.len()];
                for (zi, zone) in self.zones.iter().enumerate() {
                    for &si in &zone.surface_indices {
                        let s = &self.surfaces[si];
                        let eps = if s.is_window {
                            s.glass_emissivity
                        } else {
                            s.thermal_absorptance_inside
                        };
                        let a = s.net_area;
                        zone_sum_ea[zi] += eps * a;
                        zone_sum_eat[zi] += eps * a * s.temp_inside;
                    }
                }

                // Solve each surface
                for i in 0..self.surfaces.len() {
                    let pc = match &precomp[i] {
                        Some(p) => p,
                        None => continue,
                    };

                    let t_zone = t_zone_vec.get(pc.zi).copied().unwrap_or(21.0);

                    // Per-surface MRT: view-factor weighted or area-weighted fallback
                    let t_mrt =
                        if let (Some(face_i), Some(ref zvf), Some(ref fea), Some(ref feat)) = (
                            self.surfaces[i].box_face,
                            self.zone_view_factors.get(pc.zi).and_then(|v| v.as_ref()),
                            opaque_face_ea.get(pc.zi).and_then(|v| v.as_ref()),
                            opaque_face_eat.get(pc.zi).and_then(|v| v.as_ref()),
                        ) {
                            // VF-weighted MRT: only faces ≠ face_i contribute
                            // (surfaces on the same face have F=0, so self is excluded)
                            let mut num = 0.0_f64;
                            let mut den = 0.0_f64;
                            for k in 0..6 {
                                if k == face_i {
                                    continue;
                                }
                                let f = zvf.face_vf[face_i][k];
                                num += f * feat[k];
                                den += f * fea[k];
                            }
                            if den > 1.0e-10 {
                                num / den
                            } else {
                                t_zone
                            }
                        } else {
                            // Area-weighted MRT excluding self
                            let eps_i = self.surfaces[i].thermal_absorptance_inside;
                            let a_i = self.surfaces[i].net_area;
                            let ea_i = eps_i * a_i;
                            let sum_ea_excl = zone_sum_ea[pc.zi] - ea_i;
                            let sum_eat_excl =
                                zone_sum_eat[pc.zi] - ea_i * self.surfaces[i].temp_inside;
                            if sum_ea_excl > 1.0e-10 {
                                sum_eat_excl / sum_ea_excl
                            } else {
                                t_zone
                            }
                        };
                    let t_old = t_inside_old[i];

                    // Inside convection coefficient (updated each iteration)
                    let h_conv = convection::interior_convection(
                        self.surfaces[i].temp_inside,
                        t_zone,
                        self.surfaces[i].input.tilt,
                    )
                    .max(0.1);
                    self.surfaces[i].h_conv_inside = h_conv;

                    // Linearized interior LW radiation coefficient
                    let eps = self.surfaces[i].thermal_absorptance_inside;
                    let t_mean_k =
                        ((self.surfaces[i].temp_inside + 273.15) + (t_mrt + 273.15)) / 2.0;
                    let h_rad = 4.0 * eps * SIGMA * t_mean_k.powi(3);
                    self.surfaces[i].h_rad_inside = h_rad;
                    self.surfaces[i].q_rad_inside = h_rad * (t_mrt - self.surfaces[i].temp_inside);

                    // Surface temperature equation with linearized radiation
                    // and IterDampConst for convergence.
                    //
                    // Standard:
                    //   T_i = (Y₀·T_out + CTFConst + h_conv·T_zone + h_rad·T_mrt
                    //         + q_rad + IterDampConst·T_old)
                    //       / (Z₀ + h_conv + h_rad + IterDampConst)
                    //
                    // Adiabatic with CTF (no mass, or multi-layer):
                    //   T_i = (CTFConst + h_conv·T_zone + h_rad·T_mrt
                    //         + q_rad + IterDampConst·T_old)
                    //       / (Z₀ - Y₀ + h_conv + h_rad + IterDampConst)
                    //
                    // Adiabatic surface: T_outside = T_inside (no heat flow through).
                    //
                    // With multi-term state-space CTF, Z[0] ≠ Y[0] even for
                    // single-layer constructions, so (Z[0] - Y[0]) is non-zero
                    // and properly captures distributed thermal mass. The standard
                    // CTF adiabatic equation works correctly:
                    //
                    //   T_i = (CTFConst + h_conv·T_zone + h_rad·T_mrt
                    //         + q_rad + IterDampConst·T_old)
                    //       / (Z₀ - Y₀ + h_conv + h_rad + IterDampConst)
                    let t_new = if pc.is_adiabatic {
                        let denom = (pc.ctf_z0 - pc.ctf_y0) + h_conv + h_rad + ITER_DAMP_CONST;
                        (pc.ctf_const_in
                            + h_conv * t_zone
                            + h_rad * t_mrt
                            + pc.q_rad_flux
                            + ITER_DAMP_CONST * t_old)
                            / denom.max(0.1)
                    } else {
                        let denom = pc.ctf_z0 + h_conv + h_rad + ITER_DAMP_CONST;
                        (pc.ctf_y0 * self.surfaces[i].temp_outside
                            + pc.ctf_const_in
                            + h_conv * t_zone
                            + h_rad * t_mrt
                            + pc.q_rad_flux
                            + ITER_DAMP_CONST * t_old)
                            / denom.max(0.1)
                    };
                    // Guard against NaN or extreme values from unstable CTF history.
                    // If the surface temp diverges, hold at the previous iteration
                    // value so the iteration can recover. Physical surface temps
                    // should never exceed ±200°C; anything beyond that is numerical.
                    self.surfaces[i].temp_inside = if t_new.is_nan() || t_new.abs() > 200.0 {
                        t_old
                    } else {
                        t_new
                    };
                    if pc.is_adiabatic {
                        self.surfaces[i].temp_outside = self.surfaces[i].temp_inside;
                    }

                    // Interzone coupling: immediately propagate this surface's
                    // new inside temperature to the paired surface's outside
                    // temperature.  This tightens the within-iteration coupling
                    // so both sides converge together instead of lagging.
                    if let Some(paired) = self.interzone_pairs[i] {
                        self.surfaces[paired].temp_outside = self.surfaces[i].temp_inside;
                    }
                }

                // Convergence check
                let max_del_temp = self
                    .surfaces
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| precomp[*i].is_some())
                    .map(|(i, s)| (s.temp_inside - t_inside_old[i]).abs())
                    .fold(0.0_f64, f64::max);

                if max_del_temp <= CONVERGENCE_TOLERANCE {
                    break;
                }
            }

            // Post-iteration: update adiabatic mass node temps, CTF fluxes, convective flux
            for i in 0..self.surfaces.len() {
                if precomp[i].is_none() {
                    continue;
                }
                let pc = precomp[i].as_ref().unwrap();

                if let (Some(ctf), Some(history)) = (
                    self.ctf_coefficients[i].as_ref(),
                    self.ctf_histories[i].as_ref(),
                ) {
                    let (q_in, q_out) = apply_ctf(
                        ctf,
                        history,
                        self.surfaces[i].temp_outside,
                        self.surfaces[i].temp_inside,
                    );
                    ctf_q_inside[i] = q_in;
                    ctf_q_outside[i] = q_out;
                    self.surfaces[i].q_cond_inside = q_in;
                    self.surfaces[i].q_cond_outside = q_out;
                }

                let t_zone = t_zone_vec.get(pc.zi).copied().unwrap_or(21.0);
                self.surfaces[i].q_conv_inside =
                    self.surfaces[i].h_conv_inside * (self.surfaces[i].temp_inside - t_zone);
            }

            // 6c. Zone air heat balance
            for (zone_idx, zone) in self.zones.iter_mut().enumerate() {
                let cp_air = psych::cp_air_fn_w(zone.humidity_ratio);
                let rho_air = psych::rho_air_fn_pb_tdb_w(p_b, zone.temp, zone.humidity_ratio);

                let mut sum_ha: f64 = 0.0;
                let mut sum_hat: f64 = 0.0;

                for &si in &zone.surface_indices {
                    let surf = &self.surfaces[si];
                    let h = surf.h_conv_inside;
                    let a = surf.net_area;
                    let t_in = surf.temp_inside;
                    // Guard against NaN surface temps (e.g. from CTF instability
                    // during warmup). Use the zone's previous temperature as a
                    // neutral fallback so the zone balance remains finite.
                    if t_in.is_nan() {
                        sum_ha += h * a;
                        sum_hat += h * a * zone.temp;
                    } else {
                        sum_ha += h * a;
                        sum_hat += h * a * t_in;
                    }
                }

                // Transmitted solar through windows
                let q_solar_transmitted: f64 = zone
                    .surface_indices
                    .iter()
                    .filter(|&&si| self.surfaces[si].is_window)
                    .map(|&si| self.surfaces[si].transmitted_solar)
                    .sum();

                // If geometric distribution is active, all solar goes to surfaces
                // (beam + diffuse fully accounted for via VMULT). Otherwise use
                // fixed-fraction remainder or all-to-air.
                let q_solar_to_air = if zone_idx < has_geometric_distribution.len()
                    && has_geometric_distribution[zone_idx]
                {
                    0.0 // All solar distributed to surfaces geometrically
                } else if zone.input.solar_distribution.is_some() {
                    // Fixed-fraction fallback: remaining fraction goes to air
                    let dist = zone.input.solar_distribution.as_ref().unwrap();
                    let to_surfaces =
                        dist.floor_fraction + dist.wall_fraction + dist.ceiling_fraction;
                    q_solar_transmitted * (1.0 - to_surfaces).max(0.0)
                } else {
                    // No solar distribution: all transmitted solar goes to zone air
                    q_solar_transmitted
                };

                // Window conduction also contributes
                let q_window_cond: f64 = zone
                    .surface_indices
                    .iter()
                    .filter(|&&si| self.surfaces[si].is_window)
                    .map(|&si| self.surfaces[si].q_conv_inside * self.surfaces[si].net_area)
                    .sum();

                // Solar absorbed by window glazing that enters the zone (inward fraction)
                let q_window_absorbed: f64 = zone
                    .surface_indices
                    .iter()
                    .filter(|&&si| self.surfaces[si].is_window)
                    .map(|&si| self.surfaces[si].absorbed_solar_inside_window)
                    .sum();

                // Total outdoor air mass flow entering zone at outdoor temperature.
                //
                // For zones with external HVAC (air loops) during normal operation,
                // the ASHRAE 62.1 outdoor air is already mixed into the supply air
                // stream by the air handler — it enters the zone at supply_air_temp,
                // NOT at outdoor temp. So we exclude outdoor_air_mass_flow from mcpi
                // to avoid double-counting the OA ventilation load.
                //
                // Outdoor air handling depends on whether the zone is connected
                // to an air loop and whether we're sizing or running:
                //
                // During SIZING: Exclude outdoor_air from zone load. This matches
                // E+ behavior where dedicated ventilation systems (ERVs) are OFF
                // during design days.  The PTAC/PSZ-AC is sized for envelope +
                // internal gains + infiltration only, without the ventilation load.
                //
                // During RUNTIME with air loop: OA is suppressed because the air
                // loop handles ventilation mixing in its supply stream.
                //
                // During RUNTIME without air loop (ideal loads, free-float): OA
                // enters the zone directly at outdoor temperature.
                // Determine whether zone outdoor air should enter the zone directly.
                // If HVAC handles OA mixing (VAV with economizer, PSZ-AC), suppress
                // zone OA to avoid double-counting. If HVAC is recirculation-only
                // (PTAC/FCU with separate ERV), allow zone OA through.
                let hvac_handles_oa = hvac
                    .oa_handled_by_hvac
                    .get(&zone.input.name)
                    .copied()
                    .unwrap_or(true); // default: suppress OA when HVAC is running

                let oa_to_zone = if ctx.is_sizing && hvac_handles_oa {
                    // Sizing: exclude zone OA when HVAC handles it (VAV/PSZ-AC
                    // mix OA into the supply stream, sized via coil ΔT)
                    0.0
                } else if ctx.is_sizing && !hvac_handles_oa {
                    // Sizing with separate ventilation (ERV/DOAS handles OA
                    // independently): include zone OA so equipment is sized
                    // for the full ventilation load
                    zone.outdoor_air_mass_flow
                } else if zone.supply_air_mass_flow > 0.0 && hvac_handles_oa {
                    // External HVAC loop handles OA: suppress zone OA
                    0.0
                } else {
                    // No HVAC running, or HVAC doesn't handle OA (ERV/DOAS
                    // provides it separately): OA enters zone at outdoor temp.
                    //
                    // For VAV/PSZ zones when HVAC is off, this approximates
                    // the E+ plenum effect: some outdoor air leaks through
                    // the return air plenum, providing natural cooling/heating
                    // that prevents extreme free-float temperatures.
                    zone.outdoor_air_mass_flow
                };
                // Infiltration interaction with exhaust fans.
                //
                // Basic (default): infiltration is independent of exhaust.
                //   Matches E+ ZoneInfiltration:DesignFlowRate (no AirflowNetwork).
                //   Exhaust removes zone air at T_zone — no additional outdoor air
                //   enters to replace it (the mass imbalance is not resolved).
                //   Only infiltration brings in outdoor air at T_outdoor.
                //
                // AshraeCombined: Q_combined = sqrt(Q_infil² + Q_unbalanced²)
                //   ASHRAE Handbook of Fundamentals, "Residential Ventilation".
                //   Q_unbalanced = max(0, exhaust − balanced_supply).
                //   More physically correct but differs from E+ without AFN.
                let total_outdoor_mass_flow = match self.infiltration_interaction {
                    crate::zone_loads::InfiltrationInteraction::Basic => {
                        // Exhaust does NOT add outdoor air to the energy balance.
                        // It only removes zone air (at T_zone, so net energy = 0
                        // relative to zone). Infiltration is the only source of
                        // outdoor air at T_outdoor.
                        zone.infiltration_mass_flow
                            + zone.ventilation_mass_flow
                            + oa_to_zone
                            + zone.nat_vent_mass_flow
                    }
                    crate::zone_loads::InfiltrationInteraction::AshraeCombined => {
                        let unbalanced_exhaust = (zone.exhaust_mass_flow - oa_to_zone).max(0.0);
                        let combined_infil_exhaust = (zone.infiltration_mass_flow.powi(2)
                            + unbalanced_exhaust.powi(2))
                        .sqrt();
                        combined_infil_exhaust
                            + zone.ventilation_mass_flow
                            + oa_to_zone
                            + zone.nat_vent_mass_flow
                    }
                };
                let mcpi = total_outdoor_mass_flow * cp_air;

                let q_conv_total = zone.q_internal_conv
                    + zone.exhaust_fan_heat_to_zone  // motor waste heat staying in zone
                    + q_solar_to_air + q_window_cond + q_window_absorbed;

                // ─── E+-style higher-order backward difference ─────────
                //
                // EnergyPlus ZoneTempPredictorCorrector uses a 3rd-order
                // backward difference for dT/dt, ramping from 1st to 3rd
                // order as temperature history accumulates.  This provides
                // better numerical stability and smoother transient response
                // than simple backward Euler, reducing spurious heating/
                // cooling mode switches.
                //
                // Rather than changing every solve function, we fold the
                // higher-order coefficients into an effective timestep
                // (dt_eff) and effective previous temperature (t_prev_eff),
                // preserving the 1st-order formula structure.
                let (dt_eff, t_prev_eff) = crate::zone::backward_diff_effective(
                    zone.temp_order,
                    dt,
                    zone.temp_prev,
                    zone.temp_prev2,
                    zone.temp_prev3,
                );

                // ─── Predictor: free-floating zone temp without HVAC ─────
                //
                // E+-style predictor: what temperature would the zone reach
                // at the end of this timestep if HVAC were turned off?
                // Uses CURRENT surface temps, solar gains, and outdoor
                // conditions — not stale values from the previous timestep.
                //
                // This is used by the HVAC control layer to determine
                // whether the zone actually needs heating/cooling or can
                // coast in the deadband on its thermal mass.
                {
                    let cap_term = rho_air * zone.input.volume * cp_air / dt_eff;
                    let denom = sum_ha + mcpi + cap_term;
                    zone.temp_no_hvac = if denom > 1e-10 {
                        (sum_hat + mcpi * t_outdoor + q_conv_total + cap_term * t_prev_eff) / denom
                    } else {
                        zone.temp_prev
                    };
                }

                // ─── Ideal Loads Air System ───────────────────────────────────
                if let Some(ref ideal_loads) = zone.input.ideal_loads.clone() {
                    // Step 1: Solve zone temp without HVAC (free-float)
                    // Uses full BDF (dt_eff, t_prev_eff) for accuracy.
                    // Mode was already determined by the predictor above,
                    // so the corrector here just refines Q and zone temp.
                    let t_free = crate::zone::solve_zone_air_temp_with_q(
                        sum_ha,
                        sum_hat,
                        mcpi,
                        t_outdoor,
                        q_conv_total,
                        0.0, // no HVAC
                        rho_air,
                        zone.input.volume,
                        cp_air,
                        dt_eff,
                        t_prev_eff,
                    );

                    // Step 2: Get active setpoints (may vary by schedule)
                    let (mut heat_sp, mut cool_sp) = zone.input.active_setpoints(hour);

                    // Step 2b: Natural ventilation setpoint reset.
                    //
                    // When natural ventilation is active, widen the thermostat
                    // deadband so HVAC does not fight the outdoor air coming
                    // through open windows. When nat vent stops, linearly ramp
                    // back to normal setpoints over N timesteps.
                    if let Some(ref nv_input) = zone.input.natural_ventilation {
                        if let Some(ref reset) = nv_input.setpoint_reset {
                            if zone.nat_vent_active {
                                // Fully overridden setpoints
                                heat_sp = reset.heating_setpoint;
                                cool_sp = reset.cooling_setpoint;
                            } else if reset.ramp_timesteps > 0
                                && zone.nat_vent_off_timesteps <= reset.ramp_timesteps as u32
                            {
                                // Ramp back: linearly blend from override to normal
                                let frac = zone.nat_vent_off_timesteps as f64
                                    / reset.ramp_timesteps as f64;
                                let (normal_heat, normal_cool) = (heat_sp, cool_sp);
                                heat_sp = reset.heating_setpoint
                                    + frac * (normal_heat - reset.heating_setpoint);
                                cool_sp = reset.cooling_setpoint
                                    + frac * (normal_cool - reset.cooling_setpoint);
                            }
                        }
                    }

                    // Step 3: Compute ideal Q using the PREDICTOR's mode.
                    //
                    // The predictor determined the HVAC mode using the previous
                    // timestep's surface state.  Surfaces then iterated with
                    // the previous timestep zone temp (matching E+ call order).
                    // The mode is LOCKED to prevent oscillation / NaN.
                    //
                    // We validate the mode: if the predictor said heating but
                    // Q turns out negative, fall back to deadband.  Same for
                    // cooling.
                    let pred_mode = zone.ideal_pred_mode;
                    let (q_hvac, hvac_mode) = if pred_mode > 0 {
                        // Predictor: HEATING
                        let q_needed = crate::zone::compute_ideal_q_hvac(
                            sum_ha,
                            sum_hat,
                            mcpi,
                            t_outdoor,
                            q_conv_total,
                            rho_air,
                            zone.input.volume,
                            cp_air,
                            dt_eff,
                            t_prev_eff,
                            heat_sp,
                        );
                        if q_needed > 0.0 {
                            let q_clamped = q_needed.min(ideal_loads.heating_capacity);
                            (q_clamped, 1)
                        } else {
                            // Predictor overestimated heating need → deadband
                            (0.0, 0)
                        }
                    } else if pred_mode < 0 {
                        // Predictor: COOLING
                        let q_needed = crate::zone::compute_ideal_q_hvac(
                            sum_ha,
                            sum_hat,
                            mcpi,
                            t_outdoor,
                            q_conv_total,
                            rho_air,
                            zone.input.volume,
                            cp_air,
                            dt_eff,
                            t_prev_eff,
                            cool_sp,
                        );
                        if q_needed < 0.0 {
                            let q_clamped = q_needed.max(-ideal_loads.cooling_capacity);
                            (q_clamped, -1)
                        } else {
                            // Predictor overestimated cooling need → deadband
                            (0.0, 0)
                        }
                    } else {
                        // Predictor: DEADBAND — but corrector validates with BDF3
                        // free-float temp.  If the BDF3 corrector shows the zone
                        // has drifted outside the setpoint band (e.g. solar ramp-up
                        // that BDF1 predictor underestimated), override to
                        // heating or cooling to prevent unbounded overshoot.
                        if t_free < heat_sp {
                            let q_needed = crate::zone::compute_ideal_q_hvac(
                                sum_ha,
                                sum_hat,
                                mcpi,
                                t_outdoor,
                                q_conv_total,
                                rho_air,
                                zone.input.volume,
                                cp_air,
                                dt_eff,
                                t_prev_eff,
                                heat_sp,
                            );
                            if q_needed > 0.0 {
                                (q_needed.min(ideal_loads.heating_capacity), 1)
                            } else {
                                (0.0, 0)
                            }
                        } else if t_free > cool_sp {
                            let q_needed = crate::zone::compute_ideal_q_hvac(
                                sum_ha,
                                sum_hat,
                                mcpi,
                                t_outdoor,
                                q_conv_total,
                                rho_air,
                                zone.input.volume,
                                cp_air,
                                dt_eff,
                                t_prev_eff,
                                cool_sp,
                            );
                            if q_needed < 0.0 {
                                (q_needed.max(-ideal_loads.cooling_capacity), -1)
                            } else {
                                (0.0, 0)
                            }
                        } else {
                            (0.0, 0)
                        }
                    };

                    // Step 4: Solve zone temp with HVAC Q
                    zone.temp = crate::zone::solve_zone_air_temp_with_q(
                        sum_ha,
                        sum_hat,
                        mcpi,
                        t_outdoor,
                        q_conv_total,
                        q_hvac,
                        rho_air,
                        zone.input.volume,
                        cp_air,
                        dt_eff,
                        t_prev_eff,
                    );
                    // Step 5: Record loads and rates
                    if hvac_mode > 0 {
                        zone.heating_load = q_hvac;
                        zone.cooling_load = 0.0;
                        zone.hvac_heating_rate = q_hvac;
                        zone.hvac_cooling_rate = 0.0;
                    } else if hvac_mode < 0 {
                        zone.heating_load = 0.0;
                        zone.cooling_load = -q_hvac; // positive for cooling load
                        zone.hvac_heating_rate = 0.0;
                        zone.hvac_cooling_rate = -q_hvac;
                    } else {
                        zone.heating_load = 0.0;
                        zone.cooling_load = 0.0;
                        zone.hvac_heating_rate = 0.0;
                        zone.hvac_cooling_rate = 0.0;
                    }
                } else if zone.supply_air_mass_flow > 0.0 {
                    // ─── External HVAC (air loop controls) ────────────────────
                    let mcpsys = zone.supply_air_mass_flow * cp_air;
                    let sat = zone.supply_air_temp;

                    zone.temp = crate::zone::solve_zone_air_temp(
                        sum_ha,
                        sum_hat,
                        mcpi,
                        t_outdoor,
                        mcpsys,
                        sat,
                        q_conv_total,
                        rho_air,
                        zone.input.volume,
                        cp_air,
                        dt_eff,
                        t_prev_eff,
                    );

                    let (hl, cl) = crate::zone::calc_zone_loads(
                        zone.temp,
                        sum_ha,
                        sum_hat,
                        mcpi,
                        t_outdoor,
                        q_conv_total,
                        rho_air,
                        zone.input.volume,
                        cp_air,
                        dt_eff,
                        t_prev_eff,
                    );
                    zone.heating_load = hl;
                    zone.cooling_load = cl;

                    zone.hvac_heating_rate = 0.0;
                    zone.hvac_cooling_rate = 0.0;
                } else {
                    // ─── Free-Float (no HVAC at all) ─────────────────────────
                    // Zone temperature drifts freely. Still compute loads so that
                    // load-based PLR controllers know what the zone needs.
                    zone.temp = crate::zone::solve_zone_air_temp_with_q(
                        sum_ha,
                        sum_hat,
                        mcpi,
                        t_outdoor,
                        q_conv_total,
                        0.0, // no HVAC
                        rho_air,
                        zone.input.volume,
                        cp_air,
                        dt_eff,
                        t_prev_eff,
                    );
                    // (debug trace removed)
                    let (hl, cl) = crate::zone::calc_zone_loads(
                        zone.temp,
                        sum_ha,
                        sum_hat,
                        mcpi,
                        t_outdoor,
                        q_conv_total,
                        rho_air,
                        zone.input.volume,
                        cp_air,
                        dt_eff,
                        t_prev_eff,
                    );
                    zone.heating_load = hl;
                    zone.cooling_load = cl;
                    zone.hvac_heating_rate = 0.0;
                    zone.hvac_cooling_rate = 0.0;
                }

                // ─── Zone Air Balance Component Diagnostics ──────────────
                //
                // Compute each term of the zone air energy balance using
                // the FINAL zone temperature, for diagnostic comparison
                // with EnergyPlus zone-level output variables.
                {
                    let t_z = zone.temp;
                    let mut conv_total = 0.0_f64;
                    let mut conv_walls = 0.0_f64;
                    let mut conv_floors = 0.0_f64;
                    let mut conv_roofs = 0.0_f64;
                    let mut conv_windows = 0.0_f64;

                    for &si in &zone.surface_indices {
                        let s = &self.surfaces[si];
                        let q = s.h_conv_inside * s.net_area * (s.temp_inside - t_z);
                        conv_total += q;
                        match s.input.surface_type {
                            crate::surface::SurfaceType::Wall => conv_walls += q,
                            crate::surface::SurfaceType::Floor => conv_floors += q,
                            crate::surface::SurfaceType::Roof
                            | crate::surface::SurfaceType::Ceiling => conv_roofs += q,
                            crate::surface::SurfaceType::Window => conv_windows += q,
                        }
                    }
                    // Window convection also includes absorbed solar inward
                    // and the window conduction term (q_conv_inside × A) which
                    // was added to q_conv_total above. Add absorbed solar here.
                    let q_win_absorbed: f64 = zone
                        .surface_indices
                        .iter()
                        .filter(|&&si| self.surfaces[si].is_window)
                        .map(|&si| self.surfaces[si].absorbed_solar_inside_window)
                        .sum();
                    conv_windows += q_win_absorbed;

                    zone.q_surf_conv_total = conv_total + q_win_absorbed;
                    zone.q_surf_conv_walls = conv_walls;
                    zone.q_surf_conv_floors = conv_floors;
                    zone.q_surf_conv_roofs = conv_roofs;
                    zone.q_surf_conv_windows = conv_windows;

                    // Infiltration sensible: mcpi × (T_outdoor − T_zone)
                    zone.q_infiltration_sensible = mcpi * (t_outdoor - t_z);

                    // Thermal mass: ρVcp/dt_eff × (T_prev_eff − T_zone)
                    let cap_term = rho_air * zone.input.volume * cp_air / dt_eff;
                    zone.q_thermal_mass = cap_term * (t_prev_eff - t_z);
                }

                // ─── Zone Moisture Balance ────────────────────────────────
                //
                // Corrector-phase humidity solve, structurally identical to
                // the temperature balance. Uses 3rd-order BDF with the same
                // ramp-up strategy (1st → 2nd → 3rd order).
                //
                // Source/sink terms:
                //   - Infiltration + ventilation + nat vent: outdoor air moisture
                //   - HVAC supply air: coil outlet humidity
                //   - People latent: metabolic moisture generation
                //   - Equipment latent: cooking, laundry, etc.
                //
                // Reference: E+ ZoneAirMoisturePredictorCorrector
                {
                    let (dt_w_eff, w_prev_eff) = crate::zone::backward_diff_effective(
                        zone.w_order,
                        dt,
                        zone.w_prev,
                        zone.w_prev2,
                        zone.w_prev3,
                    );

                    // Moisture mass flows use the same outdoor air flows as the
                    // thermal balance (total_outdoor_mass_flow already computed).
                    let m_infil = total_outdoor_mass_flow;
                    let w_outdoor = ctx.outdoor_air.w;

                    // HVAC supply humidity from the air loop
                    let m_supply = zone.supply_air_mass_flow;
                    let w_supply = hvac
                        .supply_humidity_ratios
                        .get(&zone.input.name)
                        .copied()
                        .unwrap_or(zone.supply_air_humidity_ratio);
                    // Store for use in main.rs return air calculation
                    zone.supply_air_humidity_ratio = w_supply;

                    // Total latent gains [W] — people + equipment
                    let q_latent = zone.people_latent + zone.equipment_latent;

                    zone.humidity_ratio = crate::zone::solve_zone_humidity(
                        rho_air,
                        zone.input.volume,
                        dt_w_eff,
                        w_prev_eff,
                        m_infil,
                        w_outdoor,
                        m_supply,
                        w_supply,
                        q_latent,
                    );
                }

                // ─── Diagnostic accumulators ─────────────────────────────
                // Accumulate annual heat balance components for comparison
                // with E+ zone-level energy balance.
                //
                // Strategy: every HVAC iteration overwrites "pending" values
                // so we always capture the LAST (converged) iteration.  When
                // the physical timestep advances (sim_time_s changes) we
                // commit the pending values from the previous timestep into
                // the running accumulators.
                if !ctx.is_sizing {
                    // Commit previous timestep's pending values when time advances
                    if ctx.timestep.sim_time_s != zone.diag_last_sim_time {
                        if zone.diag_last_sim_time >= 0.0 {
                            // Previous timestep had valid pending values — commit
                            zone.diag_surface_loss_kwh += zone.diag_pending_surface;
                            zone.diag_infil_loss_kwh += zone.diag_pending_infil;
                            zone.diag_q_conv_kwh += zone.diag_pending_q_conv;
                            zone.diag_solar_trans_kwh += zone.diag_pending_solar;
                            zone.diag_internal_conv_kwh += zone.diag_pending_internal;
                            zone.diag_window_cond_kwh += zone.diag_pending_wincond;
                            zone.diag_window_conv_kwh += zone.diag_pending_wincond_conv;
                            zone.diag_hvac_net_kwh += zone.diag_pending_hvac;
                        }
                        zone.diag_last_sim_time = ctx.timestep.sim_time_s;
                    }

                    // Overwrite pending with THIS iteration's values (last wins)
                    let dt_kwh = dt / 3_600_000.0; // seconds → kWh factor

                    // Compute window-only convective exchange for diagnostics
                    let mut win_ha = 0.0_f64;
                    let mut win_hat = 0.0_f64;
                    for &si in &zone.surface_indices {
                        if self.surfaces[si].is_window {
                            let ha = self.surfaces[si].h_conv_inside * self.surfaces[si].net_area;
                            win_ha += ha;
                            win_hat += ha * self.surfaces[si].temp_inside;
                            // Post-convergence window reporting [W/m²]:
                            // Convective only (for diagnostics)
                            self.surfaces[si].q_conv_inside = self.surfaces[si].h_conv_inside
                                * (self.surfaces[si].temp_inside - zone.temp);
                            // Window conduction: heat flux through the glass [W/m²].
                            // q = U_glass × (T_outside_surface − T_inside_surface)
                            // This is pure conduction through the glazing assembly,
                            // matching E+'s "Surface Inside Face Conduction".
                            self.surfaces[si].q_cond_inside = self.surfaces[si].u_glass
                                * (self.surfaces[si].temp_outside - self.surfaces[si].temp_inside);
                        }
                    }
                    zone.diag_pending_wincond_conv = (win_ha * zone.temp - win_hat) * dt_kwh;

                    zone.diag_pending_surface = (sum_ha * zone.temp - sum_hat) * dt_kwh;
                    zone.diag_pending_infil = mcpi * (zone.temp - t_outdoor) * dt_kwh;
                    zone.diag_pending_q_conv = q_conv_total * dt_kwh;
                    zone.diag_pending_solar = q_solar_transmitted * dt_kwh;
                    zone.diag_pending_internal = zone.q_internal_conv * dt_kwh;
                    zone.diag_pending_wincond = q_window_cond * dt_kwh;
                    let cap = rho_air * zone.input.volume * cp_air / dt_eff;
                    let q_hvac_delivered = cap * (zone.temp - t_prev_eff)
                        + (sum_ha * zone.temp - sum_hat)
                        + mcpi * (zone.temp - t_outdoor)
                        - q_conv_total;
                    zone.diag_pending_hvac = q_hvac_delivered * dt_kwh;
                }

                // ─── Ideal Loads at Setpoint ──────────────────────────────
                //
                // Compute the HVAC energy needed to hold the zone exactly at
                // the cooling/heating setpoint. This is used for load-based
                // PLR calculations in the HVAC control layer.
                //
                // Uses ACTUAL surface temperatures (sum_hat from CTF solution)
                // so the ideal load correctly accounts for envelope conduction
                // losses through walls/windows. Setting sum_hat = sum_ha*sp
                // would zero out envelope conduction, drastically underestimating
                // heating loads in winter and cooling loads in summer.
                //
                // Uses t_prev = zone.temp (the actual zone temperature) so
                // the thermal mass term provides natural damping:
                //   - Zone warm (T > sp): Cap*(sp-T) < 0 → reduces heating load
                //   - Zone cold (T < sp): Cap*(sp-T) > 0 → increases heating load
                // This prevents the PLR from being the same regardless of
                // zone state, which caused undamped oscillation.
                let cool_sp = hvac.cooling_setpoints.get(&zone.input.name).copied();
                let heat_sp = hvac.heating_setpoints.get(&zone.input.name).copied();

                zone.ideal_cooling_load = if let Some(sp) = cool_sp {
                    let q = crate::zone::compute_ideal_q_hvac(
                        sum_ha,
                        sum_hat,
                        mcpi,
                        t_outdoor,
                        q_conv_total,
                        rho_air,
                        zone.input.volume,
                        cp_air,
                        dt_eff,
                        zone.temp,
                        sp,
                    );
                    // Negative Q = cooling needed; convert to positive cooling load
                    (-q).max(0.0)
                } else {
                    0.0
                };

                zone.ideal_heating_load = if let Some(sp) = heat_sp {
                    let q = crate::zone::compute_ideal_q_hvac(
                        sum_ha,
                        sum_hat,
                        mcpi,
                        t_outdoor,
                        q_conv_total,
                        rho_air,
                        zone.input.volume,
                        cp_air,
                        dt_eff,
                        zone.temp,
                        sp,
                    );
                    // Positive Q = heating needed
                    q.max(0.0)
                } else {
                    0.0
                };
            }
        }

        // 7. Save CTF q values for deferred history update.
        //
        // The actual CTF history shift is done in update_bdf_history(),
        // called once after HVAC convergence.  Shifting here would
        // corrupt the CTF history (same bug as the BDF history).
        for i in 0..self.surfaces.len() {
            self.ctf_q_last_inside[i] = ctf_q_inside[i];
            self.ctf_q_last_outside[i] = ctf_q_outside[i];
        }

        // 8. Update zone temperature history (3rd-order backward difference)
        //
        // IMPORTANT: When solve_timestep() is called in an HVAC iteration
        // loop, the BDF history must NOT be updated on every call — that
        // would make the BDF extrapolation see the HVAC convergence
        // trajectory instead of the physical timestep trajectory, causing
        // a positive-feedback runaway.  The caller must call
        // update_bdf_history() exactly ONCE after HVAC convergence.
        //
        // For callers that do NOT iterate (ideal loads, warmup), the
        // update is still needed — they should call update_bdf_history()
        // after solve_timestep().

        // 8b. Print diagnostic accumulators at end of year
        if ctx.timestep.month == 12
            && ctx.timestep.day == 31
            && ctx.timestep.hour == 24
            && !ctx.is_sizing
        {
            // Commit the final pending values (last timestep of the year)
            for zone in &mut self.zones {
                zone.diag_surface_loss_kwh += zone.diag_pending_surface;
                zone.diag_infil_loss_kwh += zone.diag_pending_infil;
                zone.diag_q_conv_kwh += zone.diag_pending_q_conv;
                zone.diag_solar_trans_kwh += zone.diag_pending_solar;
                zone.diag_internal_conv_kwh += zone.diag_pending_internal;
                zone.diag_window_cond_kwh += zone.diag_pending_wincond;
                zone.diag_window_conv_kwh += zone.diag_pending_wincond_conv;
                zone.diag_hvac_net_kwh += zone.diag_pending_hvac;
                // Zero out pending so they aren't double-committed
                zone.diag_pending_surface = 0.0;
                zone.diag_pending_infil = 0.0;
                zone.diag_pending_q_conv = 0.0;
                zone.diag_pending_solar = 0.0;
                zone.diag_pending_internal = 0.0;
                zone.diag_pending_wincond = 0.0;
                zone.diag_pending_wincond_conv = 0.0;
                zone.diag_pending_hvac = 0.0;
            }
            for zone in &self.zones {
                if zone.input.conditioned {
                    let opaque_conv = zone.diag_surface_loss_kwh - zone.diag_window_conv_kwh;
                    let total_window = zone.diag_solar_trans_kwh + zone.diag_window_cond_kwh
                        - zone.diag_window_conv_kwh;
                    let q_win_absorbed = zone.diag_q_conv_kwh
                        - zone.diag_internal_conv_kwh
                        - zone.diag_window_cond_kwh;
                    eprintln!("═══ ANNUAL HEAT BALANCE: {} ═══", zone.input.name);
                    eprintln!(
                        "  Surface conv (all):   {:>10.1} kWh (air→surf, neg=gain)",
                        zone.diag_surface_loss_kwh
                    );
                    eprintln!(
                        "    Opaque surf conv:   {:>10.1} kWh               E+: -7569",
                        opaque_conv
                    );
                    eprintln!(
                        "    Window convective:  {:>10.1} kWh",
                        zone.diag_window_conv_kwh
                    );
                    eprintln!(
                        "  Infiltration loss:    {:>10.1} kWh               E+: -4644",
                        zone.diag_infil_loss_kwh
                    );
                    eprintln!(
                        "  q_conv_total:         {:>10.1} kWh (internal+wincond+absorbed)",
                        zone.diag_q_conv_kwh
                    );
                    eprintln!(
                        "    Internal conv:      {:>10.1} kWh",
                        zone.diag_internal_conv_kwh
                    );
                    eprintln!(
                        "    Window rad(cond):   {:>10.1} kWh",
                        zone.diag_window_cond_kwh
                    );
                    eprintln!("    Window absorbed:    {:>10.1} kWh", q_win_absorbed);
                    eprintln!(
                        "  Transmitted solar:    {:>10.1} kWh               E+: +7850",
                        zone.diag_solar_trans_kwh
                    );
                    eprintln!("  ── Window total ──");
                    eprintln!(
                        "    Solar trans:        {:>10.1} kWh",
                        zone.diag_solar_trans_kwh
                    );
                    eprintln!(
                        "    + rad(cond):        {:>10.1} kWh",
                        zone.diag_window_cond_kwh
                    );
                    eprintln!(
                        "    − convective:       {:>10.1} kWh",
                        zone.diag_window_conv_kwh
                    );
                    eprintln!("    + absorbed:         {:>10.1} kWh", q_win_absorbed);
                    eprintln!(
                        "    = NET window:       {:>10.1} kWh   E+: +4221 (+7850-3629)",
                        total_window + q_win_absorbed
                    );
                    eprintln!(
                        "  HVAC net delivered:   {:>10.1} kWh   E+: -628 (+5639-6267)",
                        zone.diag_hvac_net_kwh
                    );
                    eprintln!("  ── E+ comparison (kWh) ──");
                    eprintln!("    Opaque cond: -7569  |  Infil: -4644  |  Internal: +8624");
                    eprintln!("    Window net: +4221   |  HVAC heat: +5639  |  HVAC cool: -6267");
                    eprintln!("═══════════════════════════════════");
                }
            }
        }

        // 9. Build results
        let mut results = EnvelopeResults::default();
        for zone in &self.zones {
            results
                .zone_temps
                .insert(zone.input.name.clone(), zone.temp);
            results
                .zone_humidity
                .insert(zone.input.name.clone(), zone.humidity_ratio);
            results
                .zone_heating_loads
                .insert(zone.input.name.clone(), zone.heating_load);
            results
                .zone_cooling_loads
                .insert(zone.input.name.clone(), zone.cooling_load);
            results
                .ideal_cooling_loads
                .insert(zone.input.name.clone(), zone.ideal_cooling_load);
            results
                .ideal_heating_loads
                .insert(zone.input.name.clone(), zone.ideal_heating_load);
            results
                .predictor_temps
                .insert(zone.input.name.clone(), zone.temp_no_hvac);

            let mut outputs = HashMap::new();
            outputs.insert("zone_temp".to_string(), zone.temp);
            outputs.insert("zone_humidity_ratio".to_string(), zone.humidity_ratio);
            outputs.insert("heating_load".to_string(), zone.heating_load);
            outputs.insert("cooling_load".to_string(), zone.cooling_load);
            outputs.insert("hvac_heating_rate".to_string(), zone.hvac_heating_rate);
            outputs.insert("hvac_cooling_rate".to_string(), zone.hvac_cooling_rate);
            outputs.insert(
                "infiltration_mass_flow".to_string(),
                zone.infiltration_mass_flow,
            );
            outputs.insert(
                "ventilation_mass_flow".to_string(),
                zone.ventilation_mass_flow,
            );
            outputs.insert("exhaust_mass_flow".to_string(), zone.exhaust_mass_flow);
            outputs.insert("exhaust_fan_power".to_string(), zone.exhaust_fan_power);
            outputs.insert(
                "exhaust_fan_heat_to_zone".to_string(),
                zone.exhaust_fan_heat_to_zone,
            );
            outputs.insert(
                "outdoor_air_mass_flow".to_string(),
                zone.outdoor_air_mass_flow,
            );
            outputs.insert("nat_vent_flow".to_string(), zone.nat_vent_flow);
            outputs.insert("nat_vent_mass_flow".to_string(), zone.nat_vent_mass_flow);
            outputs.insert(
                "nat_vent_active".to_string(),
                if zone.nat_vent_active { 1.0 } else { 0.0 },
            );
            outputs.insert("q_internal_conv".to_string(), zone.q_internal_conv);
            outputs.insert("q_internal_rad".to_string(), zone.q_internal_rad);
            outputs.insert("supply_air_temp".to_string(), zone.supply_air_temp);
            outputs.insert(
                "supply_air_mass_flow".to_string(),
                zone.supply_air_mass_flow,
            );
            // Zone air balance component diagnostics
            outputs.insert("q_surf_conv_total".to_string(), zone.q_surf_conv_total);
            outputs.insert("q_surf_conv_walls".to_string(), zone.q_surf_conv_walls);
            outputs.insert("q_surf_conv_floors".to_string(), zone.q_surf_conv_floors);
            outputs.insert("q_surf_conv_roofs".to_string(), zone.q_surf_conv_roofs);
            outputs.insert("q_surf_conv_windows".to_string(), zone.q_surf_conv_windows);
            outputs.insert(
                "q_infiltration_sensible".to_string(),
                zone.q_infiltration_sensible,
            );
            outputs.insert("q_thermal_mass".to_string(), zone.q_thermal_mass);
            results
                .zone_outputs
                .insert(zone.input.name.clone(), outputs);
        }

        results
    }

    /// Update zone temperature BDF history after HVAC convergence.
    ///
    /// Must be called exactly ONCE per physical timestep, AFTER all HVAC
    /// iterations have converged. Calling it inside the HVAC iteration
    /// loop would corrupt the backward-difference extrapolation.
    fn update_bdf_history(&mut self) {
        // Zone air temperature and humidity BDF history
        for zone in &mut self.zones {
            // Guard against NaN: if zone temp diverged this timestep, keep the
            // previous value so the BDF history stays finite.
            let t_safe = if zone.temp.is_nan() {
                zone.temp_prev
            } else {
                zone.temp
            };
            zone.temp_prev3 = zone.temp_prev2;
            zone.temp_prev2 = zone.temp_prev;
            zone.temp_prev = t_safe;
            zone.temp = t_safe; // also fix zone.temp for next timestep's t_zone_vec
            zone.temp_order = (zone.temp_order + 1).min(3);

            let w_safe = if zone.humidity_ratio.is_nan() {
                zone.w_prev
            } else {
                zone.humidity_ratio
            };
            zone.w_prev3 = zone.w_prev2;
            zone.w_prev2 = zone.w_prev;
            zone.w_prev = w_safe;
            zone.humidity_ratio = w_safe;
            zone.w_order = (zone.w_order + 1).min(3);

            // Reset predictor mode lock so it will be recomputed on next physical timestep
            zone.ideal_pred_mode_locked = false;
        }
        // CTF conduction history (surface temps + heat fluxes)
        for i in 0..self.surfaces.len() {
            if let Some(history) = &mut self.ctf_histories[i] {
                // Guard against NaN: if a surface temperature diverged this
                // timestep, use the most recent valid history value instead
                // of writing NaN into the CTF state.  The surface will
                // self-correct on subsequent timesteps once zone temps are finite.
                let t_out = if self.surfaces[i].temp_outside.is_nan() {
                    *history.t_outside.first().unwrap_or(&21.0)
                } else {
                    self.surfaces[i].temp_outside
                };
                let t_in = if self.surfaces[i].temp_inside.is_nan() {
                    *history.t_inside.first().unwrap_or(&21.0)
                } else {
                    self.surfaces[i].temp_inside
                };
                let q_in = if self.ctf_q_last_inside[i].is_nan() {
                    0.0
                } else {
                    self.ctf_q_last_inside[i]
                };
                let q_out = if self.ctf_q_last_outside[i].is_nan() {
                    0.0
                } else {
                    self.ctf_q_last_outside[i]
                };
                history.shift(t_out, t_in, q_in, q_out);
            }
        }
    }

    fn zone_names(&self) -> Vec<String> {
        self.zones.iter().map(|z| z.input.name.clone()).collect()
    }
}

impl BuildingEnvelope {
    /// Reset all CTF histories and surface temperatures for design day warmup.
    ///
    /// Before each design day, the envelope state must be re-initialized to
    /// ensure the warmup starts from a consistent state. Without this, heavy
    /// interzone surfaces (e.g. concrete floor/ceiling slabs) carry residual
    /// heat flux from a previous design day, requiring many more warmup days
    /// to reach cyclic steady-state (or never converging for short warmup).
    /// Cap the BDF order for zone temperature and humidity.
    ///
    /// During warmup, BDF3 can amplify oscillations in zones with slow-
    /// responding surfaces (e.g. heavily insulated floors). Capping to
    /// BDF1 (backward Euler) during warmup prevents this while still
    /// allowing the zone to equilibrate. Call AFTER update_bdf_history().
    pub fn cap_bdf_order(&mut self, max_order: u8) {
        for zone in &mut self.zones {
            zone.temp_order = zone.temp_order.min(max_order);
            zone.w_order = zone.w_order.min(max_order);
        }
    }

    /// Reset BDF history to current zone state after warmup.
    ///
    /// After warmup completes (using BDF1), the history terms (temp_prev2,
    /// temp_prev3, w_prev2, w_prev3) may contain stale values from early
    /// warmup iterations. When BDF order ramps to 3 during the main sim,
    /// these stale values cause wild extrapolation → divergence → NaN.
    ///
    /// This method sets all history terms to the current converged value
    /// and resets order to 1 so BDF ramps cleanly from a consistent state.
    pub fn reset_bdf_history_to_current(&mut self) {
        for zone in &mut self.zones {
            zone.temp_prev = zone.temp;
            zone.temp_prev2 = zone.temp;
            zone.temp_prev3 = zone.temp;
            zone.temp_order = 1;

            zone.w_prev = zone.humidity_ratio;
            zone.w_prev2 = zone.humidity_ratio;
            zone.w_prev3 = zone.humidity_ratio;
            zone.w_order = 1;
        }
    }

    pub fn reset_for_sizing(&mut self, temp: f64) {
        // Reset CTF conduction histories
        for history in &mut self.ctf_histories {
            if let Some(h) = history {
                h.reset(temp);
            }
        }
        // Reset surface temperatures
        for surface in &mut self.surfaces {
            surface.temp_inside = temp;
            surface.temp_outside = temp;
        }
        // Reset deferred CTF flux values
        for q in self.ctf_q_last_inside.iter_mut() {
            *q = 0.0;
        }
        for q in self.ctf_q_last_outside.iter_mut() {
            *q = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zone::IdealLoadsAirSystem;
    use openbse_core::ports::{SimulationContext, SizingInternalGains};
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

    fn make_simple_model() -> BuildingEnvelope {
        use crate::material::ConstructionLayer;
        let materials = vec![
            Material {
                name: "Concrete".to_string(),
                conductivity: 1.311,
                density: 2240.0,
                specific_heat: 836.8,
                solar_absorptance: 0.7,
                thermal_absorptance: 0.9,
                visible_absorptance: 0.7,
                roughness: Roughness::MediumRough,
                thermal_resistance: None,
                vapor_resistance_factor: None,
                sorption_isotherm: None,
                liquid_transport_coeff: None,
                thermal_absorptance_inside: None,
            },
            Material {
                name: "Insulation".to_string(),
                conductivity: 0.04,
                density: 30.0,
                specific_heat: 840.0,
                solar_absorptance: 0.7,
                thermal_absorptance: 0.9,
                visible_absorptance: 0.7,
                roughness: Roughness::Rough,
                thermal_resistance: None,
                vapor_resistance_factor: None,
                sorption_isotherm: None,
                liquid_transport_coeff: None,
                thermal_absorptance_inside: None,
            },
        ];

        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![
                ConstructionLayer {
                    material: "Concrete".to_string(),
                    thickness: 0.2,
                },
                ConstructionLayer {
                    material: "Insulation".to_string(),
                    thickness: 0.1,
                },
            ],
        }];

        let window_constructions = vec![WindowConstruction {
            name: "Window".to_string(),
            u_factor: 3.0,
            shgc: 0.7,
            visible_transmittance: 0.6,
            solar_absorptance: None,
            inside_absorbed_fraction: 0.5,
            pane_solar_transmittance: None,
            pane_solar_reflectance: None,
            num_panes: None,
            gap_width: None,
            pane_conductivity: None,
            pane_thickness: None,
            glass_emissivity: None,
        }];

        let zones = vec![ZoneInput {
            name: "TestZone".to_string(),
            volume: 150.0,
            floor_area: 50.0,
            infiltration: vec![crate::infiltration::InfiltrationInput {
                air_changes_per_hour: 0.5,
                ..Default::default()
            }],
            internal_gains: vec![crate::internal_gains::InternalGainInput::Equipment {
                power: 500.0,
                radiant_fraction: 0.3,
                lost_fraction: 0.0,
                latent_fraction: 0.0,
                schedule: None,
                submeter: "General".to_string(),
            }],
            internal_mass: vec![],

            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
        }];

        let surfaces = vec![
            SurfaceInput {
                name: "South Wall".to_string(),
                zone: "TestZone".to_string(),
                surface_type: SurfaceType::Wall,
                construction: "Wall".to_string(),
                area: 20.0,
                azimuth: 180.0,
                tilt: 90.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
            SurfaceInput {
                name: "South Window".to_string(),
                zone: "TestZone".to_string(),
                surface_type: SurfaceType::Window,
                construction: "Window".to_string(),
                area: 4.0,
                azimuth: 180.0,
                tilt: 90.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: Some("South Wall".to_string()),
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
            SurfaceInput {
                name: "Floor".to_string(),
                zone: "TestZone".to_string(),
                surface_type: SurfaceType::Floor,
                construction: "Wall".to_string(),
                area: 50.0,
                azimuth: 0.0,
                tilt: 180.0,
                boundary: BoundaryCondition::Ground,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: false,
                wind_exposure: false,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
            SurfaceInput {
                name: "Roof".to_string(),
                zone: "TestZone".to_string(),
                surface_type: SurfaceType::Roof,
                construction: "Wall".to_string(),
                area: 50.0,
                azimuth: 0.0,
                tilt: 0.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
        ];

        BuildingEnvelope::from_input(
            materials,
            constructions,
            window_constructions,
            zones,
            surfaces,
            40.0,
            -105.0,
            -7.0,
        )
    }

    fn make_ctx() -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 1,
                day: 15,
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

    fn make_weather_hour(dry_bulb: f64) -> WeatherHour {
        WeatherHour {
            year: 2023,
            month: 1,
            day: 15,
            hour: 12,
            dry_bulb,
            dew_point: -5.0,
            rel_humidity: 50.0,
            pressure: 101325.0,
            global_horiz_rad: 300.0,
            direct_normal_rad: 500.0,
            diffuse_horiz_rad: 100.0,
            wind_speed: 3.0,
            wind_direction: 180.0,
            horiz_ir_rad: 200.0,
            opaque_sky_cover: 5.0,
        }
    }

    #[test]
    fn test_envelope_initialization() {
        let mut envelope = make_simple_model();
        envelope.initialize(3600.0).unwrap();
        assert!(envelope.initialized);
        // South Wall should have CTF (not a window)
        assert!(envelope.ctf_coefficients[0].is_some());
        // South Window should NOT have CTF
        assert!(envelope.ctf_coefficients[1].is_none());
    }

    #[test]
    fn test_envelope_cold_outdoor_cools_zone() {
        use crate::material::ConstructionLayer;
        // Use a model with NO internal gains to isolate conduction cooling
        let materials = vec![
            Material {
                name: "Concrete".to_string(),
                conductivity: 1.311,
                density: 2240.0,
                specific_heat: 836.8,
                solar_absorptance: 0.7,
                thermal_absorptance: 0.9,
                visible_absorptance: 0.7,
                roughness: Roughness::MediumRough,
                thermal_resistance: None,
                vapor_resistance_factor: None,
                sorption_isotherm: None,
                liquid_transport_coeff: None,
                thermal_absorptance_inside: None,
            },
            Material {
                name: "Insulation".to_string(),
                conductivity: 0.04,
                density: 30.0,
                specific_heat: 840.0,
                solar_absorptance: 0.7,
                thermal_absorptance: 0.9,
                visible_absorptance: 0.7,
                roughness: Roughness::Rough,
                thermal_resistance: None,
                vapor_resistance_factor: None,
                sorption_isotherm: None,
                liquid_transport_coeff: None,
                thermal_absorptance_inside: None,
            },
        ];
        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![
                ConstructionLayer {
                    material: "Concrete".to_string(),
                    thickness: 0.2,
                },
                ConstructionLayer {
                    material: "Insulation".to_string(),
                    thickness: 0.1,
                },
            ],
        }];
        let zones = vec![ZoneInput {
            name: "TestZone".to_string(),
            volume: 150.0,
            floor_area: 50.0,
            infiltration: vec![crate::infiltration::InfiltrationInput {
                air_changes_per_hour: 0.5,
                ..Default::default()
            }],
            internal_gains: vec![], // No internal gains
            internal_mass: vec![],

            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
        }];
        let surfaces = vec![
            SurfaceInput {
                name: "South Wall".to_string(),
                zone: "TestZone".to_string(),
                surface_type: SurfaceType::Wall,
                construction: "Wall".to_string(),
                area: 30.0,
                azimuth: 180.0,
                tilt: 90.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
            SurfaceInput {
                name: "Roof".to_string(),
                zone: "TestZone".to_string(),
                surface_type: SurfaceType::Roof,
                construction: "Wall".to_string(),
                area: 50.0,
                azimuth: 0.0,
                tilt: 0.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
        ];
        let mut envelope = BuildingEnvelope::from_input(
            materials,
            constructions,
            vec![],
            zones,
            surfaces,
            40.0,
            -105.0,
            -7.0,
        );
        envelope.initialize(3600.0).unwrap();

        // Night context — no solar radiation
        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 1,
                day: 15,
                hour: 3,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        };
        let mut weather = make_weather_hour(0.0);
        weather.global_horiz_rad = 0.0;
        weather.direct_normal_rad = 0.0;
        weather.diffuse_horiz_rad = 0.0;
        weather.hour = 3; // nighttime
        let hvac = ZoneHvacConditions::default(); // No HVAC

        // Run several timesteps
        for _ in 0..10 {
            envelope.solve_timestep(&ctx, &weather, &hvac);
        }

        // Zone should cool down from 21°C (no HVAC, no solar, cold outside)
        let t_zone = envelope.zones[0].temp;
        assert!(t_zone < 21.0, "Zone should cool down: got {}", t_zone);
    }

    #[test]
    fn test_envelope_hvac_keeps_zone_warm() {
        let mut envelope = make_simple_model();
        envelope.initialize(3600.0).unwrap();

        let ctx = make_ctx();
        let weather = make_weather_hour(0.0);
        let mut hvac = ZoneHvacConditions::default();
        hvac.supply_temps.insert("TestZone".to_string(), 35.0);
        hvac.supply_mass_flows.insert("TestZone".to_string(), 0.5);

        for _ in 0..20 {
            envelope.solve_timestep(&ctx, &weather, &hvac);
        }

        // With HVAC supply at 35°C, zone should stay well above freezing
        let t_zone = envelope.zones[0].temp;
        assert!(t_zone > 15.0, "HVAC should keep zone warm: got {}", t_zone);
    }

    #[test]
    fn test_envelope_window_area_subtracted_from_parent() {
        let envelope = make_simple_model();
        // South Wall: gross 20m², window 4m² → net 16m²
        assert!((envelope.surfaces[0].net_area - 16.0).abs() < 0.01);
    }

    #[test]
    fn test_ideal_loads_heating() {
        use crate::material::ConstructionLayer;
        // Zone with ideal loads in cold conditions should heat to setpoint
        let materials = vec![Material {
            name: "Concrete".to_string(),
            conductivity: 1.311,
            density: 2240.0,
            specific_heat: 836.8,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            visible_absorptance: 0.7,
            roughness: Roughness::MediumRough,
            thermal_resistance: None,
            vapor_resistance_factor: None,
            sorption_isotherm: None,
            liquid_transport_coeff: None,
            thermal_absorptance_inside: None,
        }];
        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![ConstructionLayer {
                material: "Concrete".to_string(),
                thickness: 0.2,
            }],
        }];
        let zones = vec![ZoneInput {
            name: "IdealZone".to_string(),
            volume: 130.0,
            floor_area: 48.0,
            infiltration: vec![crate::infiltration::InfiltrationInput {
                air_changes_per_hour: 0.5,
                ..Default::default()
            }],
            internal_gains: vec![],
            internal_mass: vec![],

            ideal_loads: Some(IdealLoadsAirSystem {
                heating_setpoint: 20.0,
                cooling_setpoint: 27.0,
                heating_capacity: 1_000_000.0,
                cooling_capacity: 1_000_000.0,
            }),
            thermostat_schedule: vec![],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
        }];
        let surfaces = vec![
            SurfaceInput {
                name: "Wall".to_string(),
                zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Wall,
                construction: "Wall".to_string(),
                area: 60.0,
                azimuth: 180.0,
                tilt: 90.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
            SurfaceInput {
                name: "Roof".to_string(),
                zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Roof,
                construction: "Wall".to_string(),
                area: 48.0,
                azimuth: 0.0,
                tilt: 0.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
        ];
        let mut envelope = BuildingEnvelope::from_input(
            materials,
            constructions,
            vec![],
            zones,
            surfaces,
            40.0,
            -105.0,
            -7.0,
        );
        envelope.initialize(900.0).unwrap();

        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 1,
                day: 15,
                hour: 3,
                sub_hour: 1,
                timesteps_per_hour: 4,
                sim_time_s: 0.0,
                dt: 900.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(-10.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        };
        let mut weather = make_weather_hour(-10.0);
        weather.global_horiz_rad = 0.0;
        weather.direct_normal_rad = 0.0;
        weather.diffuse_horiz_rad = 0.0;
        weather.hour = 3;
        let hvac = ZoneHvacConditions::default();

        // Run many timesteps
        for _ in 0..100 {
            envelope.solve_timestep(&ctx, &weather, &hvac);
        }

        // Zone should be at heating setpoint (20°C) after convergence
        let t_zone = envelope.zones[0].temp;
        assert!(
            t_zone > 19.5 && t_zone < 20.5,
            "Ideal loads heating should maintain ~20°C, got {:.2}",
            t_zone
        );
        assert!(
            envelope.zones[0].hvac_heating_rate > 0.0,
            "Should be heating"
        );
        assert!(
            envelope.zones[0].hvac_cooling_rate < 0.01,
            "Should not be cooling"
        );
    }

    #[test]
    fn test_ideal_loads_cooling() {
        use crate::material::ConstructionLayer;
        // Zone with ideal loads in hot conditions should cool to setpoint
        let materials = vec![Material {
            name: "Concrete".to_string(),
            conductivity: 1.311,
            density: 2240.0,
            specific_heat: 836.8,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            visible_absorptance: 0.7,
            roughness: Roughness::MediumRough,
            thermal_resistance: None,
            vapor_resistance_factor: None,
            sorption_isotherm: None,
            liquid_transport_coeff: None,
            thermal_absorptance_inside: None,
        }];
        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![ConstructionLayer {
                material: "Concrete".to_string(),
                thickness: 0.2,
            }],
        }];
        let zones = vec![ZoneInput {
            name: "IdealZone".to_string(),
            volume: 130.0,
            floor_area: 48.0,
            infiltration: vec![crate::infiltration::InfiltrationInput {
                air_changes_per_hour: 0.5,
                ..Default::default()
            }],
            internal_gains: vec![crate::internal_gains::InternalGainInput::Equipment {
                power: 2000.0, // Large internal gains to force cooling
                radiant_fraction: 0.3,
                lost_fraction: 0.0,
                latent_fraction: 0.0,
                schedule: None,
                submeter: "General".to_string(),
            }],
            internal_mass: vec![],

            ideal_loads: Some(IdealLoadsAirSystem {
                heating_setpoint: 20.0,
                cooling_setpoint: 27.0,
                heating_capacity: 1_000_000.0,
                cooling_capacity: 1_000_000.0,
            }),
            thermostat_schedule: vec![],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
        }];
        let surfaces = vec![
            SurfaceInput {
                name: "Wall".to_string(),
                zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Wall,
                construction: "Wall".to_string(),
                area: 60.0,
                azimuth: 180.0,
                tilt: 90.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
            SurfaceInput {
                name: "Roof".to_string(),
                zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Roof,
                construction: "Wall".to_string(),
                area: 48.0,
                azimuth: 0.0,
                tilt: 0.0,
                boundary: BoundaryCondition::Outdoor,
                parent_surface: None,
                vertices: None,
                shading: None,
                sun_exposure: true,
                wind_exposure: true,
                exposed_perimeter: None,
                airflow: None,
                height: None,
                width: None,
            },
        ];
        let mut envelope = BuildingEnvelope::from_input(
            materials,
            constructions,
            vec![],
            zones,
            surfaces,
            40.0,
            -105.0,
            -7.0,
        );
        envelope.initialize(900.0).unwrap();

        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 7,
                day: 15,
                hour: 14,
                sub_hour: 1,
                timesteps_per_hour: 4,
                sim_time_s: 0.0,
                dt: 900.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(35.0, 0.3, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        };
        let weather = make_weather_hour(35.0);
        let hvac = ZoneHvacConditions::default();

        for _ in 0..100 {
            envelope.solve_timestep(&ctx, &weather, &hvac);
            envelope.update_bdf_history();
        }

        // Zone should be at cooling setpoint (27°C) after convergence
        let t_zone = envelope.zones[0].temp;
        assert!(
            t_zone > 26.5 && t_zone < 27.5,
            "Ideal loads cooling should maintain ~27°C, got {:.2}",
            t_zone
        );
        assert!(
            envelope.zones[0].hvac_cooling_rate > 0.0,
            "Should be cooling"
        );
    }

    #[test]
    fn test_ideal_loads_deadband() {
        use crate::material::ConstructionLayer;
        // Zone in deadband should have no HVAC
        let materials = vec![Material {
            name: "Concrete".to_string(),
            conductivity: 1.311,
            density: 2240.0,
            specific_heat: 836.8,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            visible_absorptance: 0.7,
            roughness: Roughness::MediumRough,
            thermal_resistance: None,
            vapor_resistance_factor: None,
            sorption_isotherm: None,
            liquid_transport_coeff: None,
            thermal_absorptance_inside: None,
        }];
        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![ConstructionLayer {
                material: "Concrete".to_string(),
                thickness: 0.2,
            }],
        }];
        let zones = vec![ZoneInput {
            name: "IdealZone".to_string(),
            volume: 130.0,
            floor_area: 48.0,
            infiltration: vec![crate::infiltration::InfiltrationInput::default()],
            internal_gains: vec![],
            internal_mass: vec![],

            ideal_loads: Some(IdealLoadsAirSystem {
                heating_setpoint: 20.0,
                cooling_setpoint: 27.0,
                heating_capacity: 1_000_000.0,
                cooling_capacity: 1_000_000.0,
            }),
            thermostat_schedule: vec![],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
        }];
        let surfaces = vec![SurfaceInput {
            name: "Wall".to_string(),
            zone: "IdealZone".to_string(),
            surface_type: SurfaceType::Wall,
            construction: "Wall".to_string(),
            area: 60.0,
            azimuth: 180.0,
            tilt: 90.0,
            boundary: BoundaryCondition::Outdoor,
            parent_surface: None,
            vertices: None,
            shading: None,
            sun_exposure: true,
            wind_exposure: true,
            exposed_perimeter: None,
            airflow: None,
            height: None,
            width: None,
        }];
        let mut envelope = BuildingEnvelope::from_input(
            materials,
            constructions,
            vec![],
            zones,
            surfaces,
            40.0,
            -105.0,
            -7.0,
        );
        envelope.initialize(900.0).unwrap();

        // Outdoor at 23°C — zone should be in deadband
        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 5,
                day: 15,
                hour: 12,
                sub_hour: 1,
                timesteps_per_hour: 4,
                sim_time_s: 0.0,
                dt: 900.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(23.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        };
        let mut weather = make_weather_hour(23.0);
        weather.global_horiz_rad = 0.0;
        weather.direct_normal_rad = 0.0;
        weather.diffuse_horiz_rad = 0.0;
        // Overcast sky (N=10) with warm dew point minimizes sky LWR effect,
        // keeping zone in deadband
        weather.opaque_sky_cover = 10.0;
        weather.dew_point = 15.0;
        let hvac = ZoneHvacConditions::default();

        for _ in 0..100 {
            envelope.solve_timestep(&ctx, &weather, &hvac);
        }

        // With outdoor at 23°C and no gains, zone should be in deadband
        let t_zone = envelope.zones[0].temp;
        assert!(
            t_zone > 19.0 && t_zone < 28.0,
            "Zone should be in deadband, got {:.2}",
            t_zone
        );
        // HVAC rates should be zero (in deadband)
        assert!(
            envelope.zones[0].hvac_heating_rate < 0.01,
            "Should not be heating in deadband"
        );
        assert!(
            envelope.zones[0].hvac_cooling_rate < 0.01,
            "Should not be cooling in deadband"
        );
    }

    #[test]
    fn test_sky_longwave_cools_surfaces() {
        // Test that sky longwave radiation cools outdoor surfaces below
        // the outdoor air temperature when the sky is cold.
        let mut envelope = make_simple_model();
        envelope.initialize(3600.0).unwrap();

        let ctx = make_ctx();
        // Clear sky (opaque_sky_cover=0), dry air (dew_point=-15°C)
        // This gives a cold sky temperature and surfaces should cool below outdoor
        let mut weather = make_weather_hour(0.0);
        weather.global_horiz_rad = 0.0;
        weather.direct_normal_rad = 0.0;
        weather.diffuse_horiz_rad = 0.0;
        weather.dew_point = -15.0; // dry air → cold sky
        weather.opaque_sky_cover = 0.0; // clear sky
        let hvac = ZoneHvacConditions::default();

        // With multi-term state-space CTF, thermal mass retains initial conditions
        // longer. Need enough iterations to reach thermal equilibrium.
        for _ in 0..100 {
            envelope.solve_timestep(&ctx, &weather, &hvac);
            envelope.update_bdf_history();
        }

        // Find the roof (tilt=0, full sky view) — it should be colder than outdoor
        let roof = envelope
            .surfaces
            .iter()
            .find(|s| s.input.name == "Roof")
            .expect("Roof should exist");
        assert!(
            roof.temp_outside < 0.0,
            "Roof should be below outdoor temp (0°C) due to sky LWR, got {:.2}°C",
            roof.temp_outside
        );

        // South Wall (tilt=90, partial sky view) should also be cooler but less so
        let wall = envelope
            .surfaces
            .iter()
            .find(|s| s.input.name == "South Wall")
            .expect("South Wall should exist");
        assert!(
            wall.temp_outside < 0.0,
            "South wall should be below outdoor temp due to sky LWR, got {:.2}°C",
            wall.temp_outside
        );
        // Roof should be colder than wall (full vs partial sky view)
        assert!(
            roof.temp_outside < wall.temp_outside,
            "Roof ({:.2}°C) should be colder than wall ({:.2}°C)",
            roof.temp_outside,
            wall.temp_outside
        );
    }

    #[test]
    fn test_sky_temp_berdahl_martin() {
        // Verify Berdahl-Martin sky temperature model with cloud correction.
        // ε_clear = 0.787 + 0.764 * ln(T_dp_K / 273)
        // ε_sky = ε_clear * (1 + 0.0224*N - 0.0035*N² + 0.00028*N³)
        // T_sky = ε_sky^0.25 * T_air_K - 273.15

        // Case 1: Clear sky (N=0), T_air=0°C, T_dp=-10°C
        let t_dp_k = (-10.0 + 273.15_f64).max(200.0);
        let t_db_k = (0.0 + 273.15_f64).max(200.0);
        let eps_clear = 0.787 + 0.764 * (t_dp_k / 273.0).ln();
        let t_sky = eps_clear.powf(0.25) * t_db_k - 273.15;
        // Sky should be well below outdoor
        assert!(
            t_sky < -5.0,
            "Clear sky should be well below outdoor, got {:.1}°C",
            t_sky
        );
        assert!(
            t_sky > -25.0,
            "Sky shouldn't be excessively cold, got {:.1}°C",
            t_sky
        );

        // Case 2: Overcast sky (N=10) should be warmer than clear sky
        let n = 10.0_f64;
        let cloud_factor = 1.0 + 0.0224 * n - 0.0035 * n * n + 0.00028 * n * n * n;
        let eps_overcast = (eps_clear * cloud_factor).min(1.0);
        let t_sky_overcast = eps_overcast.powf(0.25) * t_db_k - 273.15;
        assert!(
            t_sky_overcast > t_sky,
            "Overcast sky ({:.1}°C) should be warmer than clear ({:.1}°C)",
            t_sky_overcast,
            t_sky
        );
        // Overcast sky should be within ~10°C of outdoor temp
        assert!(
            (t_sky_overcast - 0.0).abs() < 12.0,
            "Overcast sky should be within 12°C of outdoor, got {:.1}°C",
            t_sky_overcast
        );

        // Case 3: Summer clear sky at 30°C, Tdp=10°C
        let t_dp_k_s = 283.15_f64;
        let t_db_k_s = 303.15_f64;
        let eps_clear_s = 0.787 + 0.764 * (t_dp_k_s / 273.0).ln();
        let t_sky_s = eps_clear_s.powf(0.25) * t_db_k_s - 273.15;
        // Summer clear sky depression should be ~12-18°C
        let depression = 30.0 - t_sky_s;
        assert!(
            depression > 8.0 && depression < 22.0,
            "Summer sky depression should be 8-22°C, got {:.1}°C (T_sky={:.1}°C)",
            depression,
            t_sky_s
        );
    }

    #[test]
    fn test_solar_cache_save_load_roundtrip() {
        // Verify SolarCache can be saved to disk, loaded back,
        // and that fingerprint mismatches cause a cache miss.
        let cache = SolarCache {
            sunlit_fractions: vec![
                vec![1.0, 0.5, 0.8],
                vec![1.0, 0.3, 0.9],
                vec![1.0, 0.7, 0.6],
            ],
            timesteps_per_hour: 6,
            start_hour: 0,
            fingerprint: 0xDEADBEEF12345678,
        };

        let dir = std::env::temp_dir();
        let path = dir.join("openbse_test_solar_cache.solar");

        // Save
        cache.save_to_file(&path).expect("save should succeed");
        assert!(path.exists(), "cache file should exist after save");

        // Load with matching fingerprint
        let loaded = SolarCache::load_from_file(&path, 0xDEADBEEF12345678);
        assert!(
            loaded.is_some(),
            "load with matching fingerprint should succeed"
        );
        let loaded = loaded.unwrap();
        assert_eq!(loaded.sunlit_fractions.len(), 3);
        assert_eq!(loaded.sunlit_fractions[0].len(), 3);
        assert_eq!(loaded.timesteps_per_hour, 6);
        assert_eq!(loaded.start_hour, 0);
        assert_eq!(loaded.fingerprint, 0xDEADBEEF12345678);
        // Verify data integrity
        assert!((loaded.sunlit_fractions[1][1] - 0.3).abs() < 1e-15);

        // Load with mismatched fingerprint → should return None
        let stale = SolarCache::load_from_file(&path, 0x1111111111111111);
        assert!(
            stale.is_none(),
            "load with mismatched fingerprint should return None"
        );

        // Load from nonexistent path → should return None
        let missing =
            SolarCache::load_from_file(&dir.join("no_such_file.solar"), 0xDEADBEEF12345678);
        assert!(
            missing.is_none(),
            "load from missing file should return None"
        );

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_solar_cache_fingerprint_changes_with_geometry() {
        // Fingerprint should change when surface vertices change.
        let env = make_simple_model();
        let fp1 = SolarCache::compute_fingerprint(
            &env.surfaces,
            &env.shading_polygons,
            40.0,
            -105.0,
            -7.0,
            6,
            0,
            8760,
        );

        // Same inputs → same fingerprint
        let fp2 = SolarCache::compute_fingerprint(
            &env.surfaces,
            &env.shading_polygons,
            40.0,
            -105.0,
            -7.0,
            6,
            0,
            8760,
        );
        assert_eq!(fp1, fp2, "same geometry should produce same fingerprint");

        // Different latitude → different fingerprint
        let fp3 = SolarCache::compute_fingerprint(
            &env.surfaces,
            &env.shading_polygons,
            41.0,
            -105.0,
            -7.0,
            6,
            0,
            8760,
        );
        assert_ne!(fp1, fp3, "different latitude should change fingerprint");

        // Different timesteps_per_hour → different fingerprint
        let fp4 = SolarCache::compute_fingerprint(
            &env.surfaces,
            &env.shading_polygons,
            40.0,
            -105.0,
            -7.0,
            4,
            0,
            8760,
        );
        assert_ne!(
            fp1, fp4,
            "different timesteps_per_hour should change fingerprint"
        );
    }
}
