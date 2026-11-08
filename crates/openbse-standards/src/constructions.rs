//! Layered constructions that meet a standard's assembly U-factor.
//!
//! A standard gives an assembly U-factor that includes the inside and outside
//! air films, not a layer build-up. OpenBSE takes the class's typical
//! assembly (fixed layers plus one insulation layer) and solves the
//! insulation's resistance so the layers and the films the table assumes give
//! exactly the target:
//!
//! ```text
//! R_insulation = 1/U_target − (R_film,out + R_film,in) − Σ R_fixed layers
//! ```
//!
//! The fixed layers keep the class's thermal mass (a mass wall keeps its
//! concrete). The insulation is a no-mass layer, so the solved resistance is
//! exact and adds no capacitance; in framed classes it is an effective layer
//! that stands for cavity insulation and framing together, as the table's
//! U-factor does. The simulation then applies its own dynamic air films to
//! the layers, as EnergyPlus does with these assemblies.

use crate::data::{AssemblyDef, LayerDef, MaterialDef};
use openbse_envelope::material::Roughness;
use openbse_envelope::{Construction, ConstructionLayer, Material};

/// Conductivity [W/(m·K)] used only to give the no-mass insulation layer a
/// geometric thickness (validation needs one; its resistance is exact).
const NOMINAL_INSULATION_K: f64 = 0.04;

/// A construction built from a typical assembly.
#[derive(Debug, Clone)]
pub struct BuiltConstruction {
    pub construction: Construction,
    /// Materials the construction needs: the shared typical materials and its
    /// own insulation layer (if any).
    pub materials: Vec<Material>,
    /// Solved insulation resistance [m²·K/W]; 0 when left out.
    pub insulation_r: f64,
    /// Assembly U-factor with the table's air films [W/(m²·K)].
    pub u_with_films: f64,
    /// Why the result differs from the target, if it does.
    pub note: Option<String>,
}

/// Name given to a typical material in the model.
pub fn typical_material_name(id: &str) -> String {
    format!("typical: {id}")
}

fn material_from_def(d: &MaterialDef) -> Material {
    Material {
        name: typical_material_name(&d.id),
        conductivity: d.conductivity,
        density: d.density,
        specific_heat: d.specific_heat,
        solar_absorptance: d.solar_absorptance.unwrap_or(0.7),
        thermal_absorptance: d.thermal_absorptance.unwrap_or(0.9),
        visible_absorptance: 0.7,
        roughness: d.roughness.unwrap_or(Roughness::MediumRough),
        thermal_resistance: None,
        vapor_resistance_factor: None,
        sorption_isotherm: None,
        liquid_transport_coeff: None,
        thermal_absorptance_inside: None,
    }
}

fn insulation_material(name: String, r: f64) -> Material {
    Material {
        name,
        conductivity: 0.0,
        density: 0.0,
        specific_heat: 0.0,
        solar_absorptance: 0.7,
        thermal_absorptance: 0.9,
        visible_absorptance: 0.7,
        roughness: Roughness::MediumRough,
        thermal_resistance: Some(r),
        vapor_resistance_factor: None,
        sorption_isotherm: None,
        liquid_transport_coeff: None,
        thermal_absorptance_inside: None,
    }
}

/// Build a construction from `assembly` meeting `target_u` [W/(m²·K), films
/// included], given the films the table assumes, `r_films` [m²·K/W]. With no
/// target (the table sets no requirement) the assembly is built uninsulated.
pub fn build_opaque(
    name: &str,
    assembly: &AssemblyDef,
    materials: &[MaterialDef],
    target_u: Option<f64>,
    r_films: f64,
) -> Result<BuiltConstruction, String> {
    let lookup = |id: &str| {
        materials
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| format!("assembly '{}' uses unknown material '{id}'", assembly.id))
    };
    let mut r_fixed = 0.0;
    for l in &assembly.layers {
        if let LayerDef::Material {
            material,
            thickness,
        } = l
        {
            r_fixed += thickness / lookup(material)?.conductivity;
        }
    }
    let (r_ins, note) = match target_u {
        Some(u) if u > 0.0 => {
            let r = 1.0 / u - r_films - r_fixed;
            if r > 0.0 {
                (r, None)
            } else {
                (
                    0.0,
                    Some(format!(
                        "uninsulated '{}' already meets the target: U = {:.4} W/(m²·K) \
                         against {u:.4}",
                        assembly.id,
                        1.0 / (r_films + r_fixed)
                    )),
                )
            }
        }
        Some(u) => return Err(format!("target U-factor {u} must be positive")),
        None => (
            0.0,
            Some("no requirement (NR): typical assembly without insulation".into()),
        ),
    };

    let insulation_name = format!("{name} insulation");
    let mut layers = Vec::new();
    let mut mats = Vec::new();
    for l in &assembly.layers {
        match l {
            LayerDef::Material {
                material,
                thickness,
            } => {
                let def = lookup(material)?;
                layers.push(ConstructionLayer {
                    material: typical_material_name(&def.id),
                    thickness: *thickness,
                });
                if !mats
                    .iter()
                    .any(|m: &Material| m.name == typical_material_name(&def.id))
                {
                    mats.push(material_from_def(def));
                }
            }
            LayerDef::Insulation(_) if r_ins > 0.0 => {
                layers.push(ConstructionLayer {
                    material: insulation_name.clone(),
                    thickness: r_ins * NOMINAL_INSULATION_K,
                });
                mats.push(insulation_material(insulation_name.clone(), r_ins));
            }
            LayerDef::Insulation(_) => {}
        }
    }
    Ok(BuiltConstruction {
        construction: Construction {
            name: name.to_string(),
            layers,
        },
        materials: mats,
        insulation_r: r_ins,
        u_with_films: 1.0 / (r_films + r_fixed + r_ins),
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::InsulationMarker;
    use std::collections::HashMap;

    fn mat(id: &str, k: f64) -> MaterialDef {
        MaterialDef {
            id: id.into(),
            conductivity: k,
            density: 2000.0,
            specific_heat: 900.0,
            solar_absorptance: None,
            thermal_absorptance: None,
            roughness: None,
            source: "test".into(),
        }
    }

    fn mass_wall() -> (AssemblyDef, Vec<MaterialDef>) {
        let a = AssemblyDef {
            id: "w".into(),
            element: crate::data::Element::WallAboveGrade,
            description: "test".into(),
            layers: vec![
                LayerDef::Material {
                    material: "concrete".into(),
                    thickness: 0.2,
                },
                LayerDef::Insulation(InsulationMarker::Insulation),
                LayerDef::Material {
                    material: "gypsum".into(),
                    thickness: 0.0127,
                },
            ],
        };
        (a, vec![mat("concrete", 1.95), mat("gypsum", 0.16)])
    }

    /// U of the built layers plus films, computed independently through the
    /// envelope crate's own resistance sum.
    fn u_through_envelope(b: &BuiltConstruction, r_films: f64) -> f64 {
        let map: HashMap<String, Material> = b
            .materials
            .iter()
            .map(|m| (m.name.clone(), m.clone()))
            .collect();
        let layers = b.construction.resolve_layers(&map).unwrap();
        1.0 / (r_films + layers.iter().map(|l| l.resistance()).sum::<f64>())
    }

    #[test]
    fn solved_insulation_hits_the_target_u() {
        let (a, m) = mass_wall();
        let r_films = 0.0299 + 0.1197;
        for target in [0.2, 0.5, 0.857, 2.0] {
            let b = build_opaque("wall", &a, &m, Some(target), r_films).unwrap();
            assert!(b.note.is_none());
            assert!((b.u_with_films - target).abs() < 1e-12);
            assert!(
                (u_through_envelope(&b, r_films) - target).abs() < 1e-9,
                "{target}"
            );
            // Mass is kept: the concrete layer is still there at full thickness.
            assert_eq!(b.construction.layers[0].thickness, 0.2);
        }
    }

    #[test]
    fn target_beyond_the_uninsulated_assembly_is_reported() {
        let (a, m) = mass_wall();
        let b = build_opaque("wall", &a, &m, Some(10.0), 0.15).unwrap();
        assert_eq!(b.insulation_r, 0.0);
        assert!(b.u_with_films < 10.0);
        assert!(b.note.unwrap().contains("already meets"));
        assert_eq!(b.construction.layers.len(), 2);
    }

    #[test]
    fn no_requirement_builds_uninsulated() {
        let (a, m) = mass_wall();
        let b = build_opaque("wall", &a, &m, None, 0.15).unwrap();
        assert_eq!(b.insulation_r, 0.0);
        assert!(b.note.unwrap().contains("NR"));
    }
}
