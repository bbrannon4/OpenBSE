import { useState, useMemo, useCallback, useEffect, useRef } from "react";
import type {
  ParsedCsv,
  CsvVariable,
  ComponentCategory,
  VariableTreeNode,
  ZoneTreeNode,
} from "../lib/csv";
import { buildVariableTree, buildZoneTree } from "../lib/csv";
import { getDisplayUnit, type UnitSystem } from "../lib/units";

interface VariableBrowserProps {
  parsed: ParsedCsv;
  selectedVarIndices: Set<number>;
  onToggleVariable: (index: number) => void;
  onClearAll: () => void;
  unitSystem?: UnitSystem;
}

type BrowseMode = "component" | "zone";

interface ContextMenu {
  x: number;
  y: number;
  /** The key that was right-clicked */
  key: string;
  /** All descendant keys (including the clicked key) */
  childKeys: string[];
}

const CATEGORY_ORDER: ComponentCategory[] = [
  "Zones",
  "Surfaces",
  "Site",
  "Air Loops",
  "Plant",
  "Energy",
];

/** Reusable variable list with checkboxes */
function VarCheckboxList({
  variables,
  selectedVarIndices,
  onToggleVariable,
  unitSystem = "SI",
}: {
  variables: CsvVariable[];
  selectedVarIndices: Set<number>;
  onToggleVariable: (index: number) => void;
  unitSystem?: UnitSystem;
}) {
  return (
    <div className="var-list">
      {variables.map((v) => (
        <label
          key={v.columnIndex}
          className={`var-item ${selectedVarIndices.has(v.columnIndex) ? "selected" : ""}`}
        >
          <input
            type="checkbox"
            checked={selectedVarIndices.has(v.columnIndex)}
            onChange={() => onToggleVariable(v.columnIndex)}
          />
          <span className="var-name">{v.variable}</span>
          <span className="var-unit">[{getDisplayUnit(v.unit, unitSystem)}]</span>
        </label>
      ))}
    </div>
  );
}

/** Collect all expandable child keys under a zone node */
function zoneChildKeys(zoneNode: ZoneTreeNode): string[] {
  const keys: string[] = [
    `zone:${zoneNode.zone}`,
    `zv:${zoneNode.zone}`,
  ];
  for (const eq of zoneNode.equipment) {
    keys.push(`eq:${eq.component}`);
  }
  return keys;
}

/** Collect all expandable child keys under a component-view category */
function categoryChildKeys(cat: string, nodes: VariableTreeNode[]): string[] {
  const keys: string[] = [`cat:${cat}`];
  for (const node of nodes) {
    keys.push(`comp:${node.component}`);
  }
  return keys;
}

/** Collect child keys for an unzoned section */
function unzonedChildKeys(nodes: VariableTreeNode[]): string[] {
  const keys: string[] = ["_unzoned"];
  for (const node of nodes) {
    keys.push(`uz:${node.component}`);
  }
  return keys;
}

export function VariableBrowser({
  parsed,
  selectedVarIndices,
  onToggleVariable,
  onClearAll,
  unitSystem = "SI",
}: VariableBrowserProps) {
  const [search, setSearch] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [browseMode, setBrowseMode] = useState<BrowseMode>("zone");
  const [contextMenu, setContextMenu] = useState<ContextMenu | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const componentTree = useMemo(
    () => buildVariableTree(parsed.variables),
    [parsed.variables]
  );

  const zoneTree = useMemo(
    () => buildZoneTree(parsed.variables),
    [parsed.variables]
  );

  const toggleExpanded = (key: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const handleContextMenu = useCallback(
    (e: React.MouseEvent, key: string, childKeys: string[]) => {
      e.preventDefault();
      e.stopPropagation();
      setContextMenu({ x: e.clientX, y: e.clientY, key, childKeys });
    },
    []
  );

  const expandAll = useCallback(
    (keys: string[]) => {
      setExpanded((prev) => {
        const next = new Set(prev);
        for (const k of keys) next.add(k);
        return next;
      });
      setContextMenu(null);
    },
    []
  );

  const collapseAll = useCallback(
    (keys: string[]) => {
      setExpanded((prev) => {
        const next = new Set(prev);
        for (const k of keys) next.delete(k);
        return next;
      });
      setContextMenu(null);
    },
    []
  );

  // Close context menu on click outside or Escape
  useEffect(() => {
    if (!contextMenu) return;
    const handleClick = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setContextMenu(null);
      }
    };
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setContextMenu(null);
    };
    document.addEventListener("mousedown", handleClick);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("mousedown", handleClick);
      document.removeEventListener("keydown", handleKey);
    };
  }, [contextMenu]);

  // Collect ALL keys for expand/collapse all at root level
  const allKeys = useMemo(() => {
    const keys: string[] = [];
    if (browseMode === "zone") {
      for (const z of zoneTree.zones) {
        keys.push(...zoneChildKeys(z));
      }
      keys.push(...unzonedChildKeys(zoneTree.unzoned));
    } else {
      for (const cat of CATEGORY_ORDER) {
        const nodes = componentTree.get(cat);
        if (nodes) keys.push(...categoryChildKeys(cat, nodes));
      }
    }
    return keys;
  }, [browseMode, zoneTree, componentTree]);

  const searchLower = search.toLowerCase();

  const matchesSearch = (v: CsvVariable) =>
    !searchLower ||
    v.variable.toLowerCase().includes(searchLower) ||
    v.component.toLowerCase().includes(searchLower) ||
    v.raw.toLowerCase().includes(searchLower);

  const filteredComponentTree = useMemo(() => {
    if (!searchLower) return componentTree;
    const filtered = new Map<ComponentCategory, VariableTreeNode[]>();
    for (const [cat, nodes] of componentTree) {
      const filteredNodes: VariableTreeNode[] = [];
      for (const node of nodes) {
        if (node.component.toLowerCase().includes(searchLower)) {
          filteredNodes.push(node);
          continue;
        }
        const filteredVars = node.variables.filter(matchesSearch);
        if (filteredVars.length > 0) {
          filteredNodes.push({ ...node, variables: filteredVars });
        }
      }
      if (filteredNodes.length > 0) {
        filtered.set(cat, filteredNodes);
      }
    }
    return filtered;
  }, [componentTree, searchLower]);

  const selectedCount = (vars: CsvVariable[]) =>
    vars.filter((v) => selectedVarIndices.has(v.columnIndex)).length;

  return (
    <div className="var-browser">
      <div className="var-browser-header">
        <h2>Variables</h2>
        <div className="var-browser-actions">
          {selectedVarIndices.size > 0 && (
            <button className="btn-small btn-secondary" onClick={onClearAll}>
              Clear ({selectedVarIndices.size})
            </button>
          )}
        </div>
      </div>
      <div className="var-browse-mode">
        <button
          className={`btn-browse-mode ${browseMode === "zone" ? "active" : ""}`}
          onClick={() => setBrowseMode("zone")}
        >
          By Zone
        </button>
        <button
          className={`btn-browse-mode ${browseMode === "component" ? "active" : ""}`}
          onClick={() => setBrowseMode("component")}
        >
          By Component
        </button>
      </div>
      <div className="var-search">
        <input
          type="text"
          className="var-search-input"
          placeholder="Filter variables..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        {search && (
          <button className="var-search-clear" onClick={() => setSearch("")}>
            x
          </button>
        )}
      </div>
      <div
        className="var-browser-list"
        onContextMenu={(e) => {
          // Right-click on empty area = expand/collapse all
          handleContextMenu(e, "_root", allKeys);
        }}
      >
        {browseMode === "zone" ? (
          <>
            {zoneTree.zones
              .filter(
                (z) =>
                  !searchLower ||
                  z.zone.toLowerCase().includes(searchLower) ||
                  z.zoneVars.some(matchesSearch) ||
                  z.equipment.some(
                    (eq) =>
                      eq.component.toLowerCase().includes(searchLower) ||
                      eq.variables.some(matchesSearch)
                  )
              )
              .map((zoneNode) => {
                const zoneKey = `zone:${zoneNode.zone}`;
                const zoneExpanded = expanded.has(zoneKey);
                const allVars = [
                  ...zoneNode.zoneVars,
                  ...zoneNode.equipment.flatMap((e) => e.variables),
                ];
                const sc = selectedCount(allVars);
                const totalCount = allVars.length;
                const childKeys = zoneChildKeys(zoneNode);

                return (
                  <div key={zoneNode.zone} className="var-category">
                    <button
                      className="var-category-label"
                      onClick={() => toggleExpanded(zoneKey)}
                      onContextMenu={(e) =>
                        handleContextMenu(e, zoneKey, childKeys)
                      }
                    >
                      <span className="expand-arrow">
                        {zoneExpanded ? "\u25BC" : "\u25B6"}
                      </span>
                      <span className="var-zone-name">{zoneNode.zone}</span>
                      {sc > 0 && (
                        <span className="var-selected-badge">{sc}</span>
                      )}
                      <span className="var-category-count">{totalCount}</span>
                    </button>
                    {zoneExpanded && (
                      <>
                        {zoneNode.zoneVars.length > 0 && (
                          <div className="var-component">
                            <button
                              className="var-component-label"
                              onClick={() =>
                                toggleExpanded(`zv:${zoneNode.zone}`)
                              }
                            >
                              <span className="expand-arrow">
                                {expanded.has(`zv:${zoneNode.zone}`)
                                  ? "\u25BC"
                                  : "\u25B6"}
                              </span>
                              <span className="var-component-name">
                                Zone Variables
                              </span>
                              <span className="var-component-count">
                                {zoneNode.zoneVars.length}
                              </span>
                            </button>
                            {expanded.has(`zv:${zoneNode.zone}`) && (
                              <VarCheckboxList
                                variables={
                                  searchLower
                                    ? zoneNode.zoneVars.filter(matchesSearch)
                                    : zoneNode.zoneVars
                                }
                                selectedVarIndices={selectedVarIndices}
                                onToggleVariable={onToggleVariable}
                                unitSystem={unitSystem}
                              />
                            )}
                          </div>
                        )}
                        {zoneNode.equipment.map((eq) => {
                          const eqKey = `eq:${eq.component}`;
                          const eqExpanded = expanded.has(eqKey);
                          const eqSc = selectedCount(eq.variables);
                          const filteredVars = searchLower
                            ? eq.variables.filter(matchesSearch)
                            : eq.variables;
                          if (searchLower && filteredVars.length === 0)
                            return null;
                          return (
                            <div key={eq.component} className="var-component">
                              <button
                                className="var-component-label"
                                onClick={() => toggleExpanded(eqKey)}
                              >
                                <span className="expand-arrow">
                                  {eqExpanded ? "\u25BC" : "\u25B6"}
                                </span>
                                <span className="var-component-name">
                                  {eq.label}
                                  <span className="var-equip-detail">
                                    {" "}
                                    ({eq.component})
                                  </span>
                                </span>
                                {eqSc > 0 && (
                                  <span className="var-selected-badge">
                                    {eqSc}
                                  </span>
                                )}
                                <span className="var-component-count">
                                  {filteredVars.length}
                                </span>
                              </button>
                              {eqExpanded && (
                                <VarCheckboxList
                                  variables={filteredVars}
                                  selectedVarIndices={selectedVarIndices}
                                  onToggleVariable={onToggleVariable}
                                />
                              )}
                            </div>
                          );
                        })}
                      </>
                    )}
                  </div>
                );
              })}
            {zoneTree.unzoned.length > 0 && (
              <div className="var-category">
                <button
                  className="var-category-label"
                  onClick={() => toggleExpanded("_unzoned")}
                  onContextMenu={(e) =>
                    handleContextMenu(
                      e,
                      "_unzoned",
                      unzonedChildKeys(zoneTree.unzoned)
                    )
                  }
                >
                  <span className="expand-arrow">
                    {expanded.has("_unzoned") ? "\u25BC" : "\u25B6"}
                  </span>
                  Other / Plant
                  <span className="var-category-count">
                    {zoneTree.unzoned.reduce(
                      (s, n) => s + n.variables.length,
                      0
                    )}
                  </span>
                </button>
                {expanded.has("_unzoned") &&
                  zoneTree.unzoned
                    .filter(
                      (node) =>
                        !searchLower ||
                        node.component.toLowerCase().includes(searchLower) ||
                        node.variables.some(matchesSearch)
                    )
                    .map((node) => {
                      const compKey = `uz:${node.component}`;
                      const compExpanded = expanded.has(compKey);
                      const sc = selectedCount(node.variables);
                      const filteredVars = searchLower
                        ? node.variables.filter(matchesSearch)
                        : node.variables;
                      return (
                        <div key={node.component} className="var-component">
                          <button
                            className="var-component-label"
                            onClick={() => toggleExpanded(compKey)}
                          >
                            <span className="expand-arrow">
                              {compExpanded ? "\u25BC" : "\u25B6"}
                            </span>
                            <span className="var-component-name">
                              {node.component}
                            </span>
                            {sc > 0 && (
                              <span className="var-selected-badge">{sc}</span>
                            )}
                            <span className="var-component-count">
                              {filteredVars.length}
                            </span>
                          </button>
                          {compExpanded && (
                            <VarCheckboxList
                              variables={filteredVars}
                              selectedVarIndices={selectedVarIndices}
                              onToggleVariable={onToggleVariable}
                            />
                          )}
                        </div>
                      );
                    })}
              </div>
            )}
          </>
        ) : (
          CATEGORY_ORDER.map((cat) => {
            const nodes = filteredComponentTree.get(cat);
            if (!nodes || nodes.length === 0) return null;
            const catKey = `cat:${cat}`;
            const catExpanded = expanded.has(catKey);
            const childKeys = categoryChildKeys(cat, nodes);
            return (
              <div key={cat} className="var-category">
                <button
                  className="var-category-label"
                  onClick={() => toggleExpanded(catKey)}
                  onContextMenu={(e) =>
                    handleContextMenu(e, catKey, childKeys)
                  }
                >
                  <span className="expand-arrow">
                    {catExpanded ? "\u25BC" : "\u25B6"}
                  </span>
                  {cat}
                  <span className="var-category-count">
                    {nodes.reduce((s, n) => s + n.variables.length, 0)}
                  </span>
                </button>
                {catExpanded &&
                  nodes.map((node) => {
                    const compKey = `comp:${node.component}`;
                    const compExpanded = expanded.has(compKey);
                    const sc = selectedCount(node.variables);
                    return (
                      <div key={node.component} className="var-component">
                        <button
                          className="var-component-label"
                          onClick={() => toggleExpanded(compKey)}
                        >
                          <span className="expand-arrow">
                            {compExpanded ? "\u25BC" : "\u25B6"}
                          </span>
                          <span className="var-component-name">
                            {node.component}
                          </span>
                          {sc > 0 && (
                            <span className="var-selected-badge">{sc}</span>
                          )}
                          <span className="var-component-count">
                            {node.variables.length}
                          </span>
                        </button>
                        {compExpanded && (
                          <VarCheckboxList
                            variables={node.variables}
                            selectedVarIndices={selectedVarIndices}
                            onToggleVariable={onToggleVariable}
                          />
                        )}
                      </div>
                    );
                  })}
              </div>
            );
          })
        )}
      </div>

      {/* Context menu */}
      {contextMenu && (
        <div
          ref={menuRef}
          className="var-context-menu"
          style={{ top: contextMenu.y, left: contextMenu.x }}
        >
          <button
            className="var-context-item"
            onClick={() => expandAll(contextMenu.childKeys)}
          >
            Expand All
          </button>
          <button
            className="var-context-item"
            onClick={() => collapseAll(contextMenu.childKeys)}
          >
            Collapse All
          </button>
        </div>
      )}
    </div>
  );
}
