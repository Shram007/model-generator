//! Validators for generated models.

use std::collections::BTreeSet;

use event_tree::EventTree;
use fault_tree::FaultTree;
use pdag::{NodeKind, Pdag};
use petgraph::algo::is_cyclic_directed;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("structural validation failed: {0}")]
    Structural(String),
    #[error("probabilistic validation failed: {0}")]
    Probabilistic(String),
    #[error("reference integrity validation failed: {0}")]
    Reference(String),
}

pub struct StructuralValidator;
pub struct ProbabilisticValidator;
pub struct ReferenceIntegrityValidator;

impl StructuralValidator {
    pub fn validate_pdag(pdag: &Pdag) -> Result<(), ValidationError> {
        if is_cyclic_directed(&pdag.graph) {
            return Err(ValidationError::Structural(
                "PDAG must be acyclic".to_string(),
            ));
        }

        for node in pdag.gates() {
            let child_count = pdag.children(node).count();
            if child_count < 2 {
                return Err(ValidationError::Structural(format!(
                    "gate {} has {child_count} children (< 2)",
                    pdag.graph[node].name
                )));
            }
        }

        for node in pdag.basic_events() {
            if !matches!(pdag.graph[node].kind, NodeKind::BasicEvent) {
                return Err(ValidationError::Structural(
                    "non-basic-event returned from basic_event iterator".to_string(),
                ));
            }
        }

        Ok(())
    }
}

impl ProbabilisticValidator {
    pub fn validate_fault_tree(ft: &FaultTree) -> Result<(), ValidationError> {
        if ft.basic_events.is_empty() {
            return Err(ValidationError::Probabilistic(
                "fault tree contains no basic events".to_string(),
            ));
        }

        for be in ft.basic_events.values() {
            if !(0.0..=1.0).contains(&be.probability) {
                return Err(ValidationError::Probabilistic(format!(
                    "basic event {} probability {} is outside [0,1]",
                    be.name, be.probability
                )));
            }
        }

        Ok(())
    }
}

impl ReferenceIntegrityValidator {
    pub fn validate(ft: &FaultTree, et: &EventTree) -> Result<(), ValidationError> {
        let refs = ft.all_references();
        if !refs.contains(&ft.top_gate) {
            return Err(ValidationError::Reference(format!(
                "top gate {} is not defined",
                ft.top_gate
            )));
        }

        if et.initiating_event.top_gate_ref != ft.top_gate {
            return Err(ValidationError::Reference(format!(
                "initiating event top gate ref {} does not match fault tree top gate {}",
                et.initiating_event.top_gate_ref, ft.top_gate
            )));
        }

        let known_trees: BTreeSet<_> = [ft.name.as_str()].into_iter().collect();
        for fe in &et.functional_events {
            if !known_trees.contains(fe.fault_tree_ref.as_str()) {
                return Err(ValidationError::Reference(format!(
                    "functional event {} references undefined fault tree {}",
                    fe.name, fe.fault_tree_ref
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use config::PdagConfig;
    use event_tree::EventTreeMapper;
    use fault_tree::FaultTreeMapper;
    use pdag::PdagBuilder;

    use super::*;

    #[test]
    fn validators_pass_for_generated_model() {
        let pdag = PdagBuilder::new(PdagConfig::default())
            .unwrap()
            .build()
            .unwrap();
        let ft = FaultTreeMapper::from_pdag("model", &pdag).unwrap();
        let et = EventTreeMapper::build("model", 4, &ft.name, &ft.top_gate).unwrap();

        StructuralValidator::validate_pdag(&pdag).unwrap();
        ProbabilisticValidator::validate_fault_tree(&ft).unwrap();
        ReferenceIntegrityValidator::validate(&ft, &et).unwrap();
    }
}
