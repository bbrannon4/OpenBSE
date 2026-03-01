//! Material and construction definitions for building envelope surfaces.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Surface roughness classification (affects exterior convection).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Roughness {
    VeryRough,
    Rough,
    MediumRough,
    MediumSmooth,
    Smooth,
    VerySmooth,
}

impl Default for Roughness {
    fn default() -> Self {
        Roughness::MediumRough
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    /// Thermal conductivity [W/(m·K)].
    /// Ignored for NoMass materials (when `thermal_resistance` is set).
    #[serde(default = "default_conductivity")]
    pub conductivity: f64,
    /// Density [kg/m³].
    /// Ignored for NoMass materials (when `thermal_resistance` is set).
    #[serde(default = "default_density")]
    pub density: f64,
    /// Specific heat [J/(kg·K)].
    /// Ignored for NoMass materials (when `thermal_resistance` is set).
    #[serde(default = "default_specific_heat")]
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
}

fn default_conductivity() -> f64 { 1.0 }
fn default_density() -> f64 { 1.0 }
fn default_specific_heat() -> f64 { 1000.0 }
fn default_solar_absorptance() -> f64 { 0.7 }
fn default_thermal_absorptance() -> f64 { 0.9 }
fn default_visible_absorptance() -> f64 { 0.7 }

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
        Self { conductivity, density, specific_heat, thickness, no_mass: false }
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
pub struct Construction {
    pub name: String,
    /// Layers ordered outside to inside, each pairing a material name with thickness
    pub layers: Vec<ConstructionLayer>,
}

impl Construction {
    /// Resolve all layers against a material map, returning ResolvedLayers.
    pub fn resolve_layers(&self, materials: &HashMap<String, Material>) -> Vec<ResolvedLayer> {
        self.layers.iter()
            .filter_map(|cl| {
                materials.get(&cl.material).map(|mat| {
                    ResolvedLayer::from_material(mat, cl.thickness)
                })
            })
            .collect()
    }

    /// Total R-value given a material map [m²·K/W].
    pub fn total_resistance(&self, materials: &HashMap<String, Material>) -> f64 {
        self.resolve_layers(materials).iter().map(|l| l.resistance()).sum()
    }

    /// Total U-factor [W/(m²·K)] (no film coefficients).
    pub fn u_factor(&self, materials: &HashMap<String, Material>) -> f64 {
        let r = self.total_resistance(materials);
        if r > 0.0 { 1.0 / r } else { 5.0 }
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

/// Simplified window construction (U-factor + SHGC based).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConstruction {
    pub name: String,
    /// Overall U-factor including film coefficients [W/(m²·K)]
    pub u_factor: f64,
    /// Solar Heat Gain Coefficient at normal incidence [0-1]
    pub shgc: f64,
    /// Visible transmittance [0-1]
    #[serde(default = "default_vt")]
    pub visible_transmittance: f64,
    /// Solar absorptance of the glazing assembly [0-1].
    ///
    /// The fraction of incident solar absorbed by the glass itself (as heat).
    /// This energy is split between inside and outside surfaces.
    /// Typical values: ~0.06 for clear double-pane, ~0.15-0.25 for tinted or
    /// low-e glass (which absorbs more but transmits less).
    ///
    /// If not specified, estimated from SHGC: absorptance ≈ (1 - SHGC) × 0.15
    /// (rough rule: high-SHGC clear glass absorbs ~6%, low-SHGC tinted absorbs more).
    #[serde(default)]
    pub solar_absorptance: Option<f64>,
    /// Fraction of absorbed solar that enters the zone (inside face).
    /// Remainder exits to outdoors. Default 0.5 (split equally).
    #[serde(default = "default_inside_absorbed_fraction")]
    pub inside_absorbed_fraction: f64,
    /// Per-pane solar transmittance at normal incidence [0-1].
    ///
    /// When provided along with `pane_solar_reflectance`, enables accurate
    /// angular SHGC(θ)/SHGC(0°) modifier computation using Fresnel optics
    /// with the correct glass extinction coefficient and inward-absorbed
    /// fraction (N_i). This properly accounts for the absorbed-inward
    /// solar component that keeps SHGC higher at oblique angles than
    /// transmittance alone.
    ///
    /// For ASHRAE 140 clear double-pane: τ_pane = 0.834 per 3mm pane.
    /// Matches EnergyPlus `WindowMaterial:Glazing` detailed model.
    #[serde(default)]
    pub pane_solar_transmittance: Option<f64>,
    /// Per-pane front solar reflectance at normal incidence [0-1].
    ///
    /// Used with `pane_solar_transmittance` to derive the system absorptance
    /// and inward-flowing fraction N_i = (SHGC − τ_system) / α_system.
    ///
    /// For ASHRAE 140 clear double-pane: ρ_pane = 0.075 per 3mm pane.
    #[serde(default)]
    pub pane_solar_reflectance: Option<f64>,

    // ── First-principles window thermal model ──────────────────────────
    //
    // When these properties are provided, the engine computes the glass
    // conductance (u_glass) from ISO 15099 sealed-gas-gap model at each
    // timestep instead of using NFRC film stripping. This matches the
    // EnergyPlus layer-by-layer approach and accounts for temperature-
    // dependent gap convection and radiation.
    //
    // If these fields are omitted, u_glass falls back to the NFRC film-
    // stripped value from u_factor (existing behavior).

    /// Number of glass panes (1=single, 2=double, 3=triple).
    /// Required for first-principles thermal model.
    #[serde(default)]
    pub num_panes: Option<u32>,
    /// Gas gap width between panes [m].
    /// For ASHRAE 140 double-pane: 0.012 m (12 mm).
    #[serde(default)]
    pub gap_width: Option<f64>,
    /// Pane thermal conductivity [W/(m·K)].
    /// For standard glass: 1.0 W/(m·K).
    #[serde(default)]
    pub pane_conductivity: Option<f64>,
    /// Pane thickness [m].
    /// For ASHRAE 140: 0.003175 m (3.175 mm).
    #[serde(default)]
    pub pane_thickness: Option<f64>,
    /// Glass LW emissivity for interior radiation and gap radiation.
    /// Default 0.84 for uncoated clear glass. For low-e coatings, the
    /// coated surface facing the gap may have ε ≈ 0.04–0.15.
    #[serde(default)]
    pub glass_emissivity: Option<f64>,
}

fn default_vt() -> f64 { 0.6 }
fn default_inside_absorbed_fraction() -> f64 { 0.5 }

impl WindowConstruction {
    /// Effective solar absorptance of the glazing.
    ///
    /// Uses explicitly set value if provided, otherwise estimates from SHGC.
    /// The estimate follows: clear glass absorbs ~6%, tinted absorbs more.
    /// SHGC ≈ τ_sol + α_in·N_in, where N_in is inward-flowing absorbed fraction.
    /// For a simple estimate: α ≈ (1 − SHGC) × 0.15 (captures ~5-8% range).
    pub fn effective_solar_absorptance(&self) -> f64 {
        self.solar_absorptance.unwrap_or_else(|| {
            // Rough model: clear glass (SHGC≈0.8) → ~0.06, low-e (SHGC≈0.25) → ~0.11
            // Clamp to physically reasonable range
            ((1.0 - self.shgc) * 0.15).clamp(0.02, 0.30)
        })
    }
}

/// Simplified opaque construction defined by overall properties.
///
/// Use when you don't want to specify individual material layers —
/// just give the overall U-factor, thickness, and thermal capacity.
/// Ideal for early design, ASHRAE 140 tests, and quick parametric studies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleConstruction {
    pub name: String,
    /// Overall U-factor [W/(m²·K)] (conductance, no film coefficients)
    pub u_factor: f64,
    /// Total wall thickness [m] (default 0.2)
    #[serde(default = "default_simple_thickness")]
    pub thickness: f64,
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
}

fn default_simple_thickness() -> f64 { 0.2 }
fn default_thermal_capacity() -> f64 { 50000.0 } // light construction ~50 kJ/(m²·K)

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let r = layer_resistance(&mat, 0.2);
        // R = 0.2 / 1.311 = 0.1526
        assert!((r - 0.1526).abs() < 0.001);
    }

    #[test]
    fn test_construction_u_factor() {
        let mut materials = HashMap::new();
        materials.insert("Concrete".to_string(), Material {
            name: "Concrete".to_string(),
            conductivity: 1.311,
            density: 2240.0,
            specific_heat: 836.8,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            visible_absorptance: 0.7,
            roughness: Roughness::MediumRough,
            thermal_resistance: None,
        });
        materials.insert("Insulation".to_string(), Material {
            name: "Insulation".to_string(),
            conductivity: 0.04,
            density: 30.0,
            specific_heat: 840.0,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            visible_absorptance: 0.7,
            roughness: Roughness::Rough,
            thermal_resistance: None,
        });

        let construction = Construction {
            name: "Test Wall".to_string(),
            layers: vec![
                ConstructionLayer { material: "Concrete".to_string(), thickness: 0.2 },
                ConstructionLayer { material: "Insulation".to_string(), thickness: 0.1 },
            ],
        };

        let r = construction.total_resistance(&materials);
        let u = construction.u_factor(&materials);
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
        };
        let resolved = ResolvedLayer::from_material(&mat, 0.2);
        assert!((resolved.resistance() - 0.1526).abs() < 0.001);
        assert!((resolved.conductivity - 1.311).abs() < 0.001);
        assert!((resolved.thickness - 0.2).abs() < 0.001);
    }
}
