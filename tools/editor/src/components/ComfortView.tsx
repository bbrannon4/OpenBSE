import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { Canvas, useFrame, useThree, type ThreeEvent } from "@react-three/fiber";
import { OrbitControls, OrthographicCamera, PerspectiveCamera } from "@react-three/drei";
import * as THREE from "three";
import {
  ANNUAL_SCALES,
  annualValue,
  buildZoneModels,
  colorFor,
  comfortPathFor,
  evaluateHour,
  hourIndex,
  hourLabel,
  metricValue,
  newellNormal,
  parseComfortData,
  ppd,
  SCALES,
  spatialComfortAutonomy,
  zoneLevels,
  type AnnualMetric,
  type ColorScale,
  type ComfortData,
  type ComfortSurface,
  type HourValues,
  type PointMetric,
} from "../lib/comfort";
import type { UnitSystem } from "../lib/units";

const HOUR_METRICS: { key: PointMetric; label: string }[] = [
  { key: "operative", label: "Operative temperature" },
  { key: "mrt", label: "Mean radiant temperature" },
  { key: "solar", label: "Sun on occupant (ΔMRT)" },
  { key: "pmv", label: "PMV" },
  { key: "ppd", label: "PPD" },
];

const ANNUAL_METRICS: { key: AnnualMetric; label: string }[] = [
  { key: "comfortable", label: "Comfortable hours (%)" },
  { key: "cold", label: "Too cold (%)" },
  { key: "hot", label: "Too warm (%)" },
  { key: "sun", label: "In direct sun (%)" },
  { key: "mean_operative", label: "Mean operative temperature" },
  { key: "max_operative", label: "Maximum operative temperature" },
];

const BACKGROUND = "#1a1b26";

type Mode = "hour" | "annual";
type View = "3d" | "plan";
type Hover = { zone: number; point: number; x: number; y: number };

interface Props {
  modelPath: string | null;
  /** Changes whenever results are reloaded, e.g. after a simulation run. */
  reloadKey: unknown;
  unitSystem: UnitSystem;
}

export function ComfortView({ modelPath, reloadKey, unitSystem }: Props) {
  const path = modelPath ? comfortPathFor(modelPath) : null;
  // Each load is tagged with its path so a result for a previous model is
  // never shown, and "loading" falls out of the tag instead of extra state.
  const [loaded, setLoaded] = useState<{ path: string; data: ComfortData | null; error: string | null } | null>(null);
  const [mode, setMode] = useState<Mode>("hour");
  const [view, setView] = useState<View>("3d");
  const [hourMetric, setHourMetric] = useState<PointMetric>("operative");
  const [annualMetric, setAnnualMetric] = useState<AnnualMetric>("comfortable");
  const [selected, setSelected] = useState<Set<number> | null>(null);
  const [showRoof, setShowRoof] = useState(false);
  const [day, setDay] = useState(0);
  const [hourOfDay, setHourOfDay] = useState(9);
  const [playing, setPlaying] = useState(false);
  const [threshold, setThreshold] = useState(80);
  const [hovered, setHovered] = useState<Hover | null>(null);
  const [exportStatus, setExportStatus] = useState<string | null>(null);
  const compass = useRef<HTMLDivElement>(null);
  const bearing = useRef(0);
  const glCanvas = useRef<HTMLCanvasElement | null>(null);
  const setBearing = useCallback((rad: number) => {
    bearing.current = rad;
    if (compass.current) compass.current.style.transform = `rotate(${-rad}rad)`;
  }, []);

  useEffect(() => {
    if (!path) return;
    let cancelled = false;
    invoke<string>("read_yaml_file", { path })
      .then((text) => {
        if (cancelled) return;
        const parsed = parseComfortData(text);
        const start = defaultHour(parsed);
        setLoaded({ path, data: parsed, error: null });
        setSelected(null);
        setDay(Math.floor(start / 24));
        setHourOfDay(start % 24);
      })
      .catch((e) => {
        if (!cancelled) setLoaded({ path, data: null, error: String(e) });
      });
    return () => {
      cancelled = true;
    };
  }, [path, reloadKey]);

  const current = loaded && loaded.path === path ? loaded : null;
  const data = current?.data ?? null;
  const error = current?.error ?? null;
  const loading = path !== null && current === null;

  const models = useMemo(() => (data ? buildZoneModels(data) : []), [data]);
  const zones = useMemo(
    () => (data ? (selected ? [...selected].sort((a, b) => a - b) : data.zones.map((_, i) => i)) : []),
    [data, selected]
  );
  const days = data ? Math.ceil(data.time.hours / 24) : 1;
  const hour = data ? Math.min(day * 24 + hourOfDay, data.time.hours - 1) : 0;
  // PMV is the costly part, so it is computed only when shown (metric or tooltip).
  const needHour = mode === "hour";
  const withPmv = needHour && (hourMetric === "pmv" || hourMetric === "ppd" || hovered !== null);
  const values = useMemo(
    () => (data && needHour ? evaluateHour(data, models, hour, withPmv) : null),
    [data, models, hour, withPmv, needHour]
  );

  useEffect(() => {
    if (!playing) return;
    const id = window.setInterval(() => setHourOfDay((h) => (h + 1) % 24), 450);
    return () => window.clearInterval(id);
  }, [playing]);

  const scale = mode === "hour" ? SCALES[hourMetric] : ANNUAL_SCALES[annualMetric];
  const valueAt = useCallback(
    (z: number, i: number) =>
      mode === "hour"
        ? values ? metricValue(values, hourMetric, z, i) : NaN
        : data ? annualValue(data.zones[z], i, annualMetric) : NaN,
    [mode, values, hourMetric, data, annualMetric]
  );

  if (!modelPath) {
    return <ComfortMessage title="No model open" body="Open a model whose run wrote a comfort grid." />;
  }
  if (loading && !data) {
    return <ComfortMessage title="Loading comfort grid…" body="" />;
  }
  if (!data) {
    return (
      <ComfortMessage
        title="No comfort grid results for this model"
        body={
          "Add a Comfort Grid block (sidebar: Output › Comfort Grid) and run the simulation. " +
          "It writes <model>_comfort.json next to the model, which this tab reads." +
          (error && !error.includes("No such file") ? `\n\n${error}` : "")
        }
      />
    );
  }
  if (data.zones.length === 0) {
    return (
      <ComfortMessage
        title="No zone could be gridded"
        body={
          "Every surface of a zone needs vertices for the comfort grid.\n\n" +
          data.skipped_zones.map((s) => `${s.name}: ${s.reason}`).join("\n")
        }
      />
    );
  }

  const label = hourLabel(data, hour);
  const lowCoverage = data.zones.filter((z) => z.coverage_min < 0.95);
  const autonomy = spatialComfortAutonomy(zones.map((z) => data.zones[z]), threshold);
  const occupiedHours = Math.max(0, ...zones.map((z) => data.zones[z].aggregates.occupied_hours));
  // A run need not cover a whole year, so the statistics are labelled with
  // the period they were collected over.
  const period = `${hourLabel(data, 0).date} – ${hourLabel(data, data.time.hours - 1).date}`;
  const caption =
    mode === "hour"
      ? `${label.date} ${label.time}`
      : `${period}, occupied hours only`;

  const exportPng = async () => {
    const canvas = glCanvas.current;
    if (!canvas) return;
    const stem = (modelPath.split("/").pop() ?? "model").replace(/\.ya?ml$/i, "");
    const what = mode === "hour" ? `${hourMetric}_${label.date.replace(" ", "")}_${label.time.slice(0, 2)}h` : `annual_${annualMetric}`;
    const target = await save({
      title: "Export comfort view",
      defaultPath: `${stem}_comfort_${what}.png`,
      filters: [{ name: "PNG image", extensions: ["png"] }],
    });
    if (!target) return;
    try {
      const bytes = await composePng(canvas, {
        title: `${stem} — ${scale.label}`,
        caption: `${caption} · ${zones.length === data.zones.length ? "all zones" : `${zones.length} zone(s)`}`,
        scale,
        format: (v) => formatValue(scale, v, unitSystem),
        bearing: bearing.current,
      });
      await invoke("write_binary_file", { path: target, contents: Array.from(bytes) });
      setExportStatus(`Saved ${target.split("/").pop()}`);
    } catch (e) {
      setExportStatus(`Export failed: ${e}`);
    }
    window.setTimeout(() => setExportStatus(null), 4000);
  };

  return (
    <div className="comfort-view">
      <div className="comfort-toolbar">
        <div className="comfort-segment">
          <button className={mode === "hour" ? "active" : ""} onClick={() => setMode("hour")}>Hour</button>
          <button
            className={mode === "annual" ? "active" : ""}
            title="Statistics over the whole run period"
            onClick={() => { setMode("annual"); setPlaying(false); }}
          >
            Annual
          </button>
        </div>
        <label>
          Metric
          {mode === "hour" ? (
            <select value={hourMetric} onChange={(e) => setHourMetric(e.target.value as PointMetric)}>
              {HOUR_METRICS.map((m) => <option key={m.key} value={m.key}>{m.label}</option>)}
            </select>
          ) : (
            <select value={annualMetric} onChange={(e) => setAnnualMetric(e.target.value as AnnualMetric)}>
              {ANNUAL_METRICS.map((m) => <option key={m.key} value={m.key}>{m.label}</option>)}
            </select>
          )}
        </label>
        <ZonePicker data={data} selected={selected} onChange={setSelected} />
        <div className="comfort-segment">
          <button className={view === "3d" ? "active" : ""} onClick={() => setView("3d")}>3D</button>
          <button className={view === "plan" ? "active" : ""} onClick={() => setView("plan")}>Plan</button>
        </div>
        <label className="comfort-check">
          <input type="checkbox" checked={showRoof} onChange={(e) => setShowRoof(e.target.checked)} />
          Roofs &amp; ceilings
        </label>
        <button className="comfort-export" onClick={exportPng}>Export PNG</button>
        {exportStatus && <span className="comfort-export-status">{exportStatus}</span>}
        <span className="comfort-note" title={data.note}>
          Air is well mixed in each zone: colours show radiant and solar variation only, not a CFD
          air-temperature field.
        </span>
      </div>

      <div className="comfort-stage">
        <Canvas
          flat
          gl={{ preserveDrawingBuffer: true }}
          onCreated={({ gl }) => { glCanvas.current = gl.domElement; }}
          onPointerMissed={() => setHovered(null)}
        >
          {view === "3d" ? (
            <PerspectiveCamera makeDefault fov={40} near={0.1} far={5000} />
          ) : (
            <OrthographicCamera makeDefault up={[0, 0, -1]} near={0.1} far={10000} />
          )}
          <color attach="background" args={[BACKGROUND]} />
          {/* The model is Z-up; three.js is Y-up. */}
          <group rotation={[-Math.PI / 2, 0, 0]}>
            <Building data={data} zones={zones} showRoof={showRoof} />
            <GridTiles data={data} zones={zones} scale={scale} valueAt={valueAt} onHover={setHovered} />
          </group>
          <CameraFit key={view} data={data} zones={zones} view={view} />
          <CompassUpdater onBearing={setBearing} />
        </Canvas>

        <div className="comfort-compass" ref={compass} title="North">
          <svg viewBox="0 0 40 40" width="40" height="40" aria-label="North arrow">
            <polygon points="20,3 27,26 20,21 13,26" fill="#c0caf5" />
            <text x="20" y="37" textAnchor="middle" fontSize="9" fill="#c0caf5">N</text>
          </svg>
        </div>

        <Legend scale={scale} unitSystem={unitSystem} hint={mode === "hour" && hourMetric === "pmv" ? "Comfortable: −0.5 to +0.5" : null} />
        {hovered && (
          mode === "hour" && values ? (
            <HourTooltip data={data} values={values} hovered={hovered} unitSystem={unitSystem} />
          ) : (
            <AnnualTooltip data={data} hovered={hovered} unitSystem={unitSystem} />
          )
        )}
        {(lowCoverage.length > 0 || data.skipped_zones.length > 0) && (
          <div className="comfort-warnings">
            {data.skipped_zones.length > 0 && (
              <div title={data.skipped_zones.map((s) => `${s.name}: ${s.reason}`).join("\n")}>
                {data.skipped_zones.length} zone(s) not gridded (hover for reasons)
              </div>
            )}
            {lowCoverage.length > 0 && (
              <div title={lowCoverage.map((z) => `${z.name}: ${Math.round(z.coverage_min * 100)}%`).join("\n")}>
                {lowCoverage.length} zone(s) with open enclosures (hover for coverage)
              </div>
            )}
          </div>
        )}
      </div>

      {mode === "hour" ? (
        <div className="comfort-time">
          <button className="comfort-play" onClick={() => setPlaying((p) => !p)}>
            {playing ? "❚❚ Pause" : "▶ Play day"}
          </button>
          <div className="comfort-time-label">
            <strong>{label.date}</strong> {label.time}
            {values && occupancyNote(values, zones)}
          </div>
          <label className="comfort-slider">
            Day
            <input type="range" min={0} max={days - 1} value={day} onChange={(e) => setDay(Number(e.target.value))} />
          </label>
          <label className="comfort-slider">
            Hour
            <input type="range" min={0} max={23} value={hourOfDay} onChange={(e) => setHourOfDay(Number(e.target.value))} />
          </label>
          <span className="comfort-scale-name">{scale.label}</span>
        </div>
      ) : (
        <div className="comfort-time">
          <div className="comfort-time-label">
            <strong>{period}</strong> · occupied hours only (up to {occupiedHours} h per zone, people schedule above zero)
          </div>
          <label className="comfort-slider" title="Share of grid points (floor area) comfortable for at least this share of occupied hours">
            Spatial comfort autonomy:&nbsp;
            <strong>{autonomy === null ? "–" : `${autonomy.toFixed(0)}%`}</strong>
            &nbsp;of floor area comfortable ≥
            <input
              type="number"
              min={0}
              max={100}
              step={5}
              value={threshold}
              onChange={(e) => setThreshold(Math.min(100, Math.max(0, Number(e.target.value))))}
            />
            % of occupied hours
          </label>
          <span className="comfort-scale-name">{scale.label}</span>
        </div>
      )}
    </div>
  );
}

function ComfortMessage({ title, body }: { title: string; body: string }) {
  return (
    <div className="comfort-view comfort-empty">
      <h3>{title}</h3>
      {body && <p>{body}</p>}
      <p className="comfort-note">
        The comfort grid shows radiant and solar variation within each zone. Zone air is well mixed,
        so it is not a CFD air-temperature field.
      </p>
    </div>
  );
}

/** First occupied 9–10 am on or after Jan 21, so the tab opens on a working morning. */
function defaultHour(data: ComfortData): number {
  const start = hourIndex(data, 1, 21, 10);
  for (let h = start; h < Math.min(data.time.hours, start + 24 * 14); h += 24) {
    if (data.zones.some((z) => z.occupancy[h] > 0)) return h;
  }
  return start;
}

function occupancyNote(values: HourValues, zones: number[]): string {
  const occ = Math.max(0, ...zones.map((z) => values.occupancy[z]));
  return occ > 0 ? ` · occupied (${Math.round(occ * 100)}%)` : " · unoccupied";
}

// ─── Zone picker ────────────────────────────────────────────────────────────

/**
 * Picks the zones to show: a whole level in one click, or any mix of zones.
 * `null` means every zone, so the surrounding building is drawn as context.
 */
function ZonePicker({
  data, selected, onChange,
}: {
  data: ComfortData;
  selected: Set<number> | null;
  onChange: (s: Set<number> | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const levels = useMemo(() => {
    const byLevel = new Map<number, number[]>();
    zoneLevels(data).forEach((lvl, i) => byLevel.set(lvl, [...(byLevel.get(lvl) ?? []), i]));
    return [...byLevel.entries()].sort((a, b) => a[0] - b[0]);
  }, [data]);
  const isOn = (i: number) => selected === null || selected.has(i);
  const toggle = (i: number) => {
    const next = new Set(selected ?? data.zones.map((_, k) => k));
    if (next.has(i)) next.delete(i);
    else next.add(i);
    onChange(next.size === data.zones.length ? null : next);
  };
  const needle = filter.trim().toLowerCase();
  const count = selected === null ? data.zones.length : selected.size;
  const summary = selected === null ? `All zones (${count})` : `${count} of ${data.zones.length} zones`;

  return (
    <div className="comfort-picker">
      <button className="comfort-picker-button" onClick={() => setOpen((o) => !o)}>
        Zones: {summary} ▾
      </button>
      {open && (
        <div className="comfort-picker-panel" onMouseLeave={() => setOpen(false)}>
          <div className="comfort-picker-row">
            <button onClick={() => onChange(null)}>All</button>
            <button onClick={() => onChange(new Set())}>None</button>
          </div>
          {levels.length > 1 && (
            <div className="comfort-picker-levels">
              <div className="comfort-picker-heading">Levels</div>
              {levels.map(([lvl, members]) => (
                <button key={lvl} onClick={() => onChange(new Set(members))}>
                  {lvl.toFixed(1)} m · {members.length} zone{members.length === 1 ? "" : "s"}
                </button>
              ))}
            </div>
          )}
          <input
            className="comfort-picker-filter"
            placeholder="Filter zones…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          <div className="comfort-picker-list">
            {data.zones.map((z, i) =>
              needle && !z.name.toLowerCase().includes(needle) ? null : (
                <label key={z.name}>
                  <input type="checkbox" checked={isOn(i)} onChange={() => toggle(i)} />
                  {z.name}
                </label>
              )
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ─── Building geometry ──────────────────────────────────────────────────────

function triangulate(verts: [number, number, number][]): number[] {
  const n = new THREE.Vector3(...newellNormal(verts)).normalize();
  const origin = new THREE.Vector3(...verts[0]);
  const u = new THREE.Vector3(...verts[1]).sub(origin).normalize();
  const v = new THREE.Vector3().crossVectors(n, u).normalize();
  const contour = verts.map((p) => {
    const d = new THREE.Vector3(...p).sub(origin);
    return new THREE.Vector2(d.dot(u), d.dot(v));
  });
  const out: number[] = [];
  for (const [a, b, c] of THREE.ShapeUtils.triangulateShape(contour, [])) {
    out.push(...verts[a], ...verts[b], ...verts[c]);
  }
  return out;
}

/** With every zone shown, all surfaces are drawn (ungridded zones too, as context). */
function surfaceVisible(data: ComfortData, zones: number[]): (s: ComfortSurface) => boolean {
  if (zones.length === data.zones.length) return () => true;
  const names = new Set(zones.map((z) => data.zones[z].name));
  return (s) => names.has(s.zone);
}

function surfaceGroups(data: ComfortData, zones: number[], showRoof: boolean) {
  const inSelection = surfaceVisible(data, zones);
  const groups = { opaque: [] as number[], floor: [] as number[], glass: [] as number[], edges: [] as number[] };
  for (const s of data.surfaces) {
    if (!inSelection(s)) continue;
    if (!showRoof && (s.type === "roof" || s.type === "ceiling")) continue;
    const tris = triangulate(s.vertices);
    if (s.type === "window") groups.glass.push(...tris);
    else if (s.type === "floor") groups.floor.push(...tris);
    else groups.opaque.push(...tris);
    for (let i = 0; i < s.vertices.length; i++) {
      groups.edges.push(...s.vertices[i], ...s.vertices[(i + 1) % s.vertices.length]);
    }
  }
  return groups;
}

function bufferGeometry(positions: number[]): THREE.BufferGeometry {
  const g = new THREE.BufferGeometry();
  g.setAttribute("position", new THREE.Float32BufferAttribute(positions, 3));
  return g;
}

function Building({ data, zones, showRoof }: { data: ComfortData; zones: number[]; showRoof: boolean }) {
  const geoms = useMemo(() => {
    const g = surfaceGroups(data, zones, showRoof);
    return {
      opaque: bufferGeometry(g.opaque),
      floor: bufferGeometry(g.floor),
      glass: bufferGeometry(g.glass),
      edges: bufferGeometry(g.edges),
    };
  }, [data, zones, showRoof]);
  useEffect(() => () => Object.values(geoms).forEach((g) => g.dispose()), [geoms]);

  return (
    <>
      <mesh geometry={geoms.opaque} renderOrder={1}>
        <meshBasicMaterial color="#9aa5ce" transparent opacity={0.07} side={THREE.DoubleSide} depthWrite={false} />
      </mesh>
      <mesh geometry={geoms.floor} renderOrder={0}>
        <meshBasicMaterial color="#565f89" transparent opacity={0.18} side={THREE.DoubleSide} depthWrite={false} />
      </mesh>
      <mesh geometry={geoms.glass} renderOrder={2}>
        <meshBasicMaterial color="#7dcfff" transparent opacity={0.28} side={THREE.DoubleSide} depthWrite={false} />
      </mesh>
      <lineSegments geometry={geoms.edges}>
        <lineBasicMaterial color="#565f89" transparent opacity={0.8} />
      </lineSegments>
    </>
  );
}

// ─── Grid tiles ─────────────────────────────────────────────────────────────

function tileOffsets(data: ComfortData, zones: number[]): { offsets: number[]; count: number } {
  const offsets: number[] = [];
  let count = 0;
  for (const z of zones) {
    offsets.push(count);
    count += data.zones[z].points.length;
  }
  return { offsets, count };
}

function GridTiles({
  data, zones, scale, valueAt, onHover,
}: {
  data: ComfortData;
  zones: number[];
  scale: ColorScale;
  valueAt: (zone: number, point: number) => number;
  onHover: (h: Hover | null) => void;
}) {
  const mesh = useRef<THREE.InstancedMesh>(null);
  const { offsets, count } = useMemo(() => tileOffsets(data, zones), [data, zones]);
  const size = data.settings.spacing * 0.9;
  const geometry = useMemo(() => new THREE.PlaneGeometry(size, size), [size]);

  useLayoutEffect(() => {
    const m = mesh.current;
    if (!m) return;
    const mat = new THREE.Matrix4();
    zones.forEach((z, k) => {
      data.zones[z].points.forEach((p, i) => {
        // A hair above the floor plane so tiles never z-fight with it.
        mat.makeTranslation(p[0], p[1], p[2] + 0.01);
        m.setMatrixAt(offsets[k] + i, mat);
      });
    });
    m.instanceMatrix.needsUpdate = true;
    m.computeBoundingSphere();
  }, [data, zones, offsets]);

  useLayoutEffect(() => {
    const m = mesh.current;
    if (!m) return;
    const c = new THREE.Color();
    zones.forEach((z, k) => {
      const n = data.zones[z].points.length;
      for (let i = 0; i < n; i++) {
        const [r, g, b] = colorFor(scale, valueAt(z, i));
        c.setRGB(r, g, b, THREE.SRGBColorSpace);
        m.setColorAt(offsets[k] + i, c);
      }
    });
    if (m.instanceColor) m.instanceColor.needsUpdate = true;
  }, [data, zones, offsets, scale, valueAt]);

  const onMove = (e: ThreeEvent<PointerEvent>) => {
    if (e.instanceId === undefined) return;
    e.stopPropagation();
    let k = offsets.length - 1;
    while (k > 0 && offsets[k] > e.instanceId) k--;
    onHover({ zone: zones[k], point: e.instanceId - offsets[k], x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY });
  };

  if (count === 0) return null;
  return (
    <instancedMesh
      key={count}
      ref={mesh}
      args={[geometry, undefined, count]}
      onPointerMove={onMove}
      onPointerOut={() => onHover(null)}
      renderOrder={3}
    >
      <meshBasicMaterial side={THREE.DoubleSide} toneMapped={false} />
    </instancedMesh>
  );
}

// ─── Camera ─────────────────────────────────────────────────────────────────

/** Frames the shown zones: from the south-west in 3D, straight down (north up) in plan. */
function CameraFit({ data, zones, view }: { data: ComfortData; zones: number[]; view: View }) {
  const get = useThree((s) => s.get);
  // drei's cameras become the default in their own effect, so the fit has to
  // run again once the active camera changes.
  const cameraId = useThree((s) => s.camera.uuid);
  const controls = useRef<React.ComponentRef<typeof OrbitControls>>(null);

  useEffect(() => {
    const box = new THREE.Box3();
    const add = (p: [number, number, number]) => box.expandByPoint(new THREE.Vector3(p[0], p[2], -p[1]));
    const inSelection = surfaceVisible(data, zones);
    data.surfaces.filter(inSelection).forEach((s) => s.vertices.forEach(add));
    zones.forEach((z) => data.zones[z].points.forEach(add));
    if (box.isEmpty()) return;
    const { camera, size } = get();
    const center = box.getCenter(new THREE.Vector3());
    const extent = box.getSize(new THREE.Vector3());
    const radius = extent.length() / 2;
    if (view === "plan" && camera instanceof THREE.OrthographicCamera) {
      camera.position.set(center.x, box.max.y + radius + 10, center.z);
      camera.zoom = 0.85 * Math.min(size.width / Math.max(extent.x, 0.1), size.height / Math.max(extent.z, 0.1));
      camera.lookAt(center);
    } else {
      const dir = new THREE.Vector3(-0.6, 0.9, 0.9).normalize();
      camera.position.copy(center.clone().add(dir.multiplyScalar(radius * 2.4)));
      camera.near = Math.max(0.05, radius / 200);
      camera.far = radius * 50;
    }
    camera.updateProjectionMatrix();
    controls.current?.target.copy(center);
    controls.current?.update();
  }, [data, zones, view, get, cameraId]);

  return (
    <OrbitControls
      ref={controls}
      makeDefault
      enableDamping={false}
      enableRotate={view === "3d"}
      screenSpacePanning
    />
  );
}

// ─── North arrow ────────────────────────────────────────────────────────────

/**
 * Turns the north arrow with the camera. North is +Y in the model, which is
 * -Z inside the rotated scene. The screen's "up" direction, projected onto
 * the ground, gives the bearing the viewer is facing; north sits at minus
 * that bearing on screen.
 */
function CompassUpdater({ onBearing }: { onBearing: (rad: number) => void }) {
  const up = useMemo(() => new THREE.Vector3(), []);
  const fwd = useMemo(() => new THREE.Vector3(), []);
  useFrame(({ camera }) => {
    up.setFromMatrixColumn(camera.matrixWorld, 1);
    camera.getWorldDirection(fwd);
    // Looking straight down, screen-up is horizontal; looking level, use forward.
    const v = Math.hypot(up.x, up.z) > Math.hypot(fwd.x, fwd.z) ? up : fwd;
    onBearing(Math.atan2(v.x, -v.z));
  });
  return null;
}

// ─── Legend, tooltips, formatting ───────────────────────────────────────────

function displayTemp(c: number, unitSystem: UnitSystem): string {
  return unitSystem === "IP" ? `${(c * 1.8 + 32).toFixed(1)} °F` : `${c.toFixed(1)} °C`;
}

function displayDelta(k: number, unitSystem: UnitSystem): string {
  return unitSystem === "IP" ? `${(k * 1.8).toFixed(1)} °F` : `${k.toFixed(1)} K`;
}

function formatValue(scale: ColorScale, v: number, unitSystem: UnitSystem): string {
  if (!Number.isFinite(v)) return "–";
  if (scale.unit === "°C") return displayTemp(v, unitSystem);
  if (scale.unit === "K") return displayDelta(v, unitSystem);
  if (scale.unit === "%") return `${v.toFixed(0)}%`;
  return v.toFixed(1);
}

function Legend({ scale, unitSystem, hint }: { scale: ColorScale; unitSystem: UnitSystem; hint: string | null }) {
  const gradient = `linear-gradient(to right, ${scale.stops.join(", ")})`;
  const mid = (scale.min + scale.max) / 2;
  return (
    <div className="comfort-legend">
      <div className="comfort-legend-title">{scale.label}</div>
      <div className="comfort-legend-bar" style={{ background: gradient }} />
      <div className="comfort-legend-ticks">
        <span>≤ {formatValue(scale, scale.min, unitSystem)}</span>
        <span>{formatValue(scale, mid, unitSystem)}</span>
        <span>≥ {formatValue(scale, scale.max, unitSystem)}</span>
      </div>
      {hint && <div className="comfort-legend-hint">{hint}</div>}
    </div>
  );
}

function TooltipFrame({ data, hovered, children }: { data: ComfortData; hovered: Hover; children: React.ReactNode }) {
  const z = data.zones[hovered.zone];
  const p = z.points[hovered.point];
  return (
    <div className="comfort-tooltip" style={{ left: hovered.x + 14, top: hovered.y + 14 }}>
      <div className="comfort-tooltip-title">{z.name}</div>
      <div className="comfort-tooltip-sub">x {p[0].toFixed(1)} · y {p[1].toFixed(1)} m</div>
      <table><tbody>{children}</tbody></table>
    </div>
  );
}

function HourTooltip({
  data, values, hovered, unitSystem,
}: { data: ComfortData; values: HourValues; hovered: Hover; unitSystem: UnitSystem }) {
  const { zone, point } = hovered;
  const solar = values.solar[zone][point];
  const pmvValue = values.pmv?.[zone][point];
  return (
    <TooltipFrame data={data} hovered={hovered}>
      <tr><td>Air (zone)</td><td>{displayTemp(values.airTemperature[zone], unitSystem)}</td></tr>
      <tr><td>RH (zone)</td><td>{Math.round(values.relativeHumidity[zone] * 100)}%</td></tr>
      <tr><td>MRT</td><td>{displayTemp(values.mrt[zone][point], unitSystem)}</td></tr>
      <tr><td>Sun ΔMRT</td><td>{solar > 0 ? displayDelta(solar, unitSystem) : "in shade"}</td></tr>
      <tr><td>Operative</td><td>{displayTemp(values.operative[zone][point], unitSystem)}</td></tr>
      {pmvValue !== undefined && (
        <>
          <tr><td>PMV</td><td>{pmvValue.toFixed(2)}</td></tr>
          <tr><td>PPD</td><td>{ppd(pmvValue).toFixed(0)}%</td></tr>
        </>
      )}
    </TooltipFrame>
  );
}

function AnnualTooltip({ data, hovered, unitSystem }: { data: ComfortData; hovered: Hover; unitSystem: UnitSystem }) {
  const z = data.zones[hovered.zone];
  const i = hovered.point;
  const a = z.aggregates;
  const row = (label: string, hours: number) => (
    <tr>
      <td>{label}</td>
      <td>{a.occupied_hours > 0 ? `${Math.round((100 * hours) / a.occupied_hours)}% · ${hours} h` : "–"}</td>
    </tr>
  );
  const temp = (v: number | null) => (v === null ? "–" : displayTemp(v, unitSystem));
  return (
    <TooltipFrame data={data} hovered={hovered}>
      <tr><td>Occupied hours</td><td>{a.occupied_hours}</td></tr>
      {row("Comfortable", a.comfortable_hours[i])}
      {row("Too cold", a.cold_hours[i])}
      {row("Too warm", a.hot_hours[i])}
      {row("In direct sun", a.sun_hours[i])}
      <tr><td>Mean operative</td><td>{temp(a.mean_operative_temperature[i])}</td></tr>
      <tr><td>Max operative</td><td>{temp(a.max_operative_temperature[i])}</td></tr>
    </TooltipFrame>
  );
}

// ─── PNG export ─────────────────────────────────────────────────────────────

/**
 * The legend, caption and north arrow live in the DOM, not in the WebGL
 * canvas, so they are redrawn onto a 2D canvas under the 3D image.
 */
async function composePng(
  source: HTMLCanvasElement,
  opts: { title: string; caption: string; scale: ColorScale; format: (v: number) => string; bearing: number }
): Promise<Uint8Array> {
  const dpr = source.width / Math.max(source.clientWidth, 1);
  const footer = Math.round(92 * dpr);
  const out = document.createElement("canvas");
  out.width = source.width;
  out.height = source.height + footer;
  const ctx = out.getContext("2d");
  if (!ctx) throw new Error("2D canvas unavailable");
  ctx.fillStyle = BACKGROUND;
  ctx.fillRect(0, 0, out.width, out.height);
  ctx.drawImage(source, 0, 0);

  const px = (n: number) => n * dpr;
  // North arrow, top-left, as on screen.
  ctx.save();
  ctx.translate(px(36), px(36));
  ctx.rotate(-opts.bearing);
  ctx.fillStyle = "#c0caf5";
  ctx.beginPath();
  ctx.moveTo(0, px(-17));
  ctx.lineTo(px(7), px(6));
  ctx.lineTo(0, px(1));
  ctx.lineTo(px(-7), px(6));
  ctx.closePath();
  ctx.fill();
  ctx.font = `${px(10)}px sans-serif`;
  ctx.textAlign = "center";
  ctx.fillText("N", 0, px(18));
  ctx.restore();

  const y0 = source.height;
  ctx.textAlign = "left";
  ctx.fillStyle = "#c0caf5";
  ctx.font = `600 ${px(14)}px sans-serif`;
  ctx.fillText(opts.title, px(16), y0 + px(24));
  ctx.fillStyle = "#9aa5ce";
  ctx.font = `${px(12)}px sans-serif`;
  ctx.fillText(opts.caption, px(16), y0 + px(44));
  ctx.fillStyle = "#565f89";
  ctx.font = `${px(11)}px sans-serif`;
  ctx.fillText(
    "Zone air is well mixed: colours show radiant and solar variation only, not a CFD air-temperature field.",
    px(16),
    y0 + px(78)
  );

  const barW = px(260);
  const barX = out.width - barW - px(20);
  const barY = y0 + px(18);
  const grad = ctx.createLinearGradient(barX, 0, barX + barW, 0);
  opts.scale.stops.forEach((c, k) => grad.addColorStop(k / (opts.scale.stops.length - 1), c));
  ctx.fillStyle = grad;
  ctx.fillRect(barX, barY, barW, px(12));
  ctx.fillStyle = "#9aa5ce";
  ctx.font = `${px(11)}px sans-serif`;
  ctx.textAlign = "left";
  ctx.fillText(`≤ ${opts.format(opts.scale.min)}`, barX, barY + px(28));
  ctx.textAlign = "center";
  ctx.fillText(opts.format((opts.scale.min + opts.scale.max) / 2), barX + barW / 2, barY + px(28));
  ctx.textAlign = "right";
  ctx.fillText(`≥ ${opts.format(opts.scale.max)}`, barX + barW, barY + px(28));

  const blob = await new Promise<Blob | null>((resolve) => out.toBlob(resolve, "image/png"));
  if (!blob) throw new Error("could not encode PNG");
  return new Uint8Array(await blob.arrayBuffer());
}
