// Saves every shipped model through the Workbench's YAML writer with a set of
// edits and checks that the result reads back as the edited model and changes
// only the edited lines.
//   cd tools/editor && node --experimental-strip-types scripts/check-yaml-save.mts
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { parseModel, serializeModel, type Model } from "../src/lib/yaml-doc.ts";

const root = join(import.meta.dirname, "../../..");
const files: string[] = [];
const walk = (d: string) => {
  for (const f of readdirSync(d)) {
    const p = join(d, f);
    if (statSync(p).isDirectory()) walk(p);
    else if (p.endsWith(".yaml")) files.push(p);
  }
};
for (const d of ["examples", "prototype_tests", "140_tests/cases"]) walk(join(root, d));

/** Lines differing between `a` and `b` once their common head and tail are removed. */
function changedLines(a: string, b: string): number {
  const x = a.split("\n");
  const y = b.split("\n");
  let i = 0;
  while (i < x.length && x[i] === y[i]) i++;
  let j = 0;
  while (j < x.length - i && j < y.length - i && x[x.length - 1 - j] === y[y.length - 1 - j]) j++;
  return Math.max(x.length - i - j, y.length - i - j);
}

// Each edit and the most lines it may change (a removed item may be long).
const edits: [string, number, (m: Model) => void][] = [
  ["unchanged", 0, () => {}],
  ["number", 1, (m) => { if (m.simulation) m.simulation.timesteps_per_hour = 7; }],
  ["add key", 1, (m) => { if (m.zones?.[0]) m.zones[0].note = "x"; }],
  ["remove key", 1, (m) => { if (m.zones?.[0]) delete m.zones[0].volume; }],
  ["string", 1, (m) => { if (m.schedules?.[0]) m.schedules[0].name = "Renamed"; }],
  ["append item", 6, (m) => { (m.schedules ??= []).push({ name: "New", weekday: [1, 1, 0.5] }); }],
  ["remove item", 200, (m) => { if (m.schedules?.length) m.schedules.splice(0, 1); }],
  ["new block", 6, (m) => { m.comfort_grid = { spacing: 0.5, zones: (m.zones ?? []).slice(0, 2).map((z: Model) => z.name) }; }],
  ["boundary", 1, (m) => {
    const s = (m.surfaces ?? []).find((x: Model) => x.boundary && typeof x.boundary === "object");
    if (s) s.boundary = { zone: `${s.boundary.zone} B` };
  }],
  ["flow list", 4, (m) => {
    const s = (m.schedules ?? []).find((x: Model) => Array.isArray(x.weekday));
    if (s) s.weekday = [...s.weekday, 0];
  }],
];

let failures = 0;
for (const f of files) {
  const text = readFileSync(f, "utf8");
  for (const [name, limit, edit] of edits) {
    const { source, model } = parseModel(text);
    const next = structuredClone(model);
    edit(next);
    const out = serializeModel(source, structuredClone(model), next);
    const back = parseModel(out).model;
    const lines = changedLines(text, out);
    if (JSON.stringify(back) !== JSON.stringify(next) || lines > limit) {
      failures++;
      console.log(`FAIL ${name}: ${f.slice(root.length + 1)} (${lines} lines changed)`);
    }
  }
}
console.log(`${files.length} models × ${edits.length} edits, ${failures} failure(s)`);
process.exit(failures > 0 ? 1 : 0);
