import { useState } from "react";
import { countFields } from "../lib/schema";
import type { ClassInfo } from "../lib/schema";

/** Group classes into logical categories */
const categoryOrder: { label: string; keys: string[] }[] = [
  {
    label: "General",
    keys: ["simulation", "climate_zone", "design_days", "schedules"],
  },
  {
    label: "Envelope",
    keys: [
      "materials",
      "constructions",
      "simple_constructions",
      "window_constructions",
      "f_factor_constructions",
    ],
  },
  {
    label: "Zones & Loads",
    keys: [
      "zones",
      "surfaces",
      "shading_surfaces",
      "people",
      "lights",
      "equipment",
      "equipment_it",
      "infiltration",
      "ventilation",
      "exhaust_fans",
      "outdoor_air",
      "ideal_loads",
    ],
  },
  {
    label: "HVAC",
    keys: [
      "air_loops",
      "plant_loops",
      "vrf_systems",
      "radiant_panels",
      "zone_groups",
      "thermostats",
      "controls",
      "performance_curves",
    ],
  },
  {
    label: "Service Water & Electrical",
    keys: ["dhw_systems", "exterior_equipment", "electrical_distribution"],
  },
  {
    label: "Output",
    keys: ["outputs", "summary_report", "comfort_grid", "parametrics"],
  },
];

// Edited elsewhere (Simulation panel), so not listed as a class.
const hiddenKeys = new Set(["weather_files"]);

const categorizedKeys = new Set(categoryOrder.flatMap((c) => c.keys));

/** Any schema class not placed above still gets listed, so new schema keys are never invisible. */
function withUncategorized(classes: ClassInfo[]) {
  const extra = classes
    .map((c) => c.key)
    .filter((k) => !categorizedKeys.has(k) && !hiddenKeys.has(k));
  return extra.length > 0
    ? [...categoryOrder, { label: "Other", keys: extra }]
    : categoryOrder;
}

interface ClassBrowserProps {
  classes: ClassInfo[];
  selectedKey: string | null;
  onSelect: (key: string) => void;
  instanceCounts: Record<string, number>;
}

export function ClassBrowser({
  classes,
  selectedKey,
  onSelect,
  instanceCounts,
}: ClassBrowserProps) {
  const [search, setSearch] = useState("");
  const classMap = new Map(classes.map((c) => [c.key, c]));
  const searchLower = search.toLowerCase();

  return (
    <div className="class-browser">
      <div className="class-browser-header">
        <h2>Object Classes</h2>
        <div className="class-search">
          <input
            type="text"
            className="class-search-input"
            placeholder="Filter classes..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          {search && (
            <button
              className="class-search-clear"
              onClick={() => setSearch("")}
              title="Clear filter"
            >
              x
            </button>
          )}
        </div>
      </div>
      <div className="class-browser-list">
        {withUncategorized(classes).map((category) => {
          const categoryClasses = category.keys
            .map((k) => classMap.get(k))
            .filter((c): c is ClassInfo => c !== undefined)
            .filter((c) =>
              searchLower === ""
                ? true
                : c.displayName.toLowerCase().includes(searchLower) ||
                  c.key.toLowerCase().includes(searchLower)
            );

          if (categoryClasses.length === 0) return null;

          return (
            <div key={category.label} className="class-category">
              <div className="category-label">{category.label}</div>
              {categoryClasses.map((cls) => {
                const fieldCount = countFields(cls.itemSchema);
                const instanceCount = instanceCounts[cls.key] ?? 0;
                const isSelected = selectedKey === cls.key;

                return (
                  <button
                    key={cls.key}
                    className={`class-item ${isSelected ? "selected" : ""}`}
                    onClick={() => onSelect(cls.key)}
                    title={cls.description}
                  >
                    <span className="class-name">{cls.displayName}</span>
                    <span className="class-meta">
                      {cls.isArray && instanceCount > 0 && (
                        <span className="instance-count">{instanceCount}</span>
                      )}
                      <span className="field-count">{fieldCount}f</span>
                    </span>
                  </button>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}
