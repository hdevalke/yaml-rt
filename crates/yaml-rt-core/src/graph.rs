use crate::{
    CollectionStyle, Diagnostic, DiagnosticKind, NodeId, Span, YamlError, YamlEvent, YamlEventKind,
    YamlScalarStyle,
};

/// Identifier for a semantic graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GraphNodeId(pub u32);

impl GraphNodeId {
    /// Creates a graph node ID from a vector index.
    ///
    /// # Panics
    ///
    /// Panics when `index` cannot fit in the u32-backed graph node ID.
    #[must_use]
    pub fn from_usize(index: usize) -> Self {
        Self(u32::try_from(index).expect("graph arena is too large for u32-based node IDs"))
    }

    /// Returns this graph node ID as a vector index.
    #[must_use]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Semantic graph node built from parser events and linked back to the CST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    /// Semantic node classification.
    pub kind: GraphKind,
    /// Source span associated with the semantic node.
    pub span: Span,
    /// Best matching CST node, when the semantic node has one.
    pub cst: Option<NodeId>,
}

/// Semantic graph node kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphKind {
    /// YAML document node.
    Document {
        /// Document content nodes.
        children: Vec<GraphNodeId>,
    },
    /// YAML mapping node.
    Mapping {
        /// Mapping spelling style.
        style: CollectionStyle,
        /// Placeholder for schema-resolved tags.
        tag: Option<String>,
        /// Placeholder for anchors.
        anchor: Option<String>,
        /// Key/value node pairs in source order.
        entries: Vec<(GraphNodeId, GraphNodeId)>,
    },
    /// YAML sequence node.
    Sequence {
        /// Sequence spelling style.
        style: CollectionStyle,
        /// Placeholder for schema-resolved tags.
        tag: Option<String>,
        /// Placeholder for anchors.
        anchor: Option<String>,
        /// Item nodes in source order.
        items: Vec<GraphNodeId>,
    },
    /// YAML scalar node.
    Scalar {
        /// Scalar spelling style.
        style: YamlScalarStyle,
        /// Decoded scalar value.
        value: String,
        /// Placeholder for schema-resolved tags.
        tag: Option<String>,
        /// Placeholder for anchors.
        anchor: Option<String>,
    },
    /// YAML alias node.
    Alias {
        /// Alias name without the leading `*`.
        name: String,
    },
}

/// CST-linked semantic graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGraph {
    /// Graph nodes in stable insertion order.
    pub nodes: Vec<GraphNode>,
    /// Root document node.
    pub root: Option<GraphNodeId>,
    /// All document nodes in stream order.
    pub documents: Vec<GraphNodeId>,
}

pub(crate) fn compose_graph(events: &[YamlEvent]) -> Result<SemanticGraph, YamlError> {
    GraphComposer::new(events).compose()
}

struct GraphComposer<'events> {
    events: &'events [YamlEvent],
    graph_nodes: Vec<GraphNode>,
    stack: Vec<OpenGraphNode>,
    root: Option<GraphNodeId>,
    documents: Vec<GraphNodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenGraphNode {
    id: GraphNodeId,
    kind: OpenGraphKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenGraphKind {
    Document,
    Mapping,
    Sequence,
}

impl<'events> GraphComposer<'events> {
    fn new(events: &'events [YamlEvent]) -> Self {
        Self {
            events,
            graph_nodes: Vec::with_capacity(events.len()),
            stack: Vec::with_capacity(8),
            root: None,
            documents: Vec::with_capacity(document_event_count(events)),
        }
    }

    fn compose(mut self) -> Result<SemanticGraph, YamlError> {
        for event in self.events {
            self.handle_event(event)?;
        }

        if !self.stack.is_empty() {
            return Err(graph_error("unclosed graph node", Span::empty(0)));
        }
        for node in &self.graph_nodes {
            if let GraphKind::Mapping { entries, .. } = &node.kind
                && entries
                    .iter()
                    .any(|(_, value)| *value == GraphNodeId(u32::MAX))
            {
                return Err(graph_error(
                    "mapping entry does not contain a value",
                    node.span,
                ));
            }
        }

        Ok(SemanticGraph {
            nodes: self.graph_nodes,
            root: self.root,
            documents: self.documents,
        })
    }

    fn handle_event(&mut self, event: &YamlEvent) -> Result<(), YamlError> {
        match &event.kind {
            YamlEventKind::StreamStart | YamlEventKind::StreamEnd => Ok(()),
            YamlEventKind::DocumentStart { .. } => {
                self.open_document(event.cst, event.span);
                Ok(())
            }
            YamlEventKind::DocumentEnd { .. } => {
                self.close_expected(OpenGraphKind::Document, event.span)?;
                Ok(())
            }
            YamlEventKind::MappingStart { style, tag, anchor } => {
                self.open_mapping(event.cst, *style, tag.clone(), anchor.clone(), event.span);
                Ok(())
            }
            YamlEventKind::MappingEnd => self.close_and_attach(OpenGraphKind::Mapping, event.span),
            YamlEventKind::SequenceStart { style, tag, anchor } => {
                self.open_sequence(event.cst, *style, tag.clone(), anchor.clone(), event.span);
                Ok(())
            }
            YamlEventKind::SequenceEnd => {
                self.close_and_attach(OpenGraphKind::Sequence, event.span)
            }
            YamlEventKind::Scalar {
                style,
                value,
                tag,
                anchor,
            } => self.attach_scalar(
                *style,
                value.clone(),
                tag.clone(),
                anchor.clone(),
                event.cst,
                event.span,
            ),
            YamlEventKind::Alias { name } => self.attach_alias(name.clone(), event.cst, event.span),
        }
    }

    fn open_document(&mut self, cst: Option<NodeId>, span: Span) {
        let id = self.push_node(GraphNode {
            kind: GraphKind::Document {
                children: Vec::with_capacity(1),
            },
            span,
            cst,
        });
        if self.root.is_none() {
            self.root = Some(id);
        }
        self.documents.push(id);
        self.stack.push(OpenGraphNode {
            id,
            kind: OpenGraphKind::Document,
        });
    }

    fn open_mapping(
        &mut self,
        cst: Option<NodeId>,
        style: CollectionStyle,
        tag: Option<String>,
        anchor: Option<String>,
        span: Span,
    ) {
        let id = self.push_node(GraphNode {
            kind: GraphKind::Mapping {
                style,
                tag,
                anchor,
                entries: Vec::with_capacity(2),
            },
            span,
            cst,
        });
        self.stack.push(OpenGraphNode {
            id,
            kind: OpenGraphKind::Mapping,
        });
    }

    fn open_sequence(
        &mut self,
        cst: Option<NodeId>,
        style: CollectionStyle,
        tag: Option<String>,
        anchor: Option<String>,
        span: Span,
    ) {
        let id = self.push_node(GraphNode {
            kind: GraphKind::Sequence {
                style,
                tag,
                anchor,
                items: Vec::with_capacity(2),
            },
            span,
            cst,
        });
        self.stack.push(OpenGraphNode {
            id,
            kind: OpenGraphKind::Sequence,
        });
    }

    fn close_and_attach(&mut self, expected: OpenGraphKind, span: Span) -> Result<(), YamlError> {
        let id = self.close_expected(expected, span)?;
        self.attach_node(id, span)
    }

    fn attach_scalar(
        &mut self,
        style: YamlScalarStyle,
        value: String,
        tag: Option<String>,
        anchor: Option<String>,
        cst: Option<NodeId>,
        span: Span,
    ) -> Result<(), YamlError> {
        let id = self.push_node(GraphNode {
            kind: GraphKind::Scalar {
                style,
                value,
                tag,
                anchor,
            },
            span,
            cst,
        });
        self.attach_node(id, span)
    }

    fn attach_alias(
        &mut self,
        name: String,
        cst: Option<NodeId>,
        span: Span,
    ) -> Result<(), YamlError> {
        let id = self.push_node(GraphNode {
            kind: GraphKind::Alias { name },
            span,
            cst,
        });
        self.attach_node(id, span)
    }

    fn push_node(&mut self, node: GraphNode) -> GraphNodeId {
        let id = GraphNodeId::from_usize(self.graph_nodes.len());
        self.graph_nodes.push(node);
        id
    }

    fn close_expected(
        &mut self,
        expected: OpenGraphKind,
        span: Span,
    ) -> Result<GraphNodeId, YamlError> {
        let Some(open) = self.stack.pop() else {
            return Err(graph_error("unexpected closing event", span));
        };
        if open.kind != expected {
            return Err(graph_error("mismatched closing event", span));
        }
        Ok(open.id)
    }

    fn attach_node(&mut self, child: GraphNodeId, span: Span) -> Result<(), YamlError> {
        let Some(parent) = self.stack.last().copied() else {
            return Ok(());
        };
        match &mut self.graph_nodes[parent.id.as_usize()].kind {
            GraphKind::Document { children } => {
                children.push(child);
                Ok(())
            }
            GraphKind::Sequence { items, .. } => {
                items.push(child);
                Ok(())
            }
            GraphKind::Mapping { entries, .. } => {
                if entries
                    .last()
                    .is_none_or(|(_, value)| *value != GraphNodeId(u32::MAX))
                {
                    entries.push((child, GraphNodeId(u32::MAX)));
                } else if let Some((_, value)) = entries.last_mut() {
                    *value = child;
                }
                Ok(())
            }
            _ => Err(graph_error("scalar nodes cannot contain children", span)),
        }
    }
}

fn document_event_count(events: &[YamlEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event.kind, YamlEventKind::DocumentStart { .. }))
        .count()
        .max(1)
}

fn graph_error(message: impl Into<String>, span: Span) -> YamlError {
    YamlError::new(Diagnostic::new(DiagnosticKind::Semantic, message, span))
}
