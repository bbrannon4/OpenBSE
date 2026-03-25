/**
 * Unit system handling for OpenBSE Workbench.
 *
 * Provides:
 * - Default unit inference for variables with missing units ([-])
 * - SI ↔ IP unit conversion
 * - Persistent unit system preference
 */

export type UnitSystem = "SI" | "IP";

/** Known variable → SI unit mappings for variables that may lack units in the CSV */
const INFERRED_UNITS: Record<string, string> = {
  electric_power: "W",
  fuel_power: "W",
  thermal_output: "W",
  component_electric_power: "W",
  component_fuel_power: "W",
  pump_electric_power: "W",
  heat_rejection_power: "W",
  humidification_power: "W",
  heat_recovery_power: "W",
  ext_lighting_power: "W",
  ext_equipment_power: "W",
  zone_lighting_power: "W",
  zone_equipment_power: "W",
  dhw_electric_power: "W",
  dhw_fuel_power: "W",
  exhaust_fan_power: "W",
  exhaust_fan_heat_to_zone: "W",
  exhaust_mass_flow: "kg/s",
  hvac_cooling_rate: "W",
  hvac_heating_rate: "W",
  ventilation_mass_flow: "kg/s",
  outdoor_air_mass_flow: "kg/s",
  nat_vent_flow: "m\u00B3/s",
  nat_vent_mass_flow: "kg/s",
  nat_vent_active: "",
  outlet_enthalpy: "J/kg",
};

/** Infer a unit for a variable name that has [-] or empty unit */
export function inferUnit(variableName: string, csvUnit: string): string {
  if (csvUnit && csvUnit !== "-") return csvUnit;
  return INFERRED_UNITS[variableName] ?? csvUnit;
}

/** SI → IP conversion factors and display units */
interface ConversionDef {
  ipUnit: string;
  factor: number;
  offset?: number; // for temperature: IP = SI * factor + offset
}

const CONVERSIONS: Record<string, ConversionDef> = {
  "\u00B0C": { ipUnit: "\u00B0F", factor: 9 / 5, offset: 32 },
  "W": { ipUnit: "Btu/h", factor: 3.412142 },
  "kW": { ipUnit: "kBtu/h", factor: 3.412142 },
  "W/m\u00B2": { ipUnit: "Btu/(h\u00B7ft\u00B2)", factor: 0.316998 },
  "W/(m\u00B2\u00B7K)": { ipUnit: "Btu/(h\u00B7ft\u00B2\u00B7\u00B0F)", factor: 0.176110 },
  "kg/s": { ipUnit: "lb/s", factor: 2.204623 },
  "m\u00B3/s": { ipUnit: "CFM", factor: 2118.88 },
  "m\u00B3": { ipUnit: "ft\u00B3", factor: 35.3147 },
  "m\u00B2": { ipUnit: "ft\u00B2", factor: 10.7639 },
  "m": { ipUnit: "ft", factor: 3.28084 },
  "Pa": { ipUnit: "in. w.c.", factor: 0.00401865 },
  "J": { ipUnit: "Btu", factor: 0.000947817 },
  "J/kg": { ipUnit: "Btu/lb", factor: 0.000429923 },
  "kJ/kg": { ipUnit: "Btu/lb", factor: 0.429923 },
  "kg/kg": { ipUnit: "lb/lb", factor: 1.0 },
  "W/K": { ipUnit: "Btu/(h\u00B7\u00B0F)", factor: 1.895634 },
  "%": { ipUnit: "%", factor: 1.0 },
};

/** Convert a SI value to IP */
export function convertToIP(value: number, siUnit: string): number {
  const conv = CONVERSIONS[siUnit];
  if (!conv) return value;
  if (conv.offset !== undefined) {
    return value * conv.factor + conv.offset;
  }
  return value * conv.factor;
}

/** Get the IP unit label for a SI unit */
export function getIPUnit(siUnit: string): string {
  return CONVERSIONS[siUnit]?.ipUnit ?? siUnit;
}

/** Get the display unit for a given unit system */
export function getDisplayUnit(siUnit: string, system: UnitSystem): string {
  if (system === "SI") return siUnit;
  return getIPUnit(siUnit);
}

/** Convert a value for display */
export function convertValue(
  value: number,
  siUnit: string,
  system: UnitSystem
): number {
  if (system === "SI") return value;
  return convertToIP(value, siUnit);
}

// ===== Persistent settings =====

const SETTINGS_KEY = "openbse-workbench-settings";

export interface WorkbenchSettings {
  unitSystem: UnitSystem;
}

const DEFAULT_SETTINGS: WorkbenchSettings = {
  unitSystem: "SI",
};

export function loadSettings(): WorkbenchSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return { ...DEFAULT_SETTINGS, ...parsed };
    }
  } catch {
    // ignore
  }
  return { ...DEFAULT_SETTINGS };
}

export function saveSettings(settings: WorkbenchSettings): void {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    // ignore
  }
}
