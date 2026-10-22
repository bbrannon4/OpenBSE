/**
 * Comfort-grid data (`<stem>_comfort.json`, format version 1) and the math to
 * recompute any hour from it in the browser.
 *
 * The engine writes view factors and hourly surface temperatures instead of
 * every point for every hour, so MRT, sun on the occupant and PMV are
 * recomputed here. The formulas mirror the engine's
 * (`openbse-psychrometrics/src/comfort.rs`, `openbse-envelope/src/comfort_grid.rs`)
 * so the tab shows the same numbers the annual statistics were built from.
 *
 * Zone air is well mixed: air temperature and humidity are uniform within a
 * zone, so every spatial difference here is radiant or solar.
 */

export interface ComfortSurface {
  name: string;
  zone: string;
  type: "wall" | "floor" | "roof" | "ceiling" | "window";
  vertices: [number, number, number][];
  inside_temperature?: number[];
  beam_irradiance?: number[];
}

export interface ComfortAggregates {
  occupied_hours: number;
  comfortable_hours: number[];
  cold_hours: number[];
  hot_hours: number[];
  sun_hours: number[];
  mean_operative_temperature: (number | null)[];
  max_operative_temperature: (number | null)[];
}

export interface ComfortZone {
  name: string;
  comfort: {
    metabolic_rate: number;
    clothing: number;
    air_velocity: number;
    solar_absorptivity: number;
  };
  coverage_min: number;
  points: [number, number, number][];
  surfaces: number[];
  view_factors: number[];
  air_temperature: number[];
  relative_humidity: number[];
  occupancy: number[];
  aggregates: ComfortAggregates;
}

export interface ComfortData {
  format: string;
  version: number;
  note: string;
  settings: {
    spacing: number;
    wall_offset: number;
    height: number;
    occupant: string;
    pmv_limit: number;
  };
  time: { start_hour_of_year: number; hours: number; resolution: string };
  sun: { x: number[]; y: number[]; z: number[] };
  surfaces: ComfortSurface[];
  zones: ComfortZone[];
  skipped_zones: { name: string; reason: string }[];
}

export function parseComfortData(text: string): ComfortData {
  const data = JSON.parse(text) as ComfortData;
  if (data.format !== "openbse-comfort-grid") {
    throw new Error("not an OpenBSE comfort-grid file");
  }
  if (data.version !== 1) {
    throw new Error(`comfort-grid format version ${data.version} is not supported`);
  }
  return data;
}

// ─── Comfort correlations (ports of comfort.rs) ─────────────────────────────

const SIGMA = 5.6704e-8;
const KELVIN = 273.15;
const BODY_EMISSIVITY = 0.95;

/** Saturation vapour pressure over water/ice [Pa] (Hyland & Wexler). */
function psatFnTemp(tDb: number): number {
  const t = tDb + KELVIN;
  if (t < 173.15) return 0.001405102123874164;
  if (t > 473.15) return 1555073.745636215;
  if (t < 273.16) {
    return Math.exp(
      -5674.5359 / t +
        6.3925247 +
        t * (-0.9677843e-2 + t * (0.62215701e-6 + t * (0.20747825e-8 + -0.9484024e-12 * t))) +
        4.1635019 * Math.log(t)
    );
  }
  return Math.exp(
    -5800.2206 / t +
      1.3914993 +
      t * (-0.048640239 + t * (0.41764768e-4 + t * -0.14452093e-7)) +
      6.5459673 * Math.log(t)
  );
}

function convectiveCoeff(tCl: number, tAir: number, vAir: number): number {
  const natural = 2.38 * Math.pow(Math.abs(tCl - tAir), 0.25);
  const forced = 12.1 * Math.sqrt(Math.max(vAir, 0));
  return Math.max(natural, forced);
}

function clothingSurfaceTemp(
  mw: number, iCl: number, fCl: number, tAir: number, tMrt: number, vAir: number
): number {
  let tCl = 35.7 - 0.028 * mw;
  const mrtK4 = Math.pow(tMrt + KELVIN, 4);
  for (let i = 0; i < 150; i++) {
    const hC = convectiveCoeff(tCl, tAir, vAir);
    const next =
      35.7 - 0.028 * mw - iCl * (3.96e-8 * fCl * (Math.pow(tCl + KELVIN, 4) - mrtK4) + fCl * hC * (tCl - tAir));
    const damped = 0.5 * (tCl + next);
    if (Math.abs(damped - tCl) < 0.00015) return damped;
    tCl = damped;
  }
  return tCl;
}

/** Fanger PMV (ISO 7730), identical to the engine's. `rh` is 0–1. */
export function pmv(
  tAir: number, tMrt: number, vAir: number, rh: number, met: number, clo: number
): number {
  const mw = met * 58.15;
  const iCl = clo * 0.155;
  const fCl = iCl <= 0.078 ? 1.0 + 1.29 * iCl : 1.05 + 0.645 * iCl;
  const pA = Math.min(Math.max(rh, 0), 1) * psatFnTemp(tAir);
  const tCl = clothingSurfaceTemp(mw, iCl, fCl, tAir, tMrt, vAir);
  const hC = convectiveCoeff(tCl, tAir, vAir);
  const heatLoss =
    3.05e-3 * (5733.0 - 6.99 * mw - pA) +
    0.42 * Math.max(mw - 58.15, 0) +
    1.7e-5 * mw * (5867.0 - pA) +
    0.0014 * mw * (34.0 - tAir) +
    3.96e-8 * fCl * (Math.pow(tCl + KELVIN, 4) - Math.pow(tMrt + KELVIN, 4)) +
    fCl * hC * (tCl - tAir);
  return (0.303 * Math.exp(-0.036 * mw) + 0.028) * (mw - heatLoss);
}

export function ppd(pmvValue: number): number {
  const p = Math.min(Math.max(pmvValue, -3), 3);
  return 100 - 95 * Math.exp(-0.03353 * p ** 4 - 0.2179 * p ** 2);
}

/** SolarCal direct-beam MRT increment [K]; same as the engine's. */
export function solarMrtCorrection(
  beam: number, altitude: number, tMrt: number, alphaSw: number
): number {
  if (altitude <= 0 || beam <= 0) return 0;
  const a = Math.min(Math.max(altitude, 0), Math.PI / 2);
  const fp = 0.308 * Math.cos(0.998 * a - 0.0875 * a * a);
  return (alphaSw * beam * fp) / (BODY_EMISSIVITY * SIGMA * 4 * Math.pow(tMrt + KELVIN, 3));
}

// ─── Geometry ────────────────────────────────────────────────────────────────

type V3 = [number, number, number];

const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: V3, b: V3): V3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const norm = (a: V3): V3 => {
  const m = Math.hypot(a[0], a[1], a[2]) || 1;
  return [a[0] / m, a[1] / m, a[2] / m];
};

export function newellNormal(v: V3[]): V3 {
  let nx = 0, ny = 0, nz = 0;
  for (let i = 0; i < v.length; i++) {
    const a = v[i], b = v[(i + 1) % v.length];
    nx += (a[1] - b[1]) * (a[2] + b[2]);
    ny += (a[2] - b[2]) * (a[0] + b[0]);
    nz += (a[0] - b[0]) * (a[1] + b[1]);
  }
  return [nx, ny, nz];
}

/** A window as a ray target, in its own plane's 2D frame. */
interface Aperture {
  column: number;
  origin: V3;
  u: V3;
  v: V3;
  normal: V3;
  outline: [number, number][];
}

function makeAperture(column: number, verts: V3[]): Aperture {
  const normal = norm(newellNormal(verts));
  const origin = verts[0];
  const u = norm(sub(verts[1], verts[0]));
  const v = norm(cross(normal, u));
  const outline = verts.map((p) => {
    const d = sub(p, origin);
    return [dot(d, u), dot(d, v)] as [number, number];
  });
  return { column, origin, u, v, normal, outline };
}

function pointInPolygon(x: number, y: number, poly: [number, number][]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i];
    const [xj, yj] = poly[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/**
 * Beam irradiance reaching a point through the zone's windows, as in the
 * engine: winding-independent, and the max over windows so overlapping
 * coplanar glass does not double count.
 */
function beamAt(p: V3, sun: V3, apertures: Aperture[], beamByColumn: number[]): number {
  let best = 0;
  for (const ap of apertures) {
    const e = beamByColumn[ap.column];
    if (!(e > 0)) continue;
    const denom = dot(sun, ap.normal);
    if (Math.abs(denom) <= 1e-9) continue;
    const t = dot(sub(ap.origin, p), ap.normal) / denom;
    if (t <= 1e-9) continue;
    const hit: V3 = [p[0] + sun[0] * t, p[1] + sun[1] * t, p[2] + sun[2] * t];
    const d = sub(hit, ap.origin);
    if (pointInPolygon(dot(d, ap.u), dot(d, ap.v), ap.outline)) best = Math.max(best, e);
  }
  return best;
}

// ─── Per-hour evaluation ─────────────────────────────────────────────────────

export type PointMetric = "operative" | "mrt" | "solar" | "pmv" | "ppd";

export interface HourValues {
  /** Per zone, per point. */
  mrt: Float32Array[];
  solar: Float32Array[];
  operative: Float32Array[];
  /** Filled only when PMV or PPD is requested (it is the costly part). */
  pmv: Float32Array[] | null;
  airTemperature: number[];
  relativeHumidity: number[];
  occupancy: number[];
}

/** Precomputed per-zone geometry for repeated hourly evaluation. */
export interface ZoneModel {
  zone: ComfortZone;
  apertures: Aperture[];
  columnSurfaces: ComfortSurface[];
}

export function buildZoneModels(data: ComfortData): ZoneModel[] {
  return data.zones.map((zone) => {
    const columnSurfaces = zone.surfaces.map((i) => data.surfaces[i]);
    const apertures = columnSurfaces
      .map((s, column) => (s.type === "window" ? makeAperture(column, s.vertices) : null))
      .filter((a): a is Aperture => a !== null);
    return { zone, apertures, columnSurfaces };
  });
}

/**
 * PMV for every point of a zone-hour. Only MRT varies across a zone, and PMV
 * is smooth in MRT, so PMV is evaluated exactly at nodes 0.25 K apart and
 * interpolated; the error is far below the 0.1 resolution PMV is read at.
 */
function zonePmv(
  mrtEff: Float32Array, tAir: number, rh: number, c: ComfortZone["comfort"]
): Float32Array {
  const out = new Float32Array(mrtEff.length);
  if (mrtEff.length === 0) return out;
  let lo = Infinity, hi = -Infinity;
  for (const m of mrtEff) { lo = Math.min(lo, m); hi = Math.max(hi, m); }
  const step = 0.25;
  const n = Math.max(2, Math.ceil((hi - lo) / step) + 1);
  const nodes = new Float64Array(n);
  for (let k = 0; k < n; k++) {
    nodes[k] = pmv(tAir, lo + k * step, c.air_velocity, rh, c.metabolic_rate, c.clothing);
  }
  for (let i = 0; i < mrtEff.length; i++) {
    const x = (mrtEff[i] - lo) / step;
    const k = Math.min(n - 2, Math.max(0, Math.floor(x)));
    const f = x - k;
    out[i] = nodes[k] * (1 - f) + nodes[k + 1] * f;
  }
  return out;
}

export function evaluateHour(
  data: ComfortData, models: ZoneModel[], hour: number, withPmv: boolean
): HourValues {
  const sun: V3 = [data.sun.x[hour], data.sun.y[hour], data.sun.z[hour]];
  const sunUp = sun[2] > 0;
  const altitude = sunUp ? Math.asin(Math.min(1, sun[2])) : 0;
  const result: HourValues = {
    mrt: [], solar: [], operative: [], pmv: withPmv ? [] : null,
    airTemperature: [], relativeHumidity: [], occupancy: [],
  };

  for (const { zone, apertures, columnSurfaces } of models) {
    const nS = columnSurfaces.length;
    const t4 = columnSurfaces.map((s) => Math.pow((s.inside_temperature?.[hour] ?? 0) + KELVIN, 4));
    const beams = columnSurfaces.map((s) => s.beam_irradiance?.[hour] ?? 0);
    const anyBeam = sunUp && beams.some((b) => b > 0);
    const tAir = zone.air_temperature[hour];
    const rh = zone.relative_humidity[hour];
    const n = zone.points.length;
    const mrt = new Float32Array(n);
    const solar = new Float32Array(n);
    const operative = new Float32Array(n);
    const mrtEff = new Float32Array(n);
    for (let i = 0; i < n; i++) {
      let s = 0;
      const row = i * nS;
      for (let k = 0; k < nS; k++) s += zone.view_factors[row + k] * t4[k];
      const m = Math.pow(s, 0.25) - KELVIN;
      const beam = anyBeam ? beamAt(zone.points[i], sun, apertures, beams) : 0;
      const d = beam > 0 ? solarMrtCorrection(beam, altitude, m, zone.comfort.solar_absorptivity) : 0;
      mrt[i] = m;
      solar[i] = d;
      mrtEff[i] = m + d;
      operative[i] = (tAir + m + d) / 2;
    }
    result.mrt.push(mrt);
    result.solar.push(solar);
    result.operative.push(operative);
    if (result.pmv) result.pmv.push(zonePmv(mrtEff, tAir, rh, zone.comfort));
    result.airTemperature.push(tAir);
    result.relativeHumidity.push(rh);
    result.occupancy.push(zone.occupancy[hour]);
  }
  return result;
}

export function metricValue(values: HourValues, metric: PointMetric, zone: number, point: number): number {
  switch (metric) {
    case "operative": return values.operative[zone][point];
    case "mrt": return values.mrt[zone][point];
    case "solar": return values.solar[zone][point];
    case "pmv": return values.pmv ? values.pmv[zone][point] : NaN;
    case "ppd": return values.pmv ? ppd(values.pmv[zone][point]) : NaN;
  }
}

// ─── Time ────────────────────────────────────────────────────────────────────

const DAYS_IN_MONTH = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Month/day label and clock range for an hour index into the file. */
export function hourLabel(data: ComfortData, hour: number): { date: string; time: string } {
  const hoy = (data.time.start_hour_of_year + hour) % 8760;
  let doy = Math.floor(hoy / 24);
  const h = hoy % 24;
  let m = 0;
  while (m < 11 && doy >= DAYS_IN_MONTH[m]) { doy -= DAYS_IN_MONTH[m]; m++; }
  const pad = (x: number) => String(x).padStart(2, "0");
  return { date: `${MONTHS[m]} ${doy + 1}`, time: `${pad(h)}:00–${pad(h + 1)}:00` };
}

/** Index of month/day/hour (hour 1–24 = the hour ending then), clamped to the file. */
export function hourIndex(data: ComfortData, month: number, day: number, hourEnding: number): number {
  const doy = DAYS_IN_MONTH.slice(0, month - 1).reduce((a, b) => a + b, 0) + day - 1;
  const idx = doy * 24 + hourEnding - 1 - data.time.start_hour_of_year;
  return Math.min(Math.max(idx, 0), data.time.hours - 1);
}

// ─── Colour scales ───────────────────────────────────────────────────────────

export interface ColorScale {
  min: number;
  max: number;
  stops: string[];
  unit: string;
  label: string;
}

// Diverging blue ↔ red with a light neutral midpoint, so "neither warm nor
// cool" reads as the absence of colour on the dark canvas.
const DIVERGING = ["#1c5cab", "#3987e5", "#9ec5f4", "#e6e6e2", "#f4b0ac", "#e66767", "#b8302f"];
// One-hue orange ramp, dark to bright, so low values recede into the canvas.
const SEQUENTIAL = ["#3b2a22", "#7a3a1c", "#b84a1f", "#d95926", "#f2905c", "#fbc8a5"];

/** Fixed per metric so colours mean the same thing at every hour. */
export const SCALES: Record<PointMetric, ColorScale> = {
  operative: { min: 16, max: 32, stops: DIVERGING, unit: "°C", label: "Operative temperature" },
  mrt: { min: 16, max: 32, stops: DIVERGING, unit: "°C", label: "Mean radiant temperature (long-wave)" },
  solar: { min: 0, max: 20, stops: SEQUENTIAL, unit: "K", label: "Sun on occupant (MRT increase)" },
  pmv: { min: -3, max: 3, stops: DIVERGING, unit: "", label: "PMV" },
  ppd: { min: 5, max: 50, stops: SEQUENTIAL, unit: "%", label: "PPD" },
};

function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

const rgbCache = new Map<string, [number, number, number][]>();

/** Linear RGB in [0,1] for a value on a scale (clamped at the ends). */
export function colorFor(scale: ColorScale, value: number): [number, number, number] {
  let rgb = rgbCache.get(scale.stops.join());
  if (!rgb) {
    rgb = scale.stops.map(hexToRgb);
    rgbCache.set(scale.stops.join(), rgb);
  }
  if (!Number.isFinite(value)) return [0.35, 0.36, 0.45];
  const t = Math.min(1, Math.max(0, (value - scale.min) / (scale.max - scale.min)));
  const x = t * (rgb.length - 1);
  const k = Math.min(rgb.length - 2, Math.floor(x));
  const f = x - k;
  const a = rgb[k], b = rgb[k + 1];
  return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f];
}

export function comfortPathFor(modelPath: string): string {
  return modelPath.replace(/\.ya?ml$/i, "") + "_comfort.json";
}
