/**
 * Convert a YAML model's HVAC definitions into React Flow nodes and edges.
 *
 * Produces TWO separate graphs:
 *   - Air-side: OA → equipment → zones (left-to-right)
 *     Water coils show a plant-loop badge but NO cross-graph edges
 *   - Water-side: Pump → boiler/chiller → coils served
 *     Coils show which air loop they belong to
 */

import type { Node, Edge } from "@xyflow/react";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Model = Record<string, any>;

/* ------------------------------------------------------------------ */
/*  Public types                                                       */
/* ------------------------------------------------------------------ */

export type HvacNodeType =
  | "oa_intake"
  | "fan"
  | "heating_coil"
  | "cooling_coil"
  | "heat_recovery"
  | "humidifier"
  | "duct"
  | "evap_cooler"
  | "vrf_outdoor"
  | "vrf_indoor"
  | "radiant_panel"
  | "zone"
  | "terminal"
  | "pump"
  | "boiler"
  | "chiller"
  | "cooling_tower"
  | "heat_exchanger"
  | "thermal_storage"
  | "gshp"
  | "coil_load";

export interface HvacNodeData {
  [key: string]: unknown;
  hvacType: HvacNodeType;
  label: string;
  sublabel?: string;
  /** Plant loop name for water coils (shown as badge on air side) */
  plantLoopRef?: string;
  /** Air loop name for coils shown on water side */
  airLoopRef?: string;
  properties: Record<string, string | number | boolean>;
  airLoop?: string;
  plantLoop?: string;
  /** Original component name (for variable browser cross-link) */
  componentName?: string;
}

export interface HvacGraph {
  nodes: Node<HvacNodeData>[];
  edges: Edge[];
}

export interface SeparatedHvacGraphs {
  air: HvacGraph;
  water: HvacGraph;
}

/* ------------------------------------------------------------------ */
/*  Color map                                                          */
/* ------------------------------------------------------------------ */

export const NODE_COLORS: Record<HvacNodeType, string> = {
  oa_intake: "#7dcfff",
  fan: "#9aa5ce",
  heating_coil: "#ff9e64",
  cooling_coil: "#7aa2f7",
  heat_recovery: "#2ac3de",
  humidifier: "#b4f9f8",
  duct: "#565f89",
  evap_cooler: "#73daca",
  vrf_outdoor: "#bb9af7",
  vrf_indoor: "#c0caf5",
  radiant_panel: "#ff9e64",
  zone: "#9ece6a",
  terminal: "#e0af68",
  pump: "#bb9af7",
  boiler: "#f7768e",
  chiller: "#7aa2f7",
  cooling_tower: "#7dcfff",
  heat_exchanger: "#2ac3de",
  thermal_storage: "#e0af68",
  gshp: "#2ac3de",
  coil_load: "#e0af68",
};

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function equipmentType(eq: Record<string, unknown>): HvacNodeType {
  const t = String(eq.type ?? "").toLowerCase();
  switch (t) {
    case "fan": return "fan";
    case "heating_coil": return "heating_coil";
    case "cooling_coil": return "cooling_coil";
    case "heat_recovery": return "heat_recovery";
    case "humidifier": return "humidifier";
    case "duct": return "duct";
    case "evap_cooler": return "evap_cooler";
    case "vrf_outdoor_unit": return "vrf_outdoor";
    default: return "fan";
  }
}

function plantEquipmentType(eq: Record<string, unknown>): HvacNodeType {
  const t = String(eq.type ?? "").toLowerCase();
  switch (t) {
    case "pump": return "pump";
    case "boiler": return "boiler";
    case "chiller": return "chiller";
    case "cooling_tower": return "cooling_tower";
    case "heat_exchanger": return "heat_exchanger";
    case "thermal_storage": return "thermal_storage";
    case "gshp": return "gshp";
    default: return "pump";
  }
}

function terminalType(term: Record<string, unknown>): string {
  const t = String(term.type ?? "").toLowerCase();
  if (t === "vav_box") return "VAV Box";
  if (t === "pfp_box") return "PFP Box";
  if (t === "dual_duct_box") return "Dual Duct Box";
  return "Terminal";
}

function terminalHvacType(term: Record<string, unknown>): HvacNodeType {
  const t = String(term.type ?? "").toLowerCase();
  if (t === "vrf_indoor_unit") return "vrf_indoor";
  if (t === "radiant_panel") return "radiant_panel";
  return "terminal";
}

function fmtVal(v: unknown): string {
  if (v === "autosize" || v === undefined || v === null) return "autosize";
  if (typeof v === "number") return v.toLocaleString();
  return String(v);
}

function extractProperties(
  eq: Record<string, unknown>,
  keys: string[]
): Record<string, string | number | boolean> {
  const props: Record<string, string | number | boolean> = {};
  for (const k of keys) {
    if (eq[k] !== undefined && eq[k] !== null) {
      const v = eq[k];
      if (typeof v === "boolean" || typeof v === "number") {
        props[k] = v;
      } else {
        props[k] = String(v);
      }
    }
  }
  return props;
}

/** Only hot_water and chilled_water coils connect to plant loops */
function isWaterCoilSource(eq: Record<string, unknown>): boolean {
  const src = String(eq.source ?? "").toLowerCase();
  return src === "hot_water" || src === "chilled_water";
}

function sourceLabel(eq: Record<string, unknown>): string {
  const src = String(eq.source ?? "");
  const map: Record<string, string> = {
    hot_water: "Hot Water", chilled_water: "Chilled Water",
    electric: "Electric", gas: "Gas", dx: "DX",
    heat_pump: "Heat Pump", vav: "VAV",
    constant_volume: "Constant", on_off: "On/Off",
    wheel: "Wheel", plate: "Plate", runaround_coil: "Runaround",
  };
  return map[src] ?? src;
}

/* ------------------------------------------------------------------ */
/*  ID generation                                                      */
/* ------------------------------------------------------------------ */

let nodeIdCounter = 0;
function nextId(prefix: string): string {
  return `${prefix}_${++nodeIdCounter}`;
}

/* ------------------------------------------------------------------ */
/*  Edge styles                                                        */
/* ------------------------------------------------------------------ */

const AIR_EDGE = { stroke: "#7dcfff", strokeWidth: 2 };
const ZONE_EDGE = { stroke: "#9ece6a", strokeWidth: 2 };
const PLANT_EDGE = { stroke: "#bb9af7", strokeWidth: 2 };
const COIL_LOAD_EDGE = {
  stroke: "#e0af68", strokeWidth: 1.5, strokeDasharray: "6 3",
};

/* ------------------------------------------------------------------ */
/*  Gather zone properties from model                                  */
/* ------------------------------------------------------------------ */

/** Resolve zone_groups: if a zones array references a group name, expand it */
function resolveZones(
  zoneNames: string[] | undefined,
  zoneGroups: Record<string, unknown>[]
): string[] {
  if (!zoneNames) return [];
  const result: string[] = [];
  for (const name of zoneNames) {
    const group = zoneGroups.find((g) => String(g.name) === name);
    if (group && Array.isArray((group as Record<string, unknown>).zones)) {
      result.push(
        ...((group as Record<string, unknown>).zones as string[])
      );
    } else {
      result.push(name);
    }
  }
  return result;
}

interface ZoneInfo {
  volume?: number;
  floor_area?: number;
  heating_setpoint?: number;
  cooling_setpoint?: number;
  people?: string;
  lights?: string;
  equipment?: string;
  infiltration?: string;
}

function gatherZoneInfo(model: Model): Map<string, ZoneInfo> {
  const info = new Map<string, ZoneInfo>();
  const zoneGroups: Record<string, unknown>[] = model.zone_groups ?? [];

  // Zones: volume, floor_area
  for (const z of (model.zones ?? []) as Record<string, unknown>[]) {
    const name = String(z.name ?? "");
    info.set(name, {
      volume: z.volume as number | undefined,
      floor_area: z.floor_area as number | undefined,
    });
  }

  // Thermostats: setpoints
  for (const t of (model.thermostats ?? []) as Record<string, unknown>[]) {
    const zones = resolveZones(t.zones as string[] | undefined, zoneGroups);
    for (const zn of zones) {
      const zi = info.get(zn) ?? {};
      if (t.heating_setpoint !== undefined)
        zi.heating_setpoint = t.heating_setpoint as number;
      if (t.cooling_setpoint !== undefined)
        zi.cooling_setpoint = t.cooling_setpoint as number;
      info.set(zn, zi);
    }
  }

  // People
  for (const p of (model.people ?? []) as Record<string, unknown>[]) {
    const zones = resolveZones(p.zones as string[] | undefined, zoneGroups);
    const label = `${fmtVal(p.count)} ppl, ${fmtVal(p.activity_level)} W/ppl`;
    for (const zn of zones) {
      const zi = info.get(zn) ?? {};
      zi.people = zi.people ? `${zi.people}; ${label}` : label;
      info.set(zn, zi);
    }
  }

  // Lights
  for (const l of (model.lights ?? []) as Record<string, unknown>[]) {
    const zones = resolveZones(l.zones as string[] | undefined, zoneGroups);
    const label = `${fmtVal(l.power)} W`;
    for (const zn of zones) {
      const zi = info.get(zn) ?? {};
      zi.lights = zi.lights ? `${zi.lights}; ${label}` : label;
      info.set(zn, zi);
    }
  }

  // Equipment (internal gains)
  for (const e of (model.equipment ?? []) as Record<string, unknown>[]) {
    const zones = resolveZones(e.zones as string[] | undefined, zoneGroups);
    const label = `${fmtVal(e.power)} W`;
    for (const zn of zones) {
      const zi = info.get(zn) ?? {};
      zi.equipment = zi.equipment ? `${zi.equipment}; ${label}` : label;
      info.set(zn, zi);
    }
  }

  // Infiltration
  for (const inf of (model.infiltration ?? []) as Record<string, unknown>[]) {
    const zones = resolveZones(
      inf.zones as string[] | undefined,
      zoneGroups
    );
    const label = inf.air_changes_per_hour
      ? `${fmtVal(inf.air_changes_per_hour)} ACH`
      : inf.flow_per_area
        ? `${fmtVal(inf.flow_per_area)} m3/s/m2`
        : "defined";
    for (const zn of zones) {
      const zi = info.get(zn) ?? {};
      zi.infiltration = zi.infiltration
        ? `${zi.infiltration}; ${label}`
        : label;
      info.set(zn, zi);
    }
  }

  return info;
}

/* ------------------------------------------------------------------ */
/*  Build separate air-side and water-side graphs                      */
/* ------------------------------------------------------------------ */

export function buildSeparatedGraphs(model: Model): SeparatedHvacGraphs {
  nodeIdCounter = 0;

  const airNodes: Node<HvacNodeData>[] = [];
  const airEdges: Edge[] = [];
  const waterNodes: Node<HvacNodeData>[] = [];
  const waterEdges: Edge[] = [];

  const airLoops: Record<string, unknown>[] = model.air_loops ?? [];
  const plantLoops: Record<string, unknown>[] = model.plant_loops ?? [];
  const zoneInfo = gatherZoneInfo(model);

  // Track coils that reference plant loops (for the water-side view)
  interface CoilRef {
    name: string;
    coilType: "heating" | "cooling";
    airLoop: string;
    plantLoop: string;
  }
  const coilRefs: CoilRef[] = [];

  // ============================================================
  // AIR-SIDE GRAPH
  // ============================================================

  const zoneNodeIds = new Map<string, string>();

  for (const loop of airLoops) {
    const loopName = String(loop.name ?? "Air Loop");
    const equipment: Record<string, unknown>[] =
      (loop as Record<string, unknown>).equipment as Record<string, unknown>[] ?? [];
    const zoneTerminals: Record<string, unknown>[] =
      (loop as Record<string, unknown>).zone_terminals as Record<string, unknown>[] ?? [];

    // OA intake
    const oaId = nextId("oa");
    airNodes.push({
      id: oaId,
      type: "hvacNode",
      position: { x: 0, y: 0 },
      data: {
        hvacType: "oa_intake",
        label: "Outside Air",
        sublabel: loopName,
        properties: {},
        airLoop: loopName,
      },
    });

    let prevId = oaId;

    for (const eq of equipment) {
      const nodeType = equipmentType(eq);
      const id = nextId(`air_${nodeType}`);
      const name = String(eq.name ?? nodeType);
      let sublabel = sourceLabel(eq);
      const props: Record<string, string | number | boolean> = {};
      let plantLoopRef: string | undefined;

      switch (nodeType) {
        case "fan":
          Object.assign(props, extractProperties(eq, [
            "source", "design_flow_rate", "pressure_rise", "motor_efficiency",
          ]));
          break;
        case "heating_coil":
          Object.assign(props, extractProperties(eq, [
            "source", "capacity", "setpoint", "efficiency", "plant_loop",
          ]));
          // Only water coils (hot_water/chilled_water source) reference a plant loop
          if (eq.plant_loop && isWaterCoilSource(eq)) {
            plantLoopRef = String(eq.plant_loop);
            coilRefs.push({
              name, coilType: "heating",
              airLoop: loopName, plantLoop: plantLoopRef,
            });
          }
          break;
        case "cooling_coil":
          Object.assign(props, extractProperties(eq, [
            "source", "capacity", "cop", "shr", "setpoint", "plant_loop",
          ]));
          if (eq.plant_loop && isWaterCoilSource(eq)) {
            plantLoopRef = String(eq.plant_loop);
            coilRefs.push({
              name, coilType: "cooling",
              airLoop: loopName, plantLoop: plantLoopRef,
            });
          }
          break;
        case "heat_recovery":
          Object.assign(props, extractProperties(eq, [
            "source", "sensible_effectiveness", "latent_effectiveness",
          ]));
          break;
        case "humidifier":
          Object.assign(props, extractProperties(eq, [
            "rated_power", "min_rh_setpoint",
          ]));
          break;
        case "duct":
          Object.assign(props, extractProperties(eq, [
            "length", "diameter", "u_value", "leakage_fraction",
          ]));
          break;
        case "evap_cooler":
          sublabel = String(eq.mode ?? "direct").replace(/_/g, " ");
          Object.assign(props, extractProperties(eq, [
            "mode", "direct_effectiveness", "indirect_effectiveness",
          ]));
          break;
        case "vrf_outdoor":
          sublabel = `Cap: ${fmtVal(eq.rated_cooling_capacity)} W`;
          Object.assign(props, extractProperties(eq, [
            "rated_cooling_capacity", "rated_heating_capacity", "cop_cooling", "cop_heating",
          ]));
          break;
      }

      airNodes.push({
        id,
        type: "hvacNode",
        position: { x: 0, y: 0 },
        data: {
          hvacType: nodeType,
          label: name,
          sublabel,
          plantLoopRef,
          properties: props,
          airLoop: loopName,
          componentName: name,
        },
      });

      airEdges.push({
        id: `e_${prevId}_${id}`,
        source: prevId,
        target: id,
        type: "smoothstep",
        style: AIR_EDGE,
      });

      prevId = id;
    }

    const lastEquipId = prevId;

    // Zone terminals + zones
    for (const zt of zoneTerminals) {
      const zoneName = String(zt.zone ?? "Zone");
      const terminal = zt.terminal as Record<string, unknown> | undefined;
      let connectTo = lastEquipId;

      if (terminal) {
        const termId = nextId("term");
        const termName = String(
          terminal.name ?? `${terminalType(terminal)} - ${zoneName}`
        );
        const termHvacType = terminalHvacType(terminal);
        const termSublabel = termHvacType === "radiant_panel"
          ? sourceLabel(terminal)
          : terminalType(terminal);
        const termProps = extractProperties(terminal, [
          "type", "max_air_flow", "min_flow_fraction", "reheat_type",
          "reheat_capacity", "max_primary_flow", "source",
        ]);

        airNodes.push({
          id: termId,
          type: "hvacNode",
          position: { x: 0, y: 0 },
          data: {
            hvacType: termHvacType,
            label: termName,
            sublabel: termSublabel,
            properties: termProps,
            airLoop: loopName,
            componentName: termName,
          },
        });

        airEdges.push({
          id: `e_${lastEquipId}_${termId}`,
          source: lastEquipId,
          target: termId,
          type: "smoothstep",
          style: AIR_EDGE,
        });

        connectTo = termId;

        // Terminal reheat coil → plant reference
        if (terminal.plant_loop) {
          coilRefs.push({
            name: termName,
            coilType: "heating",
            airLoop: loopName,
            plantLoop: String(terminal.plant_loop),
          });
        }
      }

      // Zone node (deduplicate)
      let zoneId = zoneNodeIds.get(zoneName);
      if (!zoneId) {
        zoneId = nextId("zone");
        zoneNodeIds.set(zoneName, zoneId);

        const zi = zoneInfo.get(zoneName);
        const zoneProps: Record<string, string | number | boolean> = {};
        if (zi) {
          if (zi.volume !== undefined) zoneProps.volume = `${zi.volume} m\u00B3`;
          if (zi.floor_area !== undefined) zoneProps.floor_area = `${zi.floor_area} m\u00B2`;
        }

        const sublabel = zi
          ? [
              zi.volume !== undefined ? `${zi.volume} m\u00B3` : "",
              zi.floor_area !== undefined ? `${zi.floor_area} m\u00B2` : "",
            ].filter(Boolean).join(" | ") || undefined
          : undefined;

        airNodes.push({
          id: zoneId,
          type: "hvacNode",
          position: { x: 0, y: 0 },
          data: {
            hvacType: "zone",
            label: zoneName,
            sublabel,
            properties: zoneProps,
            airLoop: loopName,
            componentName: zoneName,
          },
        });
      }

      airEdges.push({
        id: `e_${connectTo}_${zoneId}`,
        source: connectTo,
        target: zoneId,
        type: "smoothstep",
        style: ZONE_EDGE,
      });
    }
  }

  // ============================================================
  // WATER-SIDE GRAPH
  // ============================================================

  for (const loop of plantLoops) {
    const loopName = String(loop.name ?? "Plant Loop");
    const equipment: Record<string, unknown>[] =
      (loop as Record<string, unknown>).supply_equipment as Record<string, unknown>[] ?? [];

    let prevId: string | null = null;

    for (const eq of equipment) {
      const nodeType = plantEquipmentType(eq);
      const id = nextId(`plant_${nodeType}`);
      const name = String(eq.name ?? nodeType);

      let sublabel = "";
      const props: Record<string, string | number | boolean> = {};

      switch (nodeType) {
        case "pump":
          sublabel = String(eq.pump_type ?? "variable speed").replace(/_/g, " ");
          Object.assign(props, extractProperties(eq, [
            "pump_type", "design_flow_rate", "design_head", "motor_efficiency",
          ]));
          break;
        case "boiler":
          sublabel = `eff: ${fmtVal(eq.efficiency)}`;
          Object.assign(props, extractProperties(eq, [
            "capacity", "efficiency", "design_outlet_temp",
          ]));
          break;
        case "chiller":
          sublabel = `COP: ${fmtVal(eq.cop)}`;
          Object.assign(props, extractProperties(eq, [
            "capacity", "cop", "chw_setpoint", "condenser_type",
          ]));
          break;
        case "cooling_tower":
          sublabel = String(eq.tower_type ?? "variable speed").replace(/_/g, " ");
          Object.assign(props, extractProperties(eq, [
            "tower_type", "design_water_flow", "design_fan_power",
          ]));
          break;
        case "heat_exchanger":
          sublabel = `eff: ${fmtVal(eq.effectiveness)}`;
          Object.assign(props, extractProperties(eq, [
            "effectiveness", "source_loop", "control_mode",
          ]));
          break;
        case "thermal_storage": {
          const storType = String(eq.storage_type ?? "").replace(/_/g, " ");
          const strategy = String(eq.control_strategy ?? "").replace(/_/g, " ");
          sublabel = [storType, strategy].filter(Boolean).join(" | ");
          Object.assign(props, extractProperties(eq, [
            "storage_type", "control_strategy", "capacity",
          ]));
          break;
        }
        case "gshp":
          sublabel = `Cap: ${fmtVal(eq.rated_cooling_capacity)} W`;
          Object.assign(props, extractProperties(eq, [
            "rated_cooling_capacity", "rated_heating_capacity",
            "cop_cooling", "cop_heating",
          ]));
          break;
      }

      waterNodes.push({
        id,
        type: "hvacNode",
        position: { x: 0, y: 0 },
        data: {
          hvacType: nodeType,
          label: name,
          sublabel,
          properties: props,
          plantLoop: loopName,
          componentName: name,
        },
      });

      if (prevId) {
        waterEdges.push({
          id: `e_${prevId}_${id}`,
          source: prevId,
          target: id,
          type: "smoothstep",
          style: PLANT_EDGE,
        });
      }
      prevId = id;
    }

    // Add coil-load nodes: each coil that references this plant loop
    const loopCoils = coilRefs.filter((c) => c.plantLoop === loopName);
    const lastEquipId = prevId;

    for (const coil of loopCoils) {
      const coilId = nextId("coil_load");
      waterNodes.push({
        id: coilId,
        type: "hvacNode",
        position: { x: 0, y: 0 },
        data: {
          hvacType: "coil_load",
          label: coil.name,
          sublabel: `${coil.coilType === "heating" ? "Htg" : "Clg"} \u2190 ${coil.airLoop}`,
          airLoopRef: coil.airLoop,
          properties: { air_loop: coil.airLoop, type: coil.coilType },
          plantLoop: loopName,
          componentName: coil.name,
        },
      });

      if (lastEquipId) {
        waterEdges.push({
          id: `e_${lastEquipId}_${coilId}`,
          source: lastEquipId,
          target: coilId,
          type: "smoothstep",
          style: COIL_LOAD_EDGE,
        });
      }
    }
  }

  return {
    air: { nodes: airNodes, edges: airEdges },
    water: { nodes: waterNodes, edges: waterEdges },
  };
}
