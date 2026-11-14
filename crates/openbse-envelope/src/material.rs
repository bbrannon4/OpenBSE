//! Material and construction definitions for building envelope surfaces.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Surface roughness classification (affects exterior convection).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Roughness {
    VeryRough,
    Rough,
    #[default]
    MediumRough,
    MediumSmooth,
    Smooth,
    VerySmooth,
}

/// Opaque material layer properties.
///
/// Materials define thermophysical properties only. Thickness is specified
/// on the construction layer, not on the material (IES-VE convention).
///
/// For NoMass materials (matching EnergyPlus `Material:NoMass`), set
/// `thermal_resistance` to the material's R-value [m²·K/W]. The material
/// will have zero thermal mass — only its resistance matters. The
/// `conductivity`, `density`, and `specific_heat` fields are ignored.
///
/// For every other material those three fields are required, as in E+'s
/// `Material`. An omitted one parses as 0 and `validate_model` rejects it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub name: String,
    /// Thermal conductivity [W/(m·K)].
    /// Ignored for NoMass materials (when `thermal_resistance` is set).
    #[serde(default)]
    pub conductivity: f64,
    /// Density [kg/m³].
    /// Ignored for NoMass materials (when `thermal_resistance` is set).
    #[serde(default)]
    pub density: f64,
    /// Specific heat [J/(kg·K)].
    /// Ignored for NoMass materials (when `thermal_resistance` is set).
    #[serde(default)]
    pub specific_heat: f64,
    /// Solar absorptance [0-1]
    #[serde(default = "default_solar_absorptance")]
    pub solar_absorptance: f64,
    /// Thermal (LW) absorptance [0-1]
    #[serde(default = "default_thermal_absorptance")]
    pub thermal_absorptance: f64,
    /// Visible absorptance [0-1]
    #[serde(default = "default_visible_absorptance")]
    pub visible_absorptance: f64,
    /// Surface roughness
    #[serde(default)]
    pub roughness: Roughness,
    /// Thermal resistance for NoMass materials [m²·K/W].
    ///
    /// When specified, this material has zero thermal mass — only resistance.
    /// `conductivity`, `density`, and `specific_heat` are ignored.
    /// Matches EnergyPlus `Material:NoMass`.
    ///
    /// In the CTF state-space, NoMass layers are NOT given finite-difference
    /// nodes. Their resistance is added to the boundary conductance between
    /// the surface and the first massed node.
    #[serde(default)]
    pub thermal_resistance: Option<f64>,

    // ── HAMT moisture properties (optional) ───────────────────────────────
    /// Vapor resistance factor μ [-] (dimensionless).
    ///
    /// Relates material vapor diffusion resistance to still air.
    /// μ = 1 for air (no extra resistance), typical values:
    ///   concrete ~130, brick ~10, wood ~40, mineral wool ~1.
    /// For the HAMT solver, which is not wired into the run (#64): no effect today.
    #[serde(default)]
    pub vapor_resistance_factor: Option<f64>,

    /// Sorption isotherm: pairs of [relative_humidity [0-1], moisture_content [kg/kg]].
    ///
    /// Defines equilibrium moisture content at each RH level.
    /// For the HAMT solver, which is not wired into the run (#64): no effect today.
    #[serde(default)]
    pub sorption_isotherm: Option<Vec<[f64; 2]>>,

    /// Liquid transport coefficient [m²/s] as f(moisture_content [kg/kg]).
    ///
    /// Pairs of [w_c [kg/kg], D_w [m²/s]]. Used for capillary conduction term.
    /// For the HAMT solver, which is not wired into the run (#64): no effect today.
    #[serde(default)]
    pub liquid_transport_coeff: Option<Vec<[f64; 2]>>,

    /// Inside thermal absorptance (emissivity) [0-1].
    ///
    /// Emissivity of this material's face when it is a construction's inside
    /// layer. When absent, `thermal_absorptance` applies to both faces.
    #[serde(default)]
    pub thermal_absorptance_inside: Option<f64>,
}

fn default_solar_absorptance() -> f64 {
    0.7
}
fn default_thermal_absorptance() -> f64 {
    0.9
}
fn default_visible_absorptance() -> f64 {
    0.7
}

impl Material {
    /// Thermal diffusivity [m²/s].
    pub fn diffusivity(&self) -> f64 {
        self.conductivity / (self.density * self.specific_heat)
    }
}

/// Thermal resistance of a material at a given thickness [m²·K/W].
pub fn layer_resistance(mat: &Material, thickness: f64) -> f64 {
    thickness / mat.conductivity
}

/// A construction layer — pairs a material with its thickness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstructionLayer {
    /// Material name (references a Material definition)
    pub material: String,
    /// Layer thickness [m]
    pub thickness: f64,
}

/// A resolved layer with all properties needed for CTF calculation.
/// Created by looking up the material and combining with layer thickness.
#[derive(Debug, Clone)]
pub struct ResolvedLayer {
    pub conductivity: f64,
    pub density: f64,
    pub specific_heat: f64,
    pub thickness: f64,
    /// If true, this layer has zero thermal mass (NoMass).
    /// Only its resistance matters; no state-space nodes are created in the CTF.
    /// Matches EnergyPlus `Material:NoMass` behavior.
    pub no_mass: bool,
}

impl ResolvedLayer {
    /// Create a massed layer from physical properties.
    pub fn new(conductivity: f64, density: f64, specific_heat: f64, thickness: f64) -> Self {
        Self {
            conductivity,
            density,
            specific_heat,
            thickness,
            no_mass: false,
        }
    }

    /// Create a NoMass (resistance-only) layer.
    ///
    /// The layer has the given thermal resistance and geometric thickness,
    /// but zero thermal mass. In the CTF state-space, it contributes only
    /// resistance to the boundary conductance — no finite-difference nodes.
    ///
    /// Matches EnergyPlus `Material:NoMass` behavior.
    pub fn new_no_mass(thermal_resistance: f64, thickness: f64) -> Self {
        let effective_k = if thermal_resistance > 0.0 && thickness > 0.0 {
            thickness / thermal_resistance
        } else {
            1.0
        };
        Self {
            conductivity: effective_k,
            density: 0.0,
            specific_heat: 0.0,
            thickness,
            no_mass: true,
        }
    }

    /// Create from a material reference and thickness.
    ///
    /// If the material has `thermal_resistance` set, creates a NoMass layer
    /// with zero thermal mass and the specified R-value.
    pub fn from_material(mat: &Material, thickness: f64) -> Self {
        if let Some(r) = mat.thermal_resistance {
            Self::new_no_mass(r, thickness)
        } else {
            Self::new(mat.conductivity, mat.density, mat.specific_heat, thickness)
        }
    }

    /// Thermal resistance [m²·K/W].
    pub fn resistance(&self) -> f64 {
        self.thickness / self.conductivity
    }

    /// Thermal diffusivity [m²/s].
    /// Panics if called on a NoMass layer (density and specific_heat are zero).
    pub fn diffusivity(&self) -> f64 {
        debug_assert!(!self.no_mass, "diffusivity() called on NoMass layer");
        self.conductivity / (self.density * self.specific_heat)
    }

    /// Thermal capacitance per unit area [J/(m²·K)].
    /// Returns 0 for NoMass layers.
    pub fn capacitance(&self) -> f64 {
        self.density * self.specific_heat * self.thickness
    }
}

/// A multi-layer opaque construction (outside to inside).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Construction {
    pub name: String,
    /// Layers ordered outside to inside, each pairing a material name with thickness
    pub layers: Vec<ConstructionLayer>,
}

impl Construction {
    /// Resolve all layers against a material map, returning ResolvedLayers.
    ///
    /// An undefined material or an empty layer list is an error: skipping the
    /// layer would run the construction with a higher U and no warning.
    pub fn resolve_layers(
        &self,
        materials: &HashMap<String, Material>,
    ) -> Result<Vec<ResolvedLayer>, String> {
        if self.layers.is_empty() {
            return Err(format!("Construction '{}' has no layers", self.name));
        }
        self.layers
            .iter()
            .map(|cl| {
                materials
                    .get(&cl.material)
                    .map(|mat| ResolvedLayer::from_material(mat, cl.thickness))
                    .ok_or_else(|| {
                        format!(
                            "Construction '{}' uses undefined material '{}'",
                            self.name, cl.material
                        )
                    })
            })
            .collect()
    }

    /// Total R-value given a material map [m²·K/W].
    pub fn total_resistance(&self, materials: &HashMap<String, Material>) -> Result<f64, String> {
        Ok(self
            .resolve_layers(materials)?
            .iter()
            .map(|l| l.resistance())
            .sum())
    }

    /// Total U-factor [W/(m²·K)] (no film coefficients).
    pub fn u_factor(&self, materials: &HashMap<String, Material>) -> Result<f64, String> {
        let r = self.total_resistance(materials)?;
        if r > 0.0 {
            Ok(1.0 / r)
        } else {
            Err(format!("Construction '{}' has no resistance", self.name))
        }
    }

    /// Get the outside (first) material name, if any.
    pub fn outside_material(&self) -> Option<&str> {
        self.layers.first().map(|l| l.material.as_str())
    }

    /// Get the inside (last) material name, if any.
    pub fn inside_material(&self) -> Option<&str> {
        self.layers.last().map(|l| l.material.as_str())
    }
}

/// Which condition triggers blind deployment (#104).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub enum ShadingControlKind {
    /// Deploy when incident beam irradiance on the glazing exceeds `threshold` [W/m²].
    SolarThreshold,
    /// Deploy when zone air temperature exceeds `setpoint` [°C].
    Temperature,
}

/// Dynamic blind / interior shading control attached to a window construction (#104).
///
/// YAML example (solar-triggered, assembly-rating blind):
/// ```yaml
/// shading_control:
///   type: solar_threshold
///   threshold: 200     # W/m² incident beam on glazing
///   hysteresis: 50     # retract at (threshold - hysteresis) W/m²
///   blind_shgc: 0.05
///   blind_u_factor: 1.8
/// ```
///
/// With a `shade:` block the shade is modelled as its own layer (#144) and
/// `blind_shgc` / `blind_u_factor` are not used:
/// ```yaml
/// shading_control:
///   type: solar_threshold
///   threshold: 200
///   shade:
///     solar_transmittance: 0.10
///     solar_reflectance: 0.60
///     openness: 0.03
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadingControl {
    /// Control trigger type.
    #[serde(rename = "type")]
    pub kind: ShadingControlKind,
    /// Trigger threshold — irradiance [W/m²] for `solar_threshold`, temperature [°C] for `temperature`.
    #[serde(default)]
    pub threshold: Option<f64>,
    /// Same as `threshold` but for temperature control; alias accepted.
    #[serde(default)]
    pub setpoint: Option<f64>,
    /// Dead-band width — blind retracts when signal < (trigger − hysteresis). Default 0.
    #[serde(default)]
    pub hysteresis: f64,
    /// SHGC of the window+blind assembly when blind is deployed. Required
    /// unless `shade` is given.
    #[serde(default)]
    pub blind_shgc: Option<f64>,
    /// Overall U-factor [W/(m²·K)] of the window+blind assembly when deployed.
    /// Defaults to the base window U-factor when absent.
    #[serde(default)]
    pub blind_u_factor: Option<f64>,
    /// Physical interior shade layer (#144). When present it replaces the
    /// `blind_shgc` / `blind_u_factor` assembly rating.
    #[serde(default)]
    pub shade: Option<InteriorShade>,
}

/// Optical and thermal properties of an interior shade (e.g. a roller shade),
/// modelled as a layer on the room side of the glazing.
///
/// Matches the inputs of EnergyPlus `WindowMaterial:Shade` that matter for a
/// thin fabric: solar properties are hemispherical, the fabric is treated as
/// opaque to long-wave radiation, and its own conduction resistance is
/// neglected (a fabric of ~1 mm has R ≈ 0.01 m²·K/W).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteriorShade {
    /// Total solar transmittance, including the openness [0–1].
    pub solar_transmittance: f64,
    /// Solar reflectance (both faces) [0–1].
    pub solar_reflectance: f64,
    /// Fraction of beam that passes straight through the weave as beam
    /// [0–1]; the rest of the transmitted solar is diffuse.
    #[serde(default)]
    pub openness: f64,
    /// Long-wave emissivity (both faces) [0–1].
    #[serde(default = "default_shade_emissivity")]
    pub emissivity: f64,
    /// Distance between the glass and the shade [m]. Sets how freely room air
    /// circulates through the gap.
    #[serde(default = "default_shade_distance")]
    pub distance_to_glass: f64,
}

fn default_shade_emissivity() -> f64 {
    0.9
}

fn default_shade_distance() -> f64 {
    0.05
}

impl InteriorShade {
    /// Solar absorptance, 1 − τ − ρ.
    pub fn solar_absorptance(&self) -> f64 {
        (1.0 - self.solar_transmittance - self.solar_reflectance).max(0.0)
    }

    /// Problems with the properties, for input validation.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let unit = |name: &str, v: f64, out: &mut Vec<String>| {
            if !(0.0..=1.0).contains(&v) {
                out.push(format!("{name} must be between 0 and 1, got {v}"));
            }
        };
        unit("solar_transmittance", self.solar_transmittance, &mut out);
        unit("solar_reflectance", self.solar_reflectance, &mut out);
        unit("openness", self.openness, &mut out);
        unit("emissivity", self.emissivity, &mut out);
        if self.solar_transmittance + self.solar_reflectance > 1.0 {
            out.push("solar_transmittance + solar_reflectance must not exceed 1".to_string());
        }
        if self.openness > self.solar_transmittance {
            out.push("openness is part of solar_transmittance and cannot exceed it".to_string());
        }
        if self.emissivity <= 0.0 {
            out.push("emissivity must be above 0".to_string());
        }
        if self.distance_to_glass.is_nan() || self.distance_to_glass <= 0.0 {
            out.push(format!(
                "distance_to_glass must be positive, got {}",
                self.distance_to_glass
            ));
        }
        out
    }
}

impl ShadingControl {
    /// Trigger level (irradiance or temperature depending on `kind`).
    pub fn trigger_level(&self) -> f64 {
        match self.kind {
            ShadingControlKind::SolarThreshold => self.threshold.unwrap_or(200.0),
            ShadingControlKind::Temperature => self.setpoint.or(self.threshold).unwrap_or(26.0),
        }
    }

    /// Retract level = trigger − hysteresis.
    pub fn retract_level(&self) -> f64 {
        self.trigger_level() - self.hysteresis.max(0.0)
    }
}

/// A glass layer of a window, with spectral-average optical properties at
/// normal incidence, after EnergyPlus `WindowMaterial:Glazing`
/// (`SpectralAverage`). "Front" faces the outdoors.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlazingMaterial {
    pub name: String,
    /// Thickness [m]
    pub thickness: f64,
    /// Conductivity [W/(m·K)] (default 0.9, as EnergyPlus)
    #[serde(default = "default_glass_conductivity")]
    pub conductivity: f64,
    /// Solar transmittance at normal incidence [0-1]
    pub solar_transmittance: f64,
    /// Front (outdoor-side) solar reflectance at normal incidence [0-1]
    pub solar_reflectance_front: f64,
    /// Back (room-side) solar reflectance at normal incidence [0-1]
    pub solar_reflectance_back: f64,
    /// Visible transmittance at normal incidence [0-1] (default: the solar
    /// value). Visible properties only set the reported visible transmittance.
    #[serde(default)]
    pub visible_transmittance: Option<f64>,
    /// Front visible reflectance [0-1] (default: the front solar value)
    #[serde(default)]
    pub visible_reflectance_front: Option<f64>,
    /// Back visible reflectance [0-1] (default: the back solar value)
    #[serde(default)]
    pub visible_reflectance_back: Option<f64>,
    /// Long-wave transmittance (default 0). Glass is modelled opaque to
    /// long-wave radiation, so any other value is an error.
    #[serde(default)]
    pub infrared_transmittance: f64,
    /// Front long-wave hemispherical emissivity (default 0.84)
    #[serde(default = "default_glass_emissivity")]
    pub emissivity_front: f64,
    /// Back long-wave hemispherical emissivity (default 0.84)
    #[serde(default = "default_glass_emissivity")]
    pub emissivity_back: f64,
    /// Factor on the solar and visible transmittance for dirt, applied when
    /// the layer is a window's outermost (default 1, clean)
    #[serde(default = "default_dirt_factor")]
    pub dirt_correction_factor: f64,
}

fn default_glass_conductivity() -> f64 {
    0.9
}
fn default_glass_emissivity() -> f64 {
    0.84
}
fn default_dirt_factor() -> f64 {
    1.0
}

impl GlazingMaterial {
    /// The glazing layer for the window model.
    pub fn layer(&self) -> crate::glazing::GlassLayer {
        crate::glazing::GlassLayer {
            thickness: self.thickness,
            conductivity: self.conductivity,
            solar_transmittance: self.solar_transmittance,
            solar_reflectance_front: self.solar_reflectance_front,
            solar_reflectance_back: self.solar_reflectance_back,
            visible_transmittance: self
                .visible_transmittance
                .unwrap_or(self.solar_transmittance),
            visible_reflectance_front: self
                .visible_reflectance_front
                .unwrap_or(self.solar_reflectance_front),
            visible_reflectance_back: self
                .visible_reflectance_back
                .unwrap_or(self.solar_reflectance_back),
            emissivity_front: self.emissivity_front,
            emissivity_back: self.emissivity_back,
            dirt_factor: self.dirt_correction_factor,
        }
    }

    /// Inputs outside their physical range.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.thickness <= 0.0 {
            p.push("thickness must be positive".to_string());
        }
        if self.conductivity <= 0.0 {
            p.push("conductivity must be positive".to_string());
        }
        let unit = |name: &str, v: f64, p: &mut Vec<String>| {
            if !(0.0..=1.0).contains(&v) {
                p.push(format!("{name} {v} must lie in [0, 1]"));
            }
        };
        unit("solar_transmittance", self.solar_transmittance, &mut p);
        unit(
            "solar_reflectance_front",
            self.solar_reflectance_front,
            &mut p,
        );
        unit(
            "solar_reflectance_back",
            self.solar_reflectance_back,
            &mut p,
        );
        let vis = self.layer();
        unit("visible_transmittance", vis.visible_transmittance, &mut p);
        unit(
            "visible_reflectance_front",
            vis.visible_reflectance_front,
            &mut p,
        );
        unit(
            "visible_reflectance_back",
            vis.visible_reflectance_back,
            &mut p,
        );
        let mut sums = vec![(
            "solar",
            self.solar_transmittance,
            self.solar_reflectance_front,
            self.solar_reflectance_back,
        )];
        let visible_given = self.visible_transmittance.is_some()
            || self.visible_reflectance_front.is_some()
            || self.visible_reflectance_back.is_some();
        if visible_given {
            sums.push((
                "visible",
                vis.visible_transmittance,
                vis.visible_reflectance_front,
                vis.visible_reflectance_back,
            ));
        }
        for (what, t, rf, rb) in sums {
            if t + rf.max(rb) > 1.0 {
                p.push(format!(
                    "{what} transmittance plus reflectance exceeds 1 on a face"
                ));
            }
        }
        if !(self.emissivity_front > 0.0 && self.emissivity_front <= 1.0) {
            p.push("emissivity_front must lie in (0, 1]".to_string());
        }
        if !(self.emissivity_back > 0.0 && self.emissivity_back <= 1.0) {
            p.push("emissivity_back must lie in (0, 1]".to_string());
        }
        if self.infrared_transmittance != 0.0 {
            p.push(
                "infrared_transmittance must be 0: glass layers are modelled opaque to \
                 long-wave radiation"
                    .to_string(),
            );
        }
        if !(self.dirt_correction_factor > 0.0 && self.dirt_correction_factor <= 1.0) {
            p.push("dirt_correction_factor must lie in (0, 1]".to_string());
        }
        p
    }
}

/// A fill gas of a window gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapGas {
    Air,
    Argon,
    Krypton,
    Xenon,
}

impl GapGas {
    pub fn gas(self) -> crate::glazing::Gas {
        use crate::glazing::{AIR, ARGON, KRYPTON, XENON};
        match self {
            GapGas::Air => AIR,
            GapGas::Argon => ARGON,
            GapGas::Krypton => KRYPTON,
            GapGas::Xenon => XENON,
        }
    }
}

/// One gas of a gap's mixture, by mole fraction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasFraction {
    pub gas: GapGas,
    pub fraction: f64,
}

/// A layer of a window construction: a glass layer (`glazing`, naming a
/// glazing material) or a gas gap (`gas`, or `gas_mixture`, with `width`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowLayer {
    #[serde(default)]
    pub glazing: Option<String>,
    #[serde(default)]
    pub gas: Option<GapGas>,
    #[serde(default)]
    pub gas_mixture: Option<Vec<GasFraction>>,
    /// Gap width [m]
    #[serde(default)]
    pub width: Option<f64>,
}

/// Window construction.
///
/// Either a simple glazing system, described by its NFRC U-factor and SHGC
/// and modelled as EnergyPlus's equivalent single glass layer, or a
/// layer-by-layer glazing system: `layers`, outside to inside, alternating
/// glass layers and gas gaps, with one to four glass layers. A layered
/// construction's U-factor and SHGC follow from its layers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowConstruction {
    pub name: String,
    /// NFRC U-factor including film coefficients [W/(m²·K)] (simple glazing)
    #[serde(default)]
    pub u_factor: Option<f64>,
    /// Solar heat gain coefficient at normal incidence [0-1] (simple glazing)
    #[serde(default)]
    pub shgc: Option<f64>,
    /// Visible transmittance at normal incidence [0-1] (simple glazing,
    /// optional; default the solar transmittance)
    #[serde(default)]
    pub visible_transmittance: Option<f64>,
    /// Glass layers and gas gaps, outside to inside (layered glazing)
    #[serde(default)]
    pub layers: Vec<WindowLayer>,
    /// Retired pane fields: describe the panes as `layers` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_solar_transmittance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_solar_reflectance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_panes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_conductivity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_thickness: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glass_emissivity: Option<f64>,
    /// Dynamic interior shading control (#104). When present, blind state is
    /// evaluated each timestep and the effective SHGC/U are updated accordingly.
    #[serde(default)]
    pub shading_control: Option<ShadingControl>,
    /// The glass layers and gaps of `layers`, resolved against the model's
    /// glazing materials (`resolve_layers`).
    #[serde(skip)]
    pub resolved_layers: Option<(Vec<crate::glazing::GlassLayer>, Vec<crate::glazing::Gap>)>,
}

impl WindowConstruction {
    /// A simple glazing construction.
    pub fn simple(name: &str, u_factor: f64, shgc: f64) -> Self {
        Self {
            name: name.to_string(),
            u_factor: Some(u_factor),
            shgc: Some(shgc),
            visible_transmittance: None,
            layers: Vec::new(),
            pane_solar_transmittance: None,
            pane_solar_reflectance: None,
            num_panes: None,
            gap_width: None,
            pane_conductivity: None,
            pane_thickness: None,
            glass_emissivity: None,
            shading_control: None,
            resolved_layers: None,
        }
    }

    /// Whether any retired pane field is given.
    pub fn has_retired_pane_fields(&self) -> bool {
        self.pane_solar_transmittance.is_some()
            || self.pane_solar_reflectance.is_some()
            || self.num_panes.is_some()
            || self.gap_width.is_some()
            || self.pane_conductivity.is_some()
            || self.pane_thickness.is_some()
            || self.glass_emissivity.is_some()
    }

    /// Resolves `layers` against the glazing materials. Unknown names and
    /// malformed layers leave the construction unresolved; validation
    /// reports them.
    pub fn resolve_layers(&mut self, materials: &HashMap<String, GlazingMaterial>) {
        use crate::glazing::{Gap, GasFill};
        self.resolved_layers = None;
        if self.layers.is_empty() {
            return;
        }
        let mut glass = Vec::new();
        let mut gaps = Vec::new();
        for (i, l) in self.layers.iter().enumerate() {
            if i % 2 == 0 {
                match l.glazing.as_ref().and_then(|n| materials.get(n)) {
                    Some(m) => glass.push(m.layer()),
                    None => return,
                }
            } else {
                let fill = match (&l.gas, &l.gas_mixture) {
                    (Some(g), None) => GasFill::single(g.gas()),
                    (None, Some(mix)) if !mix.is_empty() => GasFill {
                        parts: mix.iter().map(|f| (f.gas.gas(), f.fraction)).collect(),
                    },
                    _ => return,
                };
                match l.width {
                    Some(w) if w > 0.0 => gaps.push(Gap { width: w, fill }),
                    _ => return,
                }
            }
        }
        if glass.len() != gaps.len() + 1 || glass.len() > crate::glazing::MAX_LAYERS {
            return;
        }
        self.resolved_layers = Some((glass, gaps));
    }

    /// The glazing system: the resolved layers, or the simple glazing
    /// equivalent layer. Validation rejects a construction that is neither
    /// before a model runs.
    pub fn glazing(&self) -> crate::glazing::Glazing {
        use crate::glazing::Glazing;
        match &self.resolved_layers {
            Some((glass, gaps)) => Glazing::layered(glass.clone(), gaps.clone()),
            None => Glazing::simple(
                self.u_factor.unwrap_or(3.0),
                self.shgc.unwrap_or(0.7),
                self.visible_transmittance,
            ),
        }
    }
}

/// Simplified opaque construction defined by overall properties.
///
/// Use when you don't want to specify individual material layers —
/// just give the overall U-factor and thermal capacity.
/// Ideal for early design, ASHRAE 140 tests, and quick parametric studies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimpleConstruction {
    pub name: String,
    /// Overall U-factor [W/(m²·K)] (conductance, no film coefficients)
    pub u_factor: f64,
    /// Thermal capacity per unit area [J/(m²·K)] = Σ(ρ·cp·thickness)
    #[serde(default = "default_thermal_capacity")]
    pub thermal_capacity: f64,
    /// Outside solar absorptance [0-1]
    #[serde(default = "default_solar_absorptance")]
    pub solar_absorptance: f64,
    /// Thermal (LW) absorptance [0-1]
    #[serde(default = "default_thermal_absorptance")]
    pub thermal_absorptance: f64,
    /// Surface roughness
    #[serde(default)]
    pub roughness: Roughness,
    /// Whether the thermal mass layer is on the outside of the insulation.
    /// Default false (mass on inside). Set true for constructions like
    /// ASHRAE 140 Case 900 where concrete block is on the exterior.
    #[serde(default)]
    pub mass_outside: bool,
    /// Thermal conductivity of the mass layer [W/(m·K)].
    /// Optional — defaults to 1.0 (generic concrete). Set to 0.51 for
    /// concrete block (CMU), 1.4 for poured concrete, etc. Only applies
    /// to heavyweight constructions (thermal_capacity ≥ 30 kJ/m²K).
    pub mass_conductivity: Option<f64>,
    /// Density of the mass layer [kg/m³].
    /// Optional — defaults to 2000 (generic concrete). Set to 1400 for
    /// concrete block (CMU), 2300 for poured concrete, etc. Only applies
    /// to heavyweight constructions (thermal_capacity ≥ 30 kJ/m²K).
    pub mass_density: Option<f64>,
}

fn default_thermal_capacity() -> f64 {
    50000.0
} // light construction ~50 kJ/(m²·K)

/// F-factor ground floor construction.
///
/// Models ground-contact floor heat loss using the ASHRAE F-factor method:
///     Q = F × P × (T_zone - T_ground)
/// where F is the slab edge heat loss coefficient [W/(m·K)] and P is the
/// exposed perimeter [m] (specified per surface, not per construction).
///
/// This is the standard approach for slab-on-grade and basement floors in
/// ASHRAE 90.1 and EnergyPlus `Construction:FfactorGroundFloor`.
///
/// ```yaml
/// f_factor_constructions:
///   - name: Ground Floor
///     f_factor: 1.264           # W/(m·K)
///     thermal_capacity: 400000  # J/(m²·K), concrete slab mass
///     ground_temperatures: [7.1, 3.0, -1.0, 0.8, -0.2, 4.8, 6.1, 13.7, 22.2, 22.7, 21.7, 18.5]
/// ```
///
/// Each surface using this construction must specify `exposed_perimeter` [m].
/// The engine converts to an effective U-factor per surface:
///     U_eff = F × P / A
/// and uses the construction's ground temperatures for the soil boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FFactorConstruction {
    pub name: String,
    /// F-factor (slab edge heat loss coefficient) [W/(m·K)]
    pub f_factor: f64,
    /// Inside solar absorptance [0-1] (floor surface)
    #[serde(default = "default_solar_absorptance")]
    pub solar_absorptance: f64,
    /// Thermal (LW) absorptance [0-1]
    #[serde(default = "default_thermal_absorptance")]
    pub thermal_absorptance: f64,
    /// Monthly ground temperatures [°C], January through December.
    /// Matches EnergyPlus `Site:GroundTemperature:FCfactorMethod`.
    /// If omitted, uses the building-level ground temperature model.
    #[serde(default)]
    pub ground_temperatures: Option<Vec<f64>>,
}

/// Fixed concrete layer of an F-factor floor (E+ `~FC_CONCRETE`):
/// k [W/(m·K)], ρ [kg/m³], cp [J/(kg·K)], thickness [m].
const FC_CONCRETE: (f64, f64, f64, f64) = (1.95, 2240.0, 900.0, 0.15);
/// Air films the F-factor already includes [m²·K/W] (E+ Rfilm_in, Rfilm_out).
const FC_FILMS: f64 = 0.135 + 0.03;
/// Resistance of a floor with no exposed perimeter [m²·K/W], as E+.
const FC_NO_PERIMETER_R: f64 = 177.0;

impl FFactorConstruction {
    /// Resistance of the floor [m²·K/W], without its air films: the F-factor
    /// includes them, and the simulation's dynamic inside film would otherwise
    /// count them twice.
    pub fn effective_resistance(&self, area: f64, perimeter: f64) -> f64 {
        if perimeter > 0.0 && self.f_factor > 0.0 {
            area / (perimeter * self.f_factor) - FC_FILMS
        } else {
            FC_NO_PERIMETER_R
        }
    }

    /// Layers from outside to inside, as E+ `CreateFCfactorConstructions`: a
    /// fictitious no-mass layer that makes up the F-factor's resistance, then
    /// 15 cm of concrete. Errors when the concrete alone resists more than the
    /// floor should, which E+ also rejects.
    pub fn layers(&self, area: f64, perimeter: f64) -> Result<Vec<ResolvedLayer>, String> {
        let (k, rho, cp, thickness) = FC_CONCRETE;
        let r_fictitious = self.effective_resistance(area, perimeter) - thickness / k;
        if r_fictitious <= 0.0 {
            return Err(format!(
                "F-factor construction '{}': A/(F·P) with A = {area:.2} m² and P = {perimeter:.2} m \
                 is less than the air films and concrete it must contain ({:.3} m²·K/W); \
                 check the F-factor and exposed perimeter",
                self.name,
                FC_FILMS + thickness / k
            ));
        }
        Ok(vec![
            ResolvedLayer::new_no_mass(r_fictitious, 1.0),
            ResolvedLayer::new(k, rho, cp, thickness),
        ])
    }
}

#[cfg(test)]
mod tests {
    /// Hand evaluation of E+ `SetupSimpleWindowGlazingSystem` for U = 2, SHGC = 0.4 (#213).
    #[test]
    fn simple_glazing_follows_the_e_plus_correlations() {
        use crate::glazing::{simple_layer_resistance, simple_solar_transmittance};
        let (u, shgc) = (2.0f64, 0.4f64);
        let tsol = 0.085775 * shgc * shgc + 0.963954 * shgc - 0.084958;
        assert!((simple_solar_transmittance(u, shgc) - tsol).abs() < 1e-12);
        let r_i = 1.0 / (0.359073 * u.ln() + 6.949915);
        let r_o = 1.0 / (0.025342 * u + 29.163853);
        let r_l = 1.0 / u - r_i - r_o;
        assert!((simple_layer_resistance(u) - r_l).abs() < 1e-12);
        // The regimes switch at 3.4 and 4.5, not 3.4068 and 4.5424.
        assert_eq!(simple_solar_transmittance(3.401, 0.5), {
            let lo = 0.085775 * 0.25 + 0.963954 * 0.5 - 0.084958;
            let hi = 0.939998 * 0.25 + 0.20332 * 0.5;
            (3.401 - 3.4) / 1.1 * (hi - lo) + lo
        });
        let hi = 0.939998 * 0.25 + 0.20332 * 0.5;
        assert_eq!(simple_solar_transmittance(4.51, 0.5), hi);
    }

    #[test]
    fn f_factor_floor_removes_films_and_uses_fixed_concrete() {
        let fc = FFactorConstruction {
            name: "slab".into(),
            f_factor: 1.264,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            ground_temperatures: None,
        };
        // 100 m² with 20 m exposed: A/(F·P) = 3.956, films 0.165, concrete 0.0769.
        let layers = fc.layers(100.0, 20.0).unwrap();
        assert!(layers[0].no_mass);
        assert!(
            (layers[0].resistance() - (100.0 / (20.0 * 1.264) - 0.165 - 0.15 / 1.95)).abs() < 1e-9
        );
        let c = layers[1].density * layers[1].specific_heat * layers[1].thickness;
        assert!((c - 302_400.0).abs() < 1e-6);
        // Total matches E+'s Reff, whatever the split.
        let total: f64 = layers.iter().map(|l| l.resistance()).sum();
        assert!((total - fc.effective_resistance(100.0, 20.0)).abs() < 1e-9);
        // No exposed perimeter: E+'s R = 177.
        assert_eq!(fc.effective_resistance(100.0, 0.0), 177.0);
        assert!(fc.layers(100.0, 0.0).is_ok());
        // Too conductive to contain its own films and concrete: E+ rejects it.
        assert!(fc.layers(1.0, 20.0).is_err());
    }

    use super::*;

    #[test]
    fn unknown_material_is_an_error_not_a_dropped_layer() {
        let c = Construction {
            name: "c".to_string(),
            layers: vec![ConstructionLayer {
                material: "typo".to_string(),
                thickness: 0.1,
            }],
        };
        let err = c.resolve_layers(&HashMap::new()).unwrap_err();
        assert!(err.contains("typo"), "{err}");
    }

    #[test]
    fn test_layer_resistance() {
        let mat = Material {
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
        };
        let r = layer_resistance(&mat, 0.2);
        // R = 0.2 / 1.311 = 0.1526
        assert!((r - 0.1526).abs() < 0.001);
    }

    #[test]
    fn test_construction_u_factor() {
        let mut materials = HashMap::new();
        materials.insert(
            "Concrete".to_string(),
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
        );
        materials.insert(
            "Insulation".to_string(),
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
        );

        let construction = Construction {
            name: "Test Wall".to_string(),
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
        };

        let r = construction.total_resistance(&materials).unwrap();
        let u = construction.u_factor(&materials).unwrap();
        // R = 0.2/1.311 + 0.1/0.04 = 0.1526 + 2.5 = 2.6526
        assert!((r - 2.6526).abs() < 0.01);
        assert!((u - 1.0 / 2.6526).abs() < 0.01);
    }

    #[test]
    fn test_resolved_layer() {
        let mat = Material {
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
        };
        let resolved = ResolvedLayer::from_material(&mat, 0.2);
        assert!((resolved.resistance() - 0.1526).abs() < 0.001);
        assert!((resolved.conductivity - 1.311).abs() < 0.001);
        assert!((resolved.thickness - 0.2).abs() < 0.001);
    }
}
