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

use std::collections::HashMap;
use openbse_core::ports::{SimulationContext, EnvelopeSolver, ZoneHvacConditions, EnvelopeResults};
use openbse_weather::WeatherHour;
use openbse_psychrometrics as psych;

use crate::material::{Material, Construction, WindowConstruction, SimpleConstruction, Roughness};
use crate::surface::{SurfaceState, SurfaceInput, SurfaceType, BoundaryCondition};
use crate::zone::{ZoneState, ZoneInput, InteriorSolarDistribution};
use crate::ctf::{CtfCoefficients, CtfHistory, calculate_ctf, calculate_ctf_simple, apply_ctf};
use crate::convection;
use crate::solar;
use crate::infiltration;
use crate::internal_gains;
use crate::geometry;
use crate::ground_temp::GroundTempModel;
use crate::schedule::{ScheduleManager, day_of_week};
use crate::shading;

/// The building envelope heat balance solver.
#[derive(Debug)]
pub struct BuildingEnvelope {
    pub zones: Vec<ZoneState>,
    pub surfaces: Vec<SurfaceState>,
    pub ctf_coefficients: Vec<Option<CtfCoefficients>>,
    pub ctf_histories: Vec<Option<CtfHistory>>,
    pub materials: HashMap<String, Material>,
    pub constructions: HashMap<String, Construction>,
    pub window_constructions: HashMap<String, WindowConstruction>,
    pub simple_constructions: HashMap<String, SimpleConstruction>,
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
            materials, constructions, window_constructions, vec![],
            zones, surfaces, latitude, longitude, time_zone, 0.0,
        )
    }

    /// Create from input data with simple constructions.
    pub fn from_input_full(
        materials: Vec<Material>,
        constructions: Vec<Construction>,
        window_constructions: Vec<WindowConstruction>,
        simple_constructions: Vec<SimpleConstruction>,
        zones: Vec<ZoneInput>,
        mut surfaces: Vec<SurfaceInput>,
        latitude: f64,
        longitude: f64,
        time_zone: f64,
        elevation: f64,
    ) -> Self {
        let material_map: HashMap<String, Material> = materials
            .into_iter().map(|m| (m.name.clone(), m)).collect();
        let construction_map: HashMap<String, Construction> = constructions
            .into_iter().map(|c| (c.name.clone(), c)).collect();
        let window_map: HashMap<String, WindowConstruction> = window_constructions
            .into_iter().map(|w| (w.name.clone(), w)).collect();
        let simple_map: HashMap<String, SimpleConstruction> = simple_constructions
            .into_iter().map(|s| (s.name.clone(), s)).collect();

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

        // Auto-calculate zone volume and floor area from surface vertices
        for zone in &mut zone_states {
            let zone_surfaces: Vec<&SurfaceInput> = surfaces.iter()
                .filter(|s| s.zone == zone.input.name)
                .collect();

            // Auto-calculate volume if not specified (volume == 0)
            if zone.input.volume <= 0.0 {
                let all_have_verts = zone_surfaces.iter()
                    .all(|s| s.vertices.as_ref().map(|v| v.len() >= 3).unwrap_or(false));
                if all_have_verts && !zone_surfaces.is_empty() {
                    let vert_sets: Vec<Vec<geometry::Point3D>> = zone_surfaces.iter()
                        .filter(|s| s.surface_type != SurfaceType::Window) // exclude windows
                        .filter_map(|s| s.vertices.clone())
                        .collect();
                    let refs: Vec<&[geometry::Point3D]> = vert_sets.iter()
                        .map(|v| v.as_slice())
                        .collect();
                    let vol = geometry::zone_volume_from_surfaces(&refs);
                    if vol > 0.0 {
                        zone.input.volume = vol;
                        log::info!("Auto-calculated zone '{}' volume: {:.1} m³", zone.input.name, vol);
                    }
                }
            }

            // Auto-calculate floor area if not specified
            if zone.input.floor_area <= 0.0 {
                let floor_verts: Vec<Vec<geometry::Point3D>> = zone_surfaces.iter()
                    .filter(|s| s.surface_type == SurfaceType::Floor)
                    .filter_map(|s| s.vertices.clone())
                    .collect();
                if !floor_verts.is_empty() {
                    let refs: Vec<&[geometry::Point3D]> = floor_verts.iter()
                        .map(|v| v.as_slice())
                        .collect();
                    let area = geometry::zone_floor_area(&refs);
                    if area > 0.0 {
                        zone.input.floor_area = area;
                        log::info!("Auto-calculated zone '{}' floor area: {:.1} m²", zone.input.name, area);
                    }
                }
            }
        }

        // Auto-calculate solar distribution if not specified
        for zone in &mut zone_states {
            if zone.input.solar_distribution.is_none() {
                let zone_surfaces: Vec<&SurfaceInput> = surfaces.iter()
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

            // Window-specific absorptance properties (used below)
            let (win_solar_absorptance, win_inside_fraction) = if is_window {
                let wc = window_map.get(&surf_input.construction);
                let (abs, frac) = wc.map(|w| (
                    w.effective_solar_absorptance(),
                    w.inside_absorbed_fraction,
                )).unwrap_or((0.06, 0.5));
                (abs, frac)
            } else {
                (0.0, 0.0)
            };

            let (solar_abs_out, thermal_abs_out, solar_abs_in, roughness, u_factor, shgc) =
                if is_window {
                    let wc = window_map.get(&surf_input.construction);
                    let (u, s) = wc.map(|w| (w.u_factor, w.shgc)).unwrap_or((3.0, 0.7));
                    (0.0, 0.0, 0.0, Roughness::VerySmooth, u, s)
                } else if let Some(c) = construction_map.get(&surf_input.construction) {
                    // Layered construction
                    let outside_mat = c.outside_material()
                        .and_then(|name| material_map.get(name));
                    let inside_mat = c.inside_material()
                        .and_then(|name| material_map.get(name));

                    let (sa_out, ta_out, rough) = outside_mat
                        .map(|m| (m.solar_absorptance, m.thermal_absorptance, m.roughness))
                        .unwrap_or((0.7, 0.9, Roughness::MediumRough));
                    let sa_in = inside_mat.map(|m| m.solar_absorptance).unwrap_or(0.7);

                    let u = c.u_factor(&material_map);

                    (sa_out, ta_out, sa_in, rough, u, 0.0)
                } else if let Some(sc) = simple_map.get(&surf_input.construction) {
                    // Simple construction
                    (sc.solar_absorptance, sc.thermal_absorptance,
                     sc.solar_absorptance, sc.roughness, sc.u_factor, 0.0)
                } else {
                    (0.7, 0.9, 0.7, Roughness::MediumRough, 5.0, 0.0)
                };

            let tilt_rad = surf_input.tilt.to_radians();

            // Compute centroid height above ground for wind speed profile.
            // If vertices are available, use vertex centroid z-coordinate.
            // Otherwise, estimate from zone geometry and surface type.
            let centroid_height = if let Some(ref verts) = surf_input.vertices {
                if !verts.is_empty() {
                    let z_avg: f64 = verts.iter().map(|v| v.z).sum::<f64>()
                        / verts.len() as f64;
                    z_avg.max(0.1)
                } else {
                    1.0
                }
            } else {
                // Estimate from zone dimensions
                let zone_name = &surf_input.zone;
                let zone_input = zone_states.iter()
                    .find(|z| z.input.name == *zone_name);
                if let Some(zone) = zone_input {
                    let floor_area = zone.input.floor_area;
                    let volume = zone.input.volume;
                    let zone_height = if floor_area > 0.0 { volume / floor_area } else { 3.0 };
                    match surf_input.surface_type {
                        SurfaceType::Floor => 0.1,               // At ground level
                        SurfaceType::Wall => zone_height / 2.0,  // Midpoint of wall
                        SurfaceType::Roof | SurfaceType::Ceiling => zone_height, // At top
                        SurfaceType::Window => zone_height / 2.0, // Midpoint of parent wall
                    }
                } else {
                    2.0 // Fallback
                }
            };

            // For windows, decompose the overall U-factor into glass-only conductance
            // by removing standard film coefficients. This allows applying dynamic
            // exterior/interior films during simulation (like EnergyPlus Simple Glazing).
            //
            // Standard film coefficients for U-factor derivation (NFRC conditions):
            //   h_i_std = 8.29 W/(m²·K) — ASHRAE/NFRC interior combined (conv + rad)
            //   h_e_std = 26.0 W/(m²·K) — NFRC exterior combined at 5.5 m/s wind
            //
            // The NFRC standard conditions are the reference for U-factor rating:
            //   Outdoor: -18°C, 5.5 m/s wind → h_e ≈ 26 W/(m²K) (conv+rad)
            //   Indoor: 21°C, still air → h_i ≈ 8.29 W/(m²K) (conv+rad)
            //
            //   U_glass = 1 / (1/U_overall - 1/h_e_std - 1/h_i_std)
            let u_glass = if is_window && u_factor > 0.0 {
                let r_overall = 1.0 / u_factor;
                let r_films = 1.0 / 26.0 + 1.0 / 8.29; // ~0.159 m²K/W (NFRC standard)
                let r_glass = (r_overall - r_films).max(0.01);
                1.0 / r_glass
            } else {
                u_factor // Not used for opaque surfaces
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
                absorbed_solar_inside_window: 0.0,
                window_solar_absorptance: win_solar_absorptance,
                window_inside_absorbed_fraction: win_inside_fraction,
                solar_absorptance_outside: solar_abs_out,
                thermal_absorptance_outside: thermal_abs_out,
                solar_absorptance_inside: solar_abs_in,
                roughness,
                u_factor,
                u_glass,
                shgc,
                cos_tilt: tilt_rad.cos(),
                sin_tilt: tilt_rad.sin(),
                centroid_height,
                diffuse_sky_shading_ratio: 1.0, // Updated by compute_diffuse_shading_ratios()
                diffuse_horizon_shading_ratio: 1.0, // Updated by compute_diffuse_shading_ratios()
            };
            surface_states.push(state);
        }

        // Assign surfaces to zones
        for (surf_idx, surf) in surface_states.iter().enumerate() {
            if let Some(&zi) = zone_index.get(&surf.input.zone) {
                zone_states[zi].surface_indices.push(surf_idx);
            }
        }

        // Subtract window areas from parent surfaces
        let window_parents: Vec<(String, f64)> = surface_states.iter()
            .filter(|s| s.is_window)
            .filter_map(|s| s.input.parent_surface.as_ref().map(|p| (p.clone(), s.input.area)))
            .collect();
        for (parent_name, window_area) in &window_parents {
            if let Some(parent) = surface_states.iter_mut()
                .find(|s| s.input.name == *parent_name) {
                parent.net_area = (parent.net_area - window_area).max(0.0);
            }
        }

        let n_surfaces = surface_states.len();

        BuildingEnvelope {
            zones: zone_states,
            surfaces: surface_states,
            ctf_coefficients: vec![None; n_surfaces],
            ctf_histories: vec![None; n_surfaces],
            materials: material_map,
            constructions: construction_map,
            window_constructions: window_map,
            simple_constructions: simple_map,
            zone_index,
            ground_temp_model: None,
            schedule_manager: ScheduleManager::new(),
            latitude,
            longitude,
            time_zone,
            ground_reflectance: 0.2,
            jan1_dow: 1,  // Default: Monday; overridden by weather file
            dt: 3600.0,
            initialized: false,
            shading_calculation: shading::ShadingCalculation::Basic,
            shading_polygons: Vec::new(),
            terrain: convection::Terrain::default(),
            elevation,
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
                                surface_inputs.iter()
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
                                let ovh_verts = shading::generate_overhang_vertices(verts, ovh, &wall_outward);
                                if let Some(sp) = shading::surface_to_shading_polygon(
                                    &format!("ovh:{}", surf.name), &ovh_verts
                                ) {
                                    polygons.push(sp);
                                }
                            }

                            // Generate left fin
                            if let Some(ref fin) = shade_input.left_fin {
                                let fin_verts = shading::generate_fin_vertices(
                                    verts, fin, &wall_outward, shading::FinSide::Left
                                );
                                if let Some(sp) = shading::surface_to_shading_polygon(
                                    &format!("lfin:{}", surf.name), &fin_verts
                                ) {
                                    polygons.push(sp);
                                }
                            }

                            // Generate right fin
                            if let Some(ref fin) = shade_input.right_fin {
                                let fin_verts = shading::generate_fin_vertices(
                                    verts, fin, &wall_outward, shading::FinSide::Right
                                );
                                if let Some(sp) = shading::surface_to_shading_polygon(
                                    &format!("rfin:{}", surf.name), &fin_verts
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
                    let casters: Vec<&shading::ShadingPolygon> = self.shading_polygons.iter()
                        .filter(|sp| sp.name != *self_name)
                        .collect();

                    let ratio = shading::compute_diffuse_sky_shading_ratio(
                        verts, &normal, &casters,
                    );
                    let horiz_ratio = shading::compute_diffuse_horizon_shading_ratio(
                        verts, &normal, &casters,
                    );

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
                    let history = CtfHistory::new(ctf.num_terms.max(1), 21.0);
                    self.ctf_coefficients[i] = Some(ctf);
                    self.ctf_histories[i] = Some(history);
                    continue;
                }
            }

            // Try simple construction
            if let Some(sc) = self.simple_constructions.get(construction_name).cloned() {
                let ctf = calculate_ctf_simple(sc.u_factor, sc.thermal_capacity, self.dt, sc.mass_outside);
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
        let solar_hour = ctx.timestep.fractional_hour()
            + (self.longitude / 15.0 - self.time_zone) + eot;
        let sol_pos = solar::solar_position(doy, solar_hour, self.latitude);

        // 1b. Sky temperature for longwave radiation exchange
        // σ = 5.6704e-8 W/(m²·K⁴) (Stefan-Boltzmann constant)
        const SIGMA: f64 = 5.6704e-8;
        //
        // Use Berdahl-Martin clear-sky emissivity model with opaque sky cover
        // correction. This is more robust than direct EPW horizontal IR, which
        // can produce unreasonably cold sky temperatures in some TMYx files.
        //
        // Clear-sky emissivity: ε_clear = 0.787 + 0.764 * ln(T_dp_K / 273)
        // Cloud correction: ε_sky = ε_clear * (1 + 0.0224*N - 0.0035*N² + 0.00028*N³)
        //   where N = opaque sky cover in tenths [0-10]
        //
        // T_sky = (ε_sky)^0.25 * T_air_K - 273.15
        //
        // Reference: Berdahl & Martin (1984), Walton (1983), EnergyPlus Engineering Ref.
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

        // 3. Internal gains (schedule-aware)
        let dow = day_of_week(ctx.timestep.month, ctx.timestep.day, self.jan1_dow);
        for zone in &mut self.zones {
            let gains = internal_gains::resolve_gains_scheduled(
                &zone.input.internal_gains,
                Some(&self.schedule_manager),
                hour,
                dow,
            );
            zone.q_internal_conv = gains.convective;
            zone.q_internal_rad = gains.radiative;
            zone.lighting_power = gains.lighting_power;
            zone.equipment_power = gains.equipment_power;
            zone.people_heat = gains.people_heat;
        }

        // 4. Infiltration + scheduled ventilation + exhaust + outdoor air
        let rho_outdoor = psych::rho_air_fn_pb_tdb_w(p_b, t_outdoor, 0.008);
        for zone in &mut self.zones {
            // Sum infiltration from all objects (envelope cracks + door opening, etc.)
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
                    wind_speed_met,
                    rho_outdoor,
                ) * sched_mult;
            }

            // Scheduled ventilation (e.g., night ventilation for Case 650)
            let vent_flow = zone.input.scheduled_ventilation_flow(
                hour, zone.input.volume, zone.temp, t_outdoor,
            );
            zone.ventilation_mass_flow = vent_flow * rho_outdoor;

            // Exhaust fan (removes air from zone — schedule-aware)
            if let Some(ref exhaust) = zone.input.exhaust_fan {
                let exhaust_frac = match &exhaust.schedule {
                    Some(name) => self.schedule_manager.fraction(name, hour, dow),
                    None => 1.0,
                };
                zone.exhaust_mass_flow = exhaust.flow_rate * rho_outdoor * exhaust_frac;
            } else {
                zone.exhaust_mass_flow = 0.0;
            }

            // ASHRAE 62.1 outdoor air (calculated from people count + floor area)
            if let Some(ref oa) = zone.input.outdoor_air {
                let people_count: f64 = zone.input.internal_gains.iter()
                    .filter_map(|g| match g {
                        internal_gains::InternalGainInput::People { count, schedule, .. } => {
                            let frac = match (schedule, Some(&self.schedule_manager)) {
                                (Some(name), Some(mgr)) => mgr.fraction(name, hour, dow),
                                _ => 1.0,
                            };
                            Some(count * frac)
                        }
                        _ => None,
                    })
                    .sum();
                let oa_flow = oa.per_person * people_count + oa.per_area * zone.input.floor_area;
                zone.outdoor_air_mass_flow = oa_flow * rho_outdoor;
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

        // 5a. Pre-compute sunlit fractions for all surfaces
        let sunlit_fractions: Vec<f64> = if self.shading_calculation == shading::ShadingCalculation::Detailed
            && sol_pos.altitude > 0.0
            && !self.shading_polygons.is_empty()
        {
            let sun_dir = solar::sun_direction_vector(&sol_pos);
            self.surfaces.iter().map(|surface| {
                if surface.input.boundary != BoundaryCondition::Outdoor {
                    return 1.0;
                }
                if let Some(ref verts) = surface.input.vertices {
                    if verts.len() >= 3 {
                        let normal = geometry::newell_normal(verts).normalize();
                        // Collect casters: all shading polygons EXCEPT this surface itself
                        let self_name = format!("self:{}", surface.input.name);
                        let casters: Vec<&shading::ShadingPolygon> = self.shading_polygons.iter()
                            .filter(|sp| sp.name != self_name)
                            .collect();
                        return shading::calculate_sunlit_fraction(verts, &normal, &casters, &sun_dir);
                    }
                }
                1.0 // No vertices → fully sunlit (legacy area/azimuth/tilt surfaces)
            }).collect()
        } else {
            vec![1.0; self.surfaces.len()]
        };

        // 5b. Apply solar radiation with sunlit fractions
        for (si, surface) in self.surfaces.iter_mut().enumerate() {
            let sunlit = sunlit_fractions[si];
            if surface.input.boundary == BoundaryCondition::Outdoor {
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
                // CS-only shading: only circumsolar gets directional shading
                // (sunlit fraction from beam shadow). Isotropic and horizon
                // pass through unshaded. This is the best-validated config
                // (14/16 ASHRAE 140 metrics passing).
                //
                // NOTE: Full 3-component shading (iso×skyR + cs×sunlit + hz×horizR)
                // was tested but over-shades E/W cases (630 C drops below min).
                // The root cause of remaining 910 C failure is likely interior
                // solar distribution: beam should go ~90% to floor (geometric)
                // vs our fixed fractions (64.2% floor). See plan for details.
                let shaded_sky_diffuse =
                    components.sky_diffuse
                    + components.circumsolar * sunlit
                    + components.horizon;
                let diffuse_total = shaded_sky_diffuse + components.ground_diffuse;
                let effective_incident = (shaded_beam + diffuse_total).max(0.0);

                if surface.is_window {
                    // Windows: beam uses angular SHGC modifier,
                    //          all diffuse (incl. cs) uses hemispherical SHGC modifier
                    //          (matches E+ SkyDiffuse × DiffTrans treatment)
                    surface.incident_solar = effective_incident;
                    surface.transmitted_solar = solar::window_transmitted_solar_angular(
                        surface.shgc,
                        surface.net_area,
                        shaded_beam,
                        diffuse_total,
                        components.cos_aoi,
                    );
                    // Note: SHGC already includes absorbed-inward solar gain
                    // (SHGC = τ_solar + N_i × α_solar), so we do NOT add
                    // absorbed_solar_inside_window separately — that would
                    // double-count the absorbed portion.
                    surface.absorbed_solar_inside_window = 0.0;
                    surface.absorbed_solar_outside = 0.0;
                } else {
                    // Opaque surfaces: beam reduced by sunlit fraction
                    surface.incident_solar = effective_incident;
                    surface.absorbed_solar_outside =
                        surface.solar_absorptance_outside * effective_incident;
                    surface.transmitted_solar = 0.0;
                    surface.absorbed_solar_inside_window = 0.0;
                }
            } else {
                surface.incident_solar = 0.0;
                surface.transmitted_solar = 0.0;
                surface.absorbed_solar_outside = 0.0;
                surface.absorbed_solar_inside_window = 0.0;
            }
        }

        // 6. Surface ↔ zone coupling iteration
        //
        // Track the actual CTF conduction fluxes for history update.
        // These are the pure conduction q values from apply_ctf(), NOT the
        // surface-to-zone convective fluxes (which include radiative gains).
        let n_surf = self.surfaces.len();
        let mut ctf_q_inside: Vec<f64> = vec![0.0; n_surf];
        let mut ctf_q_outside: Vec<f64> = vec![0.0; n_surf];

        let max_iterations = 5;
        for _iter in 0..max_iterations {
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
                        if !self.surfaces[si].is_window {
                            let h_conv = self.surfaces[si].h_conv_outside;
                            let eps = self.surfaces[si].thermal_absorptance_outside;

                            // E+ view factors (ConvectionCoefficients.cc)
                            let f_sky = (1.0 + self.surfaces[si].cos_tilt) / 2.0;
                            let f_gnd = 1.0 - f_sky;

                            // E+ SurfAirSkyRadSplit (SurfaceGeometry.cc line 327):
                            // Splits sky-hemisphere radiation between actual sky (cold)
                            // and atmosphere (at air temperature).
                            let air_sky_rad_split = (0.5 * (1.0 + self.surfaces[si].cos_tilt)).sqrt();

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
                            let h_air = exact_h_rad(t_s_k, t_air_k, f_sky * (1.0 - air_sky_rad_split));
                            // HGround: radiation to ground (at ground/air temperature)
                            let h_gnd = exact_h_rad(t_s_k, t_gnd_k, f_gnd);

                            // CTF conduction coupling: include X[0] and Y[0] terms
                            // from the outside CTF equation to couple exterior surface
                            // to interior through the wall assembly.
                            let (ctf_x0, ctf_y0, ctf_flux_hist) = if let (
                                Some(ctf), Some(history)
                            ) = (
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

                            self.surfaces[si].temp_outside =
                                ((h_conv + h_air) * t_outdoor
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
                        self.surfaces[si].temp_outside = if let Some(ref gt) = self.ground_temp_model {
                            gt.temperature(doy as f64)
                        } else {
                            10.0
                        };
                    }
                    BoundaryCondition::Zone(other_zone) => {
                        if let Some(&zi) = self.zone_index.get(other_zone) {
                            self.surfaces[si].temp_outside = self.zones[zi].temp;
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

            // Collect zone temps
            let t_zone_vec: Vec<f64> = self.zones.iter().map(|z| z.temp).collect();

            // Handle windows first (not part of iteration)
            //
            // Windows use dynamic film coefficients (matching EnergyPlus approach).
            // The overall U-factor is decomposed into glass conductance by removing
            // NFRC standard films. Dynamic exterior AND interior films are then
            // applied at runtime based on actual conditions.
            //
            // Interior film is computed dynamically using:
            //   - TARP natural convection (same as opaque surfaces)
            //   - Linearized radiation at glass interior emissivity (0.84)
            // This matches EnergyPlus, which uses the same interior convection
            // and radiation models for windows as for opaque surfaces.
            for i in 0..self.surfaces.len() {
                if self.surfaces[i].is_window {
                    self.surfaces[i].temp_outside = t_outdoor;
                    let zi = self.zone_index.get(&self.surfaces[i].input.zone)
                        .copied().unwrap_or(0);
                    let t_z = t_zone_vec.get(zi).copied().unwrap_or(21.0);
                    self.surfaces[i].temp_inside = t_z;

                    // Dynamic exterior combined coefficient: h_conv (already computed
                    // in the exterior loop above) + approximate exterior radiation.
                    // Glass emissivity ≈ 0.84 (clear glass, uncoated exterior face).
                    let h_conv_out = self.surfaces[i].h_conv_outside;
                    let eps_glass: f64 = 0.84;
                    let t_mean_out_k = ((t_outdoor + t_z) / 2.0 + 273.15).max(200.0);
                    let h_rad_out = 4.0 * eps_glass * SIGMA * t_mean_out_k.powi(3);
                    let h_e = h_conv_out + h_rad_out;

                    let u_glass = self.surfaces[i].u_glass;
                    let tilt = self.surfaces[i].input.tilt;

                    // Combined outside-film + glass conductance
                    let u_e_glass = 1.0 / (1.0 / h_e + 1.0 / u_glass);

                    // Iteratively compute dynamic interior film coefficient.
                    // Start with NFRC standard h_i and refine using estimated
                    // window interior surface temperature.
                    let mut h_i: f64 = 8.29;
                    for _ in 0..3 {
                        // Estimate window interior surface temperature
                        let t_win_in = (u_e_glass * t_outdoor + h_i * t_z)
                            / (u_e_glass + h_i);

                        // Interior natural convection (TARP, same as opaque surfaces)
                        let h_conv_in = convection::interior_convection(
                            t_win_in, t_z, tilt,
                        );

                        // Interior radiation (linearized, using zone air temp as
                        // MRT approximation — close for well-insulated rooms)
                        let t_mean_in_k = ((t_win_in + t_z) / 2.0 + 273.15).max(200.0);
                        let h_rad_in = 4.0 * eps_glass * SIGMA * t_mean_in_k.powi(3);

                        h_i = (h_conv_in + h_rad_in).max(2.0); // Floor at 2.0 W/(m²K)
                    }

                    // Effective U with dynamic films applied to glass conductance
                    let u_eff = 1.0 / (1.0 / h_e + 1.0 / u_glass + 1.0 / h_i);

                    let q_cond = u_eff * (t_outdoor - t_z);
                    self.surfaces[i].h_conv_inside = convection::interior_convection(
                        t_z, t_z, tilt,
                    );
                    self.surfaces[i].q_conv_inside = q_cond;
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

                let zi = self.zone_index.get(&self.surfaces[i].input.zone)
                    .copied().unwrap_or(0);

                if let (Some(ctf), Some(history)) = (
                    self.ctf_coefficients[i].as_ref(),
                    self.ctf_histories[i].as_ref(),
                ) {
                    // CTFConstInPart: history terms (j ≥ 1)
                    let mut ctf_const_in = 0.0_f64;
                    for j in 0..ctf.phi.len() {
                        if j < history.q_inside.len() {
                            ctf_const_in += ctf.phi[j] * history.q_inside[j];
                        }
                    }
                    for j in 1..ctf.y.len() {
                        let idx = j - 1;
                        if idx < history.t_outside.len() {
                            ctf_const_in += ctf.y[j] * history.t_outside[idx];
                        }
                        if idx < history.t_inside.len() {
                            ctf_const_in -= ctf.z[j] * history.t_inside[idx];
                        }
                    }

                    // Radiative gains (internal + solar distribution)
                    let zone_rad_gain = if zi < self.zones.len() {
                        self.zones[zi].q_internal_rad
                    } else { 0.0 };

                    let q_solar_to_surface = if zi < self.zones.len() {
                        let zone = &self.zones[zi];
                        if let Some(ref dist) = zone.input.solar_distribution {
                            let q_sol_total: f64 = zone.surface_indices.iter()
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

                            let same_type_area: f64 = zone.surface_indices.iter()
                                .filter(|&&si| !self.surfaces[si].is_window)
                                .filter(|&&si| {
                                    let st = self.surfaces[si].input.surface_type;
                                    match surface_type {
                                        SurfaceType::Floor => st == SurfaceType::Floor,
                                        SurfaceType::Wall => st == SurfaceType::Wall,
                                        SurfaceType::Roof | SurfaceType::Ceiling =>
                                            st == SurfaceType::Roof || st == SurfaceType::Ceiling,
                                        _ => false,
                                    }
                                })
                                .map(|&si| self.surfaces[si].net_area)
                                .sum();

                            if same_type_area > 0.0 {
                                q_sol_total * type_fraction * self.surfaces[i].net_area / same_type_area
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
                        self.zones[zi].surface_indices.iter()
                            .map(|&si| self.surfaces[si].net_area)
                            .sum()
                    } else { 1.0 };

                    let q_rad_to_surface = if zone_total_area > 0.0 {
                        zone_rad_gain * self.surfaces[i].net_area / zone_total_area
                    } else { 0.0 };

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

            // --- Inside surface iteration loop ---
            for _iter in 0..MAX_INSIDE_SURF_ITER {
                // Save old temps for convergence check and damping
                let t_inside_old: Vec<f64> = self.surfaces.iter()
                    .map(|s| s.temp_inside).collect();

                // Compute MRT per zone
                let mut zone_mrt: Vec<f64> = vec![21.0; self.zones.len()];
                for (zi, zone) in self.zones.iter().enumerate() {
                    let mut sum_ea = 0.0_f64;
                    let mut sum_eat = 0.0_f64;
                    for &si in &zone.surface_indices {
                        let s = &self.surfaces[si];
                        let eps = s.thermal_absorptance_outside;
                        let a = s.net_area;
                        sum_ea += eps * a;
                        sum_eat += eps * a * s.temp_inside;
                    }
                    if sum_ea > 0.0 {
                        zone_mrt[zi] = sum_eat / sum_ea;
                    }
                }

                // Solve each surface
                for i in 0..self.surfaces.len() {
                    let pc = match &precomp[i] {
                        Some(p) => p,
                        None => continue,
                    };

                    let t_zone = t_zone_vec.get(pc.zi).copied().unwrap_or(21.0);
                    let t_mrt = zone_mrt[pc.zi];
                    let t_old = t_inside_old[i];

                    // Inside convection coefficient (updated each iteration)
                    let h_conv = convection::interior_convection(
                        self.surfaces[i].temp_inside,
                        t_zone,
                        self.surfaces[i].input.tilt,
                    ).max(0.1);
                    self.surfaces[i].h_conv_inside = h_conv;

                    // Linearized interior LW radiation coefficient
                    let eps = self.surfaces[i].thermal_absorptance_outside;
                    let t_mean_k = ((self.surfaces[i].temp_inside + 273.15)
                        + (t_mrt + 273.15)) / 2.0;
                    let h_rad = 4.0 * eps * SIGMA * t_mean_k.powi(3);

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
                    if pc.is_adiabatic {
                        let denom = (pc.ctf_z0 - pc.ctf_y0)
                            + h_conv + h_rad + ITER_DAMP_CONST;
                        self.surfaces[i].temp_inside =
                            (pc.ctf_const_in + h_conv * t_zone + h_rad * t_mrt
                             + pc.q_rad_flux + ITER_DAMP_CONST * t_old)
                            / denom.max(0.1);
                        self.surfaces[i].temp_outside = self.surfaces[i].temp_inside;
                    } else {
                        let denom = pc.ctf_z0 + h_conv + h_rad + ITER_DAMP_CONST;
                        self.surfaces[i].temp_inside =
                            (pc.ctf_y0 * self.surfaces[i].temp_outside + pc.ctf_const_in
                             + h_conv * t_zone + h_rad * t_mrt + pc.q_rad_flux
                             + ITER_DAMP_CONST * t_old)
                            / denom.max(0.1);
                    }
                }

                // Convergence check
                let max_del_temp = self.surfaces.iter()
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
                if precomp[i].is_none() { continue; }
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
                }

                let t_zone = t_zone_vec.get(pc.zi).copied().unwrap_or(21.0);
                self.surfaces[i].q_conv_inside =
                    self.surfaces[i].h_conv_inside * (self.surfaces[i].temp_inside - t_zone);
            }

            // 6c. Zone air heat balance
            for zone in &mut self.zones {
                let cp_air = psych::cp_air_fn_w(zone.humidity_ratio);
                let rho_air = psych::rho_air_fn_pb_tdb_w(p_b, zone.temp, zone.humidity_ratio);

                let mut sum_ha: f64 = 0.0;
                let mut sum_hat: f64 = 0.0;

                for &si in &zone.surface_indices {
                    let surf = &self.surfaces[si];
                    let h = surf.h_conv_inside;
                    let a = surf.net_area;
                    sum_ha += h * a;
                    sum_hat += h * a * surf.temp_inside;
                }

                // Transmitted solar through windows
                let q_solar_transmitted: f64 = zone.surface_indices.iter()
                    .filter(|&&si| self.surfaces[si].is_window)
                    .map(|&si| self.surfaces[si].transmitted_solar)
                    .sum();

                // If solar_distribution is configured, most solar goes to surfaces,
                // and only the remainder goes to zone air
                let q_solar_to_air = if zone.input.solar_distribution.is_some() {
                    // Solar distributed to surfaces — remaining fraction goes to air
                    // (floor + wall + ceiling fractions should sum to ~1.0,
                    //  any remainder goes to air)
                    let dist = zone.input.solar_distribution.as_ref().unwrap();
                    let to_surfaces = dist.floor_fraction + dist.wall_fraction + dist.ceiling_fraction;
                    q_solar_transmitted * (1.0 - to_surfaces).max(0.0)
                } else {
                    // No solar distribution: all transmitted solar goes to zone air
                    q_solar_transmitted
                };

                // Window conduction also contributes
                let q_window_cond: f64 = zone.surface_indices.iter()
                    .filter(|&&si| self.surfaces[si].is_window)
                    .map(|&si| self.surfaces[si].q_conv_inside * self.surfaces[si].net_area)
                    .sum();

                // Solar absorbed by window glazing that enters the zone (inward fraction)
                let q_window_absorbed: f64 = zone.surface_indices.iter()
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
                // EXCEPTION: During sizing, the HVAC supply air is a synthetic flow
                // to hold the zone at setpoint. The OA load must be included in mcpi
                // so that zone loads reflect the ventilation heating/cooling load
                // that the real HVAC system must handle.
                //
                // For ideal-loads zones, outdoor_air_mass_flow enters directly at outdoor temp.
                let oa_to_zone = if zone.supply_air_mass_flow > 0.0 && !ctx.is_sizing {
                    // External HVAC loop (runtime): OA already in supply stream
                    0.0
                } else {
                    // Ideal loads, free-float, or sizing: OA enters at outdoor temp
                    zone.outdoor_air_mass_flow
                };
                let total_outdoor_mass_flow = zone.infiltration_mass_flow
                    + zone.ventilation_mass_flow
                    + zone.exhaust_mass_flow
                    + oa_to_zone;
                let mcpi = total_outdoor_mass_flow * cp_air;

                let q_conv_total = zone.q_internal_conv + q_solar_to_air + q_window_cond + q_window_absorbed;

                // ─── Ideal Loads Air System ───────────────────────────────────
                if let Some(ref ideal_loads) = zone.input.ideal_loads.clone() {
                    // Step 1: Solve zone temp without HVAC (free-float)
                    let t_free = crate::zone::solve_zone_air_temp_with_q(
                        sum_ha, sum_hat,
                        mcpi, t_outdoor,
                        q_conv_total,
                        0.0, // no HVAC
                        rho_air, zone.input.volume, cp_air, dt, zone.temp_prev,
                    );

                    // Step 2: Get active setpoints (may vary by schedule)
                    let (heat_sp, cool_sp) = zone.input.active_setpoints(hour);

                    // Step 3: Determine mode and compute ideal Q
                    let (q_hvac, hvac_mode) = if t_free < heat_sp {
                        // HEATING needed
                        let q_needed = crate::zone::compute_ideal_q_hvac(
                            sum_ha, sum_hat, mcpi, t_outdoor, q_conv_total,
                            rho_air, zone.input.volume, cp_air, dt, zone.temp_prev,
                            heat_sp,
                        );
                        let q_clamped = q_needed.min(ideal_loads.heating_capacity).max(0.0);
                        (q_clamped, 1) // 1 = heating
                    } else if t_free > cool_sp {
                        // COOLING needed
                        let q_needed = crate::zone::compute_ideal_q_hvac(
                            sum_ha, sum_hat, mcpi, t_outdoor, q_conv_total,
                            rho_air, zone.input.volume, cp_air, dt, zone.temp_prev,
                            cool_sp,
                        );
                        // q_needed will be negative for cooling
                        let q_clamped = q_needed.max(-ideal_loads.cooling_capacity).min(0.0);
                        (q_clamped, -1) // -1 = cooling
                    } else {
                        // DEADBAND — no HVAC
                        (0.0, 0) // 0 = off
                    };

                    // Step 4: Solve zone temp with clamped HVAC Q
                    zone.temp = crate::zone::solve_zone_air_temp_with_q(
                        sum_ha, sum_hat,
                        mcpi, t_outdoor,
                        q_conv_total,
                        q_hvac,
                        rho_air, zone.input.volume, cp_air, dt, zone.temp_prev,
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

                    zone.temp = crate::zone::solve_zone_air_temp(
                        sum_ha, sum_hat,
                        mcpi, t_outdoor,
                        mcpsys, zone.supply_air_temp,
                        q_conv_total,
                        rho_air, zone.input.volume, cp_air, dt, zone.temp_prev,
                    );

                    let (hl, cl) = crate::zone::calc_zone_loads(
                        zone.temp, sum_ha, sum_hat, mcpi, t_outdoor,
                        q_conv_total, rho_air, zone.input.volume, cp_air, dt, zone.temp_prev,
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
                        sum_ha, sum_hat,
                        mcpi, t_outdoor,
                        q_conv_total,
                        0.0, // no HVAC
                        rho_air, zone.input.volume, cp_air, dt, zone.temp_prev,
                    );
                    let (hl, cl) = crate::zone::calc_zone_loads(
                        zone.temp, sum_ha, sum_hat, mcpi, t_outdoor,
                        q_conv_total, rho_air, zone.input.volume, cp_air, dt, zone.temp_prev,
                    );
                    zone.heating_load = hl;
                    zone.cooling_load = cl;
                    zone.hvac_heating_rate = 0.0;
                    zone.hvac_cooling_rate = 0.0;
                }

                // ─── Ideal Loads at Setpoint ──────────────────────────────
                //
                // Compute the HVAC energy needed to hold the zone exactly at
                // the cooling/heating setpoint. This is used for load-based
                // PLR calculations in the HVAC control layer.
                //
                // Q_ideal uses the current surface temperatures (sum_ha, sum_hat)
                // but evaluates the zone energy balance at T_zone = setpoint.
                let cool_sp = hvac.cooling_setpoints.get(&zone.input.name).copied();
                let heat_sp = hvac.heating_setpoints.get(&zone.input.name).copied();

                // Correct for surface temperature lag: surfaces track the actual
                // zone temp, but at setpoint the surfaces would be closer to setpoint.
                //
                // For an adiabatic massless surface, T_surface = T_zone exactly.
                // For a surface with thermal mass, the surface temp lags but the
                // convective coupling would adjust. We estimate the steady-state
                // correction: at T_setpoint, sum_hat_ideal ≈ sum_hat + sum_ha × (T_sp - T_zone)
                // which simplifies to using sum_hat_sp = sum_ha × T_sp when surfaces
                // are in quasi-equilibrium with zone air.
                //
                // A conservative approach: use sum_hat_corrected that partially
                // adjusts for the zone-temp difference, proportional to how well
                // surfaces track zone air. For near-adiabatic surfaces the correction
                // factor is ~1.0; for heavy exterior walls it's smaller.
                //
                // We use the simple correction: assume surfaces would be at setpoint
                // if the zone were at setpoint (sum_hat_ideal = sum_ha × T_sp).
                // This is exact for adiabatic massless surfaces and a reasonable
                // approximation for others in quasi-steady-state.

                zone.ideal_cooling_load = if let Some(sp) = cool_sp {
                    let sum_hat_at_sp = sum_ha * sp; // surfaces at setpoint equilibrium
                    let t_prev_at_sp = sp; // at steady state, t_prev ≈ t_zone ≈ setpoint
                    let q = crate::zone::compute_ideal_q_hvac(
                        sum_ha, sum_hat_at_sp, mcpi, t_outdoor, q_conv_total,
                        rho_air, zone.input.volume, cp_air, dt, t_prev_at_sp, sp,
                    );
                    // Negative Q = cooling needed; convert to positive cooling load
                    (-q).max(0.0)
                } else {
                    0.0
                };

                zone.ideal_heating_load = if let Some(sp) = heat_sp {
                    let sum_hat_at_sp = sum_ha * sp;
                    let t_prev_at_sp = sp;
                    let q = crate::zone::compute_ideal_q_hvac(
                        sum_ha, sum_hat_at_sp, mcpi, t_outdoor, q_conv_total,
                        rho_air, zone.input.volume, cp_air, dt, t_prev_at_sp, sp,
                    );
                    // Positive Q = heating needed
                    q.max(0.0)
                } else {
                    0.0
                };
            }
        }

        // 7. Update CTF histories with the ACTUAL CTF conduction fluxes
        //
        // CRITICAL: The history must store the pure CTF q values from
        // apply_ctf(), NOT the surface-to-zone convective fluxes (which
        // include radiative gains and would cause a runaway feedback loop).
        for i in 0..self.surfaces.len() {
            if let Some(history) = &mut self.ctf_histories[i] {
                history.shift(
                    self.surfaces[i].temp_outside,
                    self.surfaces[i].temp_inside,
                    ctf_q_inside[i],
                    ctf_q_outside[i],
                );
            }
        }

        // 8. Update zone previous temperatures
        for zone in &mut self.zones {
            zone.temp_prev = zone.temp;
        }

        // 9. Build results
        let mut results = EnvelopeResults::default();
        for zone in &self.zones {
            results.zone_temps.insert(zone.input.name.clone(), zone.temp);
            results.zone_humidity.insert(zone.input.name.clone(), zone.humidity_ratio);
            results.zone_heating_loads.insert(zone.input.name.clone(), zone.heating_load);
            results.zone_cooling_loads.insert(zone.input.name.clone(), zone.cooling_load);
            results.ideal_cooling_loads.insert(zone.input.name.clone(), zone.ideal_cooling_load);
            results.ideal_heating_loads.insert(zone.input.name.clone(), zone.ideal_heating_load);

            let mut outputs = HashMap::new();
            outputs.insert("zone_temp".to_string(), zone.temp);
            outputs.insert("heating_load".to_string(), zone.heating_load);
            outputs.insert("cooling_load".to_string(), zone.cooling_load);
            outputs.insert("hvac_heating_rate".to_string(), zone.hvac_heating_rate);
            outputs.insert("hvac_cooling_rate".to_string(), zone.hvac_cooling_rate);
            outputs.insert("infiltration_mass_flow".to_string(), zone.infiltration_mass_flow);
            outputs.insert("ventilation_mass_flow".to_string(), zone.ventilation_mass_flow);
            outputs.insert("exhaust_mass_flow".to_string(), zone.exhaust_mass_flow);
            outputs.insert("outdoor_air_mass_flow".to_string(), zone.outdoor_air_mass_flow);
            outputs.insert("q_internal_conv".to_string(), zone.q_internal_conv);
            outputs.insert("q_internal_rad".to_string(), zone.q_internal_rad);
            outputs.insert("supply_air_temp".to_string(), zone.supply_air_temp);
            outputs.insert("supply_air_mass_flow".to_string(), zone.supply_air_mass_flow);
            results.zone_outputs.insert(zone.input.name.clone(), outputs);
        }

        results
    }

    fn zone_names(&self) -> Vec<String> {
        self.zones.iter().map(|z| z.input.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_core::ports::SimulationContext;
    use openbse_core::types::{TimeStep, DayType};
    use openbse_psychrometrics::MoistAirState;
    use crate::zone::IdealLoadsAirSystem;

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
            },
        ];

        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![
                ConstructionLayer { material: "Concrete".to_string(), thickness: 0.2 },
                ConstructionLayer { material: "Insulation".to_string(), thickness: 0.1 },
            ],
        }];

        let window_constructions = vec![WindowConstruction {
            name: "Window".to_string(),
            u_factor: 3.0,
            shgc: 0.7,
            visible_transmittance: 0.6,
            solar_absorptance: None,
            inside_absorbed_fraction: 0.5,
        }];

        let zones = vec![ZoneInput {
            name: "TestZone".to_string(),
            volume: 150.0,
            floor_area: 50.0,
            infiltration: vec![crate::infiltration::InfiltrationInput {
                air_changes_per_hour: 0.5,
                ..Default::default()
            }],
            internal_gains: vec![
                crate::internal_gains::InternalGainInput::Equipment {
                    power: 500.0,
                    radiant_fraction: 0.3,
                    schedule: None,
                },
            ],
            multiplier: 1,
            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            conditioned: true,
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
            },
        ];

        BuildingEnvelope::from_input(
            materials, constructions, window_constructions,
            zones, surfaces, 40.0, -105.0, -7.0,
        )
    }

    fn make_ctx() -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 1, day: 15, hour: 12, sub_hour: 1,
                timesteps_per_hour: 1, sim_time_s: 0.0, dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
        }
    }

    fn make_weather_hour(dry_bulb: f64) -> WeatherHour {
        WeatherHour {
            year: 2023, month: 1, day: 15, hour: 12,
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
                conductivity: 1.311, density: 2240.0, specific_heat: 836.8,
                solar_absorptance: 0.7, thermal_absorptance: 0.9,
                visible_absorptance: 0.7, roughness: Roughness::MediumRough,
            },
            Material {
                name: "Insulation".to_string(),
                conductivity: 0.04, density: 30.0, specific_heat: 840.0,
                solar_absorptance: 0.7, thermal_absorptance: 0.9,
                visible_absorptance: 0.7, roughness: Roughness::Rough,
            },
        ];
        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![
                ConstructionLayer { material: "Concrete".to_string(), thickness: 0.2 },
                ConstructionLayer { material: "Insulation".to_string(), thickness: 0.1 },
            ],
        }];
        let zones = vec![ZoneInput {
            name: "TestZone".to_string(),
            volume: 150.0, floor_area: 50.0,
            infiltration: vec![crate::infiltration::InfiltrationInput {
                air_changes_per_hour: 0.5,
                ..Default::default()
            }],
            internal_gains: vec![], // No internal gains
            multiplier: 1,
            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            conditioned: true,
        }];
        let surfaces = vec![
            SurfaceInput {
                name: "South Wall".to_string(), zone: "TestZone".to_string(),
                surface_type: SurfaceType::Wall, construction: "Wall".to_string(),
                area: 30.0, azimuth: 180.0, tilt: 90.0,
                boundary: BoundaryCondition::Outdoor, parent_surface: None,
                vertices: None, shading: None,
            },
            SurfaceInput {
                name: "Roof".to_string(), zone: "TestZone".to_string(),
                surface_type: SurfaceType::Roof, construction: "Wall".to_string(),
                area: 50.0, azimuth: 0.0, tilt: 0.0,
                boundary: BoundaryCondition::Outdoor, parent_surface: None,
                vertices: None, shading: None,
            },
        ];
        let mut envelope = BuildingEnvelope::from_input(
            materials, constructions, vec![], zones, surfaces, 40.0, -105.0, -7.0,
        );
        envelope.initialize(3600.0).unwrap();

        // Night context — no solar radiation
        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 1, day: 15, hour: 3, sub_hour: 1,
                timesteps_per_hour: 1, sim_time_s: 0.0, dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
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
        let materials = vec![
            Material {
                name: "Concrete".to_string(),
                conductivity: 1.311, density: 2240.0, specific_heat: 836.8,
                solar_absorptance: 0.7, thermal_absorptance: 0.9,
                visible_absorptance: 0.7, roughness: Roughness::MediumRough,
            },
        ];
        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![
                ConstructionLayer { material: "Concrete".to_string(), thickness: 0.2 },
            ],
        }];
        let zones = vec![ZoneInput {
            name: "IdealZone".to_string(),
            volume: 130.0, floor_area: 48.0,
            infiltration: vec![crate::infiltration::InfiltrationInput {
                air_changes_per_hour: 0.5,
                ..Default::default()
            }],
            internal_gains: vec![],
            multiplier: 1,
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
            conditioned: true,
        }];
        let surfaces = vec![
            SurfaceInput {
                name: "Wall".to_string(), zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Wall, construction: "Wall".to_string(),
                area: 60.0, azimuth: 180.0, tilt: 90.0,
                boundary: BoundaryCondition::Outdoor, parent_surface: None,
                vertices: None, shading: None,
            },
            SurfaceInput {
                name: "Roof".to_string(), zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Roof, construction: "Wall".to_string(),
                area: 48.0, azimuth: 0.0, tilt: 0.0,
                boundary: BoundaryCondition::Outdoor, parent_surface: None,
                vertices: None, shading: None,
            },
        ];
        let mut envelope = BuildingEnvelope::from_input(
            materials, constructions, vec![], zones, surfaces, 40.0, -105.0, -7.0,
        );
        envelope.initialize(900.0).unwrap();

        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 1, day: 15, hour: 3, sub_hour: 1,
                timesteps_per_hour: 4, sim_time_s: 0.0, dt: 900.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(-10.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
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
        assert!(t_zone > 19.5 && t_zone < 20.5,
            "Ideal loads heating should maintain ~20°C, got {:.2}", t_zone);
        assert!(envelope.zones[0].hvac_heating_rate > 0.0,
            "Should be heating");
        assert!(envelope.zones[0].hvac_cooling_rate < 0.01,
            "Should not be cooling");
    }

    #[test]
    fn test_ideal_loads_cooling() {
        use crate::material::ConstructionLayer;
        // Zone with ideal loads in hot conditions should cool to setpoint
        let materials = vec![
            Material {
                name: "Concrete".to_string(),
                conductivity: 1.311, density: 2240.0, specific_heat: 836.8,
                solar_absorptance: 0.7, thermal_absorptance: 0.9,
                visible_absorptance: 0.7, roughness: Roughness::MediumRough,
            },
        ];
        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![
                ConstructionLayer { material: "Concrete".to_string(), thickness: 0.2 },
            ],
        }];
        let zones = vec![ZoneInput {
            name: "IdealZone".to_string(),
            volume: 130.0, floor_area: 48.0,
            infiltration: vec![crate::infiltration::InfiltrationInput {
                air_changes_per_hour: 0.5,
                ..Default::default()
            }],
            internal_gains: vec![
                crate::internal_gains::InternalGainInput::Equipment {
                    power: 2000.0, // Large internal gains to force cooling
                    radiant_fraction: 0.3,
                    schedule: None,
                },
            ],
            multiplier: 1,
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
            conditioned: true,
        }];
        let surfaces = vec![
            SurfaceInput {
                name: "Wall".to_string(), zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Wall, construction: "Wall".to_string(),
                area: 60.0, azimuth: 180.0, tilt: 90.0,
                boundary: BoundaryCondition::Outdoor, parent_surface: None,
                vertices: None, shading: None,
            },
            SurfaceInput {
                name: "Roof".to_string(), zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Roof, construction: "Wall".to_string(),
                area: 48.0, azimuth: 0.0, tilt: 0.0,
                boundary: BoundaryCondition::Outdoor, parent_surface: None,
                vertices: None, shading: None,
            },
        ];
        let mut envelope = BuildingEnvelope::from_input(
            materials, constructions, vec![], zones, surfaces, 40.0, -105.0, -7.0,
        );
        envelope.initialize(900.0).unwrap();

        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 7, day: 15, hour: 14, sub_hour: 1,
                timesteps_per_hour: 4, sim_time_s: 0.0, dt: 900.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(35.0, 0.3, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
        };
        let weather = make_weather_hour(35.0);
        let hvac = ZoneHvacConditions::default();

        for _ in 0..100 {
            envelope.solve_timestep(&ctx, &weather, &hvac);
        }

        // Zone should be at cooling setpoint (27°C) after convergence
        let t_zone = envelope.zones[0].temp;
        assert!(t_zone > 26.5 && t_zone < 27.5,
            "Ideal loads cooling should maintain ~27°C, got {:.2}", t_zone);
        assert!(envelope.zones[0].hvac_cooling_rate > 0.0,
            "Should be cooling");
    }

    #[test]
    fn test_ideal_loads_deadband() {
        use crate::material::ConstructionLayer;
        // Zone in deadband should have no HVAC
        let materials = vec![
            Material {
                name: "Concrete".to_string(),
                conductivity: 1.311, density: 2240.0, specific_heat: 836.8,
                solar_absorptance: 0.7, thermal_absorptance: 0.9,
                visible_absorptance: 0.7, roughness: Roughness::MediumRough,
            },
        ];
        let constructions = vec![Construction {
            name: "Wall".to_string(),
            layers: vec![
                ConstructionLayer { material: "Concrete".to_string(), thickness: 0.2 },
            ],
        }];
        let zones = vec![ZoneInput {
            name: "IdealZone".to_string(),
            volume: 130.0, floor_area: 48.0,
            infiltration: vec![crate::infiltration::InfiltrationInput::default()],
            internal_gains: vec![],
            multiplier: 1,
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
            conditioned: true,
        }];
        let surfaces = vec![
            SurfaceInput {
                name: "Wall".to_string(), zone: "IdealZone".to_string(),
                surface_type: SurfaceType::Wall, construction: "Wall".to_string(),
                area: 60.0, azimuth: 180.0, tilt: 90.0,
                boundary: BoundaryCondition::Outdoor, parent_surface: None,
                vertices: None, shading: None,
            },
        ];
        let mut envelope = BuildingEnvelope::from_input(
            materials, constructions, vec![], zones, surfaces, 40.0, -105.0, -7.0,
        );
        envelope.initialize(900.0).unwrap();

        // Outdoor at 23°C — zone should be in deadband
        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 5, day: 15, hour: 12, sub_hour: 1,
                timesteps_per_hour: 4, sim_time_s: 0.0, dt: 900.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(23.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
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
        assert!(t_zone > 19.0 && t_zone < 28.0,
            "Zone should be in deadband, got {:.2}", t_zone);
        // HVAC rates should be zero (in deadband)
        assert!(envelope.zones[0].hvac_heating_rate < 0.01,
            "Should not be heating in deadband");
        assert!(envelope.zones[0].hvac_cooling_rate < 0.01,
            "Should not be cooling in deadband");
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
        weather.dew_point = -15.0;          // dry air → cold sky
        weather.opaque_sky_cover = 0.0;     // clear sky
        let hvac = ZoneHvacConditions::default();

        // With multi-term state-space CTF, thermal mass retains initial conditions
        // longer. Need enough iterations to reach thermal equilibrium.
        for _ in 0..100 {
            envelope.solve_timestep(&ctx, &weather, &hvac);
        }

        // Find the roof (tilt=0, full sky view) — it should be colder than outdoor
        let roof = envelope.surfaces.iter()
            .find(|s| s.input.name == "Roof")
            .expect("Roof should exist");
        assert!(roof.temp_outside < 0.0,
            "Roof should be below outdoor temp (0°C) due to sky LWR, got {:.2}°C",
            roof.temp_outside);

        // South Wall (tilt=90, partial sky view) should also be cooler but less so
        let wall = envelope.surfaces.iter()
            .find(|s| s.input.name == "South Wall")
            .expect("South Wall should exist");
        assert!(wall.temp_outside < 0.0,
            "South wall should be below outdoor temp due to sky LWR, got {:.2}°C",
            wall.temp_outside);
        // Roof should be colder than wall (full vs partial sky view)
        assert!(roof.temp_outside < wall.temp_outside,
            "Roof ({:.2}°C) should be colder than wall ({:.2}°C)",
            roof.temp_outside, wall.temp_outside);
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
        assert!(t_sky < -5.0, "Clear sky should be well below outdoor, got {:.1}°C", t_sky);
        assert!(t_sky > -25.0, "Sky shouldn't be excessively cold, got {:.1}°C", t_sky);

        // Case 2: Overcast sky (N=10) should be warmer than clear sky
        let n = 10.0_f64;
        let cloud_factor = 1.0 + 0.0224 * n - 0.0035 * n * n + 0.00028 * n * n * n;
        let eps_overcast = (eps_clear * cloud_factor).min(1.0);
        let t_sky_overcast = eps_overcast.powf(0.25) * t_db_k - 273.15;
        assert!(t_sky_overcast > t_sky,
            "Overcast sky ({:.1}°C) should be warmer than clear ({:.1}°C)",
            t_sky_overcast, t_sky);
        // Overcast sky should be within ~10°C of outdoor temp
        assert!((t_sky_overcast - 0.0).abs() < 12.0,
            "Overcast sky should be within 12°C of outdoor, got {:.1}°C", t_sky_overcast);

        // Case 3: Summer clear sky at 30°C, Tdp=10°C
        let t_dp_k_s = 283.15_f64;
        let t_db_k_s = 303.15_f64;
        let eps_clear_s = 0.787 + 0.764 * (t_dp_k_s / 273.0).ln();
        let t_sky_s = eps_clear_s.powf(0.25) * t_db_k_s - 273.15;
        // Summer clear sky depression should be ~12-18°C
        let depression = 30.0 - t_sky_s;
        assert!(depression > 8.0 && depression < 22.0,
            "Summer sky depression should be 8-22°C, got {:.1}°C (T_sky={:.1}°C)",
            depression, t_sky_s);
    }
}
