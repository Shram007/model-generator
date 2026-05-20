//! Cross-tree shared event connector.

use std::collections::BTreeMap;

use fault_tree::FaultTree;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedEvent {
    pub name: String,
    pub tree_indices: Vec<usize>,
}

pub struct TreeConnector;

impl TreeConnector {
    pub fn promote_shared_events(
        trees: &mut [FaultTree],
        common_basic_event_fraction: f64,
        seed: u64,
    ) -> Vec<SharedEvent> {
        if trees.len() < 2 {
            return Vec::new();
        }

        let min_events = trees
            .iter()
            .map(|t| t.basic_events.len())
            .min()
            .unwrap_or(0);
        if min_events == 0 {
            return Vec::new();
        }

        let shared_count = ((min_events as f64) * common_basic_event_fraction)
            .round()
            .clamp(0.0, min_events as f64) as usize;
        if shared_count == 0 {
            return Vec::new();
        }

        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut shared_events = Vec::new();

        for idx in 0..shared_count {
            let shared_name = format!("SHARED-BE-{}", idx + 1);
            for tree in trees.iter_mut() {
                let mut names = tree.basic_events.keys().cloned().collect::<Vec<_>>();
                names.sort();
                let picked = names[rng.gen_range(0..names.len())].clone();
                Self::rename_basic_event(tree, &picked, &shared_name);
            }
            shared_events.push(SharedEvent {
                name: shared_name,
                tree_indices: (0..trees.len()).collect(),
            });
        }

        shared_events
    }

    fn rename_basic_event(tree: &mut FaultTree, from: &str, to: &str) {
        if from == to {
            return;
        }
        if let Some(mut be) = tree.basic_events.remove(from) {
            be.name = to.to_string();
            tree.basic_events.insert(to.to_string(), be);
        }

        for gate in tree.gates.values_mut() {
            for input in &mut gate.inputs {
                if input == from {
                    *input = to.to_string();
                }
            }
            gate.inputs.sort();
            gate.inputs.dedup();
        }
    }
}

pub fn manifest_rows(
    paths: &[String],
    seeds: &[u64],
    node_counts: &[usize],
    edge_counts: &[usize],
) -> Vec<BTreeMap<String, String>> {
    paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let mut row = BTreeMap::new();
            row.insert("path".to_string(), path.clone());
            row.insert("seed".to_string(), seeds[i].to_string());
            row.insert("node_count".to_string(), node_counts[i].to_string());
            row.insert("edge_count".to_string(), edge_counts[i].to_string());
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use config::PdagConfig;
    use fault_tree::FaultTreeMapper;
    use pdag::PdagBuilder;

    use super::*;

    fn build_tree(name: &str, seed: u64) -> FaultTree {
        let mut cfg = PdagConfig::default();
        cfg.seed = seed;
        let pdag = PdagBuilder::new(cfg).unwrap().build().unwrap();
        FaultTreeMapper::from_pdag(name, &pdag).unwrap()
    }

    #[test]
    fn shared_event_names_appear_in_both_trees() {
        let mut trees = vec![build_tree("a", 1), build_tree("b", 2)];
        let shared = TreeConnector::promote_shared_events(&mut trees, 0.2, 123);
        assert!(!shared.is_empty());
        for se in shared {
            assert!(trees[0].basic_events.contains_key(&se.name));
            assert!(trees[1].basic_events.contains_key(&se.name));
        }
    }
}
