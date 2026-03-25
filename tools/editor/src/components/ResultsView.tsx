import { useState, useCallback } from "react";
import { VariableBrowser } from "./VariableBrowser";
import { TimeSeriesChart } from "./TimeSeriesChart";
import { SummaryStats } from "./SummaryStats";
import type { ParsedCsv, AggregationMode } from "../lib/csv";
import type { ResultCase } from "../App";
import type { UnitSystem } from "../lib/units";

interface ResultsViewProps {
  cases: ResultCase[];
  activeCaseIdx: number;
  setActiveCaseIdx: React.Dispatch<React.SetStateAction<number>>;
  loading: boolean;
  selectedVarIndices: Set<number>;
  onToggleVariable: (idx: number) => void;
  onClearVariables: () => void;
  unitSystem: UnitSystem;
}

export function ResultsView({
  cases,
  activeCaseIdx,
  setActiveCaseIdx,
  loading,
  selectedVarIndices,
  onToggleVariable,
  onClearVariables,
  unitSystem,
}: ResultsViewProps) {
  const [aggregation, setAggregation] = useState<AggregationMode>("raw");
  const [showStats, setShowStats] = useState(true);

  const handleCaseChange = useCallback(
    (idx: number) => {
      setActiveCaseIdx(idx);
      onClearVariables();
    },
    [setActiveCaseIdx, onClearVariables]
  );

  const activeCase = cases[activeCaseIdx] ?? null;
  const activeParsed: ParsedCsv | null = activeCase?.parsed ?? null;

  if (cases.length === 0) {
    return (
      <div className="results-view">
        <div className="results-empty">
          <p>No results loaded</p>
          <p className="hint">
            Use "Open CSV" or "Open Folder" in the toolbar above,
            or run a simulation to view results here.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="results-view">
      <div className="results-toolbar">
        <div className="results-toolbar-left">
          {cases.length > 1 && (
            <select
              className="field-select"
              value={activeCaseIdx}
              disabled={loading}
              onChange={(e) => handleCaseChange(parseInt(e.target.value, 10))}
            >
              {cases.map((c, i) => (
                <option key={i} value={i}>
                  {c.name}{c.parsed ? "" : " (not loaded)"}
                </option>
              ))}
            </select>
          )}
          {cases.length === 1 && (
            <span className="results-case-name">{activeCase?.name}</span>
          )}
          {activeParsed && (
            <span className="results-info">
              {activeParsed.timesteps.length.toLocaleString()} timesteps |{" "}
              {activeParsed.variables.length.toLocaleString()} variables
            </span>
          )}
        </div>
        <div className="results-toolbar-right">
          <div className="agg-buttons">
            {(["raw", "hourly", "daily", "monthly"] as AggregationMode[]).map(
              (mode) => (
                <button
                  key={mode}
                  className={`btn-agg ${aggregation === mode ? "active" : ""}`}
                  onClick={() => setAggregation(mode)}
                >
                  {mode.charAt(0).toUpperCase() + mode.slice(1)}
                </button>
              )
            )}
          </div>
          <button
            className={`btn-header ${showStats ? "active" : ""}`}
            onClick={() => setShowStats(!showStats)}
            title="Toggle summary statistics"
          >
            Stats
          </button>
        </div>
      </div>
      <div className="results-body">
        {loading && (
          <div className="results-loading-overlay">
            <div className="results-loading-spinner">Loading results...</div>
          </div>
        )}
        {activeParsed ? (
          <>
            <VariableBrowser
              parsed={activeParsed}
              selectedVarIndices={selectedVarIndices}
              onToggleVariable={onToggleVariable}
              onClearAll={onClearVariables}
              unitSystem={unitSystem}
            />
            <div className="results-main">
              <TimeSeriesChart
                parsed={activeParsed}
                selectedVarIndices={selectedVarIndices}
                aggregation={aggregation}
                unitSystem={unitSystem}
              />
              {showStats && (
                <SummaryStats
                  parsed={activeParsed}
                  selectedVarIndices={selectedVarIndices}
                  onToggleVariable={onToggleVariable}
                  unitSystem={unitSystem}
                />
              )}
            </div>
          </>
        ) : !loading ? (
          <div className="results-empty">
            <p>Select a case to load its results</p>
          </div>
        ) : null}
      </div>
    </div>
  );
}
