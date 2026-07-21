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
    /// Scalar with presentation style and node properties.
    Scalar {
        /// Scalar spelling style.
        style: YamlScalarStyle,
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
    pub(crate) end_span: Span,
    pub(crate) explicit_start: bool,
    pub(crate) explicit_end: bool,
    pub(crate) content_indent: Option<u32>,
    first_child: u32,
    last_child: u32,
    next_sibling: u32,
}

/// Compact semantic side arena indexed through CST `NodeId`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticStore {
    slots: Vec<u32>,
    nodes: Vec<SemanticNode>,
    pub(crate) documents: Vec<NodeId>,
}

impl SemanticStore {
    pub(crate) fn from_events(nodes: &[Node], events: Vec<YamlEvent>) -> Result<Self, YamlError> {
        validate_event_structure(&events)?;
        let mut store = Self {
            slots: vec![NO_SEMANTIC_NODE; nodes.len()],
            nodes: Vec::with_capacity(events.len().saturating_sub(2)),
            documents: Vec::new(),
        };

        let mut open = Vec::with_capacity(8);
        for event in events {
            let Some(cst) = event.cst else {
                match event.kind {
                    YamlEventKind::DocumentEnd { explicit } => {
                        store.close(&mut open, event.span, Some(explicit));
                    }
                    YamlEventKind::MappingEnd | YamlEventKind::SequenceEnd => {
                        store.close(&mut open, event.span, None);
                    }
                    _ => {}
                }
                continue;
            };
            let (kind, explicit_start, is_open) = match event.kind {
                YamlEventKind::DocumentStart { explicit } => {
                    store.documents.push(cst);
                    (SemanticKind::Document, explicit, true)
                }
                YamlEventKind::MappingStart { style, tag, anchor } => {
                    SemanticKind::Mapping { style, tag, anchor }.open()
                }
                YamlEventKind::SequenceStart { style, tag, anchor } => {
                    SemanticKind::Sequence { style, tag, anchor }.open()
                }
                YamlEventKind::Scalar {
                    style,
                    value: _,
                    tag,
                    anchor,
                } => SemanticKind::Scalar { style, tag, anchor }.closed(),
                YamlEventKind::Alias { name } => SemanticKind::Alias { name }.closed(),
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
                    end_span: event.span,
                    explicit_start,
                    explicit_end: false,
                    content_indent: event.content_indent,
                    first_child: NO_SEMANTIC_NODE,
                    last_child: NO_SEMANTIC_NODE,
                    next_sibling: NO_SEMANTIC_NODE,
                },
            );
            if is_open {
                open.push(cst);
            } else if let Some(parent) = open.last().copied() {
                store.attach(parent, cst);
            }
        }
        Ok(store)
    }

    fn insert(&mut self, cst: NodeId, node: SemanticNode) {
        let index = u32::try_from(self.nodes.len()).expect("semantic arena exceeds u32 capacity");
        self.slots[cst.as_usize()] = index;
        self.nodes.push(node);
    }

    fn close(&mut self, open: &mut Vec<NodeId>, span: Span, explicit: Option<bool>) {
        let Some(cst) = open.pop() else {
            return;
        };
        let index = self.slots[cst.as_usize()] as usize;
        self.nodes[index].end_span = span;
        if let Some(explicit) = explicit {
            self.nodes[index].explicit_end = explicit;
        } else if let Some(parent) = open.last().copied() {
            self.attach(parent, cst);
        }
    }

    fn attach(&mut self, parent: NodeId, child: NodeId) {
        let parent_index = self.slots[parent.as_usize()] as usize;
        let child_index = self.slots[child.as_usize()] as usize;
        let previous = self.nodes[parent_index].last_child;
        if previous == NO_SEMANTIC_NODE {
            self.nodes[parent_index].first_child = child.0;
        } else {
            let previous_index = self.slots[previous as usize] as usize;
            self.nodes[previous_index].next_sibling = child.0;
        }
        self.nodes[parent_index].last_child = child.0;
        debug_assert_eq!(self.nodes[child_index].next_sibling, NO_SEMANTIC_NODE);
    }

    pub(crate) fn get(&self, cst: NodeId) -> Option<&SemanticNode> {
        let index = *self.slots.get(cst.as_usize())?;
        (index != NO_SEMANTIC_NODE).then(|| &self.nodes[index as usize])
    }

    pub(crate) fn children(&self, parent: NodeId) -> SemanticChildren<'_> {
        let next = self
            .get(parent)
            .map_or(NO_SEMANTIC_NODE, |node| node.first_child);
        SemanticChildren { store: self, next }
    }
}

pub(crate) struct SemanticChildren<'a> {
    store: &'a SemanticStore,
    next: u32,
}

impl Iterator for SemanticChildren<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == NO_SEMANTIC_NODE {
            return None;
        }
        let node = NodeId(self.next);
        self.next = self.store.get(node)?.next_sibling;
        Some(node)
    }
}

trait SemanticState {
    fn open(self) -> (SemanticKind, bool, bool);
    fn closed(self) -> (SemanticKind, bool, bool);
}

impl SemanticState for SemanticKind {
    fn open(self) -> (SemanticKind, bool, bool) {
        (self, false, true)
    }

    fn closed(self) -> (SemanticKind, bool, bool) {
        (self, false, false)
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
