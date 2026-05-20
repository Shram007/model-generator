//! Cross-tree shared event connector.
//!
//! # Purpose
//!
//! In Probabilistic Risk Assessment (PRA) it is common for the same
//! underlying failure mechanism to affect multiple systems simultaneously
//! (a *common-cause failure*).  The [`TreeConnector`] models this by
//! renaming a fraction of the basic events in each fault tree to a shared
//! name (`SHARED-BE-N`), so that the same event appears in two or more trees.
//!
//! # Workflow
//!
//! 1. Build multiple [`FaultTree`] instances independently (different seeds).
//! 2. Call [`TreeConnector::promote_shared_events`] to mutate each tree
//!    in-place, replacing some basic-event names with shared names.
//! 3. Serialize and write each tree — the shared names will appear verbatim
//!    in multiple XML files.
//!
//! # Helper
//!
//! [`manifest_rows`] builds the per-model metadata rows that are written to
//! the optional `manifest.json` file produced by the `batch` CLI command.

use std::collections::BTreeMap;

use fault_tree::FaultTree;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

// ─── Types ───────────────────────────────────────────────────────────────────

/// Records that a particular basic event name is shared across multiple trees.
///
/// After [`TreeConnector::promote_shared_events`] runs, every tree in
/// `tree_indices` will contain a basic event with `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedEvent {
    /// The canonical name of the shared event (e.g. `"SHARED-BE-1"`).
    pub name: String,
    /// Indices (into the caller's tree slice) of the trees that contain this
    /// shared event.
    pub tree_indices: Vec<usize>,
}

// ─── Connector ───────────────────────────────────────────────────────────────

/// Promotes basic events to be shared across multiple fault trees.
pub struct TreeConnector;

impl TreeConnector {
    /// Mutates each tree in `trees` so that a fraction of their basic events
    /// share the same name, modeling common-cause failures.
    ///
    /// # Algorithm
    ///
    /// 1. Determine how many events to share:
    ///    `shared_count = round(min_events × common_basic_event_fraction)`.
    /// 2. For each shared slot `i` (from 0 to `shared_count - 1`):
    ///    - Choose a canonical name `"SHARED-BE-{i+1}"`.
    ///    - For each tree, pick a random basic event and rename it (and all
    ///      gate input references to it) to the canonical name.
    ///
    /// # Arguments
    ///
    /// * `trees` — slice of fault trees to modify in-place.
    /// * `common_basic_event_fraction` — fraction of the *smallest* tree's
    ///   basic events to promote (clamped to `[0, min_events]`).
    /// * `seed` — RNG seed for reproducible promotion decisions.
    ///
    /// # Returns
    ///
    /// A [`Vec<SharedEvent>`] describing which events were promoted and in
    /// which trees they appear.  Returns an empty vec if `trees.len() < 2`.
    pub fn promote_shared_events(
        trees: &mut [FaultTree],
        common_basic_event_fraction: f64,
        seed: u64,
    ) -> Vec<SharedEvent> {
        // Need at least two trees for cross-tree sharing to make sense.
        if trees.len() < 2 {
            return Vec::new();
        }

        // The number of shared events is bounded by the smallest tree so
        // every tree can contribute at least one event per slot.
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

            // Pick a random basic event from each tree and rename it.
            for tree in trees.iter_mut() {
                // Sort the names for deterministic selection from the same seed.
                let mut names = tree.basic_events.keys().cloned().collect::<Vec<_>>();
                names.sort();
                let picked = names[rng.gen_range(0..names.len())].clone();
                Self::rename_basic_event(tree, &picked, &shared_name);
            }

            shared_events.push(SharedEvent {
                name: shared_name,
                // All trees participate in every shared event.
                tree_indices: (0..trees.len()).collect(),
            });
        }

        shared_events
    }

    /// Renames basic event `from` to `to` inside `tree`.
    ///
    /// The rename has two parts:
    /// 1. Update the key and `.name` field in `tree.basic_events`.
    /// 2. Update every gate `inputs` list that referenced the old name.
    ///
    /// After updating gate inputs the list is re-sorted and deduplicated
    /// because two events that had different names may now have the same name
    /// (the renamed event was already present as a different input).
    fn rename_basic_event(tree: &mut FaultTree, from: &str, to: &str) {
        // No-op if names are already the same.
        if from == to {
            return;
        }

        // Re-insert the BasicEvent under the new key.
        if let Some(mut be) = tree.basic_events.remove(from) {
            be.name = to.to_string();
            tree.basic_events.insert(to.to_string(), be);
        }

        // Patch every gate whose inputs list contained the old name.
        for gate in tree.gates.values_mut() {
            for input in &mut gate.inputs {
                if input == from {
                    *input = to.to_string();
                }
            }
            // Sort + dedup to keep inputs canonical in case the shared name
            // was already present as a different input of this gate.
            gate.inputs.sort();
            gate.inputs.dedup();
        }
    }
}

// ─── Manifest helper ─────────────────────────────────────────────────────────

/// Builds the per-model metadata rows written to `manifest.json` in the
/// `batch` CLI command.
///
/// Each row is a `BTreeMap<String, String>` with the keys `"path"`,
/// `"seed"`, `"node_count"`, and `"edge_count"`.  The caller is responsible
/// for ensuring the four input slices have the same length.
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use config::PdagConfig;
    use fault_tree::FaultTreeMapper;
    use pdag::PdagBuilder;

    use super::*;

    /// Build a fault tree from a given seed for use in tests.
    fn build_tree(name: &str, seed: u64) -> FaultTree {
        let mut cfg = PdagConfig::default();
        cfg.seed = seed;
        let pdag = PdagBuilder::new(cfg).unwrap().build().unwrap();
        FaultTreeMapper::from_pdag(name, &pdag).unwrap()
    }

    // ── Shared event propagation ─────────────────────────────────────────────

    #[test]
    fn shared_event_names_appear_in_both_trees() {
        // After promotion the canonical shared-event name must be present as a
        // basic event key in every participating tree.
        let mut trees = vec![build_tree("a", 1), build_tree("b", 2)];
        let shared = TreeConnector::promote_shared_events(&mut trees, 0.2, 123);
        assert!(!shared.is_empty());
        for se in shared {
            assert!(
                trees[0].basic_events.contains_key(&se.name),
                "tree 0 missing shared event {}",
                se.name
            );
            assert!(
                trees[1].basic_events.contains_key(&se.name),
                "tree 1 missing shared event {}",
                se.name
            );
        }
    }
}
