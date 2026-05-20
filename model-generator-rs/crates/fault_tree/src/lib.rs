//! Fault Tree domain model, PDAG mapper, and Open-PSA MEF XML serializer.
//!
//! # Responsibility
//!
//! This crate sits between the [`pdag`] crate (which builds a raw directed
//! acyclic graph) and the final output files.  It does two things:
//!
//! 1. **Model mapping** (`FaultTreeMapper`) — walks every node in a [`Pdag`]
//!    and classifies it as either a [`Gate`] or a [`BasicEvent`], collecting
//!    them into a typed [`FaultTree`] struct.
//!
//! 2. **XML serialization** (`XmlSerializer`) — writes a [`FaultTree`] (and an
//!    optional [`EventTree`]) to a string that conforms to the Open-PSA Model
//!    Exchange Format (MEF) v2.0 schema.
//!
//! # Data-flow summary
//!
//! ```text
//! PdagConfig ──► PdagBuilder ──► Pdag
//!                                  │
//!                         FaultTreeMapper::from_pdag
//!                                  │
//!                              FaultTree
//!                                  │
//!                    XmlSerializer::serialize_model
//!                                  │
//!                          Open-PSA MEF XML string
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use event_tree::EventTree;
use pdag::{GateType as PdagGateType, NodeKind, Pdag};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::Writer;
use thiserror::Error;

// ─── Gate type ───────────────────────────────────────────────────────────────

/// The logical operator applied by a fault-tree gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateType {
    /// All children must occur for the gate to be `true`.
    And,
    /// At least one child must occur for the gate to be `true`.
    Or,
    /// At least `k` out of N children must occur.
    KofN(usize),
}

// ─── Model types ─────────────────────────────────────────────────────────────

/// A logical gate in the fault tree.
///
/// A gate has a name, a logical operator ([`GateType`]), and a sorted list of
/// child names (which may be other gate names or basic-event names).
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    /// Unique identifier for this gate within the fault tree.
    pub name: String,
    /// The logical operator applied to the inputs.
    pub gate_type: GateType,
    /// Sorted list of child names (gate or basic-event names).
    pub inputs: Vec<String>,
}

/// A leaf failure event with an associated probability.
#[derive(Debug, Clone, PartialEq)]
pub struct BasicEvent {
    /// Unique identifier for this basic event.
    pub name: String,
    /// Unconditional failure probability in `[0, 1]`.
    pub probability: f64,
}

/// A boundary condition whose state is fixed at the start of analysis.
///
/// House events are typically used to enable or disable parts of the fault
/// tree (e.g. for plant-state analysis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HouseEvent {
    /// Unique identifier for this house event.
    pub name: String,
    /// `true` means the condition is assumed to hold (event has occurred).
    pub state: bool,
}

/// A complete Fault Tree model ready for analysis or serialization.
///
/// The tree is stored as flat look-up maps rather than a recursive structure;
/// the hierarchy is implied by the `inputs` lists in each [`Gate`].
#[derive(Debug, Clone, PartialEq)]
pub struct FaultTree {
    /// Name of the fault tree (used as the XML `name` attribute).
    pub name: String,
    /// Name of the root (top-level) gate.
    pub top_gate: String,
    /// All gates in the tree, keyed by gate name.
    pub gates: BTreeMap<String, Gate>,
    /// All basic events, keyed by event name.
    pub basic_events: BTreeMap<String, BasicEvent>,
    /// All house events, keyed by event name.
    pub house_events: BTreeMap<String, HouseEvent>,
}

// ─── Error type ──────────────────────────────────────────────────────────────

/// Errors that can arise while mapping a PDAG to a [`FaultTree`] or while
/// serializing to XML.
#[derive(Debug, Error)]
pub enum FaultTreeError {
    /// The PDAG contained no gate nodes (e.g. it was empty or had only leaves).
    #[error("invalid PDAG: no gates were generated")]
    NoGates,

    /// A basic-event node in the PDAG was missing its probability value.
    #[error("invalid PDAG: basic event node `{0}` missing probability")]
    MissingProbability(String),

    /// The XML writer returned an error.
    #[error("XML write error: {0}")]
    Xml(String),

    /// The byte buffer produced by the XML writer was not valid UTF-8.
    #[error("UTF-8 conversion error: {0}")]
    Utf8(String),
}

// ─── PDAG → FaultTree mapper ─────────────────────────────────────────────────

/// Maps a [`Pdag`] into a typed [`FaultTree`].
///
/// The mapping is a single pass over every node in the graph:
/// - `NodeKind::Root` and `NodeKind::Gate` become [`Gate`] entries.
/// - `NodeKind::BasicEvent` becomes a [`BasicEvent`] entry.
pub struct FaultTreeMapper;

impl FaultTreeMapper {
    /// Converts a [`Pdag`] to a [`FaultTree`].
    ///
    /// # Arguments
    ///
    /// * `model_name` — base name used to derive the fault-tree's XML name
    ///   (result will be `"{model_name}-fault-tree"`).
    /// * `pdag` — the directed acyclic graph to map.
    ///
    /// # Errors
    ///
    /// Returns [`FaultTreeError::NoGates`] if the PDAG has no gate nodes,
    /// or [`FaultTreeError::MissingProbability`] if a basic-event node has no
    /// probability value set.
    pub fn from_pdag(model_name: &str, pdag: &Pdag) -> Result<FaultTree, FaultTreeError> {
        let mut gates: BTreeMap<String, Gate> = BTreeMap::new();
        let mut basic_events: BTreeMap<String, BasicEvent> = BTreeMap::new();

        // Walk every node in the PDAG and classify it.
        for node_idx in pdag.graph.node_indices() {
            let node = &pdag.graph[node_idx];
            match &node.kind {
                // The root node becomes an OR gate over its direct children.
                NodeKind::Root => {
                    let mut inputs = pdag
                        .children(node_idx)
                        .map(|child| pdag.graph[child].name.clone())
                        .collect::<Vec<_>>();
                    // Sort for deterministic XML output.
                    inputs.sort();
                    gates.insert(
                        node.name.clone(),
                        Gate {
                            name: node.name.clone(),
                            gate_type: GateType::Or,
                            inputs,
                        },
                    );
                }

                // Intermediate gates are mapped 1:1 to their PDAG gate type.
                NodeKind::Gate(gate_type) => {
                    let mut inputs = pdag
                        .children(node_idx)
                        .map(|child| pdag.graph[child].name.clone())
                        .collect::<Vec<_>>();
                    inputs.sort();
                    let mapped_type = match gate_type {
                        PdagGateType::And => GateType::And,
                        PdagGateType::Or => GateType::Or,
                        PdagGateType::KofN(k) => GateType::KofN(*k),
                    };
                    gates.insert(
                        node.name.clone(),
                        Gate {
                            name: node.name.clone(),
                            gate_type: mapped_type,
                            inputs,
                        },
                    );
                }

                // Leaf nodes become basic events with their sampled probability.
                NodeKind::BasicEvent => {
                    let probability = node
                        .probability
                        .ok_or_else(|| FaultTreeError::MissingProbability(node.name.clone()))?;
                    basic_events.insert(
                        node.name.clone(),
                        BasicEvent {
                            name: node.name.clone(),
                            probability,
                        },
                    );
                }
            }
        }

        if gates.is_empty() {
            return Err(FaultTreeError::NoGates);
        }

        Ok(FaultTree {
            name: format!("{}-fault-tree", model_name),
            // The top gate is always the root node of the PDAG.
            top_gate: pdag.graph[pdag.root].name.clone(),
            gates,
            basic_events,
            // House events are not generated from the PDAG; they can be added
            // programmatically after construction.
            house_events: BTreeMap::new(),
        })
    }
}

// ─── XML serializer ──────────────────────────────────────────────────────────

/// Serializes a [`FaultTree`] (and an optional [`EventTree`]) to an
/// Open-PSA MEF v2.0 XML string.
///
/// The top-level XML element is `<opsa-mef version="2.0">`.  Inside it the
/// serializer emits:
/// - A `<define-fault-tree>` block for the fault tree.
/// - Optionally, a `<define-event-tree>` block when an [`EventTree`] is
///   provided.
pub struct XmlSerializer;

impl XmlSerializer {
    /// Serializes a [`FaultTree`] alone (no event tree).
    ///
    /// Convenience wrapper around [`serialize_model`].
    pub fn serialize_fault_tree(fault_tree: &FaultTree) -> Result<String, FaultTreeError> {
        Self::serialize_model(fault_tree, None)
    }

    /// Serializes a [`FaultTree`] and an optional [`EventTree`] into a single
    /// Open-PSA MEF XML document.
    ///
    /// The writer uses 2-space indentation for readability.
    pub fn serialize_model(
        fault_tree: &FaultTree,
        event_tree: Option<&EventTree>,
    ) -> Result<String, FaultTreeError> {
        // Use an in-memory byte buffer as the write target.
        let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);

        // XML declaration: <?xml version="1.0" encoding="UTF-8"?>
        writer
            .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // Root element: <opsa-mef version="2.0">
        let mut root = BytesStart::new("opsa-mef");
        root.push_attribute(("version", "2.0"));
        writer
            .write_event(Event::Start(root))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // Write the fault tree block.
        Self::write_fault_tree(&mut writer, fault_tree)?;

        // Optionally write an event tree block.
        if let Some(et) = event_tree {
            Self::write_event_tree(&mut writer, et)?;
        }

        // Close root element: </opsa-mef>
        writer
            .write_event(Event::End(BytesEnd::new("opsa-mef")))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // Convert the byte buffer to a UTF-8 string.
        let bytes = writer.into_inner().into_inner();
        String::from_utf8(bytes).map_err(|e| FaultTreeError::Utf8(e.to_string()))
    }

    /// Writes the `<define-fault-tree>` block for `fault_tree` into `writer`.
    ///
    /// Structure emitted:
    /// ```xml
    /// <define-fault-tree name="…">
    ///   <top-gate name="…"/>
    ///   <define-gate name="…" type="and|or|kofn" [k="…"]>
    ///     <input ref="…"/>
    ///     …
    ///   </define-gate>
    ///   …
    ///   <define-basic-event name="…">
    ///     <float value="…"/>
    ///   </define-basic-event>
    ///   …
    ///   <define-house-event name="…" state="true|false"/>
    ///   …
    /// </define-fault-tree>
    /// ```
    fn write_fault_tree(
        writer: &mut Writer<Cursor<Vec<u8>>>,
        fault_tree: &FaultTree,
    ) -> Result<(), FaultTreeError> {
        // <define-fault-tree name="…">
        let mut def_ft = BytesStart::new("define-fault-tree");
        def_ft.push_attribute(("name", fault_tree.name.as_str()));
        writer
            .write_event(Event::Start(def_ft))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // <top-gate name="…"/>  — declares the entry point of the tree
        let mut top_gate = BytesStart::new("top-gate");
        top_gate.push_attribute(("name", fault_tree.top_gate.as_str()));
        writer
            .write_event(Event::Empty(top_gate))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // Emit each gate definition.  BTreeMap guarantees alphabetical order,
        // making the output deterministic across runs.
        for gate in fault_tree.gates.values() {
            let mut gate_tag = BytesStart::new("define-gate");
            gate_tag.push_attribute(("name", gate.name.as_str()));
            match gate.gate_type {
                GateType::And => gate_tag.push_attribute(("type", "and")),
                GateType::Or => gate_tag.push_attribute(("type", "or")),
                GateType::KofN(k) => {
                    gate_tag.push_attribute(("type", "kofn"));
                    // `k` must be stored in a local binding so the &str borrow
                    // lives long enough to be passed to push_attribute.
                    let k_string = k.to_string();
                    gate_tag.push_attribute(("k", k_string.as_str()));
                }
            }
            writer
                .write_event(Event::Start(gate_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

            // Emit one <input ref="…"/> element for each child name.
            for input in &gate.inputs {
                let mut input_tag = BytesStart::new("input");
                input_tag.push_attribute(("ref", input.as_str()));
                writer
                    .write_event(Event::Empty(input_tag))
                    .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
            }

            writer
                .write_event(Event::End(BytesEnd::new("define-gate")))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        }

        // Emit each basic-event definition with its failure probability.
        for be in fault_tree.basic_events.values() {
            let mut be_tag = BytesStart::new("define-basic-event");
            be_tag.push_attribute(("name", be.name.as_str()));
            writer
                .write_event(Event::Start(be_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

            // Probability encoded as a 12-decimal-place float literal.
            let prob_value = format!("{:.12}", be.probability);
            let mut float_tag = BytesStart::new("float");
            float_tag.push_attribute(("value", prob_value.as_str()));
            writer
                .write_event(Event::Empty(float_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

            writer
                .write_event(Event::End(BytesEnd::new("define-basic-event")))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        }

        // Emit any house events (fixed boundary conditions).
        for house in fault_tree.house_events.values() {
            let mut house_tag = BytesStart::new("define-house-event");
            house_tag.push_attribute(("name", house.name.as_str()));
            house_tag.push_attribute(("state", if house.state { "true" } else { "false" }));
            writer
                .write_event(Event::Empty(house_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        }

        // </define-fault-tree>
        writer
            .write_event(Event::End(BytesEnd::new("define-fault-tree")))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        Ok(())
    }

    /// Writes a `<define-event-tree>` block into `writer`.
    ///
    /// Structure emitted:
    /// ```xml
    /// <define-event-tree name="…">
    ///   <initiating-event name="…" top-gate-ref="…"/>
    ///   <branch-set name="…">
    ///     <functional-event name="…" fault-tree-ref="…"/>
    ///     …
    ///     <sequence id="…">
    ///       <outcome functional-event-ref="…" state="success|failure"/>
    ///       …
    ///     </sequence>
    ///     …
    ///   </branch-set>
    /// </define-event-tree>
    /// ```
    fn write_event_tree(
        writer: &mut Writer<Cursor<Vec<u8>>>,
        event_tree: &EventTree,
    ) -> Result<(), FaultTreeError> {
        // <define-event-tree name="…">
        let mut def_et = BytesStart::new("define-event-tree");
        def_et.push_attribute(("name", event_tree.name.as_str()));
        writer
            .write_event(Event::Start(def_et))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // <initiating-event …/> — the top-level trigger for the event sequence.
        let mut init = BytesStart::new("initiating-event");
        init.push_attribute(("name", event_tree.initiating_event.name.as_str()));
        init.push_attribute((
            "top-gate-ref",
            event_tree.initiating_event.top_gate_ref.as_str(),
        ));
        writer
            .write_event(Event::Empty(init))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // <branch-set> — groups all functional events and their sequences.
        let mut branch_set = BytesStart::new("branch-set");
        branch_set.push_attribute(("name", event_tree.branch_set.name.as_str()));
        writer
            .write_event(Event::Start(branch_set))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // Emit one <functional-event> element per mitigation system.
        for fe in &event_tree.functional_events {
            let mut fe_tag = BytesStart::new("functional-event");
            fe_tag.push_attribute(("name", fe.name.as_str()));
            fe_tag.push_attribute(("fault-tree-ref", fe.fault_tree_ref.as_str()));
            writer
                .write_event(Event::Empty(fe_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        }

        // Emit one <sequence> per outcome combination (2^N in total).
        for seq in &event_tree.sequences {
            let mut seq_tag = BytesStart::new("sequence");
            seq_tag.push_attribute(("id", seq.id.as_str()));
            writer
                .write_event(Event::Start(seq_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

            // Each outcome records whether the corresponding functional event
            // succeeded (`true`) or failed (`false`) in this sequence.
            for (idx, outcome) in seq.outcomes.iter().enumerate() {
                let mut outcome_tag = BytesStart::new("outcome");
                outcome_tag.push_attribute((
                    "functional-event-ref",
                    event_tree.functional_events[idx].name.as_str(),
                ));
                outcome_tag.push_attribute(("state", if *outcome { "success" } else { "failure" }));
                writer
                    .write_event(Event::Empty(outcome_tag))
                    .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
            }

            writer
                .write_event(Event::End(BytesEnd::new("sequence")))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        }

        writer
            .write_event(Event::End(BytesEnd::new("branch-set")))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        // </define-event-tree>
        writer
            .write_event(Event::End(BytesEnd::new("define-event-tree")))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        Ok(())
    }
}

// ─── FaultTree helpers ────────────────────────────────────────────────────────

impl FaultTree {
    /// Returns the union of all gate names, basic-event names, and house-event
    /// names defined in this fault tree.
    ///
    /// Used by [`validator::ReferenceIntegrityValidator`] to check that every
    /// name referenced in an [`EventTree`] is actually defined.
    pub fn all_references(&self) -> BTreeSet<String> {
        self.gates
            .keys()
            .chain(self.basic_events.keys())
            .chain(self.house_events.keys())
            .cloned()
            .collect()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use config::PdagConfig;
    use pdag::PdagBuilder;
    use quick_xml::events::Event;
    use quick_xml::Reader;

    use super::*;

    /// Build a [`FaultTree`] from the default PDAG config for reuse in tests.
    fn sample_fault_tree() -> FaultTree {
        let pdag = PdagBuilder::new(PdagConfig::default())
            .unwrap()
            .build()
            .unwrap();
        FaultTreeMapper::from_pdag("model", &pdag).unwrap()
    }

    // ── Mapping consistency ──────────────────────────────────────────────────

    #[test]
    fn round_trip_node_count_consistency() {
        // The number of gates and basic events in the FaultTree must equal
        // the numbers reported by the Pdag iterators.
        let pdag = PdagBuilder::new(PdagConfig::default())
            .unwrap()
            .build()
            .unwrap();
        let ft = FaultTreeMapper::from_pdag("model", &pdag).unwrap();
        assert_eq!(ft.basic_events.len(), pdag.basic_events().count());
        assert_eq!(ft.gates.len(), pdag.gates().count());
    }

    // ── XML well-formedness ──────────────────────────────────────────────────

    #[test]
    fn xml_is_well_formed() {
        // Parse the generated XML and verify that the root <opsa-mef> element
        // is present.  Any parse error is a test failure.
        let ft = sample_fault_tree();
        let xml = XmlSerializer::serialize_fault_tree(&ft).unwrap();

        let mut reader = Reader::from_str(&xml);
        reader.config_mut().trim_text(true);
        let mut saw_root = false;

        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) if e.name().as_ref() == b"opsa-mef" => saw_root = true,
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("xml parse failed: {e}"),
            }
        }

        assert!(saw_root);
    }

    // ── Determinism ──────────────────────────────────────────────────────────

    #[test]
    fn xml_output_is_deterministic() {
        // Serializing the same FaultTree twice must produce byte-for-byte
        // identical output.  This relies on BTreeMap iteration order.
        let ft = sample_fault_tree();
        let a = XmlSerializer::serialize_fault_tree(&ft).unwrap();
        let b = XmlSerializer::serialize_fault_tree(&ft).unwrap();
        assert_eq!(a, b);
    }
}
