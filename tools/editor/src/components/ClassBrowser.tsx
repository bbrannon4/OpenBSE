import { useState } from "react";
import { countFields } from "../lib/schema";
import type { ClassInfo } from "../lib/schema";

/** Group classes into logical categories */
const categoryOrder: { label: string; keys: string[] }[] = [
  {
    label: "General",
    keys: ["simulation", "weather_files", "design_days", "schedules"],
  },
  {
    label: "Envelope",
    keys: [
      "materials",
      "constructions",
      "simple_constructions",
      "window_constructions",
    ],
  },
  {
    label: "Zones & Loads",
    keys: [
      "zones",
      "surfaces",
      "people",
      "lights",
      "equipment",
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
      "zone_groups",
      "thermostats",
      "controls",
      "performance_curves",
    ],
  },
  {
    label: "Output",
    keys: ["outputs", "summary_report", "parametrics"],
  },
];

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
        {categoryOrder.map((category) => {
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
