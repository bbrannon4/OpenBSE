import { useMemo, useCallback, memo } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  type Node,
  type Edge,
  type NodeProps,
  Handle,
  Position,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import dagre from "@dagrejs/dagre";
import {
  NODE_COLORS,
  type HvacNodeData,
  type HvacGraph,
} from "../lib/hvac-graph";

/* ------------------------------------------------------------------ */
/*  Dagre layout                                                       */
/* ------------------------------------------------------------------ */

function applyDagreLayout(
  nodes: Node<HvacNodeData>[],
  edges: Edge[]
): Node<HvacNodeData>[] {
  if (nodes.length === 0) return [];

  const nodeIds = new Set(nodes.map((n) => n.id));
  const subEdges = edges.filter(
    (e) => nodeIds.has(e.source) && nodeIds.has(e.target)
  );

  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({
    rankdir: "LR",
    nodesep: 50,
    ranksep: 100,
    marginx: 30,
    marginy: 30,
  });

  for (const node of nodes) {
    g.setNode(node.id, { width: 180, height: 60 });
  }
  for (const edge of subEdges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  return nodes.map((node) => {
    const pos = g.node(node.id);
    return {
      ...node,
      position: {
        x: (pos?.x ?? 0) - 90,
        y: (pos?.y ?? 0) - 30,
      },
    };
  });
}

/* ------------------------------------------------------------------ */
/*  Custom HVAC Node                                                   */
/* ------------------------------------------------------------------ */

const ICON_MAP: Record<string, string> = {
  oa_intake: "\u{1F32C}\uFE0F",
  fan: "\u{1FA81}",
  heating_coil: "\u{1F525}",
  cooling_coil: "\u2744\uFE0F",
  heat_recovery: "\u267B\uFE0F",
  humidifier: "\u{1F4A7}",
  duct: "\u25AD",
  zone: "\u{1F3E0}",
  terminal: "\u25A3",
  pump: "\u{1F504}",
  boiler: "\u{1F525}",
  chiller: "\u2744\uFE0F",
  cooling_tower: "\u{1F3ED}",
  heat_exchanger: "\u21C4",
  coil_load: "\u{1F50C}",
};

const HvacNode = memo(function HvacNode({
  data,
}: NodeProps<Node<HvacNodeData>>) {
  const color = NODE_COLORS[data.hvacType] ?? "#565f89";
  const icon = ICON_MAP[data.hvacType] ?? "\u25A0";

  const tooltip = Object.entries(data.properties)
    .map(([k, v]) => `${k}: ${v}`)
    .join("\n");

  return (
    <div
      className="hvac-node"
      style={{ borderColor: color }}
      title={tooltip || data.label}
    >
      <Handle type="target" position={Position.Left} className="hvac-handle" />
      <div className="hvac-node-icon" style={{ color }}>
        {icon}
      </div>
      <div className="hvac-node-content">
        <div className="hvac-node-label">{data.label}</div>
        {data.sublabel && (
          <div className="hvac-node-sublabel">{data.sublabel}</div>
        )}
        {data.plantLoopRef && (
          <div className="hvac-node-badge hvac-badge-water">
            {"\uD83D\uDCA7"} {data.plantLoopRef}
          </div>
        )}
        {data.airLoopRef && (
          <div className="hvac-node-badge hvac-badge-air">
            {"\uD83C\uDF2C\uFE0F"} {data.airLoopRef}
          </div>
        )}
      </div>
      <Handle
        type="source"
        position={Position.Right}
        className="hvac-handle"
      />
    </div>
  );
});

const nodeTypes = { hvacNode: HvacNode };

/* ------------------------------------------------------------------ */
/*  Main component                                                     */
/* ------------------------------------------------------------------ */

interface HvacDiagramProps {
  graph: HvacGraph;
  onNodeClick?: (componentName: string) => void;
}

export function HvacDiagram({ graph, onNodeClick }: HvacDiagramProps) {
  const { nodes: layoutNodes, edges: layoutEdges } = useMemo(() => {
    const laid = applyDagreLayout(graph.nodes, graph.edges);
    // Filter edges to only valid nodes
    const validIds = new Set(laid.map((n) => n.id));
    const validEdges = graph.edges.filter(
      (e) => validIds.has(e.source) && validIds.has(e.target)
    );
    return { nodes: laid, edges: validEdges };
  }, [graph]);

  const handleNodeClick = useCallback(
    (_: React.MouseEvent, node: Node) => {
      const data = node.data as HvacNodeData;
      if (onNodeClick && data?.componentName) {
        onNodeClick(data.componentName);
      }
    },
    [onNodeClick]
  );

  if (layoutNodes.length === 0) {
    return (
      <div className="hvac-empty">
        <p>No components to display.</p>
      </div>
    );
  }

  return (
    <div className="hvac-diagram-container">
      <ReactFlow
        nodes={layoutNodes}
        edges={layoutEdges}
        nodeTypes={nodeTypes}
        onNodeClick={handleNodeClick}
        fitView
        fitViewOptions={{ padding: 0.15 }}
        minZoom={0.1}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={true}
      >
        <Background color="#2f3146" gap={20} size={1} />
        <Controls
          showInteractive={false}
          style={{ background: "#1f2033", borderColor: "#2f3146" }}
        />
        <MiniMap
          nodeColor={(node) => {
            const d = node.data as HvacNodeData;
            return NODE_COLORS[d?.hvacType] ?? "#565f89";
          }}
          style={{
            background: "#1a1b26",
            border: "1px solid #2f3146",
          }}
          maskColor="rgba(26, 27, 38, 0.7)"
        />
      </ReactFlow>
    </div>
  );
}
