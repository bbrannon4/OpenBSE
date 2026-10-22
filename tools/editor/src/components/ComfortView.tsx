import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Canvas, useThree, type ThreeEvent } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import * as THREE from "three";
import {
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
  type ComfortData,
  type ComfortSurface,
  type HourValues,
  type PointMetric,
} from "../lib/comfort";
import type { UnitSystem } from "../lib/units";

const METRICS: { key: PointMetric; label: string }[] = [
  { key: "operative", label: "Operative temperature" },
  { key: "mrt", label: "Mean radiant temperature" },
  { key: "solar", label: "Sun on occupant (ΔMRT)" },
  { key: "pmv", label: "PMV" },
  { key: "ppd", label: "PPD" },
];

const BACKGROUND = "#1a1b26";

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
  const [metric, setMetric] = useState<PointMetric>("operative");
  const [zoneFilter, setZoneFilter] = useState<number | "all">("all");
  const [showRoof, setShowRoof] = useState(false);
  const [day, setDay] = useState(0);
  const [hourOfDay, setHourOfDay] = useState(9);
  const [playing, setPlaying] = useState(false);
  const [hovered, setHovered] = useState<{ zone: number; point: number; x: number; y: number } | null>(null);

  useEffect(() => {
    if (!path) return;
    let cancelled = false;
    invoke<string>("read_yaml_file", { path })
      .then((text) => {
        if (cancelled) return;
        const parsed = parseComfortData(text);
        const start = defaultHour(parsed);
        setLoaded({ path, data: parsed, error: null });
        setZoneFilter("all");
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
  const days = data ? Math.ceil(data.time.hours / 24) : 1;
  const hour = data ? Math.min(day * 24 + hourOfDay, data.time.hours - 1) : 0;
  // PMV is the costly part, so it is computed only when shown (metric or tooltip).
  const withPmv = metric === "pmv" || metric === "ppd" || hovered !== null;
  const values = useMemo(
    () => (data ? evaluateHour(data, models, hour, withPmv) : null),
    [data, models, hour, withPmv]
  );

  useEffect(() => {
    if (!playing) return;
    const id = window.setInterval(() => setHourOfDay((h) => (h + 1) % 24), 450);
    return () => window.clearInterval(id);
  }, [playing]);

  if (!modelPath) {
    return <ComfortMessage title="No model open" body="Open a model whose run wrote a comfort grid." />;
  }
  if (loading && !data) {
    return <ComfortMessage title="Loading comfort grid…" body="" />;
  }
  if (!data || !values) {
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

  const scale = SCALES[metric];
  const label = hourLabel(data, hour);
  const lowCoverage = data.zones.filter((z) => z.coverage_min < 0.95);

  return (
    <div className="comfort-view">
      <div className="comfort-toolbar">
        <label>
          Metric
          <select value={metric} onChange={(e) => setMetric(e.target.value as PointMetric)}>
            {METRICS.map((m) => (
              <option key={m.key} value={m.key}>{m.label}</option>
            ))}
          </select>
        </label>
        <label>
          Zone
          <select
            value={zoneFilter}
            onChange={(e) => setZoneFilter(e.target.value === "all" ? "all" : Number(e.target.value))}
          >
            <option value="all">All zones ({data.zones.length})</option>
            {data.zones.map((z, i) => (
              <option key={z.name} value={i}>{z.name}</option>
            ))}
          </select>
        </label>
        <label className="comfort-check">
          <input type="checkbox" checked={showRoof} onChange={(e) => setShowRoof(e.target.checked)} />
          Roofs &amp; ceilings
        </label>
        <span className="comfort-note" title={data.note}>
          Air is well mixed in each zone: colours show radiant and solar variation only, not a CFD
          air-temperature field.
        </span>
      </div>

      <div className="comfort-stage">
        <Canvas flat camera={{ fov: 40, near: 0.1, far: 5000 }} onPointerMissed={() => setHovered(null)}>
          <color attach="background" args={[BACKGROUND]} />
          <ambientLight intensity={0.9} />
          <directionalLight position={[50, 80, 30]} intensity={0.6} />
          {/* The model is Z-up; three.js is Y-up. */}
          <group rotation={[-Math.PI / 2, 0, 0]}>
            <Building data={data} zoneFilter={zoneFilter} showRoof={showRoof} />
            <GridTiles
              data={data}
              values={values}
              metric={metric}
              zoneFilter={zoneFilter}
              onHover={setHovered}
            />
          </group>
          <CameraFit data={data} zoneFilter={zoneFilter} />
        </Canvas>

        <Legend metric={metric} unitSystem={unitSystem} />
        {hovered && (
          <Tooltip
            data={data}
            values={values}
            hovered={hovered}
            unitSystem={unitSystem}
          />
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

      <div className="comfort-time">
        <button className="comfort-play" onClick={() => setPlaying((p) => !p)}>
          {playing ? "❚❚ Pause" : "▶ Play day"}
        </button>
        <div className="comfort-time-label">
          <strong>{label.date}</strong> {label.time}
          {zoneOccupancyNote(values, zoneFilter)}
        </div>
        <label className="comfort-slider">
          Day
          <input type="range" min={0} max={days - 1} value={day} onChange={(e) => setDay(Number(e.target.value))} />
        </label>
        <label className="comfort-slider">
          Hour
          <input
            type="range"
            min={0}
            max={23}
            value={hourOfDay}
            onChange={(e) => setHourOfDay(Number(e.target.value))}
          />
        </label>
        <span className="comfort-scale-name">{scale.label}</span>
      </div>
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

function zoneOccupancyNote(values: HourValues, zoneFilter: number | "all"): string {
  const occ = zoneFilter === "all" ? Math.max(0, ...values.occupancy) : values.occupancy[zoneFilter];
  return occ > 0 ? ` · occupied (${Math.round(occ * 100)}%)` : " · unoccupied";
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

function surfaceGroups(data: ComfortData, zoneFilter: number | "all", showRoof: boolean) {
  const focus = zoneFilter === "all" ? null : data.zones[zoneFilter].name;
  const groups = { opaque: [] as number[], floor: [] as number[], glass: [] as number[], edges: [] as number[] };
  const visible = (s: ComfortSurface) =>
    (showRoof || (s.type !== "roof" && s.type !== "ceiling")) && (focus === null || s.zone === focus);
  for (const s of data.surfaces) {
    if (!visible(s)) continue;
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

function Building({
  data, zoneFilter, showRoof,
}: { data: ComfortData; zoneFilter: number | "all"; showRoof: boolean }) {
  const geoms = useMemo(() => {
    const g = surfaceGroups(data, zoneFilter, showRoof);
    return {
      opaque: bufferGeometry(g.opaque),
      floor: bufferGeometry(g.floor),
      glass: bufferGeometry(g.glass),
      edges: bufferGeometry(g.edges),
    };
  }, [data, zoneFilter, showRoof]);
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

interface TileIndex {
  zones: number[];
  offsets: number[];
  count: number;
}

function tileIndex(data: ComfortData, zoneFilter: number | "all"): TileIndex {
  const zones = zoneFilter === "all" ? data.zones.map((_, i) => i) : [zoneFilter];
  const offsets: number[] = [];
  let count = 0;
  for (const z of zones) {
    offsets.push(count);
    count += data.zones[z].points.length;
  }
  return { zones, offsets, count };
}

function GridTiles({
  data, values, metric, zoneFilter, onHover,
}: {
  data: ComfortData;
  values: HourValues;
  metric: PointMetric;
  zoneFilter: number | "all";
  onHover: (h: { zone: number; point: number; x: number; y: number } | null) => void;
}) {
  const mesh = useRef<THREE.InstancedMesh>(null);
  const index = useMemo(() => tileIndex(data, zoneFilter), [data, zoneFilter]);
  const size = data.settings.spacing * 0.9;
  const geometry = useMemo(() => new THREE.PlaneGeometry(size, size), [size]);

  useLayoutEffect(() => {
    const m = mesh.current;
    if (!m) return;
    const mat = new THREE.Matrix4();
    index.zones.forEach((z, k) => {
      data.zones[z].points.forEach((p, i) => {
        // A hair above the floor plane so tiles never z-fight with it.
        mat.makeTranslation(p[0], p[1], p[2] + 0.01);
        m.setMatrixAt(index.offsets[k] + i, mat);
      });
    });
    m.instanceMatrix.needsUpdate = true;
    m.computeBoundingSphere();
  }, [data, index]);

  useLayoutEffect(() => {
    const m = mesh.current;
    if (!m) return;
    const scale = SCALES[metric];
    const c = new THREE.Color();
    index.zones.forEach((z, k) => {
      const n = data.zones[z].points.length;
      for (let i = 0; i < n; i++) {
        const [r, g, b] = colorFor(scale, metricValue(values, metric, z, i));
        c.setRGB(r, g, b, THREE.SRGBColorSpace);
        m.setColorAt(index.offsets[k] + i, c);
      }
    });
    if (m.instanceColor) m.instanceColor.needsUpdate = true;
  }, [data, index, values, metric]);

  const locate = useCallback(
    (instance: number) => {
      let k = index.offsets.length - 1;
      while (k > 0 && index.offsets[k] > instance) k--;
      return { zone: index.zones[k], point: instance - index.offsets[k] };
    },
    [index]
  );

  const onMove = (e: ThreeEvent<PointerEvent>) => {
    if (e.instanceId === undefined) return;
    e.stopPropagation();
    onHover({ ...locate(e.instanceId), x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY });
  };

  return (
    <instancedMesh
      key={index.count}
      ref={mesh}
      args={[geometry, undefined, index.count]}
      onPointerMove={onMove}
      onPointerOut={() => onHover(null)}
      renderOrder={3}
    >
      <meshBasicMaterial side={THREE.DoubleSide} toneMapped={false} />
    </instancedMesh>
  );
}

// ─── Camera ─────────────────────────────────────────────────────────────────

function CameraFit({ data, zoneFilter }: { data: ComfortData; zoneFilter: number | "all" }) {
  const get = useThree((s) => s.get);
  const controls = useRef<React.ComponentRef<typeof OrbitControls>>(null);

  useEffect(() => {
    const box = new THREE.Box3();
    const add = (p: [number, number, number]) => box.expandByPoint(new THREE.Vector3(p[0], p[2], -p[1]));
    if (zoneFilter === "all") {
      data.surfaces.forEach((s) => s.vertices.forEach(add));
    } else {
      const z = data.zones[zoneFilter];
      data.surfaces.filter((s) => s.zone === z.name).forEach((s) => s.vertices.forEach(add));
      z.points.forEach(add);
    }
    if (box.isEmpty()) return;
    const camera = get().camera as THREE.PerspectiveCamera;
    const center = box.getCenter(new THREE.Vector3());
    const radius = box.getSize(new THREE.Vector3()).length() / 2;
    // From the south-west and above, the usual architectural view.
    const dir = new THREE.Vector3(-0.6, 0.9, 0.9).normalize();
    camera.position.copy(center.clone().add(dir.multiplyScalar(radius * 2.4)));
    camera.near = Math.max(0.05, radius / 200);
    camera.far = radius * 50;
    camera.updateProjectionMatrix();
    controls.current?.target.copy(center);
    controls.current?.update();
  }, [data, zoneFilter, get]);

  return <OrbitControls ref={controls} makeDefault enableDamping={false} />;
}

// ─── Legend and tooltip ─────────────────────────────────────────────────────

function displayTemp(c: number, unitSystem: UnitSystem): string {
  return unitSystem === "IP" ? `${(c * 1.8 + 32).toFixed(1)} °F` : `${c.toFixed(1)} °C`;
}

function displayDelta(k: number, unitSystem: UnitSystem): string {
  return unitSystem === "IP" ? `${(k * 1.8).toFixed(1)} °F` : `${k.toFixed(1)} K`;
}

function formatScaleValue(metric: PointMetric, v: number, unitSystem: UnitSystem): string {
  if (metric === "operative" || metric === "mrt") return displayTemp(v, unitSystem);
  if (metric === "solar") return displayDelta(v, unitSystem);
  if (metric === "ppd") return `${v.toFixed(0)}%`;
  return v.toFixed(1);
}

function Legend({ metric, unitSystem }: { metric: PointMetric; unitSystem: UnitSystem }) {
  const scale = SCALES[metric];
  const gradient = `linear-gradient(to right, ${scale.stops.join(", ")})`;
  const mid = (scale.min + scale.max) / 2;
  return (
    <div className="comfort-legend">
      <div className="comfort-legend-title">{scale.label}</div>
      <div className="comfort-legend-bar" style={{ background: gradient }} />
      <div className="comfort-legend-ticks">
        <span>≤ {formatScaleValue(metric, scale.min, unitSystem)}</span>
        <span>{formatScaleValue(metric, mid, unitSystem)}</span>
        <span>≥ {formatScaleValue(metric, scale.max, unitSystem)}</span>
      </div>
      {metric === "pmv" && <div className="comfort-legend-hint">Comfortable: −0.5 to +0.5</div>}
    </div>
  );
}

function Tooltip({
  data, values, hovered, unitSystem,
}: {
  data: ComfortData;
  values: HourValues;
  hovered: { zone: number; point: number; x: number; y: number };
  unitSystem: UnitSystem;
}) {
  const { zone, point } = hovered;
  const z = data.zones[zone];
  const p = z.points[point];
  const mrt = values.mrt[zone][point];
  const solar = values.solar[zone][point];
  const pmvValue = values.pmv?.[zone][point];
  return (
    <div className="comfort-tooltip" style={{ left: hovered.x + 14, top: hovered.y + 14 }}>
      <div className="comfort-tooltip-title">{z.name}</div>
      <div className="comfort-tooltip-sub">
        x {p[0].toFixed(1)} · y {p[1].toFixed(1)} m
      </div>
      <table>
        <tbody>
          <tr><td>Air (zone)</td><td>{displayTemp(values.airTemperature[zone], unitSystem)}</td></tr>
          <tr><td>RH (zone)</td><td>{Math.round(values.relativeHumidity[zone] * 100)}%</td></tr>
          <tr><td>MRT</td><td>{displayTemp(mrt, unitSystem)}</td></tr>
          <tr><td>Sun ΔMRT</td><td>{solar > 0 ? displayDelta(solar, unitSystem) : "in shade"}</td></tr>
          <tr><td>Operative</td><td>{displayTemp(values.operative[zone][point], unitSystem)}</td></tr>
          {pmvValue !== undefined && (
            <>
              <tr><td>PMV</td><td>{pmvValue.toFixed(2)}</td></tr>
              <tr><td>PPD</td><td>{ppd(pmvValue).toFixed(0)}%</td></tr>
            </>
          )}
        </tbody>
      </table>
    </div>
  );
}
