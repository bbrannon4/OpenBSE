/**
 * Parser for the engine's `<model>_summary.csv` report (written when
 * `summary_report: true`). The file is a sequence of titled sections separated
 * by blank lines, e.g.:
 *
 *   Monthly Energy End-Use [kWh]
 *   End Use,Jan,...,Dec,Total
 *   Interior Lighting,108.4,...,1280.1
 *
 *   Unmet Hours
 *   Unmet Heating Hours,732.0
 */

export interface SummaryEndUse {
  label: string;
  /** 12 monthly values [kWh] */
  monthly: number[];
  total: number;
}

export interface SummaryReport {
  endUses: SummaryEndUse[];
  totals: SummaryEndUse[];
  unmetHeatingHours: number | null;
  unmetCoolingHours: number | null;
}

function splitSections(text: string): string[][] {
  const sections: string[][] = [];
  let current: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === "") {
      if (current.length > 0) sections.push(current);
      current = [];
    } else {
      current.push(line);
    }
  }
  if (current.length > 0) sections.push(current);
  return sections;
}

function parseMonthlyRow(line: string): SummaryEndUse | null {
  const cells = line.split(",");
  if (cells.length < 14) return null;
  const nums = cells.slice(1, 14).map((c) => parseFloat(c));
  if (nums.some((n) => !Number.isFinite(n))) return null;
  return { label: cells[0], monthly: nums.slice(0, 12), total: nums[12] };
}

export function parseSummaryCsv(text: string): SummaryReport {
  const report: SummaryReport = {
    endUses: [],
    totals: [],
    unmetHeatingHours: null,
    unmetCoolingHours: null,
  };
  for (const section of splitSections(text)) {
    const title = section[0].toLowerCase();
    if (title.startsWith("monthly energy end-use")) {
      for (const line of section.slice(2)) {
        const row = parseMonthlyRow(line);
        if (!row) continue;
        if (/^total\b/i.test(row.label)) report.totals.push(row);
        else report.endUses.push(row);
      }
    } else if (title.startsWith("unmet hours")) {
      for (const line of section.slice(1)) {
        const [label, value] = line.split(",");
        const v = parseFloat(value);
        if (!Number.isFinite(v)) continue;
        const l = label.toLowerCase();
        if (l.includes("heating")) report.unmetHeatingHours = v;
        else if (l.includes("cooling")) report.unmetCoolingHours = v;
      }
    }
  }
  return report;
}

/** Path of the summary report that sits next to a results CSV, if derivable. */
export function summaryPathFor(resultsPath: string): string | null {
  if (/_summary\.csv$/i.test(resultsPath)) return resultsPath;
  if (/_results\.csv$/i.test(resultsPath)) {
    return resultsPath.replace(/_results\.csv$/i, "_summary.csv");
  }
  return null;
}
