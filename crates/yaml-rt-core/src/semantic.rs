use crate::{
    CollectionStyle, Diagnostic, DiagnosticKind, NodeId, Span, YamlError, YamlEventKind,
    YamlScalarStyle,
};

const NO_SEMANTIC_NODE: u32 = u32::MAX;
const NO_PROPERTIES: u32 = u32::MAX;
const EXPLICIT_START: u8 = 1 << 0;
const EXPLICIT_END: u8 = 1 << 1;

/// Semantic interpretation attached to a lossless CST node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticKind {
    /// YAML document.
    Document,
    /// YAML mapping with its presentation style.
    Mapping {
        /// Block or flow spelling.
        style: CollectionStyle,
    },
    /// YAML sequence with its presentation style.
    Sequence {
        /// Block or flow spelling.
        style: CollectionStyle,
    },
    /// Scalar with presentation style.
    Scalar {
        /// Scalar spelling style.
        style: YamlScalarStyle,
    },
    /// Alias reference.
    Alias,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SemanticNode {
    pub(crate) kind: SemanticKind,
    flags: u8,
    padding: u8,
    pub(crate) span_start: u32,
    pub(crate) end_offset: u32,
    property: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SemanticProperties {
    pub(crate) tag: Option<Span>,
    pub(crate) anchor: Option<Span>,
    pub(crate) alias: Option<Span>,
    pub(crate) content_indent: Option<u32>,
}

impl SemanticProperties {
    pub(crate) const NONE: Self = Self {
        tag: None,
        anchor: None,
        alias: None,
        content_indent: None,
    };

    fn is_empty(self) -> bool {
        self == Self::NONE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PropertyRecord {
    properties: SemanticProperties,
    document: NodeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AnchorBinding {
    name: Span,
    target: NodeId,
    document: NodeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TagDirectiveBinding {
    handle: Span,
    prefix: Span,
    document: NodeId,
}

/// Compact semantic side arena indexed through CST `NodeId`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticStore {
    slots: Vec<u32>,
    nodes: Vec<SemanticNode>,
    properties: Vec<PropertyRecord>,
    anchors: Vec<AnchorBinding>,
    tag_directives: Vec<TagDirectiveBinding>,
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
        self.nodes[index].end_offset = span.end;
        if let Some(explicit) = explicit {
            self.nodes[index].set_flag(EXPLICIT_END, explicit);
        }
    }

    pub(crate) fn get(&self, cst: NodeId) -> Option<&SemanticNode> {
        let index = *self.slots.get(cst.as_usize())?;
        (index != NO_SEMANTIC_NODE).then(|| &self.nodes[index as usize])
    }

    pub(crate) fn properties(&self, cst: NodeId) -> Option<SemanticProperties> {
        let node = self.get(cst)?;
        (node.property != NO_PROPERTIES).then(|| self.properties[node.property as usize].properties)
    }

    pub(crate) fn clear_tag(&mut self, cst: NodeId) {
        let Some(property) = self.get(cst).map(|node| node.property) else {
            return;
        };
        if property != NO_PROPERTIES {
            self.properties[property as usize].properties.tag = None;
        }
    }

    pub(crate) fn property_document(&self, cst: NodeId) -> Option<NodeId> {
        let node = self.get(cst)?;
        (node.property != NO_PROPERTIES).then(|| self.properties[node.property as usize].document)
    }

    pub(crate) fn anchors(&self) -> impl DoubleEndedIterator<Item = (Span, NodeId, NodeId)> + '_ {
        self.anchors
            .iter()
            .map(|binding| (binding.name, binding.target, binding.document))
    }

    pub(crate) fn tag_directives(
        &self,
        document: NodeId,
    ) -> impl Iterator<Item = (Span, Span)> + '_ {
        self.tag_directives
            .iter()
            .filter(move |binding| binding.document == document)
            .map(|binding| (binding.handle, binding.prefix))
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
                properties: Vec::new(),
                anchors: Vec::new(),
                tag_directives: Vec::new(),
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
        properties: SemanticProperties,
    ) {
        if self.error.is_some() {
            return;
        }
        let result = self.try_push(&kind, span, cst, properties);
        drop(kind);
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    pub(crate) fn register_cst_node(&mut self) {
        self.store.slots.push(NO_SEMANTIC_NODE);
    }

    pub(crate) fn push_property_free_plain_scalar(&mut self, cst: NodeId, span: Span) {
        if self.error.is_some() {
            return;
        }
        self.store.insert(
            cst,
            SemanticNode::new(
                SemanticKind::Scalar {
                    style: YamlScalarStyle::Plain,
                },
                span,
                false,
                NO_PROPERTIES,
            ),
        );
        if let Err(error) = self.attach_child(cst, span) {
            self.error = Some(error);
        }
    }

    pub(crate) fn push_collection_start(
        &mut self,
        cst: NodeId,
        span: Span,
        style: CollectionStyle,
        mapping: bool,
        properties: SemanticProperties,
    ) {
        if self.error.is_some() {
            return;
        }
        let property = self.insert_properties(cst, properties);
        let kind = if mapping {
            SemanticKind::Mapping { style }
        } else {
            SemanticKind::Sequence { style }
        };
        self.store
            .insert(cst, SemanticNode::new(kind, span, false, property));
        if mapping {
            self.open.push(OpenNode::Mapping {
                cst,
                waiting_for_value: false,
            });
        } else {
            self.open.push(OpenNode::Sequence { cst });
        }
    }

    pub(crate) fn push_collection_end(&mut self, span: Span, mapping: bool) {
        if self.error.is_some() {
            return;
        }
        let result = if mapping {
            let Some(OpenNode::Mapping {
                cst,
                waiting_for_value,
            }) = self.open.pop()
            else {
                self.error = Some(structure_error("mismatched mapping end event", span));
                return;
            };
            if waiting_for_value {
                self.error = Some(structure_error(
                    "mapping entry does not contain a value",
                    span,
                ));
                return;
            }
            self.store.close(cst, span, None);
            self.attach_child(cst, span)
        } else {
            let Some(OpenNode::Sequence { cst }) = self.open.pop() else {
                self.error = Some(structure_error("mismatched sequence end event", span));
                return;
            };
            self.store.close(cst, span, None);
            self.attach_child(cst, span)
        };
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    fn try_push(
        &mut self,
        kind: &YamlEventKind,
        span: Span,
        cst: Option<NodeId>,
        properties: SemanticProperties,
    ) -> Result<(), YamlError> {
        match kind {
            YamlEventKind::StreamStart | YamlEventKind::StreamEnd => Ok(()),
            YamlEventKind::DocumentStart { explicit } => {
                let cst = required_cst(cst, span)?;
                for directive in self
                    .store
                    .tag_directives
                    .iter_mut()
                    .rev()
                    .take_while(|directive| directive.document == NodeId(u32::MAX))
                {
                    directive.document = cst;
                }
                self.store.documents.push(cst);
                let property = self.insert_properties(cst, properties);
                self.store.insert(
                    cst,
                    SemanticNode::new(SemanticKind::Document, span, *explicit, property),
                );
                self.open.push(OpenNode::Document { cst, children: 0 });
                Ok(())
            }
            YamlEventKind::MappingStart { style, .. } => {
                let cst = required_cst(cst, span)?;
                let property = self.insert_properties(cst, properties);
                self.store.insert(
                    cst,
                    SemanticNode::new(
                        SemanticKind::Mapping { style: *style },
                        span,
                        false,
                        property,
                    ),
                );
                self.open.push(OpenNode::Mapping {
                    cst,
                    waiting_for_value: false,
                });
                Ok(())
            }
            YamlEventKind::SequenceStart { style, .. } => {
                let cst = required_cst(cst, span)?;
                let property = self.insert_properties(cst, properties);
                self.store.insert(
                    cst,
                    SemanticNode::new(
                        SemanticKind::Sequence { style: *style },
                        span,
                        false,
                        property,
                    ),
                );
                self.open.push(OpenNode::Sequence { cst });
                Ok(())
            }
            YamlEventKind::Scalar { style, .. } => {
                let cst = required_cst(cst, span)?;
                let property = self.insert_properties(cst, properties);
                self.store.insert(
                    cst,
                    SemanticNode::new(
                        SemanticKind::Scalar { style: *style },
                        span,
                        false,
                        property,
                    ),
                );
                self.attach_child(cst, span)
            }
            YamlEventKind::Alias { .. } => {
                let cst = required_cst(cst, span)?;
                let property = self.insert_properties(cst, properties);
                self.store.insert(
                    cst,
                    SemanticNode::new(SemanticKind::Alias, span, false, property),
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
                self.store.close(cst, span, Some(*explicit));
                Ok(())
            }
        }
    }

    pub(crate) fn push_tag_directive(&mut self, handle: Span, prefix: Span) {
        self.store.tag_directives.push(TagDirectiveBinding {
            handle,
            prefix,
            document: NodeId(u32::MAX),
        });
    }

    fn insert_properties(&mut self, target: NodeId, properties: SemanticProperties) -> u32 {
        if properties.is_empty() {
            return NO_PROPERTIES;
        }
        let document = self
            .open
            .iter()
            .find_map(|node| match node {
                OpenNode::Document { cst, .. } => Some(*cst),
                _ => None,
            })
            .unwrap_or(target);
        let index = u32::try_from(self.store.properties.len())
            .expect("semantic property arena exceeds u32 capacity");
        self.store.properties.push(PropertyRecord {
            properties,
            document,
        });
        if let Some(name) = properties.anchor {
            self.store.anchors.push(AnchorBinding {
                name,
                target,
                document,
            });
        }
        index
    }

    fn attach_child(&mut self, _child: NodeId, span: Span) -> Result<(), YamlError> {
        let Some(parent) = self.open.last_mut() else {
            return Ok(());
        };
        match parent {
            OpenNode::Document { children, .. } => {
                *children += 1;
                if *children > 1 {
                    return Err(structure_error(
                        "document contains multiple root nodes",
                        span,
                    ));
                }
            }
            OpenNode::Mapping {
                waiting_for_value, ..
            } => {
                *waiting_for_value = !*waiting_for_value;
            }
            OpenNode::Sequence { .. } => {}
        }
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
    fn new(kind: SemanticKind, span: Span, explicit_start: bool, property: u32) -> Self {
        Self {
            kind,
            flags: u8::from(explicit_start) * EXPLICIT_START,
            padding: 0,
            span_start: span.start,
            end_offset: span.end,
            property,
        }
    }

    pub(crate) const fn explicit_start(self) -> bool {
        self.flags & EXPLICIT_START != 0
    }

    pub(crate) const fn explicit_end(self) -> bool {
        self.flags & EXPLICIT_END != 0
    }

    fn set_flag(&mut self, flag: u8, value: bool) {
        if value {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }
}

fn required_cst(cst: Option<NodeId>, span: Span) -> Result<NodeId, YamlError> {
    cst.ok_or_else(|| structure_error("semantic node is missing its CST origin", span))
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
    use super::{SemanticBuilder, SemanticNode, SemanticProperties};
    use crate::{CollectionStyle, NodeId, Span, YamlEventKind, YamlScalarStyle};

    #[test]
    fn direct_builder_rejects_dangling_mapping_value() {
        let mut builder = SemanticBuilder::with_capacity(4, 4);
        builder.push(
            YamlEventKind::DocumentStart { explicit: false },
            Span::empty(0),
            Some(NodeId(0)),
            SemanticProperties::NONE,
        );
        builder.push(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Block,
                tag: None,
                anchor: None,
            },
            Span::empty(0),
            Some(NodeId(1)),
            SemanticProperties::NONE,
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
            SemanticProperties::NONE,
        );
        builder.push(
            YamlEventKind::MappingEnd,
            Span::empty(0),
            None,
            SemanticProperties::NONE,
        );

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
            SemanticProperties::NONE,
        );
        builder.push(
            YamlEventKind::MappingEnd,
            Span::empty(1),
            None,
            SemanticProperties::NONE,
        );

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
            SemanticProperties::NONE,
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
                SemanticProperties::NONE,
            );
        }

        let error = builder.finish(3).expect_err("documents have one root");
        assert!(error.to_string().contains("multiple root nodes"));
    }

    #[test]
    fn semantic_record_is_at_most_sixteen_bytes() {
        assert!(std::mem::size_of::<SemanticNode>() <= 16);
    }

    #[test]
    fn undecorated_nodes_do_not_populate_sparse_arenas() {
        let mut builder = SemanticBuilder::with_capacity(2, 2);
        builder.push(
            YamlEventKind::DocumentStart { explicit: false },
            Span::empty(0),
            Some(NodeId(0)),
            SemanticProperties::NONE,
        );
        builder.push(
            YamlEventKind::Scalar {
                style: YamlScalarStyle::Plain,
                value: String::new(),
                tag: None,
                anchor: None,
            },
            Span::empty(0),
            Some(NodeId(1)),
            SemanticProperties::NONE,
        );
        builder.push(
            YamlEventKind::DocumentEnd { explicit: false },
            Span::empty(0),
            None,
            SemanticProperties::NONE,
        );

        let store = builder.finish(2).expect("semantic structure closes");
        assert!(store.properties.is_empty());
        assert!(store.anchors.is_empty());
        assert!(store.tag_directives.is_empty());
    }
}
