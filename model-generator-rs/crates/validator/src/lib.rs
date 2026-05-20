//! Validators for generated PRA models.
//!
//! Three independent validators are provided, each as a unit struct with a
//! single public `validate_*` method.  The CLI calls all three automatically
//! after generation; any failure returns a non-zero exit code.
//!
//! | Validator | Struct | Checks |
//! |-----------|--------|--------|
//! | Structural | [`StructuralValidator`] | Acyclicity, gate fan-in ≥ 2 |
//! | Probabilistic | [`ProbabilisticValidator`] | Every basic-event probability in `[0, 1]` |
//! | Reference integrity | [`ReferenceIntegrityValidator`] | All FT/ET cross-references resolve |

use std::collections::BTreeSet;

use event_tree::EventTree;
use fault_tree::FaultTree;
use pdag::{NodeKind, Pdag};
use petgraph::algo::is_cyclic_directed;
use thiserror::Error;

// ─── Error type ──────────────────────────────────────────────────────────────

/// A validation failure from any of the three validators.
///
/// Each variant wraps a human-readable description of what went wrong.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// The PDAG or fault tree has a structural defect (cycle, low fan-in, etc.)
    #[error("structural validation failed: {0}")]
    Structural(String),

    /// A basic-event probability is outside `[0, 1]`.
    #[error("probabilistic validation failed: {0}")]
    Probabilistic(String),

    /// A name referenced in the event tree is not defined in the fault tree.
    #[error("reference integrity validation failed: {0}")]
    Reference(String),
}

// ─── Structural validator ────────────────────────────────────────────────────

/// Validates structural properties of a [`Pdag`].
///
/// Checks performed:
/// 1. **Acyclicity** — uses `petgraph::algo::is_cyclic_directed`.
/// 2. **Gate fan-in** — every gate node must have ≥ 2 children.
/// 3. **BasicEvent kind consistency** — basic-event iterator returns nodes of
///    the correct kind (sanity check against iterator bugs).
pub struct StructuralValidator;

impl StructuralValidator {
    /// Validates the structure of `pdag`.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::Structural`] with a message describing the
    /// first defect found.
    pub fn validate_pdag(pdag: &Pdag) -> Result<(), ValidationError> {
        // 1. The graph must be a DAG (no cycles).
        if is_cyclic_directed(&pdag.graph) {
            return Err(ValidationError::Structural(
                "PDAG must be acyclic".to_string(),
            ));
        }

        // 2. Every gate (including root) must have at least 2 children, because
        //    a gate with 0 or 1 inputs is degenerate (equivalent to a wire or a
        //    NOT gate, neither of which is valid in fault-tree logic).
        for node in pdag.gates() {
            let child_count = pdag.children(node).count();
            if child_count < 2 {
                return Err(ValidationError::Structural(format!(
                    "gate {} has {child_count} children (< 2)",
                    pdag.graph[node].name
                )));
            }
        }

        // 3. Consistency sanity check: the basic-event iterator must only
        //    return nodes whose kind is BasicEvent.
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

// ─── Probabilistic validator ─────────────────────────────────────────────────

/// Validates that all basic-event probabilities in a [`FaultTree`] are valid.
///
/// Checks performed:
/// 1. The fault tree contains at least one basic event.
/// 2. Every basic-event probability is in `[0.0, 1.0]`.
pub struct ProbabilisticValidator;

impl ProbabilisticValidator {
    /// Validates the probabilities in `ft`.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::Probabilistic`] if the fault tree has no
    /// basic events or if any probability is out of range.
    pub fn validate_fault_tree(ft: &FaultTree) -> Result<(), ValidationError> {
        // A fault tree with no basic events cannot be quantified.
        if ft.basic_events.is_empty() {
            return Err(ValidationError::Probabilistic(
                "fault tree contains no basic events".to_string(),
            ));
        }

        // Check every basic event individually.
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

// ─── Reference integrity validator ───────────────────────────────────────────

/// Validates that all names used in an [`EventTree`] resolve to definitions
/// in its associated [`FaultTree`].
///
/// Checks performed:
/// 1. The fault tree's declared `top_gate` is in its own reference set.
/// 2. The event tree's initiating event `top_gate_ref` matches the fault
///    tree's `top_gate`.
/// 3. Every functional event's `fault_tree_ref` names a defined fault tree.
pub struct ReferenceIntegrityValidator;

impl ReferenceIntegrityValidator {
    /// Validates cross-references between `ft` and `et`.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::Reference`] with details of the first
    /// broken reference found.
    pub fn validate(ft: &FaultTree, et: &EventTree) -> Result<(), ValidationError> {
        // Build the complete set of names defined in the fault tree.
        let refs = ft.all_references();

        // 1. The top gate must be defined inside the fault tree itself.
        if !refs.contains(&ft.top_gate) {
            return Err(ValidationError::Reference(format!(
                "top gate {} is not defined",
                ft.top_gate
            )));
        }

        // 2. The event tree must point at the correct top gate.
        if et.initiating_event.top_gate_ref != ft.top_gate {
            return Err(ValidationError::Reference(format!(
                "initiating event top gate ref {} does not match fault tree top gate {}",
                et.initiating_event.top_gate_ref, ft.top_gate
            )));
        }

        // 3. Every functional event must reference a fault tree that is defined
        //    in the current model.  (In this single-tree model all FEs reference
        //    the one fault tree.)
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use config::PdagConfig;
    use event_tree::EventTreeMapper;
    use fault_tree::FaultTreeMapper;
    use pdag::PdagBuilder;

    use super::*;

    // ── Happy-path integration test ──────────────────────────────────────────

    #[test]
    fn validators_pass_for_generated_model() {
        // Build a complete model (PDAG → FaultTree → EventTree) with default
        // settings and assert that all three validators accept it.
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
