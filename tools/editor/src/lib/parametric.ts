/**
 * Parametric run detection and analysis utilities.
 *
 * Detects parametric runs from file naming patterns and YAML model config,
 * computes cross-run summary statistics, and provides data for comparison charts.
 */

import type { ParsedCsv, CsvVariable } from "./csv";
import type { ResultCase } from "../App";

export interface ParametricRun {
  /** Run name (e.g. "baseline", "tight_setpoints", "sweep_heating_setpoint_22") */
  name: string;
  /** Index into the ResultCase array */
  caseIndex: number;
  /** Parameter overrides for this run (parsed from YAML or inferred from name) */
  overrides: Record<string, number | string>;
  /** Whether this is the baseline run */
  isBaseline: boolean;
}

export interface ParametricGroup {
  /** All detected parametric runs */
  runs: ParametricRun[];
  /** Sweep parameter name (if detected), e.g. "heating_setpoint" */
  sweepParameter: string | null;
  /** Sweep values in order (if detected) */
  sweepValues: number[];
}

/**
 * Detect parametric runs from a set of loaded result cases.
 *
 * Looks for the pattern: short-name CSVs like "baseline.csv",
 * "tight_setpoints.csv", "sweep_heating_setpoint_22.csv"
 * that are NOT the main model results or sizing files.
 */
export function detectParametricRuns(
  cases: ResultCase[],
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  model?: Record<string, any>
): ParametricGroup | null {
  // Filter to only short-name CSVs (not *_results.csv, not *_sizing.csv, not *_summary.*)
  const candidateIndices: number[] = [];
  for (let i = 0; i < cases.length; i++) {
    const name = cases[i].name;
    if (
      !name.endsWith("_results.csv") &&
      !name.includes("sizing") &&
      !name.includes("summary") &&
      name.endsWith(".csv")
    ) {
      candidateIndices.push(i);
    }
  }

  if (candidateIndices.length < 2) return null;

  // Try to extract run info from model YAML
  const modelRuns = model?.parametrics?.runs as
    | { name: string; overrides?: Record<string, unknown> }[]
    | undefined;

  const runs: ParametricRun[] = [];
  let sweepParameter: string | null = null;
  const sweepValues: number[] = [];

  for (const idx of candidateIndices) {
    const name = cases[idx].name.replace(/\.csv$/, "");

    // Try to match to a model-defined run
    const modelRun = modelRuns?.find((r) => r.name === name);
    const overrides: Record<string, number | string> = {};

    if (modelRun?.overrides) {
      for (const [k, v] of Object.entries(modelRun.overrides)) {
        overrides[k] = v as number | string;
      }
    }

    // Detect sweep pattern: "sweep_{param}_{value}"
    const sweepMatch = /^sweep_(.+?)_([\d.]+)$/.exec(name);
    if (sweepMatch) {
      const param = sweepMatch[1];
      const value = parseFloat(sweepMatch[2]);
      if (!sweepParameter) sweepParameter = param;
      if (sweepParameter === param) {
        sweepValues.push(value);
        overrides[param] = value;
      }
    }

    runs.push({
      name,
      caseIndex: idx,
      overrides,
      isBaseline: name === "baseline" || name === "base",
    });
  }

  // Sort: baseline first, then by sweep value or name
  runs.sort((a, b) => {
    if (a.isBaseline && !b.isBaseline) return -1;
    if (!a.isBaseline && b.isBaseline) return 1;
    // If both are sweeps, sort by value
    if (sweepParameter) {
      const aVal = a.overrides[sweepParameter];
      const bVal = b.overrides[sweepParameter];
      if (typeof aVal === "number" && typeof bVal === "number") {
        return aVal - bVal;
      }
    }
    return a.name.localeCompare(b.name);
  });

  sweepValues.sort((a, b) => a - b);

  return { runs, sweepParameter, sweepValues };
}

/** Summary metric for a variable across all runs */
export interface RunMetric {
  runName: string;
  caseIndex: number;
  min: number;
  max: number;
  mean: number;
  total: number;
}

/**
 * Compute a summary metric for a specific variable across all parametric runs.
 * Returns one row per run.
 */
export function computeParametricSummary(
  runs: ParametricRun[],
  cases: ResultCase[],
  variableName: string
): RunMetric[] {
  const results: RunMetric[] = [];

  for (const run of runs) {
    const c = cases[run.caseIndex];
    if (!c?.parsed) continue;

    // Find the variable by matching component:variable
    const v = c.parsed.variables.find((cv) => {
      const full = cv.component ? `${cv.component}:${cv.variable}` : cv.variable;
      return full === variableName || cv.variable === variableName || cv.raw === variableName;
    });

    if (!v) continue;

    const data = c.parsed.data[v.columnIndex];
    let min = Infinity, max = -Infinity, sum = 0, count = 0;
    for (let i = 0; i < data.length; i++) {
      const val = data[i];
      if (!isFinite(val)) continue;
      if (val < min) min = val;
      if (val > max) max = val;
      sum += val;
      count++;
    }

    results.push({
      runName: run.name,
      caseIndex: run.caseIndex,
      min: count > 0 ? min : 0,
      max: count > 0 ? max : 0,
      mean: count > 0 ? sum / count : 0,
      total: sum,
    });
  }

  return results;
}

/**
 * Get time-series data for a variable from a specific parsed CSV.
 * Returns the raw array of values.
 */
export function getRunTimeSeries(
  parsed: ParsedCsv,
  variableName: string
): { variable: CsvVariable; data: number[] } | null {
  const v = parsed.variables.find((cv) => {
    const full = cv.component ? `${cv.component}:${cv.variable}` : cv.variable;
    return full === variableName || cv.variable === variableName || cv.raw === variableName;
  });

  if (!v) return null;
  return { variable: v, data: Array.from(parsed.data[v.columnIndex]) };
}

/**
 * Get all common variables across all parsed runs.
 * Returns variables that exist in every run (by component:variable name).
 */
export function getCommonVariables(
  runs: ParametricRun[],
  cases: ResultCase[]
): CsvVariable[] {
  const parsedRuns = runs
    .map((r) => cases[r.caseIndex])
    .filter((c) => c?.parsed) as (ResultCase & { parsed: ParsedCsv })[];

  if (parsedRuns.length === 0) return [];

  // Use first run's variables as reference
  const firstVars = parsedRuns[0].parsed.variables;

  // Build set of variable keys from each run
  const allSets = parsedRuns.map((r) => {
    const s = new Set<string>();
    for (const v of r.parsed.variables) {
      s.add(v.component ? `${v.component}:${v.variable}` : v.variable);
    }
    return s;
  });

  // Keep only variables that appear in all runs
  return firstVars.filter((v) => {
    const key = v.component ? `${v.component}:${v.variable}` : v.variable;
    return allSets.every((s) => s.has(key));
  });
}
