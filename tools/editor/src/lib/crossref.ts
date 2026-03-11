/**
 * Cross-reference mapping for OpenBSE schema fields.
 * Maps field names to the model key that provides valid options.
 *
 * When a field name matches a key here, the editor renders a combobox
 * (dropdown + free text) populated with names from the referenced collection.
 */

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Model = Record<string, any>;

/** Field name -> model key that provides options */
const FIELD_TO_SOURCE: Record<string, string | string[]> = {
  // Schedule references
  schedule: "schedules",
  availability_schedule: "schedules",

  // Zone references (single string fields)
  zone: "zones",

  // Construction references
  construction: ["constructions", "simple_constructions", "window_constructions"],

  // Material references
  material: "materials",

  // Performance curve references
  cap_ft_curve: "performance_curves",
  eir_ft_curve: "performance_curves",

  // Plant loop references
  loop_name: "plant_loops",

  // Component references (air loop equipment names)
  component: "__equipment_names__",

  // Parent surface
  parent_surface: "surfaces",
};

/**
 * Array-of-string field name -> model key(s) that provide options.
 * These fields are arrays where each item should be selectable from existing objects.
 */
const ARRAY_FIELD_TO_SOURCE: Record<string, string | string[]> = {
  zones: ["zones", "zone_groups"],
  variables: "__output_variables__",
};

/**
 * Get available names for a cross-reference field.
 * Returns an array of valid name strings from the model.
 */
export function getCrossRefOptions(
  fieldName: string,
  model: Model,
): string[] {
  const source = FIELD_TO_SOURCE[fieldName];
  if (!source) return [];
  return collectNames(source, model);
}

/**
 * Get available names for an array-of-string cross-reference field.
 */
export function getArrayCrossRefOptions(
  fieldName: string,
  model: Model,
): string[] {
  const source = ARRAY_FIELD_TO_SOURCE[fieldName];
  if (!source) return [];
  return collectNames(source, model);
}

/**
 * Check if a field name is a known cross-reference.
 */
export function isCrossRef(fieldName: string): boolean {
  return fieldName in FIELD_TO_SOURCE;
}

/**
 * Check if an array-of-string field is a known cross-reference.
 */
export function isArrayCrossRef(fieldName: string): boolean {
  return fieldName in ARRAY_FIELD_TO_SOURCE;
}

/** All output variables the engine supports (from openbse-io/src/output.rs). */
const OUTPUT_VARIABLES: string[] = [
  // Zone variables
  "zone_temperature",
  "zone_humidity_ratio",
  "zone_heating_rate",
  "zone_cooling_rate",
  "zone_heating_energy",
  "zone_cooling_energy",
  "zone_infiltration_mass_flow",
  "zone_nat_vent_flow",
  "zone_nat_vent_mass_flow",
  "zone_nat_vent_active",
  "zone_internal_gains_convective",
  "zone_internal_gains_radiative",
  "zone_supply_air_temperature",
  "zone_supply_air_mass_flow",
  // Surface variables
  "surface_inside_temperature",
  "surface_outside_temperature",
  "surface_inside_convection_coefficient",
  "surface_incident_solar",
  "surface_transmitted_solar",
  "surface_conduction_inside",
  "surface_convection_inside",
  // Site/weather variables
  "site_outdoor_temperature",
  "site_wind_speed",
  "site_direct_normal_radiation",
  "site_diffuse_horizontal_radiation",
  "site_relative_humidity",
  // Air loop / HVAC variables
  "air_loop_outlet_temperature",
  "air_loop_mass_flow",
  "air_loop_outlet_humidity_ratio",
];

function collectNames(source: string | string[], model: Model): string[] {
  const sources = Array.isArray(source) ? source : [source];
  const names: string[] = [];

  for (const src of sources) {
    if (src === "__output_variables__") {
      names.push(...OUTPUT_VARIABLES);
      continue;
    }

    if (src === "__equipment_names__") {
      // Collect equipment names from all air loops
      const airLoops = model.air_loops || [];
      for (const loop of airLoops) {
        if (Array.isArray(loop.equipment)) {
          for (const eq of loop.equipment) {
            if (eq.name) names.push(eq.name);
          }
        }
      }
      // Also from plant loops
      const plantLoops = model.plant_loops || [];
      for (const loop of plantLoops) {
        if (Array.isArray(loop.supply_equipment)) {
          for (const eq of loop.supply_equipment) {
            if (eq.name) names.push(eq.name);
          }
        }
      }
      continue;
    }

    const arr = model[src];
    if (!Array.isArray(arr)) continue;
    for (const item of arr) {
      if (item && typeof item === "object" && item.name) {
        names.push(item.name);
      }
    }
  }

  // Add built-in schedules
  if (sources.includes("schedules")) {
    if (!names.includes("always_on")) names.push("always_on");
    if (!names.includes("always_off")) names.push("always_off");
  }

  return [...new Set(names)]; // dedupe
}
