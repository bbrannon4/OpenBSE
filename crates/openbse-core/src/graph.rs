//! Graph-based HVAC topology.
//!
//! The user defines components and connections via YAML. The engine builds a directed
//! graph internally, creates anonymous nodes at connection points, determines simulation
//! order via topological sort, and handles circular dependencies with iteration.
//!
//! Users never define nodes, branches, branch lists, connector lists, or node lists.

use crate::ports::*;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::algo::toposort;
use petgraph::Direction;
use std::collections::HashMap;

// ─── Graph Structures ────────────────────────────────────────────────────────

/// A component wrapped in the graph — either air-side or plant-side.
pub enum GraphComponent {
    Air(Box<dyn AirComponent>),
    Plant(Box<dyn PlantComponent>),
}

impl std::fmt::Debug for GraphComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphComponent::Air(c) => write!(f, "Air({})", c.name()),
            GraphComponent::Plant(c) => write!(f, "Plant({})", c.name()),
        }
    }
}

/// Edge type in the simulation graph.
#[derive(Debug, Clone)]
pub enum ConnectionType {
    /// Air flows from one component to the next
    AirFlow,
    /// Water/fluid flows from one component to the next
    WaterFlow,
    /// A coil's water side is connected to a plant loop component
    AirToPlant,
}

/// The simulation topology graph.
///
/// Components are nodes. Connections are directed edges.
/// Simulation order is determined by topological sort.
pub struct SimulationGraph {
    graph: DiGraph<GraphComponent, ConnectionType>,
    /// Map from component name to graph node index
    name_to_node: HashMap<String, NodeIndex>,
    /// Simulation order (topologically sorted node indices)
    sim_order: Vec<NodeIndex>,
}

impl SimulationGraph {
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            name_to_node: HashMap::new(),
            sim_order: Vec::new(),
        }
    }

    /// Add an air-side component to the graph.
    pub fn add_air_component(&mut self, component: Box<dyn AirComponent>) -> NodeIndex {
        let name = component.name().to_string();
        let idx = self.graph.add_node(GraphComponent::Air(component));
        self.name_to_node.insert(name, idx);
        idx
    }

    /// Add a plant-side component to the graph.
    pub fn add_plant_component(&mut self, component: Box<dyn PlantComponent>) -> NodeIndex {
        let name = component.name().to_string();
        let idx = self.graph.add_node(GraphComponent::Plant(component));
        self.name_to_node.insert(name, idx);
        idx
    }

    /// Connect two components with an air flow edge.
    pub fn connect_air(&mut self, from: NodeIndex, to: NodeIndex) {
        self.graph.add_edge(from, to, ConnectionType::AirFlow);
    }

    /// Connect two components with a water flow edge.
    pub fn connect_water(&mut self, from: NodeIndex, to: NodeIndex) {
        self.graph.add_edge(from, to, ConnectionType::WaterFlow);
    }

    /// Connect a coil's water side to a plant component.
    pub fn connect_air_to_plant(&mut self, air_node: NodeIndex, plant_node: NodeIndex) {
        self.graph.add_edge(plant_node, air_node, ConnectionType::AirToPlant);
    }

    /// Look up a component's node index by name.
    pub fn node_by_name(&self, name: &str) -> Option<NodeIndex> {
        self.name_to_node.get(name).copied()
    }

    /// Compute simulation order via topological sort.
    /// Must be called after all components and connections are added.
    ///
    /// Returns an error if there's a true cycle that can't be resolved
    /// (plant loops with feedback are handled separately via iteration).
    pub fn compute_simulation_order(&mut self) -> Result<(), GraphError> {
        match toposort(&self.graph, None) {
            Ok(order) => {
                self.sim_order = order;
                Ok(())
            }
            Err(_) => {
                // Cycles exist (expected for plant loops).
                // Fall back to a BFS-based ordering with cycle-breaking.
                self.sim_order = self.compute_order_with_cycles();
                Ok(())
            }
        }
    }

    /// Fallback ordering for graphs with cycles (plant loops).
    /// Uses a modified Kahn's algorithm that breaks cycles at the
    /// component with the most incoming edges.
    fn compute_order_with_cycles(&self) -> Vec<NodeIndex> {
        let mut in_degree: HashMap<NodeIndex, usize> = HashMap::new();
        let mut order = Vec::new();

        for idx in self.graph.node_indices() {
            in_degree.insert(idx, self.graph.edges_directed(idx, Direction::Incoming).count());
        }

        let mut queue: Vec<NodeIndex> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&n, _)| n)
            .collect();

        let mut visited = std::collections::HashSet::new();

        while order.len() < self.graph.node_count() {
            if queue.is_empty() {
                // Break cycle: pick unvisited node with fewest remaining in-degree
                if let Some((&node, _)) = in_degree
                    .iter()
                    .filter(|(&n, &d)| !visited.contains(&n) && d > 0)
                    .min_by_key(|(_, &d)| d)
                {
                    queue.push(node);
                } else {
                    break;
                }
            }

            while let Some(node) = queue.pop() {
                if visited.contains(&node) {
                    continue;
                }
                visited.insert(node);
                order.push(node);

                for neighbor in self.graph.neighbors(node) {
                    if let Some(deg) = in_degree.get_mut(&neighbor) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 && !visited.contains(&neighbor) {
                            queue.push(neighbor);
                        }
                    }
                }
            }
        }

        order
    }

    /// Get the computed simulation order.
    pub fn simulation_order(&self) -> &[NodeIndex] {
        &self.sim_order
    }

    /// Get a reference to a component by its node index.
    pub fn component(&self, idx: NodeIndex) -> &GraphComponent {
        &self.graph[idx]
    }

    /// Get a mutable reference to a component by its node index.
    pub fn component_mut(&mut self, idx: NodeIndex) -> &mut GraphComponent {
        &mut self.graph[idx]
    }

    /// Get the upstream (predecessor) node indices for a given node.
    pub fn predecessors(&self, idx: NodeIndex) -> Vec<NodeIndex> {
        self.graph
            .neighbors_directed(idx, Direction::Incoming)
            .collect()
    }

    /// Number of components in the graph.
    pub fn component_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Iterate over all component names.
    pub fn component_names(&self) -> impl Iterator<Item = &str> {
        self.name_to_node.keys().map(|s| s.as_str())
    }

    /// Iterate over all air components mutably (for autosizing).
    pub fn air_components_mut(&mut self) -> Vec<&mut Box<dyn AirComponent>> {
        self.graph.node_weights_mut()
            .filter_map(|gc| match gc {
                GraphComponent::Air(comp) => Some(comp),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("Component not found: {0}")]
    ComponentNotFound(String),
    #[error("Invalid connection: {0}")]
    InvalidConnection(String),
    #[error("Unresolvable cycle in graph")]
    UnresolvableCycle,
}

// ─── Air Loop ────────────────────────────────────────────────────────────────

/// High-level air loop definition.
/// This is what the user defines in YAML — the engine builds the graph from it.
#[derive(Debug)]
pub struct AirLoop {
    pub name: String,
    /// Ordered list of supply-side component node indices.
    pub supply_components: Vec<NodeIndex>,
    /// Zone connections (zone name -> terminal unit node index).
    pub zone_terminals: Vec<(String, NodeIndex)>,
}

/// High-level plant loop definition.
#[derive(Debug)]
pub struct PlantLoop {
    pub name: String,
    /// Supply-side component node indices.
    pub supply_components: Vec<NodeIndex>,
    /// Demand-side connections (component node indices that draw from this loop).
    pub demand_components: Vec<NodeIndex>,
    /// Design loop temperature [°C]
    pub design_supply_temp: f64,
    /// Design loop delta-T [°C]
    pub design_delta_t: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal test component for graph testing.
    #[derive(Debug)]
    struct TestAirComponent {
        name: String,
    }

    impl AirComponent for TestAirComponent {
        fn name(&self) -> &str {
            &self.name
        }

        fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
            *inlet // pass-through
        }
    }

    #[test]
    fn test_graph_construction_and_toposort() {
        let mut graph = SimulationGraph::new();

        let fan = graph.add_air_component(Box::new(TestAirComponent {
            name: "Fan".to_string(),
        }));
        let coil = graph.add_air_component(Box::new(TestAirComponent {
            name: "Coil".to_string(),
        }));
        let mixer = graph.add_air_component(Box::new(TestAirComponent {
            name: "Mixer".to_string(),
        }));

        // Flow: Mixer -> Coil -> Fan
        graph.connect_air(mixer, coil);
        graph.connect_air(coil, fan);

        graph.compute_simulation_order().unwrap();

        let order = graph.simulation_order();
        assert_eq!(order.len(), 3);

        // Mixer must come before Coil, Coil before Fan
        let mixer_pos = order.iter().position(|&n| n == mixer).unwrap();
        let coil_pos = order.iter().position(|&n| n == coil).unwrap();
        let fan_pos = order.iter().position(|&n| n == fan).unwrap();
        assert!(mixer_pos < coil_pos);
        assert!(coil_pos < fan_pos);
    }

    #[test]
    fn test_name_lookup() {
        let mut graph = SimulationGraph::new();
        graph.add_air_component(Box::new(TestAirComponent {
            name: "My Fan".to_string(),
        }));

        assert!(graph.node_by_name("My Fan").is_some());
        assert!(graph.node_by_name("Nonexistent").is_none());
    }
}
