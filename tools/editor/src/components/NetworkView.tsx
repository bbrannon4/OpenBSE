import { useState, useMemo, useCallback } from "react";
import { HvacDiagram } from "./HvacDiagram";
import { buildSeparatedGraphs } from "../lib/hvac-graph";
import type { ParsedCsv, CsvVariable } from "../lib/csv";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Model = Record<string, any>;

type NetworkMode = "air" | "water";

interface NetworkViewProps {
  model: Model;
  parsedCsv: ParsedCsv | null;
  selectedVarIndices: Set<number>;
  onToggleVariable: (idx: number) => void;
  onSetVariables: (indices: Set<number>) => void;
  onClearVariables: () => void;
}

export function NetworkView({
  model,
  parsedCsv,
  selectedVarIndices,
  onToggleVariable,
  onSetVariables,
  onClearVariables,
}: NetworkViewProps) {
  const [mode, setMode] = useState<NetworkMode>("air");
  const [selectedComponent, setSelectedComponent] = useState<string | null>(null);

  const graphs = useMemo(() => buildSeparatedGraphs(model), [model]);

  const hasAir = graphs.air.nodes.length > 0;
  const hasWater = graphs.water.nodes.length > 0;
  const hasHvac = hasAir || hasWater;

  // Get variables for the selected component from the CSV
  const componentVariables: CsvVariable[] = useMemo(() => {
    if (!parsedCsv || !selectedComponent) return [];
    return parsedCsv.variables.filter(
      (v) => v.component === selectedComponent
    );
  }, [parsedCsv, selectedComponent]);

  const handleNodeClick = useCallback((componentName: string) => {
    setSelectedComponent((prev) =>
      prev === componentName ? null : componentName
    );
  }, []);

  const selectAllComponentVars = useCallback(() => {
    if (componentVariables.length === 0) return;
    const next = new Set(selectedVarIndices);
    for (const v of componentVariables) {
      next.add(v.columnIndex);
    }
    onSetVariables(next);
  }, [componentVariables, selectedVarIndices, onSetVariables]);

  const deselectAllComponentVars = useCallback(() => {
    if (componentVariables.length === 0) return;
    const next = new Set(selectedVarIndices);
    for (const v of componentVariables) {
      next.delete(v.columnIndex);
    }
    onSetVariables(next);
  }, [componentVariables, selectedVarIndices, onSetVariables]);

  const componentSelectedCount = componentVariables.filter(
    (v) => selectedVarIndices.has(v.columnIndex)
  ).length;

  if (!hasHvac) {
    return (
      <div className="network-view">
        <div className="hvac-empty">
          <p>No HVAC systems in model</p>
          <p className="hint">
            Open a model with air_loops or plant_loops defined to see the
            network topology.
          </p>
        </div>
      </div>
    );
  }

  const activeGraph = mode === "air" ? graphs.air : graphs.water;

  return (
    <div className="network-view">
      <div className="network-toolbar">
        <div className="network-mode-toggle">
          <button
            className={`btn-agg ${mode === "air" ? "active" : ""}`}
            onClick={() => setMode("air")}
            disabled={!hasAir}
          >
            Air Side
          </button>
          <button
            className={`btn-agg ${mode === "water" ? "active" : ""}`}
            onClick={() => setMode("water")}
            disabled={!hasWater}
          >
            Water Side
          </button>
        </div>
        <span className="network-info">
          {activeGraph.nodes.length} components | {activeGraph.edges.length} connections
          {selectedVarIndices.size > 0 && (
            <> | <strong>{selectedVarIndices.size} vars selected</strong></>
          )}
        </span>
      </div>
      <div className="network-body">
        <HvacDiagram graph={activeGraph} onNodeClick={handleNodeClick} />
        {selectedComponent && (
          <div className="network-var-panel">
            <div className="network-var-panel-header">
              <h3>{selectedComponent}</h3>
              <button
                className="btn-icon"
                onClick={() => setSelectedComponent(null)}
                title="Close"
              >
                x
              </button>
            </div>
            {!parsedCsv ? (
              <div className="network-var-panel-empty">
                No results CSV loaded. Open a CSV to select variables.
              </div>
            ) : componentVariables.length === 0 ? (
              <div className="network-var-panel-empty">
                No variables found for "{selectedComponent}" in the results CSV.
              </div>
            ) : (
              <>
                <div className="network-var-panel-actions">
                  <button
                    className="btn-small btn-secondary"
                    onClick={selectAllComponentVars}
                  >
                    Select All ({componentVariables.length})
                  </button>
                  {componentSelectedCount > 0 && (
                    <button
                      className="btn-small btn-secondary"
                      onClick={deselectAllComponentVars}
                    >
                      Deselect All
                    </button>
                  )}
                </div>
                <div className="network-var-list">
                  {componentVariables.map((v) => (
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
                      <span className="var-unit">[{v.unit}]</span>
                    </label>
                  ))}
                </div>
              </>
            )}
            {selectedVarIndices.size > 0 && (
              <div className="network-var-panel-footer">
                <span className="network-var-count">
                  {selectedVarIndices.size} total variables selected
                </span>
                <button
                  className="btn-small btn-danger"
                  onClick={onClearVariables}
                >
                  Clear All
                </button>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
