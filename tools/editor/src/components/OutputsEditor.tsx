import { useState } from "react";

// ===== Variable catalog (mirrors openbse_schema.json variable descriptions) =====

interface VarEntry {
  name: string;
  unit: string;
  desc: string;
}

interface CatalogGroup {
  category: string;
  label: string;
  vars: VarEntry[];
}

const CATALOG: CatalogGroup[] = [
  {
    category: "site",
    label: "Site / Weather",
    vars: [
      { name: "outdoor_temperature",          unit: "°C",    desc: "Outdoor dry-bulb temperature" },
      { name: "wind_speed",                   unit: "m/s",   desc: "Wind speed" },
      { name: "direct_normal_radiation",      unit: "W/m²",  desc: "Direct normal solar radiation" },
      { name: "diffuse_horizontal_radiation", unit: "W/m²",  desc: "Diffuse horizontal radiation" },
      { name: "relative_humidity",            unit: "%",     desc: "Outdoor relative humidity" },
    ],
  },
  {
    category: "zone",
    label: "Zone State",
    vars: [
      { name: "temperature",              unit: "°C",    desc: "Zone air temperature" },
      { name: "humidity_ratio",           unit: "kg/kg", desc: "Zone humidity ratio" },
      { name: "heating_rate",             unit: "W",     desc: "Zone heating rate" },
      { name: "cooling_rate",             unit: "W",     desc: "Zone cooling rate" },
      { name: "heating_energy",           unit: "J",     desc: "Zone heating energy" },
      { name: "cooling_energy",           unit: "J",     desc: "Zone cooling energy" },
      { name: "infiltration_mass_flow",   unit: "kg/s",  desc: "Infiltration mass flow" },
      { name: "nat_vent_flow",            unit: "m³/s",  desc: "Natural ventilation volumetric flow" },
      { name: "nat_vent_mass_flow",       unit: "kg/s",  desc: "Natural ventilation mass flow" },
      { name: "nat_vent_active",          unit: "-",     desc: "Natural ventilation active (0/1)" },
      { name: "internal_gains_convective",unit: "W",     desc: "Convective internal gains" },
      { name: "internal_gains_radiative", unit: "W",     desc: "Radiative internal gains" },
      { name: "supply_air_temperature",   unit: "°C",    desc: "Supply air temperature" },
      { name: "supply_air_mass_flow",     unit: "kg/s",  desc: "Supply air mass flow" },
      { name: "mean_radiant_temperature", unit: "°C",    desc: "Mean radiant temperature" },
      { name: "operative_temperature",    unit: "°C",    desc: "Operative temperature" },
      { name: "unmet_heating",            unit: "-",     desc: "Unmet heating (0 or 1)" },
      { name: "unmet_cooling",            unit: "-",     desc: "Unmet cooling (0 or 1)" },
    ],
  },
  {
    category: "zone",
    label: "Zone Gains",
    vars: [
      { name: "gain_people_sensible",           unit: "W", desc: "People sensible gain" },
      { name: "gain_people_latent",             unit: "W", desc: "People latent gain" },
      { name: "gain_lighting",                  unit: "W", desc: "Lighting gain" },
      { name: "gain_equipment_sensible",        unit: "W", desc: "Equipment sensible gain" },
      { name: "gain_equipment_latent",          unit: "W", desc: "Equipment latent gain" },
      { name: "gain_infiltration_sensible",     unit: "W", desc: "Infiltration sensible gain" },
      { name: "gain_infiltration_latent",       unit: "W", desc: "Infiltration latent gain" },
      { name: "gain_ventilation_sensible",      unit: "W", desc: "Ventilation sensible gain" },
      { name: "gain_ventilation_latent",        unit: "W", desc: "Ventilation latent gain" },
      { name: "gain_natural_ventilation_sensible", unit: "W", desc: "Natural ventilation sensible gain" },
      { name: "gain_natural_ventilation_latent",   unit: "W", desc: "Natural ventilation latent gain" },
      { name: "gain_solar",                     unit: "W", desc: "Solar gain" },
      { name: "gain_hvac_sensible",             unit: "W", desc: "HVAC sensible gain" },
      { name: "gain_hvac_latent",               unit: "W", desc: "HVAC latent gain" },
    ],
  },
  {
    category: "surface",
    label: "Surface",
    vars: [
      { name: "inside_temperature",            unit: "°C",     desc: "Inside surface temperature" },
      { name: "outside_temperature",           unit: "°C",     desc: "Outside surface temperature" },
      { name: "inside_convection_coefficient", unit: "W/(m²·K)", desc: "Inside convection coefficient" },
      { name: "incident_solar",               unit: "W/m²",   desc: "Incident solar radiation" },
      { name: "transmitted_solar",            unit: "W",      desc: "Transmitted solar (windows)" },
      { name: "cond_inside",                  unit: "W",      desc: "Conduction heat flow (inside face)" },
      { name: "convection_inside",            unit: "W",      desc: "Convective heat flow (inside face)" },
    ],
  },
  {
    category: "component",
    label: "Component",
    vars: [
      { name: "outlet_temperature",   unit: "°C",    desc: "Outlet air temperature" },
      { name: "inlet_temperature",    unit: "°C",    desc: "Inlet air temperature" },
      { name: "mass_flow",            unit: "kg/s",  desc: "Mass flow rate" },
      { name: "outlet_humidity_ratio",unit: "kg/kg", desc: "Outlet humidity ratio" },
      { name: "electric_power",       unit: "W",     desc: "Electric power consumption" },
      { name: "fuel_power",           unit: "W",     desc: "Fuel power consumption" },
      { name: "thermal_output",       unit: "W",     desc: "Thermal output" },
      { name: "cop_operating",        unit: "-",     desc: "Operating COP" },
      { name: "plr",                  unit: "-",     desc: "Part load ratio" },
      { name: "rtf",                  unit: "-",     desc: "Runtime fraction" },
      { name: "sensible_load",        unit: "W",     desc: "Sensible load" },
      { name: "latent_load",          unit: "W",     desc: "Latent load" },
      { name: "total_load",           unit: "W",     desc: "Total load" },
      { name: "conduction_loss",      unit: "W",     desc: "Duct conduction loss" },
      { name: "leakage_loss",         unit: "W",     desc: "Duct leakage loss" },
      { name: "pressure_rise",        unit: "Pa",    desc: "Fan pressure rise" },
      { name: "total_efficiency",     unit: "-",     desc: "Total efficiency" },
      { name: "effectiveness",        unit: "-",     desc: "Heat exchanger effectiveness" },
    ],
  },
  {
    category: "building",
    label: "Building Energy Totals",
    vars: [
      { name: "fan_electric",      unit: "W", desc: "Fan electric power" },
      { name: "cooling_electric",  unit: "W", desc: "Cooling electric power" },
      { name: "heating_electric",  unit: "W", desc: "Heating electric power" },
      { name: "heating_gas",       unit: "W", desc: "Heating gas power" },
      { name: "pump_electric",     unit: "W", desc: "Pump electric power" },
      { name: "heat_rejection",    unit: "W", desc: "Heat rejection power" },
      { name: "humidification",    unit: "W", desc: "Humidification power" },
      { name: "heat_recovery",     unit: "W", desc: "Heat recovery power" },
      { name: "dhw_electric",      unit: "W", desc: "DHW electric power" },
      { name: "dhw_gas",           unit: "W", desc: "DHW gas power" },
      { name: "lighting",          unit: "W", desc: "Lighting electric power" },
      { name: "ext_lighting",      unit: "W", desc: "Exterior lighting power" },
      { name: "equipment",         unit: "W", desc: "Equipment electric power" },
      { name: "ext_equipment",     unit: "W", desc: "Exterior equipment power" },
      { name: "total_electric",    unit: "W", desc: "Total electric power" },
      { name: "total_gas",         unit: "W", desc: "Total gas power" },
    ],
  },
  {
    category: "submeter",
    label: "Submeter",
    vars: [
      { name: "total_electric",   unit: "W", desc: "Total electric power" },
      { name: "total_gas",        unit: "W", desc: "Total gas power" },
      { name: "total",            unit: "W", desc: "Total all-fuel power" },
      { name: "lighting",         unit: "W", desc: "Lighting power" },
      { name: "equipment",        unit: "W", desc: "Equipment power" },
      { name: "heating_gas",      unit: "W", desc: "Heating gas power" },
      { name: "cooling_electric", unit: "W", desc: "Cooling electric power" },
      { name: "fan_electric",     unit: "W", desc: "Fan electric power" },
      { name: "dhw_gas",          unit: "W", desc: "DHW gas power" },
    ],
  },
];

/** Categories where a name filter (entity selector) makes sense */
const FILTERABLE = new Set(["zone", "surface", "component", "submeter"]);

const FREQUENCIES = ["timestep", "hourly", "daily", "monthly", "run_period"] as const;
const AGGREGATIONS = ["mean", "sum", "min", "max"] as const;

// ===== Types =====

interface OutputFile {
  file: string;
  frequency?: string;
  aggregation?: string;
  variables: string[];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  [key: string]: any;
}

interface OutputsEditorProps {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  instances: any[];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  onUpdate: (instances: any[]) => void;
}

// ===== Helpers =====

function parseSpec(spec: string): { category: string; variable: string; filter: string } {
  const parts = spec.split(":");
  return {
    category: parts[0] ?? "",
    variable: parts[1] ?? "",
    filter: parts.slice(2).join(":"),
  };
}

function buildSpec(category: string, variable: string, filter: string): string {
  const f = filter.trim();
  return f ? `${category}:${variable}:${f}` : `${category}:${variable}`;
}

function toOutputFile(raw: unknown): OutputFile {
  if (typeof raw === "object" && raw !== null) {
    const r = raw as Record<string, unknown>;
    return {
      file: typeof r.file === "string" ? r.file : "",
      frequency: typeof r.frequency === "string" ? r.frequency : "hourly",
      aggregation: typeof r.aggregation === "string" ? r.aggregation : "mean",
      variables: Array.isArray(r.variables)
        ? (r.variables as unknown[]).filter((v): v is string => typeof v === "string")
        : [],
    };
  }
  return { file: "", frequency: "hourly", aggregation: "mean", variables: [] };
}

// ===== Component =====

export function OutputsEditor({ instances, onUpdate }: OutputsEditorProps) {
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [search, setSearch] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  const safeInstances: OutputFile[] = instances.map(toOutputFile);
  const idx = Math.min(selectedIdx, Math.max(0, safeInstances.length - 1));
  const current: OutputFile = safeInstances[idx] ?? {
    file: "",
    frequency: "hourly",
    aggregation: "mean",
    variables: [],
  };

  function updateCurrent(updates: Partial<OutputFile>) {
    const arr = [...safeInstances];
    arr[idx] = { ...current, ...updates };
    onUpdate(arr);
  }

  function addOutputFile() {
    const n = safeInstances.length + 1;
    const newFile: OutputFile = {
      file: `output_${n}.csv`,
      frequency: "hourly",
      aggregation: "mean",
      variables: [],
    };
    const arr = [...safeInstances, newFile];
    onUpdate(arr);
    setSelectedIdx(arr.length - 1);
  }

  function deleteOutputFile(i: number) {
    const arr = safeInstances.filter((_, k) => k !== i);
    onUpdate(arr);
    setSelectedIdx(Math.max(0, i - 1));
  }

  function addVariable(category: string, varName: string) {
    const spec = `${category}:${varName}`;
    // Allow duplicates only if adding manually — for catalog clicks, avoid exact dups
    if (current.variables.includes(spec)) return;
    updateCurrent({ variables: [...current.variables, spec] });
  }

  function removeVariable(i: number) {
    const vars = [...current.variables];
    vars.splice(i, 1);
    updateCurrent({ variables: vars });
  }

  function updateVariableFilter(i: number, filter: string) {
    const { category, variable } = parseSpec(current.variables[i]);
    const vars = [...current.variables];
    vars[i] = buildSpec(category, variable, filter);
    updateCurrent({ variables: vars });
  }

  function moveVariable(i: number, dir: "up" | "down") {
    const vars = [...current.variables];
    const j = dir === "up" ? i - 1 : i + 1;
    if (j < 0 || j >= vars.length) return;
    [vars[i], vars[j]] = [vars[j], vars[i]];
    updateCurrent({ variables: vars });
  }

  function toggleCollapse(key: string) {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  const searchLower = search.toLowerCase();

  const filteredCatalog = CATALOG.map((group) => ({
    ...group,
    vars: group.vars.filter(
      (v) =>
        searchLower === "" ||
        v.name.includes(searchLower) ||
        v.desc.toLowerCase().includes(searchLower) ||
        group.category.includes(searchLower) ||
        group.label.toLowerCase().includes(searchLower)
    ),
  })).filter((g) => g.vars.length > 0);

  // Track which base specs (no filter) are already selected
  const selectedBaseSpecs = new Set(
    current.variables.map((s) => {
      const { category, variable } = parseSpec(s);
      return `${category}:${variable}`;
    })
  );

  return (
    <div className="outputs-editor">
      {/* ---- File tabs ---- */}
      <div className="outputs-editor-header">
        <h2>Output Files</h2>
        <div className="output-file-tabs">
          {safeInstances.map((inst, i) => (
            <button
              key={i}
              className={`output-file-tab ${i === idx ? "active" : ""}`}
              onClick={() => setSelectedIdx(i)}
            >
              {inst.file || `output_${i + 1}.csv`}
              {safeInstances.length > 1 && (
                <span
                  className="tab-close"
                  onClick={(e) => {
                    e.stopPropagation();
                    deleteOutputFile(i);
                  }}
                  title="Remove output file"
                >
                  ×
                </span>
              )}
            </button>
          ))}
          <button className="btn-add output-file-add" onClick={addOutputFile} title="Add output file">
            + New File
          </button>
        </div>
      </div>

      {safeInstances.length === 0 ? (
        <div className="empty-state">
          <p>No output files defined.</p>
          <button className="btn-add" onClick={addOutputFile}>
            + Add Output File
          </button>
        </div>
      ) : (
        <div className="outputs-editor-body">
          {/* ---- Settings row ---- */}
          <div className="output-file-settings">
            <label className="output-setting-field">
              <span className="output-setting-label">File name</span>
              <input
                type="text"
                className="output-setting-input"
                value={current.file}
                onChange={(e) => updateCurrent({ file: e.target.value })}
                placeholder="results.csv"
              />
            </label>
            <label className="output-setting-field">
              <span className="output-setting-label">Frequency</span>
              <select
                className="output-setting-select"
                value={current.frequency ?? "hourly"}
                onChange={(e) => updateCurrent({ frequency: e.target.value })}
              >
                {FREQUENCIES.map((f) => (
                  <option key={f} value={f}>
                    {f}
                  </option>
                ))}
              </select>
            </label>
            <label className="output-setting-field">
              <span className="output-setting-label">Aggregation</span>
              <select
                className="output-setting-select"
                value={current.aggregation ?? "mean"}
                onChange={(e) => updateCurrent({ aggregation: e.target.value })}
              >
                {AGGREGATIONS.map((a) => (
                  <option key={a} value={a}>
                    {a}
                  </option>
                ))}
              </select>
            </label>
          </div>

          {/* ---- Split pane ---- */}
          <div className="output-var-split">
            {/* Left: catalog */}
            <div className="output-var-catalog">
              <div className="output-var-catalog-header">
                <strong>Variable Catalog</strong>
                <input
                  type="text"
                  className="class-search-input"
                  placeholder="Search variables..."
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </div>
              <div className="output-var-catalog-list">
                {filteredCatalog.map((group) => {
                  const groupKey = `${group.category}-${group.label}`;
                  const isCollapsed = collapsed.has(groupKey);
                  return (
                    <div key={groupKey} className="output-var-group">
                      <button
                        className="output-var-group-header"
                        onClick={() => toggleCollapse(groupKey)}
                      >
                        <span className="collapse-icon">{isCollapsed ? "▶" : "▼"}</span>
                        <span className={`cat-badge cat-${group.category}`}>{group.category}</span>
                        <span>{group.label}</span>
                      </button>
                      {!isCollapsed &&
                        group.vars.map((v) => {
                          const base = `${group.category}:${v.name}`;
                          const inSelected = selectedBaseSpecs.has(base);
                          return (
                            <div
                              key={v.name}
                              className={`output-var-row ${inSelected ? "var-in-selected" : ""}`}
                            >
                              <button
                                className={`output-var-add-btn ${inSelected ? "added" : ""}`}
                                onClick={() => addVariable(group.category, v.name)}
                                title={inSelected ? `${base} already added` : `Add ${base}`}
                              >
                                {inSelected ? "✓" : "+"}
                              </button>
                              <span className="output-var-name">{v.name}</span>
                              <span className="output-var-unit">[{v.unit}]</span>
                              <span className="output-var-desc">{v.desc}</span>
                            </div>
                          );
                        })}
                    </div>
                  );
                })}
              </div>
            </div>

            {/* Right: selected */}
            <div className="output-var-selected">
              <div className="output-var-selected-header">
                <strong>Selected Variables</strong>
                <span className="var-count">
                  {current.variables.length} var{current.variables.length !== 1 ? "s" : ""}
                </span>
              </div>

              {current.variables.length === 0 ? (
                <div className="output-var-empty">
                  <p>Click "+" in the catalog to add variables to this output file.</p>
                  <p className="hint">
                    Use name filter to target a specific zone, surface, component, or submeter. Leave blank for all.
                  </p>
                </div>
              ) : (
                <table className="output-var-table">
                  <thead>
                    <tr>
                      <th>Variable</th>
                      <th>
                        Name filter{" "}
                        <span className="hint" title="Leave blank = all entities. Use glob * to match partial names. Example: 'living*' or 'Zone North'">
                          ⓘ
                        </span>
                      </th>
                      <th>Spec</th>
                      <th></th>
                    </tr>
                  </thead>
                  <tbody>
                    {current.variables.map((spec, i) => {
                      const { category, variable, filter } = parseSpec(spec);
                      const filterable = FILTERABLE.has(category);
                      return (
                        <tr key={i}>
                          <td className="var-spec-cell">
                            <span className={`cat-badge cat-${category}`}>{category}</span>
                            <span className="var-name-text">{variable}</span>
                          </td>
                          <td className="var-filter-cell">
                            {filterable ? (
                              <input
                                type="text"
                                className="var-filter-input"
                                value={filter}
                                onChange={(e) => updateVariableFilter(i, e.target.value)}
                                placeholder="all"
                                title="Blank = all; use * as wildcard, e.g. 'Living*'"
                              />
                            ) : (
                              <span className="var-no-filter">—</span>
                            )}
                          </td>
                          <td className="var-full-spec">
                            <code>{spec}</code>
                          </td>
                          <td className="var-actions-cell">
                            <button
                              className="btn-icon"
                              onClick={() => moveVariable(i, "up")}
                              disabled={i === 0}
                              title="Move up"
                            >
                              ↑
                            </button>
                            <button
                              className="btn-icon"
                              onClick={() => moveVariable(i, "down")}
                              disabled={i === current.variables.length - 1}
                              title="Move down"
                            >
                              ↓
                            </button>
                            <button
                              className="btn-danger btn-icon"
                              onClick={() => removeVariable(i)}
                              title="Remove"
                            >
                              ×
                            </button>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
