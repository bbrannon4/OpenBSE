import { useMemo } from "react";
import ReactEChartsCore from "echarts-for-react/lib/core";
import * as echarts from "echarts/core";
import { BarChart } from "echarts/charts";
import {
  GridComponent,
  TooltipComponent,
  LegendComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import type { ResultCase } from "../App";
import type { UnitSystem } from "../lib/units";
import type { CsvVariable, ParsedCsv } from "../lib/csv";

echarts.use([BarChart, GridComponent, TooltipComponent, LegendComponent, CanvasRenderer]);

const MONTH_NAMES = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const KWH_TO_KBTU = 3.41214;

const END_USE_COLORS: Record<string, string> = {
  cooling: "#7aa2f7",
  heating: "#f7768e",
  fans: "#9aa5ce",
  fan: "#9aa5ce",
  pumps: "#bb9af7",
  pump: "#bb9af7",
  lighting: "#e0af68",
  lights: "#e0af68",
  equipment: "#9ece6a",
  dhw: "#73daca",
  hot_water: "#73daca",
  refrigeration: "#7dcfff",
};

function endUseColor(endUse: string): string {
  for (const [key, color] of Object.entries(END_USE_COLORS)) {
    if (endUse.includes(key)) return color;
  }
  return "#9aa5ce";
}

function formatFuelLabel(fuel: string): string {
  switch (fuel) {
    case "electricity": return "Electric";
    case "natural_gas": return "Gas";
    case "district_heat": return "District Heat";
    case "district_cool": return "District Cool";
    default: return fuel.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
  }
}

function formatEndUseLabel(endUse: string): string {
  return endUse.replace(/_kwh$/, "").replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

interface EndUseVariable {
  varIdx: number;
  variable: CsvVariable;
  fuel: string;
  endUse: string;
  label: string;
  color: string;
}

function findEnergyEndUses(parsed: ParsedCsv): EndUseVariable[] {
  const result: EndUseVariable[] = [];
  const fuelPrefixes = ["electricity", "natural_gas", "district_heat", "district_cool"];

  for (let i = 0; i < parsed.variables.length; i++) {
    const v = parsed.variables[i];
    if (v.component !== "") continue; // must be site-level

    const lower = v.variable.toLowerCase();
    let fuel = "";
    let endUse = "";

    for (const fp of fuelPrefixes) {
      if (lower.startsWith(fp + "_")) {
        fuel = fp;
        endUse = lower.slice(fp.length + 1).replace(/_kwh$/, "");
        break;
      }
    }

    if (!fuel && lower.endsWith("_kwh")) {
      fuel = "other";
      endUse = lower.replace(/_kwh$/, "");
    }

    if (fuel && endUse) {
      const label =
        fuel === "other"
          ? formatEndUseLabel(endUse)
          : `${formatFuelLabel(fuel)}: ${formatEndUseLabel(endUse)}`;
      result.push({
        varIdx: i,
        variable: v,
        fuel,
        endUse,
        label,
        color: endUseColor(endUse),
      });
    }
  }

  return result;
}

function aggregateMonthlySum(parsed: ParsedCsv, varIdx: number): number[] {
  const monthTotals = new Array<number>(12).fill(0);
  const col = parsed.data[varIdx];
  for (let i = 0; i < parsed.timesteps.length; i++) {
    const m = parsed.timesteps[i].month - 1;
    if (m >= 0 && m < 12) monthTotals[m] += col[i];
  }
  return monthTotals;
}

interface UnmetHoursEntry {
  varIdx: number;
  zone: string;
  type: "heating" | "cooling";
  total: number;
}

function findUnmetHours(parsed: ParsedCsv): UnmetHoursEntry[] {
  const result: UnmetHoursEntry[] = [];
  for (let i = 0; i < parsed.variables.length; i++) {
    const v = parsed.variables[i];
    const lower = v.variable.toLowerCase();
    if (lower === "unmet_hours_heating" || lower === "unmet_hours_cooling") {
      const col = parsed.data[i];
      let total = 0;
      for (let j = 0; j < col.length; j++) total += col[j];
      result.push({
        varIdx: i,
        zone: v.component,
        type: lower.includes("heating") ? "heating" : "cooling",
        total,
      });
    }
  }
  return result;
}

export interface EnergyViewProps {
  cases: ResultCase[];
  activeCaseIdx: number;
  setActiveCaseIdx: React.Dispatch<React.SetStateAction<number>>;
  loading: boolean;
  unitSystem: UnitSystem;
}

export function EnergyView({
  cases,
  activeCaseIdx,
  setActiveCaseIdx,
  loading,
  unitSystem,
}: EnergyViewProps) {
  const activeCase = cases[activeCaseIdx] ?? null;
  const parsed: ParsedCsv | null = activeCase?.parsed ?? null;

  const endUses = useMemo(() => (parsed ? findEnergyEndUses(parsed) : []), [parsed]);
  const unmetHours = useMemo(() => (parsed ? findUnmetHours(parsed) : []), [parsed]);

  const chartOption = useMemo(() => {
    if (!parsed || endUses.length === 0) return null;

    const multiplier = unitSystem === "IP" ? KWH_TO_KBTU : 1;
    const unit = unitSystem === "IP" ? "kBtu" : "kWh";

    const series = endUses.map((eu) => {
      const monthly = aggregateMonthlySum(parsed, eu.varIdx);
      return {
        name: eu.label,
        type: "bar" as const,
        stack: "total",
        data: monthly.map((v) => +(v * multiplier).toFixed(1)),
        itemStyle: { color: eu.color },
      };
    });

    return {
      backgroundColor: "transparent",
      tooltip: {
        trigger: "axis" as const,
        axisPointer: { type: "shadow" as const },
        backgroundColor: "#1a1b26",
        borderColor: "#414868",
        textStyle: { color: "#a9b1d6", fontSize: 12 },
        formatter: (paramsRaw: unknown): string => {
          if (!Array.isArray(paramsRaw) || paramsRaw.length === 0) return "";
          const params = paramsRaw as Array<{
            seriesName: string;
            value: number;
            color: string;
            name: string;
          }>;
          const total = params.reduce((s, p) => s + (p.value || 0), 0);
          let html = `<b>${params[0].name}</b><br/>`;
          for (const p of params) {
            if (p.value) {
              html += `<span style="display:inline-block;width:8px;height:8px;border-radius:50%;background:${p.color};margin-right:6px"></span>${p.seriesName}: ${p.value.toLocaleString()} ${unit}<br/>`;
            }
          }
          html += `<b>Total: ${total.toFixed(0)} ${unit}</b>`;
          return html;
        },
      },
      legend: {
        type: "scroll" as const,
        top: 0,
        textStyle: { color: "#a9b1d6", fontSize: 11 },
        pageIconColor: "#7aa2f7",
        pageTextStyle: { color: "#a9b1d6" },
      },
      grid: { top: 50, left: 60, right: 20, bottom: 36, containLabel: false },
      xAxis: {
        type: "category" as const,
        data: MONTH_NAMES,
        axisLabel: { color: "#a9b1d6", fontSize: 11 },
        axisLine: { lineStyle: { color: "#414868" } },
        axisTick: { lineStyle: { color: "#414868" } },
      },
      yAxis: {
        type: "value" as const,
        name: unit,
        nameTextStyle: { color: "#a9b1d6", fontSize: 11 },
        axisLabel: {
          color: "#a9b1d6",
          fontSize: 11,
          formatter: (v: number) => v >= 1000 ? `${(v / 1000).toFixed(0)}k` : String(v),
        },
        splitLine: { lineStyle: { color: "#2d3149" } },
      },
      series,
    };
  }, [parsed, endUses, unitSystem]);

  const annualSummary = useMemo(() => {
    if (!parsed || endUses.length === 0) return [];
    const multiplier = unitSystem === "IP" ? KWH_TO_KBTU : 1;
    const rows = endUses.map((eu) => {
      const monthly = aggregateMonthlySum(parsed, eu.varIdx);
      const annual = monthly.reduce((s, v) => s + v, 0) * multiplier;
      return { label: eu.label, color: eu.color, annual };
    });
    const total = rows.reduce((s, r) => s + r.annual, 0);
    return rows
      .map((r) => ({ ...r, pct: total > 0 ? (r.annual / total) * 100 : 0 }))
      .sort((a, b) => b.annual - a.annual);
  }, [parsed, endUses, unitSystem]);

  const unmetSummary = useMemo(() => {
    const heatingTotal = unmetHours
      .filter((u) => u.type === "heating")
      .reduce((s, u) => s + u.total, 0);
    const coolingTotal = unmetHours
      .filter((u) => u.type === "cooling")
      .reduce((s, u) => s + u.total, 0);
    const byZone = new Map<string, { heating: number; cooling: number }>();
    for (const u of unmetHours) {
      const z = byZone.get(u.zone) ?? { heating: 0, cooling: 0 };
      if (u.type === "heating") z.heating = u.total;
      else z.cooling = u.total;
      byZone.set(u.zone, z);
    }
    return { heatingTotal, coolingTotal, byZone };
  }, [unmetHours]);

  const unit = unitSystem === "IP" ? "kBtu" : "kWh";

  if (cases.length === 0) {
    return (
      <div className="energy-view">
        <div className="results-empty">
          <p>No results loaded</p>
          <p className="hint">Use "Open CSV" or "Open Folder" to load results.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="energy-view">
      <div className="results-toolbar">
        <div className="results-toolbar-left">
          {cases.length > 1 && (
            <select
              className="field-select"
              value={activeCaseIdx}
              disabled={loading}
              onChange={(e) => setActiveCaseIdx(parseInt(e.target.value, 10))}
            >
              {cases.map((c, i) => (
                <option key={i} value={i}>
                  {c.name}
                  {c.parsed ? "" : " (not loaded)"}
                </option>
              ))}
            </select>
          )}
          {cases.length === 1 && (
            <span className="results-case-name">{activeCase?.name}</span>
          )}
          <span className="results-info">Energy End-Use Dashboard</span>
        </div>
      </div>

      <div className="energy-body">
        {loading && (
          <div className="results-loading-overlay">
            <div className="results-loading-spinner">Loading results...</div>
          </div>
        )}

        {parsed && endUses.length === 0 && (
          <div className="results-empty">
            <p>No energy end-use data found.</p>
            <p className="hint">
              Make sure the simulation has run and outputs includes end-use variables,
              or that the CSV file contains site-level energy columns.
            </p>
          </div>
        )}

        {parsed && endUses.length > 0 && (
          <div className="energy-content">
            <div className="energy-section">
              <h3 className="energy-section-title">Monthly Energy by End Use</h3>
              <div className="energy-chart-container">
                {chartOption && (
                  <ReactEChartsCore
                    echarts={echarts}
                    option={chartOption}
                    style={{ width: "100%", height: "100%" }}
                    notMerge
                  />
                )}
              </div>
            </div>

            <div className="energy-section">
              <h3 className="energy-section-title">Annual End-Use Summary</h3>
              <div className="energy-table-wrapper">
                <table className="energy-table">
                  <thead>
                    <tr>
                      <th>End Use</th>
                      <th className="energy-col-num">Annual Total ({unit})</th>
                      <th className="energy-col-num">% of Total</th>
                    </tr>
                  </thead>
                  <tbody>
                    {annualSummary.map((row) => (
                      <tr key={row.label}>
                        <td>
                          <span
                            className="energy-color-dot"
                            style={{ background: row.color }}
                          />
                          {row.label}
                        </td>
                        <td className="energy-col-num">
                          {row.annual.toLocaleString(undefined, { maximumFractionDigits: 0 })}
                        </td>
                        <td className="energy-col-num">{row.pct.toFixed(1)}%</td>
                      </tr>
                    ))}
                    <tr className="energy-total-row">
                      <td>Total</td>
                      <td className="energy-col-num">
                        {annualSummary
                          .reduce((s, r) => s + r.annual, 0)
                          .toLocaleString(undefined, { maximumFractionDigits: 0 })}
                      </td>
                      <td className="energy-col-num">100%</td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>

            {unmetHours.length > 0 && (
              <div className="energy-section">
                <h3 className="energy-section-title">Unmet Hours</h3>
                <div className="energy-unmet-summary">
                  <div
                    className={`energy-unmet-stat ${unmetSummary.heatingTotal > 0 ? "warn" : "ok"}`}
                  >
                    <span className="energy-unmet-label">Heating Unmet</span>
                    <span className="energy-unmet-value">
                      {unmetSummary.heatingTotal.toFixed(0)} hrs
                    </span>
                  </div>
                  <div
                    className={`energy-unmet-stat ${unmetSummary.coolingTotal > 0 ? "warn" : "ok"}`}
                  >
                    <span className="energy-unmet-label">Cooling Unmet</span>
                    <span className="energy-unmet-value">
                      {unmetSummary.coolingTotal.toFixed(0)} hrs
                    </span>
                  </div>
                </div>
                {(unmetSummary.heatingTotal > 0 || unmetSummary.coolingTotal > 0) && (
                  <div className="energy-table-wrapper">
                    <table className="energy-table">
                      <thead>
                        <tr>
                          <th>Zone</th>
                          <th className="energy-col-num">Heating Unmet (hrs)</th>
                          <th className="energy-col-num">Cooling Unmet (hrs)</th>
                        </tr>
                      </thead>
                      <tbody>
                        {Array.from(unmetSummary.byZone.entries())
                          .filter(([, v]) => v.heating > 0 || v.cooling > 0)
                          .sort(
                            ([, a], [, b]) =>
                              b.heating + b.cooling - (a.heating + a.cooling)
                          )
                          .map(([zone, v]) => (
                            <tr key={zone}>
                              <td>{zone}</td>
                              <td
                                className={`energy-col-num${v.heating > 0 ? " warn-text" : ""}`}
                              >
                                {v.heating.toFixed(0)}
                              </td>
                              <td
                                className={`energy-col-num${v.cooling > 0 ? " warn-text" : ""}`}
                              >
                                {v.cooling.toFixed(0)}
                              </td>
                            </tr>
                          ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </div>
            )}
          </div>
        )}

        {!parsed && !loading && (
          <div className="results-empty">
            <p>Select a case to view energy data</p>
          </div>
        )}
      </div>
    </div>
  );
}
