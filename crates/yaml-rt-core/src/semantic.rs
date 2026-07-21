use crate::{
    CollectionStyle, Diagnostic, DiagnosticKind, Node, NodeId, Span, YamlError, YamlEvent,
    YamlEventKind, YamlScalarStyle,
};

const NO_SEMANTIC_NODE: u32 = u32::MAX;

/// Semantic interpretation attached to a lossless CST node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticKind {
    /// YAML document.
    Document,
    /// YAML mapping with its presentation style and node properties.
    Mapping {
        /// Block or flow spelling.
        style: CollectionStyle,
        /// Resolved explicit tag.
        tag: Option<String>,
        /// Anchor name.
        anchor: Option<String>,
    },
    /// YAML sequence with its presentation style and node properties.
    Sequence {
        /// Block or flow spelling.
        style: CollectionStyle,
        /// Resolved explicit tag.
        tag: Option<String>,
        /// Anchor name.
        anchor: Option<String>,
    },
    /// Scalar with decoded text and node properties.
    Scalar {
        /// Scalar spelling style.
        style: YamlScalarStyle,
        /// Decoded scalar text.
        value: String,
        /// Resolved explicit tag.
        tag: Option<String>,
        /// Anchor name.
        anchor: Option<String>,
    },
    /// Alias reference.
    Alias {
        /// Referenced anchor name.
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticNode {
    pub(crate) kind: SemanticKind,
    pub(crate) span: Span,
}

/// Compact semantic side arena indexed through CST `NodeId`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticStore {
    slots: Vec<u32>,
    nodes: Vec<SemanticNode>,
    pub(crate) documents: Vec<NodeId>,
}

impl SemanticStore {
    pub(crate) fn from_events(nodes: &[Node], events: &[YamlEvent]) -> Result<Self, YamlError> {
        validate_event_structure(events)?;
        let mut store = Self {
            slots: vec![NO_SEMANTIC_NODE; nodes.len()],
            nodes: Vec::with_capacity(events.len().saturating_sub(2)),
            documents: Vec::new(),
        };

        for event in events {
            let Some(cst) = event.cst else {
                continue;
            };
            let kind = match &event.kind {
                YamlEventKind::DocumentStart { .. } => {
                    store.documents.push(cst);
                    SemanticKind::Document
                }
                YamlEventKind::MappingStart { style, tag, anchor } => SemanticKind::Mapping {
                    style: *style,
                    tag: tag.clone(),
                    anchor: anchor.clone(),
                },
                YamlEventKind::SequenceStart { style, tag, anchor } => SemanticKind::Sequence {
                    style: *style,
                    tag: tag.clone(),
                    anchor: anchor.clone(),
                },
                YamlEventKind::Scalar {
                    style,
                    value,
                    tag,
                    anchor,
                } => SemanticKind::Scalar {
                    style: *style,
                    value: value.clone(),
                    tag: tag.clone(),
                    anchor: anchor.clone(),
                },
                YamlEventKind::Alias { name } => SemanticKind::Alias { name: name.clone() },
                YamlEventKind::StreamStart
                | YamlEventKind::StreamEnd
                | YamlEventKind::DocumentEnd { .. }
                | YamlEventKind::MappingEnd
                | YamlEventKind::SequenceEnd => continue,
            };
            store.insert(
                cst,
                SemanticNode {
                    kind,
                    span: event.span,
                },
            );
        }
        Ok(store)
    }

    fn insert(&mut self, cst: NodeId, node: SemanticNode) {
        let index = u32::try_from(self.nodes.len()).expect("semantic arena exceeds u32 capacity");
        self.slots[cst.as_usize()] = index;
        self.nodes.push(node);
    }

    pub(crate) fn get(&self, cst: NodeId) -> Option<&SemanticNode> {
        let index = *self.slots.get(cst.as_usize())?;
        (index != NO_SEMANTIC_NODE).then(|| &self.nodes[index as usize])
    }
}

#[derive(Clone, Copy)]
enum OpenNode {
    Document { children: usize },
    Mapping { waiting_for_value: bool },
    Sequence,
}

fn validate_event_structure(events: &[YamlEvent]) -> Result<(), YamlError> {
    let mut stack = Vec::with_capacity(8);
    for event in events {
        match &event.kind {
            YamlEventKind::StreamStart | YamlEventKind::StreamEnd => {}
            YamlEventKind::DocumentStart { .. } => {
                stack.push(OpenNode::Document { children: 0 });
            }
            YamlEventKind::MappingStart { .. } => {
                stack.push(OpenNode::Mapping {
                    waiting_for_value: false,
                });
            }
            YamlEventKind::SequenceStart { .. } => stack.push(OpenNode::Sequence),
            YamlEventKind::Scalar { .. } | YamlEventKind::Alias { .. } => {
                attach_semantic_child(&mut stack, event.span)?;
            }
            YamlEventKind::MappingEnd => {
                let Some(OpenNode::Mapping { waiting_for_value }) = stack.pop() else {
                    return Err(structure_error("mismatched mapping end event", event.span));
                };
                if waiting_for_value {
                    return Err(structure_error(
                        "mapping entry does not contain a value",
                        event.span,
                    ));
                }
                attach_semantic_child(&mut stack, event.span)?;
            }
            YamlEventKind::SequenceEnd => {
                if !matches!(stack.pop(), Some(OpenNode::Sequence)) {
                    return Err(structure_error("mismatched sequence end event", event.span));
                }
                attach_semantic_child(&mut stack, event.span)?;
            }
            YamlEventKind::DocumentEnd { .. } => {
                if !matches!(stack.pop(), Some(OpenNode::Document { .. })) {
                    return Err(structure_error("mismatched document end event", event.span));
                }
            }
        }
    }
    if stack.is_empty() {
        Ok(())
    } else {
        Err(structure_error("unclosed semantic node", Span::empty(0)))
    }
}

fn attach_semantic_child(stack: &mut [OpenNode], span: Span) -> Result<(), YamlError> {
    let Some(parent) = stack.last_mut() else {
        return Ok(());
    };
    match parent {
        OpenNode::Document { children } => {
            *children += 1;
            if *children > 1 {
                return Err(structure_error(
                    "document contains multiple root nodes",
                    span,
                ));
            }
        }
        OpenNode::Mapping { waiting_for_value } => {
            *waiting_for_value = !*waiting_for_value;
        }
        OpenNode::Sequence => {}
    }
    Ok(())
}

fn structure_error(message: &str, span: Span) -> YamlError {
    YamlError::new(Diagnostic::new(DiagnosticKind::Semantic, message, span))
}
