//! Event Tree domain model and mapper.
//!
//! # Concepts
//!
//! An **Event Tree** is a forward logic model used in Probabilistic Risk
//! Assessment (PRA) to enumerate accident sequences that can follow an
//! initiating event.  In this implementation an event tree is composed of:
//!
//! | Element | Role |
//! |---------|------|
//! | [`InitiatingEvent`] | The trigger (e.g. loss of coolant) backed by the top gate of a fault tree. |
//! | [`FunctionalEvent`] | A safety system (e.g. ECCS) that either succeeds or fails after the initiating event. |
//! | [`BranchSet`] | The ordered list of functional events that form the tree's header row. |
//! | [`Sequence`] | One accident-progression path — a combination of success/failure outcomes for every functional event. |
//! | [`EventTree`] | The container that groups all of the above. |
//!
//! # Sequence enumeration
//!
//! For `N` functional events the mapper generates `2^N` sequences by
//! treating each sequence index as an N-bit integer:
//! - bit `b` of index `i` set → functional event `b` succeeded in sequence `i`.
//! - bit `b` of index `i` clear → functional event `b` failed in sequence `i`.
//!
//! Sequence IDs are the zero-padded binary representations of their index,
//! e.g. `"SEQ-011"` for index 3 with N = 3.

use std::collections::BTreeSet;

use thiserror::Error;

// ─── Model types ─────────────────────────────────────────────────────────────

/// The event that starts the accident sequence.
///
/// `top_gate_ref` links back to the root gate of the Fault Tree that
/// quantifies the probability of the initiating event occurring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitiatingEvent {
    /// Human-readable name for the initiating event.
    pub name: String,
    /// Name of the fault-tree gate that represents this event's probability.
    pub top_gate_ref: String,
}

/// A mitigation or barrier system evaluated after the initiating event.
///
/// Each functional event is backed by a fault tree (`fault_tree_ref`), whose
/// top event quantifies the probability that the system fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionalEvent {
    /// Unique name for this functional event (e.g. `"FE-1"`).
    pub name: String,
    /// Name of the fault tree that models this system's failure probability.
    pub fault_tree_ref: String,
}

/// One accident-progression path through the event tree.
///
/// `outcomes` is a parallel array to the `functional_events` list of the
/// parent [`EventTree`]:  `outcomes[i]` is `true` if functional event `i`
/// succeeded in this sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequence {
    /// Zero-padded binary string that uniquely identifies this sequence
    /// (e.g. `"SEQ-01"` for the second sequence when N = 2).
    pub id: String,
    /// Per-functional-event outcome: `true` = success, `false` = failure.
    pub outcomes: Vec<bool>,
}

/// An ordered collection of functional events grouped under a single name.
///
/// In MEF XML this becomes the `<branch-set>` element.  The `functional_event_refs`
/// list must be in the same order as the `functional_events` list of the
/// parent [`EventTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSet {
    /// Name of this branch set.
    pub name: String,
    /// Ordered list of functional-event names belonging to this branch set.
    pub functional_event_refs: Vec<String>,
}

/// A complete Event Tree ready for XML serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventTree {
    /// Name of the event tree (used as the MEF XML `name` attribute).
    pub name: String,
    /// The event that triggers the accident sequence.
    pub initiating_event: InitiatingEvent,
    /// Ordered list of mitigation systems evaluated after the initiating event.
    pub functional_events: Vec<FunctionalEvent>,
    /// The branch-set header grouping all functional events.
    pub branch_set: BranchSet,
    /// All `2^N` accident sequences (one per combination of outcomes).
    pub sequences: Vec<Sequence>,
}

// ─── Error type ──────────────────────────────────────────────────────────────

/// Errors that can arise when building an [`EventTree`].
#[derive(Debug, Error)]
pub enum EventTreeError {
    /// `number_of_functional_events` was zero, which would produce no sequences.
    #[error("number_of_functional_events must be >= 1, got {0}")]
    InvalidFunctionalEventCount(usize),
}

// ─── Mapper ──────────────────────────────────────────────────────────────────

/// Builds an [`EventTree`] from configuration values.
///
/// Unlike the PDAG and FaultTree, the event tree structure is derived
/// analytically rather than from a graph: given N functional events, exactly
/// `2^N` sequences are created by enumerating all N-bit integers.
pub struct EventTreeMapper;

impl EventTreeMapper {
    /// Constructs an [`EventTree`] with `number_of_functional_events` branches.
    ///
    /// # Arguments
    ///
    /// * `model_name` — used to derive names for the event tree, initiating
    ///   event, and branch set.
    /// * `number_of_functional_events` — number of mitigation systems (≥ 1).
    ///   Determines the sequence count: `2^number_of_functional_events`.
    /// * `fault_tree_name` — name of the fault tree that backs every
    ///   functional event (all FEs reference the same FT in this model).
    /// * `top_gate_ref` — the root gate name used by the initiating event.
    ///
    /// # Errors
    ///
    /// Returns [`EventTreeError::InvalidFunctionalEventCount`] if
    /// `number_of_functional_events` is 0.
    pub fn build(
        model_name: &str,
        number_of_functional_events: usize,
        fault_tree_name: &str,
        top_gate_ref: &str,
    ) -> Result<EventTree, EventTreeError> {
        if number_of_functional_events == 0 {
            return Err(EventTreeError::InvalidFunctionalEventCount(0));
        }

        // Create one FunctionalEvent per mitigation system, all referencing
        // the same fault tree (single-tree model).
        let functional_events = (0..number_of_functional_events)
            .map(|idx| FunctionalEvent {
                name: format!("FE-{}", idx + 1),
                fault_tree_ref: fault_tree_name.to_string(),
            })
            .collect::<Vec<_>>();

        // Enumerate all 2^N combinations of success/failure outcomes.
        // Index i is treated as an N-bit integer where bit b represents the
        // outcome of functional event b (1 = success, 0 = failure).
        let sequence_count = 1usize << number_of_functional_events;
        let mut sequences = Vec::with_capacity(sequence_count);
        for i in 0..sequence_count {
            let mut outcomes = Vec::with_capacity(number_of_functional_events);
            for bit in 0..number_of_functional_events {
                outcomes.push((i & (1 << bit)) != 0);
            }
            sequences.push(Sequence {
                // Zero-padded binary ID, e.g. "SEQ-011" for i=3, N=3.
                id: format!("SEQ-{:0width$b}", i, width = number_of_functional_events),
                outcomes,
            });
        }

        Ok(EventTree {
            name: format!("{}-event-tree", model_name),
            initiating_event: InitiatingEvent {
                name: format!("{}-initiator", model_name),
                top_gate_ref: top_gate_ref.to_string(),
            },
            branch_set: BranchSet {
                name: format!("{}-branch-set", model_name),
                functional_event_refs: functional_events.iter().map(|fe| fe.name.clone()).collect(),
            },
            functional_events,
            sequences,
        })
    }
}

// ─── EventTree helpers ────────────────────────────────────────────────────────

impl EventTree {
    /// Returns the set of distinct fault tree names referenced by any
    /// functional event in this event tree.
    ///
    /// Used by the [`validator`] crate to verify that all referenced fault
    /// trees are actually defined in the model.
    pub fn referenced_fault_trees(&self) -> BTreeSet<&str> {
        self.functional_events
            .iter()
            .map(|fe| fe.fault_tree_ref.as_str())
            .collect()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Sequence count ───────────────────────────────────────────────────────

    #[test]
    fn creates_expected_sequence_count() {
        // 4 functional events → 2^4 = 16 sequences
        let et = EventTreeMapper::build("m", 4, "m-ft", "root").unwrap();
        assert_eq!(et.sequences.len(), 16);
    }

    // ── FT references ────────────────────────────────────────────────────────

    #[test]
    fn all_functional_events_reference_fault_tree() {
        let et = EventTreeMapper::build("m", 3, "m-ft", "root").unwrap();
        assert!(et
            .functional_events
            .iter()
            .all(|fe| fe.fault_tree_ref == "m-ft"));
    }
}
