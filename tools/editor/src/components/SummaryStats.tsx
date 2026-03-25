import { useMemo } from "react";
import type { ParsedCsv } from "../lib/csv";
import { computeStats } from "../lib/csv";
import { convertValue, getDisplayUnit, type UnitSystem } from "../lib/units";

interface SummaryStatsProps {
  parsed: ParsedCsv;
  selectedVarIndices: Set<number>;
  onToggleVariable?: (idx: number) => void;
  unitSystem?: UnitSystem;
}

function formatNumber(n: number): string {
  if (Math.abs(n) >= 1e6) return n.toExponential(2);
  if (Math.abs(n) >= 1000) return n.toFixed(1);
  if (Math.abs(n) >= 1) return n.toFixed(2);
  if (n === 0) return "0";
  return n.toFixed(4);
}

/** Check if a variable represents energy/power that should show totals */
function isEnergyVariable(variable: string): boolean {
  const vl = variable.toLowerCase();
  return (
    vl.includes("energy") ||
    vl.includes("power") ||
    vl.includes("load") ||
    vl.includes("rate") ||
    vl.includes("thermal_output")
  );
}

export function SummaryStats({ parsed, selectedVarIndices, onToggleVariable, unitSystem = "SI" }: SummaryStatsProps) {
  const stats = useMemo(() => {
    const result: {
      raw: string;
      columnIndex: number;
      variable: string;
      unit: string;
      stats: ReturnType<typeof computeStats>;
      showTotal: boolean;
    }[] = [];

    for (const idx of selectedVarIndices) {
      const v = parsed.variables[idx];
      const s = computeStats(parsed.data[idx]);
      result.push({
        raw: v.raw,
        columnIndex: v.columnIndex,
        variable: `${v.component ? v.component + ":" : ""}${v.variable}`,
        unit: v.unit,
        stats: s,
        showTotal: isEnergyVariable(v.variable),
      });
    }

    return result;
  }, [parsed, selectedVarIndices]);

  if (stats.length === 0) {
    return (
      <div className="summary-stats">
        <div className="summary-empty">No variables selected</div>
      </div>
    );
  }

  return (
    <div className="summary-stats">
      <table className="summary-table">
        <thead>
          <tr>
            <th className="summary-col-remove"></th>
            <th>Variable</th>
            <th>Unit</th>
            <th>Min</th>
            <th>Max</th>
            <th>Mean</th>
            <th>Total</th>
          </tr>
        </thead>
        <tbody>
          {stats.map((s) => (
            <tr key={s.raw}>
              <td className="summary-col-remove">
                {onToggleVariable && (
                  <button
                    className="btn-remove-var"
                    onClick={() => onToggleVariable(s.columnIndex)}
                    title="Remove from selection"
                  >
                    x
                  </button>
                )}
              </td>
              <td className="summary-var-name" title={s.raw}>
                {s.variable}
              </td>
              <td className="summary-unit">{getDisplayUnit(s.unit, unitSystem)}</td>
              <td className="summary-num">{formatNumber(convertValue(s.stats.min, s.unit, unitSystem))}</td>
              <td className="summary-num">{formatNumber(convertValue(s.stats.max, s.unit, unitSystem))}</td>
              <td className="summary-num">{formatNumber(convertValue(s.stats.mean, s.unit, unitSystem))}</td>
              <td className="summary-num">
                {s.showTotal ? formatNumber(convertValue(s.stats.total, s.unit, unitSystem)) : "\u2014"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
