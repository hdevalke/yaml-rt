use crate::inline_vec::InlineVec;
use crate::syntax::{
    COMMON_SEMANTIC_NODE, NO_SEMANTIC_NODE, NODE_EXPLICIT_END, NODE_EXPLICIT_START,
    NODE_SCALAR_DOUBLE_QUOTED, NODE_SCALAR_PLAIN, NODE_SCALAR_SINGLE_QUOTED,
    NODE_SCALAR_STYLE_MASK, NODE_SEMANTIC_ALIAS,
};
use crate::{
    CollectionStyle, Diagnostic, DiagnosticKind, Node, NodeId, NodeKind, Span, YamlError,
    YamlEventKind, YamlScalarStyle,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SemanticMetadata {
    end_offset: u32,
    property: u32,
}

/// Compact semantic side arena indexed through CST `NodeId`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticStore {
    metadata: Vec<SemanticMetadata>,
    properties: Vec<PropertyRecord>,
    anchors: Vec<AnchorBinding>,
    tag_directives: Vec<TagDirectiveBinding>,
    pub(crate) documents: InlineVec<NodeId, 1>,
}

impl SemanticStore {
    pub(crate) fn get(&self, node: &Node) -> Option<SemanticNode> {
        let index = node.semantic;
        if index == NO_SEMANTIC_NODE {
            return None;
        }
        let metadata = if index == COMMON_SEMANTIC_NODE {
            SemanticMetadata {
                end_offset: node.span.end,
                property: NO_PROPERTIES,
            }
        } else {
            self.metadata[index as usize]
        };
        Some(SemanticNode {
            kind: semantic_kind_from_node(node),
            flags: (u8::from(node.syntax_flags & NODE_EXPLICIT_START != 0) * EXPLICIT_START)
                | (u8::from(node.syntax_flags & NODE_EXPLICIT_END != 0) * EXPLICIT_END),
            padding: 0,
            end_offset: metadata.end_offset,
            property: metadata.property,
        })
    }

    pub(crate) fn properties(&self, cst: &Node) -> Option<SemanticProperties> {
        let node = self.get(cst)?;
        (node.property != NO_PROPERTIES).then(|| self.properties[node.property as usize].properties)
    }

    pub(crate) fn span_start(&self, cst: &Node, cst_start: u32) -> u32 {
        let Some(properties) = self.properties(cst) else {
            return cst_start;
        };
        let tag_start = properties.tag.map(|span| span.start);
        let anchor_start = properties.anchor.map(|span| span.start.saturating_sub(1));
        let alias_start = properties.alias.map(|span| span.start.saturating_sub(1));
        [tag_start, anchor_start, alias_start]
            .into_iter()
            .flatten()
            .fold(cst_start, u32::min)
    }

    pub(crate) fn clear_tag(&mut self, cst: &Node) {
        let Some(property) = self.get(cst).map(|node| node.property) else {
            return;
        };
        if property != NO_PROPERTIES {
            self.properties[property as usize].properties.tag = None;
        }
    }

    pub(crate) fn property_document(&self, cst: &Node) -> Option<NodeId> {
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
    current_document: Option<NodeId>,
    error: Option<YamlError>,
}

impl SemanticBuilder {
    pub(crate) fn with_capacity(_cst_capacity: usize, _semantic_capacity: usize) -> Self {
        Self {
            store: SemanticStore {
                metadata: Vec::new(),
                properties: Vec::new(),
                anchors: Vec::new(),
                tag_directives: Vec::new(),
                documents: InlineVec::new(),
            },
            open: Vec::with_capacity(8),
            current_document: None,
            error: None,
        }
    }

    pub(crate) fn push(
        &mut self,
        nodes: &mut [Node],
        kind: YamlEventKind,
        span: Span,
        cst: Option<NodeId>,
        properties: SemanticProperties,
    ) {
        if self.error.is_some() {
            return;
        }
        let result = self.try_push(nodes, &kind, span, cst, properties);
        drop(kind);
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    pub(crate) fn push_property_free_scalar(
        &mut self,
        nodes: &mut [Node],
        cst: NodeId,
        span: Span,
        style: YamlScalarStyle,
    ) {
        if self.error.is_some() {
            return;
        }
        self.write_node(
            nodes,
            cst,
            SemanticKind::Scalar { style },
            span,
            false,
            NO_PROPERTIES,
        );
        if let Err(error) = self.attach_child(cst, span) {
            self.error = Some(error);
        }
    }

    pub(crate) fn register_flow_scalar(
        &mut self,
        nodes: &mut [Node],
        cst: NodeId,
        span: Span,
        kind: SemanticKind,
        properties: SemanticProperties,
    ) {
        if self.error.is_some() {
            return;
        }
        let property = self.insert_properties(cst, properties);
        self.write_node(nodes, cst, kind, span, false, property);
    }

    pub(crate) fn register_flow_collection(
        &mut self,
        nodes: &mut [Node],
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
        self.write_node(nodes, cst, kind, span, false, property);
    }

    pub(crate) fn finish_flow_collection(&mut self, nodes: &mut [Node], cst: NodeId, span: Span) {
        if self.error.is_none() {
            self.close(nodes, cst, span, None);
        }
    }

    pub(crate) fn attach_flow_root(&mut self, cst: NodeId, span: Span) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.attach_child(cst, span) {
            self.error = Some(error);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive match maintains semantic transitions for every event kind"
    )]
    fn try_push(
        &mut self,
        nodes: &mut [Node],
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
                self.current_document = Some(cst);
                self.store.documents.push(cst);
                let property = self.insert_properties(cst, properties);
                self.write_node(
                    nodes,
                    cst,
                    SemanticKind::Document,
                    span,
                    *explicit,
                    property,
                );
                self.open.push(OpenNode::Document { cst, children: 0 });
                Ok(())
            }
            YamlEventKind::MappingStart { style, .. } => {
                let cst = required_cst(cst, span)?;
                let property = self.insert_properties(cst, properties);
                self.write_node(
                    nodes,
                    cst,
                    SemanticKind::Mapping { style: *style },
                    span,
                    false,
                    property,
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
                self.write_node(
                    nodes,
                    cst,
                    SemanticKind::Sequence { style: *style },
                    span,
                    false,
                    property,
                );
                self.open.push(OpenNode::Sequence { cst });
                Ok(())
            }
            YamlEventKind::Scalar { style, .. } => {
                let cst = required_cst(cst, span)?;
                let property = self.insert_properties(cst, properties);
                self.write_node(
                    nodes,
                    cst,
                    SemanticKind::Scalar { style: *style },
                    span,
                    false,
                    property,
                );
                self.attach_child(cst, span)
            }
            YamlEventKind::Alias { .. } => {
                let cst = required_cst(cst, span)?;
                let property = self.insert_properties(cst, properties);
                self.write_node(nodes, cst, SemanticKind::Alias, span, false, property);
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
                self.close(nodes, cst, span, None);
                self.attach_child(cst, span)
            }
            YamlEventKind::SequenceEnd => {
                let Some(OpenNode::Sequence { cst }) = self.open.pop() else {
                    return Err(structure_error("mismatched sequence end event", span));
                };
                self.close(nodes, cst, span, None);
                self.attach_child(cst, span)
            }
            YamlEventKind::DocumentEnd { explicit } => {
                let Some(OpenNode::Document { cst, .. }) = self.open.pop() else {
                    return Err(structure_error("mismatched document end event", span));
                };
                self.close(nodes, cst, span, Some(*explicit));
                self.current_document = None;
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

    fn write_node(
        &mut self,
        nodes: &mut [Node],
        cst: NodeId,
        kind: SemanticKind,
        span: Span,
        explicit_start: bool,
        property: u32,
    ) {
        let node = &mut nodes[cst.as_usize()];
        node.syntax_flags &= !(NODE_SEMANTIC_ALIAS
            | NODE_EXPLICIT_START
            | NODE_EXPLICIT_END
            | NODE_SCALAR_STYLE_MASK);
        match kind {
            SemanticKind::Scalar { style } => {
                node.syntax_flags |= match style {
                    YamlScalarStyle::Plain => NODE_SCALAR_PLAIN,
                    YamlScalarStyle::SingleQuoted => NODE_SCALAR_SINGLE_QUOTED,
                    YamlScalarStyle::DoubleQuoted => NODE_SCALAR_DOUBLE_QUOTED,
                    YamlScalarStyle::Literal | YamlScalarStyle::Folded => 0,
                };
            }
            SemanticKind::Alias => node.syntax_flags |= NODE_SEMANTIC_ALIAS,
            SemanticKind::Document
            | SemanticKind::Mapping { .. }
            | SemanticKind::Sequence { .. } => {}
        }
        if explicit_start {
            node.syntax_flags |= NODE_EXPLICIT_START;
        }
        node.semantic = if property == NO_PROPERTIES {
            COMMON_SEMANTIC_NODE
        } else {
            self.push_metadata(SemanticMetadata {
                end_offset: span.end,
                property,
            })
        };
    }

    fn close(&mut self, nodes: &mut [Node], cst: NodeId, span: Span, explicit: Option<bool>) {
        let node = &mut nodes[cst.as_usize()];
        if node.semantic == COMMON_SEMANTIC_NODE {
            if span.end != node.span.end {
                node.semantic = self.push_metadata(SemanticMetadata {
                    end_offset: span.end,
                    property: NO_PROPERTIES,
                });
            }
        } else {
            self.store.metadata[node.semantic as usize].end_offset = span.end;
        }
        if let Some(explicit) = explicit {
            if explicit {
                node.syntax_flags |= NODE_EXPLICIT_END;
            } else {
                node.syntax_flags &= !NODE_EXPLICIT_END;
            }
        }
    }

    fn push_metadata(&mut self, metadata: SemanticMetadata) -> u32 {
        let index = u32::try_from(self.store.metadata.len())
            .expect("semantic metadata arena exceeds u32 capacity");
        self.store.metadata.push(metadata);
        index
    }

    fn insert_properties(&mut self, target: NodeId, properties: SemanticProperties) -> u32 {
        if properties.is_empty() {
            return NO_PROPERTIES;
        }
        let document = self.current_document.unwrap_or(target);
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

    pub(crate) fn finish(self) -> Result<SemanticStore, YamlError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !self.open.is_empty() {
            return Err(structure_error("unclosed semantic node", Span::empty(0)));
        }
        Ok(self.store)
    }
}

impl SemanticNode {
    pub(crate) const fn explicit_start(self) -> bool {
        self.flags & EXPLICIT_START != 0
    }

    pub(crate) const fn explicit_end(self) -> bool {
        self.flags & EXPLICIT_END != 0
    }
}

fn semantic_kind_from_node(node: &Node) -> SemanticKind {
    match node.kind {
        NodeKind::Document => SemanticKind::Document,
        NodeKind::BlockMapping => SemanticKind::Mapping {
            style: CollectionStyle::Block,
        },
        NodeKind::FlowMapping => SemanticKind::Mapping {
            style: CollectionStyle::Flow,
        },
        NodeKind::BlockSequence => SemanticKind::Sequence {
            style: CollectionStyle::Block,
        },
        NodeKind::FlowSequence => SemanticKind::Sequence {
            style: CollectionStyle::Flow,
        },
        NodeKind::Scalar if node.syntax_flags & NODE_SEMANTIC_ALIAS != 0 => SemanticKind::Alias,
        NodeKind::Scalar => SemanticKind::Scalar {
            style: match node.syntax_flags & NODE_SCALAR_STYLE_MASK {
                NODE_SCALAR_SINGLE_QUOTED => YamlScalarStyle::SingleQuoted,
                NODE_SCALAR_DOUBLE_QUOTED => YamlScalarStyle::DoubleQuoted,
                _ => YamlScalarStyle::Plain,
            },
        },
        NodeKind::LiteralScalar => SemanticKind::Scalar {
            style: YamlScalarStyle::Literal,
        },
        NodeKind::FoldedScalar => SemanticKind::Scalar {
            style: YamlScalarStyle::Folded,
        },
        _ => unreachable!("only semantic CST nodes carry semantic metadata"),
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
    use super::{SemanticBuilder, SemanticMetadata, SemanticNode, SemanticProperties};
    use crate::syntax::{NO_NODE, NO_SEMANTIC_NODE};
    use crate::{CollectionStyle, Node, NodeId, NodeKind, Span, YamlEventKind, YamlScalarStyle};

    fn cst_nodes(len: usize) -> Vec<Node> {
        (0..len)
            .map(|_| Node {
                kind: NodeKind::Scalar,
                syntax_flags: 0,
                span: Span::empty(0),
                parent: NO_NODE,
                first_child: NO_NODE,
                last_child: NO_NODE,
                next_sibling: NO_NODE,
                semantic: NO_SEMANTIC_NODE,
            })
            .collect()
    }

    #[test]
    fn direct_builder_rejects_dangling_mapping_value() {
        let mut builder = SemanticBuilder::with_capacity(4, 4);
        let mut nodes = cst_nodes(3);
        builder.push(
            &mut nodes,
            YamlEventKind::DocumentStart { explicit: false },
            Span::empty(0),
            Some(NodeId(0)),
            SemanticProperties::NONE,
        );
        builder.push(
            &mut nodes,
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
            &mut nodes,
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
            &mut nodes,
            YamlEventKind::MappingEnd,
            Span::empty(0),
            None,
            SemanticProperties::NONE,
        );

        let error = builder.finish().expect_err("mapping value is required");
        assert!(error.to_string().contains("does not contain a value"));
    }

    #[test]
    fn direct_builder_rejects_mismatched_collection_end() {
        let mut builder = SemanticBuilder::with_capacity(2, 2);
        let mut nodes = cst_nodes(1);
        builder.push(
            &mut nodes,
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
            &mut nodes,
            YamlEventKind::MappingEnd,
            Span::empty(1),
            None,
            SemanticProperties::NONE,
        );

        let error = builder.finish().expect_err("collection ends must match");
        assert!(error.to_string().contains("mismatched mapping end"));
    }

    #[test]
    fn direct_builder_rejects_multiple_document_roots() {
        let mut builder = SemanticBuilder::with_capacity(3, 3);
        let mut nodes = cst_nodes(3);
        builder.push(
            &mut nodes,
            YamlEventKind::DocumentStart { explicit: false },
            Span::empty(0),
            Some(NodeId(0)),
            SemanticProperties::NONE,
        );
        for cst in [NodeId(1), NodeId(2)] {
            builder.push(
                &mut nodes,
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

        let error = builder.finish().expect_err("documents have one root");
        assert!(error.to_string().contains("multiple root nodes"));
    }

    #[test]
    fn semantic_records_have_compact_layouts() {
        assert_eq!(std::mem::size_of::<SemanticNode>(), 12);
        assert_eq!(std::mem::size_of::<SemanticMetadata>(), 8);
    }

    #[test]
    fn undecorated_nodes_do_not_populate_sparse_arenas() {
        let mut builder = SemanticBuilder::with_capacity(2, 2);
        let mut nodes = cst_nodes(2);
        builder.push(
            &mut nodes,
            YamlEventKind::DocumentStart { explicit: false },
            Span::empty(0),
            Some(NodeId(0)),
            SemanticProperties::NONE,
        );
        builder.push(
            &mut nodes,
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
            &mut nodes,
            YamlEventKind::DocumentEnd { explicit: false },
            Span::empty(0),
            None,
            SemanticProperties::NONE,
        );

        let store = builder.finish().expect("semantic structure closes");
        assert!(store.metadata.is_empty());
        assert!(store.properties.is_empty());
        assert!(store.anchors.is_empty());
        assert!(store.tag_directives.is_empty());
    }
}
