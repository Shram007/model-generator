//! Event Tree domain model and mapper.

use std::collections::BTreeSet;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitiatingEvent {
    pub name: String,
    pub top_gate_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionalEvent {
    pub name: String,
    pub fault_tree_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequence {
    pub id: String,
    pub outcomes: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSet {
    pub name: String,
    pub functional_event_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventTree {
    pub name: String,
    pub initiating_event: InitiatingEvent,
    pub functional_events: Vec<FunctionalEvent>,
    pub branch_set: BranchSet,
    pub sequences: Vec<Sequence>,
}

#[derive(Debug, Error)]
pub enum EventTreeError {
    #[error("number_of_functional_events must be >= 1, got {0}")]
    InvalidFunctionalEventCount(usize),
}

pub struct EventTreeMapper;

impl EventTreeMapper {
    pub fn build(
        model_name: &str,
        number_of_functional_events: usize,
        fault_tree_name: &str,
        top_gate_ref: &str,
    ) -> Result<EventTree, EventTreeError> {
        if number_of_functional_events == 0 {
            return Err(EventTreeError::InvalidFunctionalEventCount(0));
        }

        let functional_events = (0..number_of_functional_events)
            .map(|idx| FunctionalEvent {
                name: format!("FE-{}", idx + 1),
                fault_tree_ref: fault_tree_name.to_string(),
            })
            .collect::<Vec<_>>();

        let sequence_count = 1usize << number_of_functional_events;
        let mut sequences = Vec::with_capacity(sequence_count);
        for i in 0..sequence_count {
            let mut outcomes = Vec::with_capacity(number_of_functional_events);
            for bit in 0..number_of_functional_events {
                outcomes.push((i & (1 << bit)) != 0);
            }
            sequences.push(Sequence {
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

impl EventTree {
    pub fn referenced_fault_trees(&self) -> BTreeSet<&str> {
        self.functional_events
            .iter()
            .map(|fe| fe.fault_tree_ref.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_expected_sequence_count() {
        let et = EventTreeMapper::build("m", 4, "m-ft", "root").unwrap();
        assert_eq!(et.sequences.len(), 16);
    }

    #[test]
    fn all_functional_events_reference_fault_tree() {
        let et = EventTreeMapper::build("m", 3, "m-ft", "root").unwrap();
        assert!(et
            .functional_events
            .iter()
            .all(|fe| fe.fault_tree_ref == "m-ft"));
    }
}
