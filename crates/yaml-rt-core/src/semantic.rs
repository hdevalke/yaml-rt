use crate::{
    CollectionStyle, Diagnostic, DiagnosticKind, NodeId, Span, YamlError, YamlEventKind,
    YamlScalarStyle,
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
    fn insert(&mut self, cst: NodeId, node: SemanticNode) {
        if self.slots.len() <= cst.as_usize() {
            self.slots
                .resize(cst.as_usize().saturating_add(1), NO_SEMANTIC_NODE);
        }
        let index = u32::try_from(self.nodes.len()).expect("semantic arena exceeds u32 capacity");
        self.slots[cst.as_usize()] = index;
        self.nodes.push(node);
    }

    fn close(&mut self, cst: NodeId, span: Span, explicit: Option<bool>) {
        let index = self.slots[cst.as_usize()] as usize;
        self.nodes[index].end_span = span;
        if let Some(explicit) = explicit {
            self.nodes[index].explicit_end = explicit;
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

pub(crate) struct SemanticBuilder {
    store: SemanticStore,
    open: Vec<OpenNode>,
    error: Option<YamlError>,
}

impl SemanticBuilder {
    pub(crate) fn with_capacity(cst_capacity: usize, semantic_capacity: usize) -> Self {
        Self {
            store: SemanticStore {
                slots: Vec::with_capacity(cst_capacity),
                nodes: Vec::with_capacity(semantic_capacity),
                documents: Vec::with_capacity(1),
            },
            open: Vec::with_capacity(8),
            error: None,
        }
    }

    pub(crate) fn push(
        &mut self,
        kind: YamlEventKind,
        span: Span,
        cst: Option<NodeId>,
        content_indent: Option<u32>,
    ) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.try_push(kind, span, cst, content_indent) {
            self.error = Some(error);
        }
    }

    fn try_push(
        &mut self,
        kind: YamlEventKind,
        span: Span,
        cst: Option<NodeId>,
        content_indent: Option<u32>,
    ) -> Result<(), YamlError> {
        match kind {
            YamlEventKind::StreamStart | YamlEventKind::StreamEnd => Ok(()),
            YamlEventKind::DocumentStart { explicit } => {
                let cst = required_cst(cst, span)?;
                self.store.documents.push(cst);
                self.store.insert(
                    cst,
                    SemanticNode::new(SemanticKind::Document, span, explicit, content_indent),
                );
                self.open.push(OpenNode::Document { cst, children: 0 });
                Ok(())
            }
            YamlEventKind::MappingStart { style, tag, anchor } => {
                let cst = required_cst(cst, span)?;
                self.store.insert(
                    cst,
                    SemanticNode::new(
                        SemanticKind::Mapping { style, tag, anchor },
                        span,
                        false,
                        content_indent,
                    ),
                );
                self.open.push(OpenNode::Mapping {
                    cst,
                    waiting_for_value: false,
                });
                Ok(())
            }
            YamlEventKind::SequenceStart { style, tag, anchor } => {
                let cst = required_cst(cst, span)?;
                self.store.insert(
                    cst,
                    SemanticNode::new(
                        SemanticKind::Sequence { style, tag, anchor },
                        span,
                        false,
                        content_indent,
                    ),
                );
                self.open.push(OpenNode::Sequence { cst });
                Ok(())
            }
            YamlEventKind::Scalar {
                style,
                value: _,
                tag,
                anchor,
            } => {
                let cst = required_cst(cst, span)?;
                self.store.insert(
                    cst,
                    SemanticNode::new(
                        SemanticKind::Scalar { style, tag, anchor },
                        span,
                        false,
                        content_indent,
                    ),
                );
                self.attach_child(cst, span)
            }
            YamlEventKind::Alias { name } => {
                let cst = required_cst(cst, span)?;
                self.store.insert(
                    cst,
                    SemanticNode::new(SemanticKind::Alias { name }, span, false, content_indent),
                );
                self.attach_child(cst, span)
            }
            YamlEventKind::MappingEnd => {
                let Some(OpenNode::Mapping {
                    cst,
                    waiting_for_value,
                }) = self.open.pop()
                else {
                    return Err(structure_error("mismatched mapping end event", span));
                };
                if waiting_for_value {
                    return Err(structure_error(
                        "mapping entry does not contain a value",
                        span,
                    ));
                }
                self.store.close(cst, span, None);
                self.attach_child(cst, span)
            }
            YamlEventKind::SequenceEnd => {
                let Some(OpenNode::Sequence { cst }) = self.open.pop() else {
                    return Err(structure_error("mismatched sequence end event", span));
                };
                self.store.close(cst, span, None);
                self.attach_child(cst, span)
            }
            YamlEventKind::DocumentEnd { explicit } => {
                let Some(OpenNode::Document { cst, .. }) = self.open.pop() else {
                    return Err(structure_error("mismatched document end event", span));
                };
                self.store.close(cst, span, Some(explicit));
                Ok(())
            }
        }
    }

    fn attach_child(&mut self, child: NodeId, span: Span) -> Result<(), YamlError> {
        let Some(parent) = self.open.last_mut() else {
            return Ok(());
        };
        let parent_cst = match parent {
            OpenNode::Document { cst, children } => {
                *children += 1;
                if *children > 1 {
                    return Err(structure_error(
                        "document contains multiple root nodes",
                        span,
                    ));
                }
                *cst
            }
            OpenNode::Mapping {
                cst,
                waiting_for_value,
            } => {
                *waiting_for_value = !*waiting_for_value;
                *cst
            }
            OpenNode::Sequence { cst } => *cst,
        };
        self.store.attach(parent_cst, child);
        Ok(())
    }

    pub(crate) fn finish(mut self, cst_len: usize) -> Result<SemanticStore, YamlError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !self.open.is_empty() {
            return Err(structure_error("unclosed semantic node", Span::empty(0)));
        }
        self.store.slots.resize(cst_len, NO_SEMANTIC_NODE);
        Ok(self.store)
    }
}

impl SemanticNode {
    fn new(
        kind: SemanticKind,
        span: Span,
        explicit_start: bool,
        content_indent: Option<u32>,
    ) -> Self {
        Self {
            kind,
            span,
            end_span: span,
            explicit_start,
            explicit_end: false,
            content_indent,
            first_child: NO_SEMANTIC_NODE,
            last_child: NO_SEMANTIC_NODE,
            next_sibling: NO_SEMANTIC_NODE,
        }
    }
}

fn required_cst(cst: Option<NodeId>, span: Span) -> Result<NodeId, YamlError> {
    cst.ok_or_else(|| structure_error("semantic node is missing its CST origin", span))
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

#[derive(Clone, Copy)]
enum OpenNode {
    Document {
        cst: NodeId,
        children: usize,
    },
    Mapping {
        cst: NodeId,
        waiting_for_value: bool,
    },
    Sequence {
        cst: NodeId,
    },
}

fn structure_error(message: &str, span: Span) -> YamlError {
    YamlError::new(Diagnostic::new(DiagnosticKind::Semantic, message, span))
}

#[cfg(test)]
mod tests {
    use super::SemanticBuilder;
    use crate::{CollectionStyle, NodeId, Span, YamlEventKind, YamlScalarStyle};

    #[test]
    fn direct_builder_rejects_dangling_mapping_value() {
        let mut builder = SemanticBuilder::with_capacity(4, 4);
        builder.push(
            YamlEventKind::DocumentStart { explicit: false },
            Span::empty(0),
            Some(NodeId(0)),
            None,
        );
        builder.push(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Block,
                tag: None,
                anchor: None,
            },
            Span::empty(0),
            Some(NodeId(1)),
            None,
        );
        builder.push(
            YamlEventKind::Scalar {
                style: YamlScalarStyle::Plain,
                value: String::new(),
                tag: None,
                anchor: None,
            },
            Span::empty(0),
            Some(NodeId(2)),
            None,
        );
        builder.push(YamlEventKind::MappingEnd, Span::empty(0), None, None);

        let error = builder.finish(3).expect_err("mapping value is required");
        assert!(error.to_string().contains("does not contain a value"));
    }

    #[test]
    fn direct_builder_rejects_mismatched_collection_end() {
        let mut builder = SemanticBuilder::with_capacity(2, 2);
        builder.push(
            YamlEventKind::SequenceStart {
                style: CollectionStyle::Flow,
                tag: None,
                anchor: None,
            },
            Span::empty(0),
            Some(NodeId(0)),
            None,
        );
        builder.push(YamlEventKind::MappingEnd, Span::empty(1), None, None);

        let error = builder.finish(1).expect_err("collection ends must match");
        assert!(error.to_string().contains("mismatched mapping end"));
    }

    #[test]
    fn direct_builder_rejects_multiple_document_roots() {
        let mut builder = SemanticBuilder::with_capacity(3, 3);
        builder.push(
            YamlEventKind::DocumentStart { explicit: false },
            Span::empty(0),
            Some(NodeId(0)),
            None,
        );
        for cst in [NodeId(1), NodeId(2)] {
            builder.push(
                YamlEventKind::Scalar {
                    style: YamlScalarStyle::Plain,
                    value: String::new(),
                    tag: None,
                    anchor: None,
                },
                Span::empty(0),
                Some(cst),
                None,
            );
        }

        let error = builder.finish(3).expect_err("documents have one root");
        assert!(error.to_string().contains("multiple root nodes"));
    }
}
