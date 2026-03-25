/**
 * CSV parser for OpenBSE simulation results.
 *
 * Header format: Month,Day,Hour[,SubHour],ComponentName:variable_name [unit],...
 * Site-level variables have no colon: variable_name [unit]
 */

export interface CsvVariable {
  /** Full column header text */
  raw: string;
  /** Component name (empty string for site-level) */
  component: string;
  /** Variable name without component prefix */
  variable: string;
  /** Unit string, e.g. "W", "°C", "kg/s", or "-" */
  unit: string;
  /** Column index in the CSV data */
  columnIndex: number;
}

export interface CsvTimestep {
  month: number;
  day: number;
  hour: number;
  subHour: number;
  /** Index into the data arrays */
  index: number;
}

export interface ParsedCsv {
  variables: CsvVariable[];
  timesteps: CsvTimestep[];
  /** Raw numeric data: data[columnIndex][rowIndex] */
  data: Float64Array[];
  hasSubHour: boolean;
}

/** Category for grouping components in the variable browser */
export type ComponentCategory =
  | "Zones"
  | "Surfaces"
  | "Site"
  | "Air Loops"
  | "Plant"
  | "Energy";

/** Classify a variable into a component category based on its variable name and component name */
export function categorizeVariable(v: CsvVariable): ComponentCategory {
  const vl = v.variable.toLowerCase();

  if (vl.startsWith("energy_") || vl.startsWith("total_")) return "Energy";
  if (vl.startsWith("site_") || v.component === "") return "Site";
  if (vl.startsWith("surface_")) return "Surfaces";

  // Zone variables: zone_temp, zone_humidity_ratio, heating_load, cooling_load,
  // infiltration, nat_vent, supply_air, q_internal, outdoor_air, hvac_*
  const zoneVars = [
    "zone_temp", "zone_humidity_ratio", "heating_load", "cooling_load",
    "infiltration", "nat_vent", "supply_air", "q_internal", "outdoor_air",
    "hvac_cooling", "hvac_heating", "exhaust_fan", "exhaust_mass",
    "ventilation_mass",
  ];
  if (zoneVars.some((zv) => vl.startsWith(zv))) return "Zones";

  // Equipment names that indicate plant
  const comp = v.component.toLowerCase();
  const plantKeywords = [
    "pump", "boiler", "chiller", "tower", "hhw", "chw", "cw ",
    "condenser", "heat exchanger",
  ];
  if (plantKeywords.some((k) => comp.includes(k))) return "Plant";

  // Everything else (fans, coils, etc.) -> Air Loops
  return "Air Loops";
}

/** Variable tree node for the browser */
export interface VariableTreeNode {
  component: string;
  category: ComponentCategory;
  variables: CsvVariable[];
}

/** Build a tree: category -> component -> variables */
export function buildVariableTree(
  variables: CsvVariable[]
): Map<ComponentCategory, VariableTreeNode[]> {
  const componentMap = new Map<string, CsvVariable[]>();
  for (const v of variables) {
    const key = v.component || "(Site)";
    const arr = componentMap.get(key) ?? [];
    arr.push(v);
    componentMap.set(key, arr);
  }

  const tree = new Map<ComponentCategory, VariableTreeNode[]>();
  for (const [component, vars] of componentMap) {
    const cat = categorizeVariable(vars[0]);
    const nodes = tree.get(cat) ?? [];
    nodes.push({ component, category: cat, variables: vars });
    tree.set(cat, nodes);
  }

  // Sort components within each category
  for (const nodes of tree.values()) {
    nodes.sort((a, b) => a.component.localeCompare(b.component));
  }

  return tree;
}

/** Infer which zone a component serves based on naming patterns.
 *  E.g. "CC T N1 Apt" → "T N1 Apt", "Fan G NE Apt" → "G NE Apt",
 *  "HC M S1 Apt" → "M S1 Apt". We try to extract a zone suffix. */
function inferZoneFromComponent(component: string): string | null {
  // Pattern: equipment-type-prefix + zone-identifier
  // Common prefixes: CC, HC, Fan, VAV, PFP, Reheat, etc.
  // We look for a known prefix followed by the zone part
  const prefixes = [
    /^(CC|HC|Fan|VAV|PFP|Reheat|DX|HRV|Humidifier|Duct)\s+/i,
    /^(Cooling Coil|Heating Coil|Supply Fan|Return Fan|Exhaust Fan)\s+/i,
  ];
  for (const re of prefixes) {
    const match = re.exec(component);
    if (match) {
      return component.slice(match[0].length).trim();
    }
  }
  return null;
}

/** Classify what type of equipment a component is */
function inferEquipmentType(component: string): string {
  const cl = component.toLowerCase();
  if (cl.startsWith("cc ") || cl.includes("cooling coil") || cl.includes("dx ")) return "Cooling Coil";
  if (cl.startsWith("hc ") || cl.includes("heating coil") || cl.includes("reheat")) return "Heating Coil";
  if (cl.startsWith("fan ") || cl.includes("supply fan") || cl.includes("return fan")) return "Fan";
  if (cl.includes("vav") || cl.includes("pfp")) return "Terminal";
  if (cl.includes("hrv") || cl.includes("heat recovery")) return "Heat Recovery";
  if (cl.includes("humidifier")) return "Humidifier";
  if (cl.includes("pump")) return "Pump";
  if (cl.includes("boiler")) return "Boiler";
  if (cl.includes("chiller")) return "Chiller";
  if (cl.includes("tower")) return "Cooling Tower";
  return "Equipment";
}

export interface ZoneTreeNode {
  zone: string;
  /** Direct zone variables (zone_temp, etc.) */
  zoneVars: CsvVariable[];
  /** Equipment serving this zone: equipType -> variables */
  equipment: { label: string; component: string; variables: CsvVariable[] }[];
}

/** Build a zone-centric tree: zone -> [zone vars, equipment vars] */
export function buildZoneTree(variables: CsvVariable[]): {
  zones: ZoneTreeNode[];
  unzoned: VariableTreeNode[];
} {
  // First, find actual zone components (ones with zone variables)
  const zoneComponents = new Set<string>();
  for (const v of variables) {
    if (!v.component) continue;
    const cat = categorizeVariable(v);
    if (cat === "Zones") {
      zoneComponents.add(v.component);
    }
  }

  // For equipment, try to match to a zone
  const zoneMap = new Map<string, ZoneTreeNode>();
  const unzonedComponents = new Map<string, CsvVariable[]>();

  // Initialize zones
  for (const zoneName of zoneComponents) {
    zoneMap.set(zoneName, {
      zone: zoneName,
      zoneVars: [],
      equipment: [],
    });
  }

  // Assign variables
  for (const v of variables) {
    if (!v.component) {
      // Site-level
      const arr = unzonedComponents.get("(Site)") ?? [];
      arr.push(v);
      unzonedComponents.set("(Site)", arr);
      continue;
    }

    if (zoneComponents.has(v.component)) {
      // Direct zone variable
      zoneMap.get(v.component)!.zoneVars.push(v);
      continue;
    }

    // Try to match equipment to a zone by suffix
    const zoneSuffix = inferZoneFromComponent(v.component);
    let matched = false;
    if (zoneSuffix) {
      // Try exact match first, then substring
      for (const zoneName of zoneComponents) {
        if (zoneName === zoneSuffix || zoneName.includes(zoneSuffix) || zoneSuffix.includes(zoneName)) {
          const node = zoneMap.get(zoneName)!;
          // Find or create equipment entry
          let equip = node.equipment.find((e) => e.component === v.component);
          if (!equip) {
            equip = {
              label: inferEquipmentType(v.component),
              component: v.component,
              variables: [],
            };
            node.equipment.push(equip);
          }
          equip.variables.push(v);
          matched = true;
          break;
        }
      }
    }

    if (!matched) {
      const arr = unzonedComponents.get(v.component) ?? [];
      arr.push(v);
      unzonedComponents.set(v.component, arr);
    }
  }

  const zones = Array.from(zoneMap.values()).sort((a, b) =>
    a.zone.localeCompare(b.zone)
  );

  const unzoned: VariableTreeNode[] = [];
  for (const [component, vars] of unzonedComponents) {
    unzoned.push({
      component,
      category: categorizeVariable(vars[0]),
      variables: vars,
    });
  }
  unzoned.sort((a, b) => a.component.localeCompare(b.component));

  return { zones, unzoned };
}

const HEADER_REGEX = /^(.+?):\s*(.+?)\s*\[(.+?)\]$/;
const SITE_REGEX = /^(.+?)\s*\[(.+?)\]$/;

/** Infer a unit for variables where the engine outputs [-] */
const INFERRED_UNITS: Record<string, string> = {
  electric_power: "W",
  fuel_power: "W",
  thermal_output: "W",
  exhaust_fan_power: "W",
  exhaust_fan_heat_to_zone: "W",
  hvac_cooling_rate: "W",
  hvac_heating_rate: "W",
  exhaust_mass_flow: "kg/s",
  outdoor_air_mass_flow: "kg/s",
  ventilation_mass_flow: "kg/s",
  nat_vent_flow: "m\u00B3/s",
  nat_vent_mass_flow: "kg/s",
  outlet_enthalpy: "J/kg",
};

function inferUnit(variable: string, csvUnit: string): string {
  if (csvUnit && csvUnit !== "-") return csvUnit;
  return INFERRED_UNITS[variable] ?? csvUnit;
}

function parseHeader(raw: string, columnIndex: number): CsvVariable | null {
  const trimmed = raw.trim();

  // Try component:variable [unit]
  const match = HEADER_REGEX.exec(trimmed);
  if (match) {
    const variable = match[2].trim();
    return {
      raw: trimmed,
      component: match[1].trim(),
      variable,
      unit: inferUnit(variable, match[3].trim()),
      columnIndex,
    };
  }

  // Try site-level variable [unit]
  const siteMatch = SITE_REGEX.exec(trimmed);
  if (siteMatch) {
    const variable = siteMatch[1].trim();
    return {
      raw: trimmed,
      component: "",
      variable,
      unit: inferUnit(variable, siteMatch[2].trim()),
      columnIndex,
    };
  }

  return null;
}

/**
 * Parse a CSV string into structured data.
 * Optimized for large files (50K+ rows, 2K+ columns).
 */
export function parseCsv(text: string): ParsedCsv {
  const lines = text.split("\n");
  if (lines.length < 2) {
    return { variables: [], timesteps: [], data: [], hasSubHour: false };
  }

  // Parse header
  const headerLine = lines[0];
  const headers = splitCsvLine(headerLine);

  // Detect time columns
  const hasSubHour = headers.length > 3 && headers[3].trim() === "SubHour";
  const dataStartCol = hasSubHour ? 4 : 3;

  // Parse variable definitions
  const variables: CsvVariable[] = [];
  for (let i = dataStartCol; i < headers.length; i++) {
    const v = parseHeader(headers[i], i - dataStartCol);
    if (v) variables.push(v);
  }

  // Count data rows
  let rowCount = 0;
  for (let i = 1; i < lines.length; i++) {
    if (lines[i].trim().length > 0) rowCount++;
  }

  // Allocate typed arrays for performance
  const data: Float64Array[] = [];
  for (let i = 0; i < variables.length; i++) {
    data.push(new Float64Array(rowCount));
  }

  const timesteps: CsvTimestep[] = new Array(rowCount);

  // Parse data rows
  let rowIdx = 0;
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i];
    if (line.trim().length === 0) continue;

    const cols = splitCsvLine(line);
    timesteps[rowIdx] = {
      month: parseInt(cols[0], 10),
      day: parseInt(cols[1], 10),
      hour: parseInt(cols[2], 10),
      subHour: hasSubHour ? parseInt(cols[3], 10) : 0,
      index: rowIdx,
    };

    for (let j = 0; j < variables.length; j++) {
      const colIdx = j + dataStartCol;
      data[j][rowIdx] = colIdx < cols.length ? parseFloat(cols[colIdx]) : NaN;
    }

    rowIdx++;
  }

  return { variables, timesteps, data, hasSubHour };
}

/** Split a CSV line on commas (no quoted fields expected in numeric data) */
function splitCsvLine(line: string): string[] {
  return line.split(",");
}

/** Compute summary statistics for a variable's data */
export interface VariableStats {
  min: number;
  max: number;
  mean: number;
  total: number;
  count: number;
}

export function computeStats(data: Float64Array): VariableStats {
  let min = Infinity;
  let max = -Infinity;
  let sum = 0;
  let count = 0;

  for (let i = 0; i < data.length; i++) {
    const v = data[i];
    if (!isFinite(v)) continue;
    if (v < min) min = v;
    if (v > max) max = v;
    sum += v;
    count++;
  }

  return {
    min: count > 0 ? min : 0,
    max: count > 0 ? max : 0,
    mean: count > 0 ? sum / count : 0,
    total: sum,
    count,
  };
}

export type AggregationMode = "raw" | "hourly" | "daily" | "monthly";

/** Aggregate data by the given mode. Returns [timestamps, aggregatedData] */
export function aggregateData(
  parsed: ParsedCsv,
  selectedIndices: number[],
  mode: AggregationMode
): { labels: string[]; series: Map<number, number[]> } {
  if (mode === "raw") {
    const labels = parsed.timesteps.map(
      (t) =>
        `${t.month}/${String(t.day).padStart(2, "0")} ${String(t.hour).padStart(2, "0")}:${String(t.subHour * 15).padStart(2, "0")}`
    );
    const series = new Map<number, number[]>();
    for (const idx of selectedIndices) {
      series.set(idx, Array.from(parsed.data[idx]));
    }
    return { labels, series };
  }

  // Group timesteps by bucket
  const buckets = new Map<string, number[]>();
  for (let i = 0; i < parsed.timesteps.length; i++) {
    const t = parsed.timesteps[i];
    let key: string;
    if (mode === "hourly") {
      key = `${t.month}/${String(t.day).padStart(2, "0")} ${String(t.hour).padStart(2, "0")}:00`;
    } else if (mode === "daily") {
      key = `${t.month}/${String(t.day).padStart(2, "0")}`;
    } else {
      key = `Month ${t.month}`;
    }
    const arr = buckets.get(key) ?? [];
    arr.push(i);
    buckets.set(key, arr);
  }

  const labels = Array.from(buckets.keys());
  const bucketArrays = Array.from(buckets.values());

  const series = new Map<number, number[]>();
  for (const varIdx of selectedIndices) {
    const col = parsed.data[varIdx];
    const agg: number[] = new Array(bucketArrays.length);
    for (let b = 0; b < bucketArrays.length; b++) {
      const rows = bucketArrays[b];
      let sum = 0;
      for (const r of rows) sum += col[r];
      agg[b] = sum / rows.length; // mean
    }
    series.set(varIdx, agg);
  }

  return { labels, series };
}
