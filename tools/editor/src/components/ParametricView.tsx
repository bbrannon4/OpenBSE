import { useState, useMemo, useCallback, useEffect } from "react";
import ReactEChartsCore from "echarts-for-react/lib/core";
import * as echarts from "echarts/core";
import { LineChart, BarChart, ScatterChart } from "echarts/charts";
import {
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import type { ResultCase } from "../App";
import type { CsvVariable } from "../lib/csv";
import {
  detectParametricRuns,
  getCommonVariables,
  computeParametricSummary,
  getRunTimeSeries,
  type ParametricGroup,
  type ParametricRun,
} from "../lib/parametric";
import { convertValue, getDisplayUnit, type UnitSystem } from "../lib/units";

echarts.use([
  LineChart,
  BarChart,
  ScatterChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
  CanvasRenderer,
]);

const RUN_COLORS = [
  "#7aa2f7", "#9ece6a", "#f7768e", "#e0af68", "#7dcfff",
  "#bb9af7", "#ff9e64", "#73daca", "#c0caf5", "#2ac3de",
];

type ViewMode = "summary" | "compare" | "sweep";

interface ParametricViewProps {
  cases: ResultCase[];
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  model: Record<string, any>;
  unitSystem: UnitSystem;
  loadAndParseCsv: (path: string) => Promise<ResultCase | null>;
  setCases: React.Dispatch<React.SetStateAction<ResultCase[]>>;
}

export function ParametricView({
  cases,
  model,
  unitSystem,
  loadAndParseCsv,
  setCases,
}: ParametricViewProps) {
  const [viewMode, setViewMode] = useState<ViewMode>("summary");
  const [selectedVariable, setSelectedVariable] = useState<string | null>(null);
  const [selectedRuns, setSelectedRuns] = useState<Set<number>>(new Set());
  const [loading, setLoading] = useState(false);

  const group = useMemo(
    () => detectParametricRuns(cases, model),
    [cases, model]
  );

  // Auto-select all runs initially
  useEffect(() => {
    if (group) {
      setSelectedRuns(new Set(group.runs.map((_, i) => i)));
    }
  }, [group]);

  // Load all unloaded parametric CSVs
  useEffect(() => {
    if (!group) return;
    const unloaded = group.runs.filter(
      (r) => !cases[r.caseIndex]?.parsed
    );
    if (unloaded.length === 0) return;

    let cancelled = false;
    (async () => {
      setLoading(true);
      for (const run of unloaded) {
        if (cancelled) break;
        const result = await loadAndParseCsv(cases[run.caseIndex].path);
        if (result && !cancelled) {
          setCases((prev) => {
            const updated = [...prev];
            updated[run.caseIndex] = result;
            return updated;
          });
        }
      }
      setLoading(false);
    })();
    return () => { cancelled = true; };
  }, [group, cases, loadAndParseCsv, setCases]);

  // Get variables common to all parsed runs
  const commonVars = useMemo(() => {
    if (!group) return [];
    return getCommonVariables(group.runs, cases);
  }, [group, cases]);

  // Auto-select first variable
  useEffect(() => {
    if (!selectedVariable && commonVars.length > 0) {
      // Try to find a zone temperature or energy variable
      const preferred = commonVars.find(
        (v) => v.variable === "zone_temp" || v.variable === "zone_temperature"
      );
      const key = preferred ?? commonVars[0];
      setSelectedVariable(
        key.component ? `${key.component}:${key.variable}` : key.variable
      );
    }
  }, [commonVars, selectedVariable]);

  const toggleRun = useCallback((runIdx: number) => {
    setSelectedRuns((prev) => {
      const next = new Set(prev);
      if (next.has(runIdx)) next.delete(runIdx);
      else next.add(runIdx);
      return next;
    });
  }, []);

  if (!group) {
    return (
      <div className="parametric-view">
        <div className="parametric-empty">
          <p>No parametric runs detected</p>
          <p className="hint">
            Open a folder containing parametric results (multiple run CSVs
            like baseline.csv, tight_setpoints.csv, etc.)
          </p>
        </div>
      </div>
    );
  }

  const activeRuns = group.runs.filter((_, i) => selectedRuns.has(i));
  const hasSweep = !!group.sweepParameter;

  return (
    <div className="parametric-view">
      <div className="parametric-toolbar">
        <div className="toolbar-left">
          <span className="parametric-info">
            {group.runs.length} runs
            {hasSweep && ` | Sweep: ${group.sweepParameter}`}
          </span>
        </div>
        <div className="parametric-mode-toggle">
          <button
            className={`btn-agg ${viewMode === "summary" ? "active" : ""}`}
            onClick={() => setViewMode("summary")}
          >
            Summary
          </button>
          <button
            className={`btn-agg ${viewMode === "compare" ? "active" : ""}`}
            onClick={() => setViewMode("compare")}
          >
            Compare
          </button>
          {hasSweep && (
            <button
              className={`btn-agg ${viewMode === "sweep" ? "active" : ""}`}
              onClick={() => setViewMode("sweep")}
            >
              Sweep
            </button>
          )}
        </div>
      </div>
      {loading && (
        <div className="results-loading-bar">
          Loading parametric results...
        </div>
      )}
      <div className="parametric-body">
        <div className="parametric-sidebar">
          <div className="parametric-runs-section">
            <h3>Runs</h3>
            {group.runs.map((run, i) => (
              <label key={run.name} className={`parametric-run-item ${selectedRuns.has(i) ? "selected" : ""}`}>
                <input
                  type="checkbox"
                  checked={selectedRuns.has(i)}
                  onChange={() => toggleRun(i)}
                />
                <span
                  className="parametric-run-color"
                  style={{ background: RUN_COLORS[i % RUN_COLORS.length] }}
                />
                <span className="parametric-run-name">{run.name}</span>
                {run.isBaseline && <span className="parametric-baseline-badge">base</span>}
              </label>
            ))}
          </div>
          <div className="parametric-var-section">
            <h3>Variable</h3>
            <div className="parametric-var-list">
              {commonVars.length === 0 ? (
                <div className="parametric-empty-hint">
                  {loading ? "Loading..." : "No common variables found"}
                </div>
              ) : (
                <VariableGroupedList
                  variables={commonVars}
                  selectedVariable={selectedVariable}
                  onSelect={setSelectedVariable}
                  unitSystem={unitSystem}
                />
              )}
            </div>
          </div>
        </div>
        <div className="parametric-main">
          {viewMode === "summary" && selectedVariable && (
            <SummaryTable
              group={group}
              cases={cases}
              selectedVariable={selectedVariable}
              selectedRuns={selectedRuns}
              unitSystem={unitSystem}
            />
          )}
          {viewMode === "compare" && selectedVariable && (
            <ComparisonChart
              group={group}
              cases={cases}
              activeRuns={activeRuns}
              selectedVariable={selectedVariable}
              unitSystem={unitSystem}
            />
          )}
          {viewMode === "sweep" && selectedVariable && hasSweep && (
            <SweepChart
              group={group}
              cases={cases}
              activeRuns={activeRuns}
              selectedVariable={selectedVariable}
              unitSystem={unitSystem}
            />
          )}
          {!selectedVariable && (
            <div className="parametric-empty">
              <p>Select a variable from the left panel</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** Variable list grouped by component */
function VariableGroupedList({
  variables,
  selectedVariable,
  onSelect,
  unitSystem,
}: {
  variables: CsvVariable[];
  selectedVariable: string | null;
  onSelect: (key: string) => void;
  unitSystem: UnitSystem;
}) {
  const groups = useMemo(() => {
    const m = new Map<string, CsvVariable[]>();
    for (const v of variables) {
      const comp = v.component || "(Site)";
      const arr = m.get(comp) ?? [];
      arr.push(v);
      m.set(comp, arr);
    }
    return m;
  }, [variables]);

  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  return (
    <>
      {Array.from(groups.entries()).map(([comp, vars]) => {
        const isExpanded = expanded.has(comp);
        return (
          <div key={comp} className="parametric-var-group">
            <button
              className="parametric-var-group-label"
              onClick={() => {
                setExpanded((prev) => {
                  const next = new Set(prev);
                  if (next.has(comp)) next.delete(comp);
                  else next.add(comp);
                  return next;
                });
              }}
            >
              <span className="expand-arrow">{isExpanded ? "\u25BC" : "\u25B6"}</span>
              <span>{comp}</span>
              <span className="var-component-count">{vars.length}</span>
            </button>
            {isExpanded && vars.map((v) => {
              const key = v.component ? `${v.component}:${v.variable}` : v.variable;
              return (
                <button
                  key={key}
                  className={`parametric-var-item ${selectedVariable === key ? "active" : ""}`}
                  onClick={() => onSelect(key)}
                >
                  {v.variable}
                  <span className="var-unit">[{getDisplayUnit(v.unit, unitSystem)}]</span>
                </button>
              );
            })}
          </div>
        );
      })}
    </>
  );
}

/** Summary table: rows = runs, columns = min/max/mean/total */
function SummaryTable({
  group,
  cases,
  selectedVariable,
  selectedRuns,
  unitSystem,
}: {
  group: ParametricGroup;
  cases: ResultCase[];
  selectedVariable: string;
  selectedRuns: Set<number>;
  unitSystem: UnitSystem;
}) {
  const metrics = useMemo(
    () => computeParametricSummary(group.runs, cases, selectedVariable),
    [group.runs, cases, selectedVariable]
  );

  // Find the unit for this variable
  const unit = useMemo(() => {
    for (const run of group.runs) {
      const c = cases[run.caseIndex];
      if (!c?.parsed) continue;
      const v = c.parsed.variables.find((cv) => {
        const full = cv.component ? `${cv.component}:${cv.variable}` : cv.variable;
        return full === selectedVariable;
      });
      if (v) return v.unit;
    }
    return "";
  }, [group.runs, cases, selectedVariable]);

  const displayUnit = getDisplayUnit(unit, unitSystem);

  const fmt = (n: number) => {
    const converted = convertValue(n, unit, unitSystem);
    const abs = Math.abs(converted);
    if (abs === 0) return "0";
    if (abs >= 1e6) return converted.toExponential(2);
    if (abs >= 100) return converted.toLocaleString(undefined, { maximumFractionDigits: 1 });
    if (abs >= 1) return converted.toLocaleString(undefined, { maximumFractionDigits: 2 });
    return converted.toLocaleString(undefined, { maximumFractionDigits: 4 });
  };

  // Highlight min/max across runs
  const means = metrics.map((m) => m.mean);
  const minMean = Math.min(...means);
  const maxMean = Math.max(...means);

  const filtered = metrics.filter((_, i) => {
    const runIdx = group.runs.findIndex((r) => r.caseIndex === metrics[i].caseIndex);
    return selectedRuns.has(runIdx);
  });

  return (
    <div className="parametric-summary">
      <h3 className="parametric-section-title">
        {selectedVariable} [{displayUnit}]
      </h3>
      <table className="summary-table parametric-table">
        <thead>
          <tr>
            <th>Run</th>
            {group.sweepParameter && <th>{group.sweepParameter.replace(/_/g, " ")}</th>}
            <th>Min</th>
            <th>Max</th>
            <th>Mean</th>
            <th>Total</th>
          </tr>
        </thead>
        <tbody>
          {filtered.map((m) => {
            const run = group.runs.find((r) => r.caseIndex === m.caseIndex);
            const sweepVal = run && group.sweepParameter
              ? run.overrides[group.sweepParameter]
              : null;
            const isBest = m.mean === minMean && means.length > 1;
            const isWorst = m.mean === maxMean && means.length > 1;
            return (
              <tr
                key={m.runName}
                className={isBest ? "parametric-best" : isWorst ? "parametric-worst" : ""}
              >
                <td className="parametric-run-cell">
                  {run?.isBaseline && <span className="parametric-baseline-badge">base</span>}
                  {m.runName}
                </td>
                {group.sweepParameter && (
                  <td className="summary-num">{sweepVal != null ? String(sweepVal) : "-"}</td>
                )}
                <td className="summary-num">{fmt(m.min)}</td>
                <td className="summary-num">{fmt(m.max)}</td>
                <td className="summary-num">{fmt(m.mean)}</td>
                <td className="summary-num">{fmt(m.total)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** Overlay time-series comparison chart with stats table */
function ComparisonChart({
  group,
  cases,
  activeRuns,
  selectedVariable,
  unitSystem,
}: {
  group: ParametricGroup;
  cases: ResultCase[];
  activeRuns: ParametricRun[];
  selectedVariable: string;
  unitSystem: UnitSystem;
}) {
  const [showStats, setShowStats] = useState(true);
  const option = useMemo(() => {
    // Get time labels from first parsed run
    let labels: string[] = [];
    const series: { name: string; data: number[]; color: string }[] = [];

    for (const run of activeRuns) {
      const c = cases[run.caseIndex];
      if (!c?.parsed) continue;

      const ts = getRunTimeSeries(c.parsed, selectedVariable);
      if (!ts) continue;

      if (labels.length === 0) {
        labels = c.parsed.timesteps.map(
          (t) => `${t.month}/${String(t.day).padStart(2, "0")} ${String(t.hour).padStart(2, "0")}:00`
        );
      }

      const runIdx = group.runs.findIndex((r) => r.name === run.name);
      const unit = ts.variable.unit;
      const data = unitSystem === "IP" && unit && unit !== "-"
        ? ts.data.map((v) => convertValue(v, unit, unitSystem))
        : ts.data;

      series.push({
        name: run.name,
        data,
        color: RUN_COLORS[runIdx % RUN_COLORS.length],
      });
    }

    // Get display unit
    let displayUnit = "";
    for (const run of activeRuns) {
      const c = cases[run.caseIndex];
      if (!c?.parsed) continue;
      const v = c.parsed.variables.find((cv) => {
        const full = cv.component ? `${cv.component}:${cv.variable}` : cv.variable;
        return full === selectedVariable;
      });
      if (v) {
        displayUnit = getDisplayUnit(v.unit, unitSystem);
        break;
      }
    }

    return {
      backgroundColor: "transparent",
      animation: false,
      tooltip: {
        trigger: "axis" as const,
        backgroundColor: "#1f2033",
        borderColor: "#2f3146",
        textStyle: { color: "#c0caf5", fontSize: 11 },
        confine: true,
        valueFormatter: (value: unknown) => {
          if (typeof value !== "number" || !isFinite(value)) return String(value);
          const abs = Math.abs(value);
          if (abs >= 1e6) return value.toExponential(2);
          if (abs >= 100) return value.toLocaleString(undefined, { maximumFractionDigits: 1 });
          if (abs >= 1) return value.toLocaleString(undefined, { maximumFractionDigits: 2 });
          return value.toLocaleString(undefined, { maximumFractionDigits: 3 });
        },
      },
      legend: {
        show: true,
        bottom: 40,
        textStyle: { color: "#9aa5ce", fontSize: 10 },
      },
      grid: { left: 60, right: 30, top: 30, bottom: 100 },
      xAxis: {
        type: "category" as const,
        data: labels,
        axisLabel: { color: "#9aa5ce", fontSize: 10 },
        axisLine: { lineStyle: { color: "#2f3146" } },
      },
      yAxis: {
        type: "value" as const,
        name: displayUnit,
        axisLabel: { color: "#9aa5ce", fontSize: 10 },
        axisLine: { show: true, lineStyle: { color: "#2f3146" } },
        splitLine: { lineStyle: { color: "#2f3146", type: "dashed" as const } },
        nameTextStyle: { color: "#9aa5ce", fontSize: 11 },
      },
      series: series.map((s) => ({
        name: s.name,
        type: "line" as const,
        data: s.data,
        showSymbol: false,
        lineStyle: { width: 1.5, color: s.color },
        itemStyle: { color: s.color },
        sampling: "lttb" as const,
        large: true,
        largeThreshold: 5000,
      })),
      dataZoom: [
        { type: "inside" as const, xAxisIndex: 0, filterMode: "none" as const },
        {
          type: "slider" as const,
          xAxisIndex: 0,
          bottom: 5,
          height: 20,
          borderColor: "#2f3146",
          backgroundColor: "#1a1b26",
          fillerColor: "rgba(122, 162, 247, 0.15)",
          handleStyle: { color: "#7aa2f7" },
          textStyle: { color: "#9aa5ce", fontSize: 10 },
        },
      ],
    };
  }, [activeRuns, cases, selectedVariable, unitSystem, group.runs]);

  // Per-run stats for the stats table
  const runStats = useMemo(() => {
    return computeParametricSummary(activeRuns, cases, selectedVariable);
  }, [activeRuns, cases, selectedVariable]);

  // Find unit
  const unit = useMemo(() => {
    for (const run of activeRuns) {
      const c = cases[run.caseIndex];
      if (!c?.parsed) continue;
      const v = c.parsed.variables.find((cv) => {
        const full = cv.component ? `${cv.component}:${cv.variable}` : cv.variable;
        return full === selectedVariable;
      });
      if (v) return v.unit;
    }
    return "";
  }, [activeRuns, cases, selectedVariable]);

  const displayUnit = getDisplayUnit(unit, unitSystem);

  const fmt = (n: number) => {
    const converted = convertValue(n, unit, unitSystem);
    const abs = Math.abs(converted);
    if (abs === 0) return "0";
    if (abs >= 1e6) return converted.toExponential(2);
    if (abs >= 100) return converted.toLocaleString(undefined, { maximumFractionDigits: 1 });
    if (abs >= 1) return converted.toLocaleString(undefined, { maximumFractionDigits: 2 });
    return converted.toLocaleString(undefined, { maximumFractionDigits: 4 });
  };

  return (
    <div className="parametric-chart">
      <div className="parametric-chart-header">
        <h3 className="parametric-section-title">{selectedVariable}</h3>
        <button
          className={`btn-header btn-small ${showStats ? "active" : ""}`}
          onClick={() => setShowStats(!showStats)}
        >
          Stats
        </button>
      </div>
      <div className="chart-container">
        <ReactEChartsCore
          echarts={echarts}
          option={option}
          style={{ height: "100%", width: "100%" }}
          notMerge={true}
          lazyUpdate={true}
        />
      </div>
      {showStats && runStats.length > 0 && (
        <div className="summary-stats">
          <table className="summary-table">
            <thead>
              <tr>
                <th>Run</th>
                <th>Unit</th>
                <th>Min</th>
                <th>Max</th>
                <th>Mean</th>
                <th>Total</th>
              </tr>
            </thead>
            <tbody>
              {runStats.map((m) => {
                const runIdx = group.runs.findIndex((r) => r.caseIndex === m.caseIndex);
                return (
                  <tr key={m.runName}>
                    <td className="summary-var-name">
                      <span
                        className="parametric-run-color"
                        style={{ background: RUN_COLORS[runIdx % RUN_COLORS.length], display: "inline-block", verticalAlign: "middle", marginRight: 6 }}
                      />
                      {m.runName}
                    </td>
                    <td className="summary-unit">{displayUnit}</td>
                    <td className="summary-num">{fmt(m.min)}</td>
                    <td className="summary-num">{fmt(m.max)}</td>
                    <td className="summary-num">{fmt(m.mean)}</td>
                    <td className="summary-num">{fmt(m.total)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/** Sweep scatter/line chart: X = parameter value, Y = metric */
function SweepChart({
  group,
  cases,
  activeRuns,
  selectedVariable,
  unitSystem,
}: {
  group: ParametricGroup;
  cases: ResultCase[];
  activeRuns: ParametricRun[];
  selectedVariable: string;
  unitSystem: UnitSystem;
}) {
  const [metric, setMetric] = useState<"mean" | "total" | "min" | "max">("mean");

  const option = useMemo(() => {
    if (!group.sweepParameter) return {};

    const metrics = computeParametricSummary(activeRuns, cases, selectedVariable);

    // Find unit
    let unit = "";
    for (const run of activeRuns) {
      const c = cases[run.caseIndex];
      if (!c?.parsed) continue;
      const v = c.parsed.variables.find((cv) => {
        const full = cv.component ? `${cv.component}:${cv.variable}` : cv.variable;
        return full === selectedVariable;
      });
      if (v) { unit = v.unit; break; }
    }
    const displayUnit = getDisplayUnit(unit, unitSystem);

    const dataPoints: [number, number][] = [];
    for (const m of metrics) {
      const run = activeRuns.find((r) => r.caseIndex === m.caseIndex);
      if (!run) continue;
      const xVal = run.overrides[group.sweepParameter!];
      if (typeof xVal !== "number") continue;
      const yRaw = m[metric];
      const yVal = convertValue(yRaw, unit, unitSystem);
      dataPoints.push([xVal, yVal]);
    }

    dataPoints.sort((a, b) => a[0] - b[0]);

    return {
      backgroundColor: "transparent",
      animation: false,
      tooltip: {
        trigger: "item" as const,
        backgroundColor: "#1f2033",
        borderColor: "#2f3146",
        textStyle: { color: "#c0caf5", fontSize: 11 },
        formatter: (params: { value: [number, number] }) => {
          const [x, y] = params.value;
          return `${group.sweepParameter!.replace(/_/g, " ")}: ${x}<br/>${metric}: ${y.toLocaleString(undefined, { maximumFractionDigits: 2 })} ${displayUnit}`;
        },
      },
      grid: { left: 80, right: 30, top: 40, bottom: 60 },
      xAxis: {
        type: "value" as const,
        name: group.sweepParameter!.replace(/_/g, " "),
        axisLabel: { color: "#9aa5ce", fontSize: 10 },
        axisLine: { lineStyle: { color: "#2f3146" } },
        nameTextStyle: { color: "#9aa5ce", fontSize: 11 },
        nameLocation: "middle" as const,
        nameGap: 30,
      },
      yAxis: {
        type: "value" as const,
        name: `${metric} [${displayUnit}]`,
        axisLabel: { color: "#9aa5ce", fontSize: 10 },
        axisLine: { show: true, lineStyle: { color: "#2f3146" } },
        splitLine: { lineStyle: { color: "#2f3146", type: "dashed" as const } },
        nameTextStyle: { color: "#9aa5ce", fontSize: 11 },
      },
      series: [
        {
          type: "scatter" as const,
          data: dataPoints,
          symbolSize: 10,
          itemStyle: { color: "#7aa2f7" },
        },
        {
          type: "line" as const,
          data: dataPoints,
          showSymbol: false,
          lineStyle: { width: 2, color: "#7aa2f7", type: "dashed" as const },
          itemStyle: { color: "#7aa2f7" },
        },
      ],
    };
  }, [activeRuns, cases, selectedVariable, unitSystem, group, metric]);

  return (
    <div className="parametric-chart">
      <div className="parametric-sweep-header">
        <h3 className="parametric-section-title">
          {selectedVariable} vs {group.sweepParameter?.replace(/_/g, " ")}
        </h3>
        <div className="agg-buttons">
          {(["mean", "total", "min", "max"] as const).map((m) => (
            <button
              key={m}
              className={`btn-agg ${metric === m ? "active" : ""}`}
              onClick={() => setMetric(m)}
            >
              {m.charAt(0).toUpperCase() + m.slice(1)}
            </button>
          ))}
        </div>
      </div>
      <div className="chart-container">
        <ReactEChartsCore
          echarts={echarts}
          option={option}
          style={{ height: "100%", width: "100%" }}
          notMerge={true}
          lazyUpdate={true}
        />
      </div>
    </div>
  );
}
