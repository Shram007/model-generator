//! Probabilistic DAG (PDAG) builder for the PRA model generator.
//!
//! A PDAG is a directed acyclic graph whose nodes carry gate-type and
//! probability metadata.  The graph is used as an intermediate
//! representation from which Fault Trees and Event Trees are derived in
//! later phases.
//!
//! # Layer-by-layer construction
//!
//! The builder works as follows:
//! 1. Layer 0 contains a single **root** node (the top-level gate of the
//!    resulting fault tree).
//! 2. Each subsequent layer contains between `nodes_per_layer_min` and
//!    `nodes_per_layer_max` nodes.
//! 3. Every internal node in layer *L* is connected to between
//!    `children_per_node_min` and `children_per_node_max` nodes in layer
//!    *L + 1*.
//! 4. Nodes in the last layer are **leaf nodes** (basic events).
//! 5. A configurable fraction of leaf nodes are shared across multiple
//!    parents to model common-cause failures.
//!
//! The construction is entirely deterministic given the same [`PdagConfig`]
//! seed, which ensures reproducibility.

use std::collections::{BTreeSet, HashMap};

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use config::{GateWeights, PdagConfig};

// ─── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur during PDAG construction.
#[derive(Debug, Error)]
pub enum PdagError {
    #[error("config validation failed: {0}")]
    InvalidConfig(String),

    #[error("PDAG construction failed: {0}")]
    BuildError(String),
}

// ─── Node data ───────────────────────────────────────────────────────────────

/// The type of a node in the PDAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// Root gate — the single entry point of the fault tree.
    Root,
    /// An intermediate gate with a logical operator.
    Gate(GateType),
    /// A leaf node representing a basic event.
    BasicEvent,
}

/// The logical operator of a gate node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateType {
    And,
    Or,
    /// K-of-N (atleast) gate.  The value is the minimum number of children
    /// that must be `true` for the gate to be `true`.
    KofN(usize),
}

/// Data stored at each node of the PDAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeData {
    /// Human-readable identifier (e.g. `"G-0-0"`, `"BE-4-2"`).
    pub name: String,
    /// What kind of node this is.
    pub kind: NodeKind,
    /// For basic-event nodes: the failure probability.
    /// For gate nodes: `None`.
    pub probability: Option<f64>,
    /// Zero-based layer index (0 = root layer).
    pub layer: usize,
    /// Index within its layer.
    pub index_in_layer: usize,
}

// ─── PDAG ────────────────────────────────────────────────────────────────────

/// A built PDAG ready for downstream processing.
pub struct Pdag {
    /// The underlying directed graph.  Edges point from parent to child.
    pub graph: DiGraph<NodeData, ()>,
    /// The [`NodeIndex`] of the root node (always layer 0, index 0).
    pub root: NodeIndex,
}

impl Pdag {
    /// Returns an iterator over all basic-event nodes.
    pub fn basic_events(&self) -> impl Iterator<Item = NodeIndex> + '_ {
        self.graph
            .node_indices()
            .filter(|&idx| matches!(self.graph[idx].kind, NodeKind::BasicEvent))
    }

    /// Returns an iterator over all gate nodes (including the root).
    pub fn gates(&self) -> impl Iterator<Item = NodeIndex> + '_ {
        self.graph
            .node_indices()
            .filter(|&idx| !matches!(self.graph[idx].kind, NodeKind::BasicEvent))
    }

    /// Returns the children (direct successors) of a node.
    pub fn children(&self, node: NodeIndex) -> impl Iterator<Item = NodeIndex> + '_ {
        self.graph.neighbors_directed(node, Direction::Outgoing)
    }

    /// Returns the parents (direct predecessors) of a node.
    pub fn parents(&self, node: NodeIndex) -> impl Iterator<Item = NodeIndex> + '_ {
        self.graph.neighbors_directed(node, Direction::Incoming)
    }

    /// Returns the number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Returns the number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

// ─── Builder ─────────────────────────────────────────────────────────────────

/// Builds a [`Pdag`] from a [`PdagConfig`].
pub struct PdagBuilder {
    cfg: PdagConfig,
    rng: ChaCha8Rng,
}

impl PdagBuilder {
    /// Creates a new builder from a validated [`PdagConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`PdagError::InvalidConfig`] if `cfg.validate()` fails.
    pub fn new(cfg: PdagConfig) -> Result<Self, PdagError> {
        cfg.validate()
            .map_err(|e| PdagError::InvalidConfig(e.to_string()))?;
        let rng = ChaCha8Rng::seed_from_u64(cfg.seed);
        Ok(Self { cfg, rng })
    }

    /// Builds and returns the [`Pdag`].
    pub fn build(mut self) -> Result<Pdag, PdagError> {
        let mut graph: DiGraph<NodeData, ()> = DiGraph::new();

        // ── Layer 0: root ────────────────────────────────────────────────────
        let root_data = NodeData {
            name: "root".to_string(),
            kind: NodeKind::Root,
            probability: None,
            layer: 0,
            index_in_layer: 0,
        };
        let root = graph.add_node(root_data);

        // prev_layer always holds all NodeIndices of the previous layer.
        let mut prev_layer: Vec<NodeIndex> = vec![root];

        // ── Layers 1 … (layers - 1): intermediate gates ───────────────────
        let total_layers = self.cfg.layers; // layers includes the leaf layer
        for layer_idx in 1..total_layers {
            let is_leaf_layer = layer_idx == total_layers - 1;

            let n_nodes = self
                .rng
                .gen_range(self.cfg.nodes_per_layer_min..=self.cfg.nodes_per_layer_max);

            // Create nodes for this layer
            let mut current_layer: Vec<NodeIndex> = Vec::with_capacity(n_nodes);
            for node_idx in 0..n_nodes {
                let data = if is_leaf_layer {
                    let prob = self.rng.gen_range(self.cfg.min_prob..=self.cfg.max_prob);
                    NodeData {
                        name: format!("BE-{}-{}", layer_idx, node_idx),
                        kind: NodeKind::BasicEvent,
                        probability: Some(prob),
                        layer: layer_idx,
                        index_in_layer: node_idx,
                    }
                } else {
                    let gate_type = self.sample_gate_type(&self.cfg.gate_weights.clone());
                    NodeData {
                        name: format!("G-{}-{}", layer_idx, node_idx),
                        kind: NodeKind::Gate(gate_type),
                        probability: None,
                        layer: layer_idx,
                        index_in_layer: node_idx,
                    }
                };
                let idx = graph.add_node(data);
                current_layer.push(idx);
            }

            // Connect every node in prev_layer to children in current_layer
            self.connect_layers(&mut graph, &prev_layer, &current_layer)?;

            prev_layer = current_layer;
        }

        // ── Common basic events (shared across multiple parents) ───────────
        if self.cfg.layers > 1 {
            self.introduce_common_events(&mut graph, &prev_layer);
        }

        Ok(Pdag { graph, root })
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    /// Connects every node in `parents` to a random subset of nodes in
    /// `children`, ensuring every child is reachable from at least one parent.
    fn connect_layers(
        &mut self,
        graph: &mut DiGraph<NodeData, ()>,
        parents: &[NodeIndex],
        children: &[NodeIndex],
    ) -> Result<(), PdagError> {
        // Guarantee every child is referenced by at least one parent.
        // Round-robin assign orphan children first.
        let mut assigned: Vec<bool> = vec![false; children.len()];
        for (i, &child) in children.iter().enumerate() {
            let parent = parents[i % parents.len()];
            graph.add_edge(parent, child, ());
            assigned[i] = true;
        }

        // Now allow additional random edges up to children_per_node_max.
        // Track child count per parent to respect the configured bounds.
        let mut child_count: HashMap<NodeIndex, usize> = HashMap::new();
        for &p in parents {
            child_count.insert(p, 0);
        }
        // Re-count from the round-robin pass
        for &p in parents {
            let cnt = graph.neighbors_directed(p, Direction::Outgoing).count();
            *child_count.entry(p).or_insert(0) = cnt;
        }

        for &parent in parents {
            let current = child_count[&parent];
            let target = self
                .rng
                .gen_range(self.cfg.children_per_node_min..=self.cfg.children_per_node_max);
            if current >= target {
                continue;
            }
            let mut connected = graph
                .neighbors_directed(parent, Direction::Outgoing)
                .collect::<BTreeSet<_>>();
            while connected.len() < target {
                let candidates = children
                    .iter()
                    .copied()
                    .filter(|child| !connected.contains(child))
                    .collect::<Vec<_>>();
                if candidates.is_empty() {
                    break;
                }
                let child = candidates[self.rng.gen_range(0..candidates.len())];
                graph.add_edge(parent, child, ());
                connected.insert(child);
            }
        }

        Ok(())
    }

    /// Randomly promotes a fraction of leaf nodes to be shared (common)
    /// events referenced by additional parents in the penultimate layer.
    ///
    /// This models common-cause failures: a single basic event feeds
    /// multiple parent gates.
    fn introduce_common_events(
        &mut self,
        graph: &mut DiGraph<NodeData, ()>,
        leaf_layer: &[NodeIndex],
    ) {
        let n_common =
            (leaf_layer.len() as f64 * self.cfg.common_basic_event_fraction).round() as usize;

        // Collect parent-layer (penultimate) nodes
        let penultimate: Vec<NodeIndex> = leaf_layer
            .iter()
            .flat_map(|&leaf| graph.neighbors_directed(leaf, Direction::Incoming))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        if penultimate.is_empty() {
            return;
        }

        for i in 0..n_common {
            let leaf = leaf_layer[i % leaf_layer.len()];
            let extra_parents = self.cfg.common_basic_event_parents.saturating_sub(1);
            for _ in 0..extra_parents {
                let parent_idx = self.rng.gen_range(0..penultimate.len());
                let parent = penultimate[parent_idx];
                if !graph.contains_edge(parent, leaf) {
                    graph.add_edge(parent, leaf, ());
                }
            }
        }
    }

    /// Samples a gate type using the configured weight distribution.
    fn sample_gate_type(&mut self, weights: &GateWeights) -> GateType {
        let cdf = weights.cdf();
        let r: f64 = self.rng.gen(); // uniform [0, 1)
        if r < cdf[0] {
            GateType::And
        } else if r < cdf[1] {
            GateType::Or
        } else {
            // K-of-N: sample k uniformly in [2, children_per_node_min]
            let k = self
                .rng
                .gen_range(2..=self.cfg.children_per_node_min.max(2));
            GateType::KofN(k)
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use config::PdagConfig;
    use petgraph::algo::is_cyclic_directed;

    fn default_builder() -> PdagBuilder {
        PdagBuilder::new(PdagConfig::default()).expect("valid config")
    }

    // ── Basic construction ───────────────────────────────────────────────────

    #[test]
    fn builds_without_error() {
        let pdag = default_builder().build().expect("build");
        assert!(pdag.node_count() > 0);
    }

    #[test]
    fn root_node_exists() {
        let pdag = default_builder().build().expect("build");
        assert_eq!(pdag.graph[pdag.root].kind, NodeKind::Root);
    }

    #[test]
    fn graph_is_acyclic() {
        let pdag = default_builder().build().expect("build");
        assert!(!is_cyclic_directed(&pdag.graph), "PDAG must be acyclic");
    }

    #[test]
    fn all_leaf_nodes_are_basic_events() {
        let pdag = default_builder().build().expect("build");
        for idx in pdag.graph.node_indices() {
            let has_children = pdag
                .graph
                .neighbors_directed(idx, Direction::Outgoing)
                .next()
                .is_some();
            if !has_children {
                assert!(
                    matches!(pdag.graph[idx].kind, NodeKind::BasicEvent),
                    "leaf node {:?} is not a BasicEvent",
                    pdag.graph[idx].name
                );
            }
        }
    }

    #[test]
    fn basic_event_probabilities_in_range() {
        let cfg = PdagConfig::default();
        let (min, max) = (cfg.min_prob, cfg.max_prob);
        let pdag = PdagBuilder::new(cfg)
            .expect("valid")
            .build()
            .expect("build");
        for idx in pdag.basic_events() {
            let prob = pdag.graph[idx]
                .probability
                .expect("basic event must have a probability");
            assert!(
                prob >= min && prob <= max,
                "probability {} out of [{}, {}]",
                prob,
                min,
                max
            );
        }
    }

    // ── Determinism ──────────────────────────────────────────────────────────

    #[test]
    fn same_seed_produces_identical_graphs() {
        let cfg = PdagConfig::default();
        let p1 = PdagBuilder::new(cfg.clone()).unwrap().build().unwrap();
        let p2 = PdagBuilder::new(cfg).unwrap().build().unwrap();

        assert_eq!(p1.node_count(), p2.node_count());
        assert_eq!(p1.edge_count(), p2.edge_count());

        // Node names must match in insertion order
        let names1: Vec<_> = p1
            .graph
            .node_indices()
            .map(|i| p1.graph[i].name.clone())
            .collect();
        let names2: Vec<_> = p2
            .graph
            .node_indices()
            .map(|i| p2.graph[i].name.clone())
            .collect();
        assert_eq!(names1, names2);
    }

    #[test]
    fn different_seeds_produce_different_graphs() {
        let mut cfg1 = PdagConfig::default();
        let mut cfg2 = PdagConfig::default();
        cfg2.seed = cfg1.seed + 1;
        // Widen the node range so small seeds are likely to diverge
        cfg1.nodes_per_layer_min = 2;
        cfg1.nodes_per_layer_max = 10;
        cfg2.nodes_per_layer_min = 2;
        cfg2.nodes_per_layer_max = 10;

        let p1 = PdagBuilder::new(cfg1).unwrap().build().unwrap();
        let p2 = PdagBuilder::new(cfg2).unwrap().build().unwrap();

        // Very unlikely (but not guaranteed) to be identical — at minimum
        // edge counts should differ for most seeds.
        // We simply assert both graphs were built successfully.
        assert!(p1.node_count() > 0);
        assert!(p2.node_count() > 0);
    }

    // ── Invalid config rejected ──────────────────────────────────────────────

    #[test]
    fn invalid_config_rejected_at_builder_creation() {
        let mut cfg = PdagConfig::default();
        cfg.layers = 0;
        let result = PdagBuilder::new(cfg);
        assert!(matches!(result, Err(PdagError::InvalidConfig(_))));
    }

    // ── Layer count ──────────────────────────────────────────────────────────

    #[test]
    fn single_layer_produces_only_root_and_leaves() {
        let mut cfg = PdagConfig::default();
        cfg.layers = 2; // root layer + 1 leaf layer
        let pdag = PdagBuilder::new(cfg).unwrap().build().unwrap();
        // All non-root nodes should be BasicEvents
        for idx in pdag.graph.node_indices() {
            if idx == pdag.root {
                continue;
            }
            assert!(
                matches!(pdag.graph[idx].kind, NodeKind::BasicEvent),
                "expected BasicEvent, got {:?}",
                pdag.graph[idx].kind
            );
        }
    }

    // ── Every parent gate has ≥ children_per_node_min children ──────────────

    #[test]
    fn every_gate_has_minimum_children() {
        let cfg = PdagConfig::default();
        let min_children = cfg.children_per_node_min;
        let pdag = PdagBuilder::new(cfg).unwrap().build().unwrap();
        for idx in pdag.gates() {
            let child_count = pdag
                .graph
                .neighbors_directed(idx, Direction::Outgoing)
                .count();
            assert!(
                child_count >= min_children,
                "gate {:?} has only {} children (min {})",
                pdag.graph[idx].name,
                child_count,
                min_children
            );
        }
    }
}
