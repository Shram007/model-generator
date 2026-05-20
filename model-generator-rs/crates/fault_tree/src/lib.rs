//! Fault Tree model, PDAG mapper, and Open-PSA MEF serialization.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use event_tree::EventTree;
use pdag::{GateType as PdagGateType, NodeKind, Pdag};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::Writer;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateType {
    And,
    Or,
    KofN(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub name: String,
    pub gate_type: GateType,
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BasicEvent {
    pub name: String,
    pub probability: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HouseEvent {
    pub name: String,
    pub state: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FaultTree {
    pub name: String,
    pub top_gate: String,
    pub gates: BTreeMap<String, Gate>,
    pub basic_events: BTreeMap<String, BasicEvent>,
    pub house_events: BTreeMap<String, HouseEvent>,
}

#[derive(Debug, Error)]
pub enum FaultTreeError {
    #[error("invalid PDAG: no gates were generated")]
    NoGates,
    #[error("invalid PDAG: basic event node `{0}` missing probability")]
    MissingProbability(String),
    #[error("XML write error: {0}")]
    Xml(String),
    #[error("UTF-8 conversion error: {0}")]
    Utf8(String),
}

pub struct FaultTreeMapper;

impl FaultTreeMapper {
    pub fn from_pdag(model_name: &str, pdag: &Pdag) -> Result<FaultTree, FaultTreeError> {
        let mut gates: BTreeMap<String, Gate> = BTreeMap::new();
        let mut basic_events: BTreeMap<String, BasicEvent> = BTreeMap::new();

        for node_idx in pdag.graph.node_indices() {
            let node = &pdag.graph[node_idx];
            match &node.kind {
                NodeKind::Root => {
                    let mut inputs = pdag
                        .children(node_idx)
                        .map(|child| pdag.graph[child].name.clone())
                        .collect::<Vec<_>>();
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
            top_gate: pdag.graph[pdag.root].name.clone(),
            gates,
            basic_events,
            house_events: BTreeMap::new(),
        })
    }
}

pub struct XmlSerializer;

impl XmlSerializer {
    pub fn serialize_fault_tree(fault_tree: &FaultTree) -> Result<String, FaultTreeError> {
        Self::serialize_model(fault_tree, None)
    }

    pub fn serialize_model(
        fault_tree: &FaultTree,
        event_tree: Option<&EventTree>,
    ) -> Result<String, FaultTreeError> {
        let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);

        writer
            .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        let mut root = BytesStart::new("opsa-mef");
        root.push_attribute(("version", "2.0"));
        writer
            .write_event(Event::Start(root))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        Self::write_fault_tree(&mut writer, fault_tree)?;
        if let Some(et) = event_tree {
            Self::write_event_tree(&mut writer, et)?;
        }

        writer
            .write_event(Event::End(BytesEnd::new("opsa-mef")))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        let bytes = writer.into_inner().into_inner();
        String::from_utf8(bytes).map_err(|e| FaultTreeError::Utf8(e.to_string()))
    }

    fn write_fault_tree(
        writer: &mut Writer<Cursor<Vec<u8>>>,
        fault_tree: &FaultTree,
    ) -> Result<(), FaultTreeError> {
        let mut def_ft = BytesStart::new("define-fault-tree");
        def_ft.push_attribute(("name", fault_tree.name.as_str()));
        writer
            .write_event(Event::Start(def_ft))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        let mut top_gate = BytesStart::new("top-gate");
        top_gate.push_attribute(("name", fault_tree.top_gate.as_str()));
        writer
            .write_event(Event::Empty(top_gate))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        for gate in fault_tree.gates.values() {
            let mut gate_tag = BytesStart::new("define-gate");
            gate_tag.push_attribute(("name", gate.name.as_str()));
            match gate.gate_type {
                GateType::And => gate_tag.push_attribute(("type", "and")),
                GateType::Or => gate_tag.push_attribute(("type", "or")),
                GateType::KofN(k) => {
                    gate_tag.push_attribute(("type", "kofn"));
                    let k_string = k.to_string();
                    gate_tag.push_attribute(("k", k_string.as_str()));
                }
            }
            writer
                .write_event(Event::Start(gate_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

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

        for be in fault_tree.basic_events.values() {
            let mut be_tag = BytesStart::new("define-basic-event");
            be_tag.push_attribute(("name", be.name.as_str()));
            writer
                .write_event(Event::Start(be_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

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

        for house in fault_tree.house_events.values() {
            let mut house_tag = BytesStart::new("define-house-event");
            house_tag.push_attribute(("name", house.name.as_str()));
            house_tag.push_attribute(("state", if house.state { "true" } else { "false" }));
            writer
                .write_event(Event::Empty(house_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        }

        writer
            .write_event(Event::End(BytesEnd::new("define-fault-tree")))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        Ok(())
    }

    fn write_event_tree(
        writer: &mut Writer<Cursor<Vec<u8>>>,
        event_tree: &EventTree,
    ) -> Result<(), FaultTreeError> {
        let mut def_et = BytesStart::new("define-event-tree");
        def_et.push_attribute(("name", event_tree.name.as_str()));
        writer
            .write_event(Event::Start(def_et))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        let mut init = BytesStart::new("initiating-event");
        init.push_attribute(("name", event_tree.initiating_event.name.as_str()));
        init.push_attribute((
            "top-gate-ref",
            event_tree.initiating_event.top_gate_ref.as_str(),
        ));
        writer
            .write_event(Event::Empty(init))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        let mut branch_set = BytesStart::new("branch-set");
        branch_set.push_attribute(("name", event_tree.branch_set.name.as_str()));
        writer
            .write_event(Event::Start(branch_set))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

        for fe in &event_tree.functional_events {
            let mut fe_tag = BytesStart::new("functional-event");
            fe_tag.push_attribute(("name", fe.name.as_str()));
            fe_tag.push_attribute(("fault-tree-ref", fe.fault_tree_ref.as_str()));
            writer
                .write_event(Event::Empty(fe_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        }

        for seq in &event_tree.sequences {
            let mut seq_tag = BytesStart::new("sequence");
            seq_tag.push_attribute(("id", seq.id.as_str()));
            writer
                .write_event(Event::Start(seq_tag))
                .map_err(|e| FaultTreeError::Xml(e.to_string()))?;

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

        writer
            .write_event(Event::End(BytesEnd::new("define-event-tree")))
            .map_err(|e| FaultTreeError::Xml(e.to_string()))?;
        Ok(())
    }
}

impl FaultTree {
    pub fn all_references(&self) -> BTreeSet<String> {
        self.gates
            .keys()
            .chain(self.basic_events.keys())
            .chain(self.house_events.keys())
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use config::PdagConfig;
    use pdag::PdagBuilder;
    use quick_xml::events::Event;
    use quick_xml::Reader;

    use super::*;

    fn sample_fault_tree() -> FaultTree {
        let pdag = PdagBuilder::new(PdagConfig::default())
            .unwrap()
            .build()
            .unwrap();
        FaultTreeMapper::from_pdag("model", &pdag).unwrap()
    }

    #[test]
    fn round_trip_node_count_consistency() {
        let pdag = PdagBuilder::new(PdagConfig::default())
            .unwrap()
            .build()
            .unwrap();
        let ft = FaultTreeMapper::from_pdag("model", &pdag).unwrap();
        assert_eq!(ft.basic_events.len(), pdag.basic_events().count());
        assert_eq!(ft.gates.len(), pdag.gates().count());
    }

    #[test]
    fn xml_is_well_formed() {
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

    #[test]
    fn xml_output_is_deterministic() {
        let ft = sample_fault_tree();
        let a = XmlSerializer::serialize_fault_tree(&ft).unwrap();
        let b = XmlSerializer::serialize_fault_tree(&ft).unwrap();
        assert_eq!(a, b);
    }
}
