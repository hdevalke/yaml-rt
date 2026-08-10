use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use crate::syntax::node_link;
use crate::{
    Children, CollectionStyle, Diagnostic, DiagnosticKind, FromYamlDoc, Node, NodeId, NodeKind,
    Parser, ScalarStyle, SemanticKind, SemanticStore, Source, Span, ToYamlDoc, ToYamlFragment,
    Token, YamlEditError, YamlError, YamlEvent, YamlFragment,
    decode_scalar_value_with_content_indent, directive_emit_error, double_quoted_scalar_end,
    edits_conflict, events_to_test_string, format_scalar_value, lex, parse_node_properties,
    plain_scalar_end, resolve_tag, single_quoted_scalar_end, strip_inline_comment,
    validate_plain_mapping_fragment, validate_tag_directive_parts_for_emit, validate_yaml_chars,
    validate_yaml_directive_version_for_emit,
};

/// Pending source edit used by the patch-based emitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Span to replace. Empty spans represent insertions.
    pub span: Span,
    /// Replacement text.
    pub replacement: String,
}

/// On-demand iterator over semantic YAML events.
pub struct YamlEvents<'doc> {
    doc: &'doc YamlDoc,
    tasks: Vec<EventTask>,
}

impl Iterator for YamlEvents<'_> {
    type Item = YamlEvent;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(task) = self.tasks.pop() {
            match task {
                EventTask::StreamStart => {
                    return Some(YamlEvent {
                        kind: crate::YamlEventKind::StreamStart,
                        span: Span::from_usize(0, self.doc.source.len()),
                        cst: None,
                        content_indent: None,
                    });
                }
                EventTask::Documents(index) => self.schedule_document(index),
                EventTask::DocumentStart(document) => {
                    let Some(metadata) = self.doc.semantics.get(document) else {
                        continue;
                    };
                    return Some(YamlEvent {
                        kind: crate::YamlEventKind::DocumentStart {
                            explicit: metadata.explicit_start(),
                        },
                        span: self.doc.semantic_span(document, metadata),
                        cst: Some(document),
                        content_indent: None,
                    });
                }
                EventTask::DocumentChildren(next) => self.schedule_document_child(next),
                EventTask::DocumentEnd(document) => {
                    let Some(metadata) = self.doc.semantics.get(document) else {
                        continue;
                    };
                    return Some(YamlEvent {
                        kind: crate::YamlEventKind::DocumentEnd {
                            explicit: metadata.explicit_end(),
                        },
                        span: self.doc.semantic_end_span(document, metadata),
                        cst: None,
                        content_indent: None,
                    });
                }
                EventTask::Node(node) => {
                    if let Some(event) = self.schedule_node(node) {
                        return Some(event);
                    }
                }
                EventTask::MappingEntries(next) => self.schedule_mapping_entry(next),
                EventTask::SequenceEntries(next) => self.schedule_sequence_entry(next),
                EventTask::CollectionEnd { node, mapping } => {
                    let Some(metadata) = self.doc.semantics.get(node) else {
                        continue;
                    };
                    return Some(YamlEvent {
                        kind: if mapping {
                            crate::YamlEventKind::MappingEnd
                        } else {
                            crate::YamlEventKind::SequenceEnd
                        },
                        span: self.doc.semantic_end_span(node, metadata),
                        cst: None,
                        content_indent: None,
                    });
                }
                EventTask::StreamEnd => {
                    return Some(YamlEvent {
                        kind: crate::YamlEventKind::StreamEnd,
                        span: Span::empty_from_usize(self.doc.source.len()),
                        cst: None,
                        content_indent: None,
                    });
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventTask {
    StreamStart,
    Documents(usize),
    DocumentStart(NodeId),
    DocumentChildren(u32),
    DocumentEnd(NodeId),
    Node(NodeId),
    MappingEntries(u32),
    SequenceEntries(u32),
    CollectionEnd { node: NodeId, mapping: bool },
    StreamEnd,
}

impl YamlEvents<'_> {
    fn schedule_document(&mut self, index: usize) {
        let Some(&document) = self.doc.semantics.documents.get(index) else {
            return;
        };
        let Some(node) = self.doc.node(document) else {
            self.tasks.push(EventTask::Documents(index + 1));
            return;
        };
        self.tasks.push(EventTask::Documents(index + 1));
        self.tasks.push(EventTask::DocumentEnd(document));
        self.tasks
            .push(EventTask::DocumentChildren(node.first_child));
        self.tasks.push(EventTask::DocumentStart(document));
    }

    fn schedule_document_child(&mut self, next: u32) {
        let Some(child) = node_link(next) else {
            return;
        };
        self.tasks.push(EventTask::DocumentChildren(
            self.doc.nodes[child.as_usize()].next_sibling,
        ));
        if self.doc.semantics.get(child).is_some() {
            self.tasks.push(EventTask::Node(child));
        }
    }

    fn schedule_node(&mut self, node: NodeId) -> Option<YamlEvent> {
        let metadata = self.doc.semantics.get(node)?;
        let span = self.doc.semantic_span(node, metadata);
        match metadata.kind {
            SemanticKind::Document => None,
            SemanticKind::Mapping { style } => {
                self.tasks.push(EventTask::CollectionEnd {
                    node,
                    mapping: true,
                });
                self.tasks.push(EventTask::MappingEntries(
                    self.doc.nodes[node.as_usize()].first_child,
                ));
                Some(YamlEvent {
                    kind: crate::YamlEventKind::MappingStart {
                        style,
                        tag: self
                            .doc
                            .resolved_tag(node)
                            .ok()
                            .flatten()
                            .map(Cow::into_owned),
                        anchor: self.doc.anchor(node).map(str::to_owned),
                    },
                    span,
                    cst: Some(node),
                    content_indent: None,
                })
            }
            SemanticKind::Sequence { style } => {
                self.tasks.push(EventTask::CollectionEnd {
                    node,
                    mapping: false,
                });
                self.tasks.push(EventTask::SequenceEntries(
                    self.doc.nodes[node.as_usize()].first_child,
                ));
                Some(YamlEvent {
                    kind: crate::YamlEventKind::SequenceStart {
                        style,
                        tag: self
                            .doc
                            .resolved_tag(node)
                            .ok()
                            .flatten()
                            .map(Cow::into_owned),
                        anchor: self.doc.anchor(node).map(str::to_owned),
                    },
                    span,
                    cst: Some(node),
                    content_indent: None,
                })
            }
            SemanticKind::Scalar { style } => Some(YamlEvent {
                kind: crate::YamlEventKind::Scalar {
                    style,
                    value: self
                        .doc
                        .scalar_value(node)
                        .map(Cow::into_owned)
                        .unwrap_or_default(),
                    tag: self
                        .doc
                        .resolved_tag(node)
                        .ok()
                        .flatten()
                        .map(Cow::into_owned),
                    anchor: self.doc.anchor(node).map(str::to_owned),
                },
                span,
                cst: Some(node),
                content_indent: self
                    .doc
                    .semantics
                    .properties(node)
                    .and_then(|properties| properties.content_indent),
            }),
            SemanticKind::Alias => Some(YamlEvent {
                kind: crate::YamlEventKind::Alias {
                    name: self.doc.alias_name(node).unwrap_or_default().to_owned(),
                },
                span,
                cst: Some(node),
                content_indent: None,
            }),
        }
    }

    fn schedule_mapping_entry(&mut self, next: u32) {
        let Some(entry) = node_link(next) else {
            return;
        };
        self.tasks.push(EventTask::MappingEntries(
            self.doc.nodes[entry.as_usize()].next_sibling,
        ));
        if self.doc.nodes[entry.as_usize()].kind != NodeKind::MappingEntry {
            return;
        }
        let Some(key) = self.first_semantic_child(entry) else {
            return;
        };
        let Some(value) = self.next_semantic_sibling(key) else {
            return;
        };
        self.tasks.push(EventTask::Node(value));
        self.tasks.push(EventTask::Node(key));
    }

    fn schedule_sequence_entry(&mut self, next: u32) {
        let Some(entry) = node_link(next) else {
            return;
        };
        self.tasks.push(EventTask::SequenceEntries(
            self.doc.nodes[entry.as_usize()].next_sibling,
        ));
        if self.doc.nodes[entry.as_usize()].kind != NodeKind::SequenceEntry {
            return;
        }
        if let Some(item) = self.first_semantic_child(entry) {
            self.tasks.push(EventTask::Node(item));
        }
    }

    fn first_semantic_child(&self, parent: NodeId) -> Option<NodeId> {
        let next = self.doc.nodes[parent.as_usize()].first_child;
        self.next_semantic(next)
    }

    fn next_semantic_sibling(&self, node: NodeId) -> Option<NodeId> {
        let next = self.doc.nodes[node.as_usize()].next_sibling;
        self.next_semantic(next)
    }

    fn next_semantic(&self, mut next: u32) -> Option<NodeId> {
        while let Some(node) = node_link(next) {
            if self.doc.semantics.get(node).is_some() {
                return Some(node);
            }
            next = self.doc.nodes[node.as_usize()].next_sibling;
        }
        None
    }
}

/// Parsed `%YAML` directive metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlDirective {
    /// Directive version text, such as `1.2`.
    pub version: String,
    /// CST directive node.
    pub node: NodeId,
}

/// Parsed `%TAG` directive metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagDirective {
    /// Tag handle, such as `!` or `!e!`.
    pub handle: String,
    /// Tag prefix text.
    pub prefix: String,
    /// CST directive node.
    pub node: NodeId,
}

/// Parsed reserved directive metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedDirective {
    /// Directive name, including the leading `%`.
    pub name: String,
    /// Whitespace-separated directive parameters.
    pub parameters: Vec<String>,
    /// CST directive node.
    pub node: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedDirective {
    Yaml(YamlDirective),
    Tag(TagDirective),
    Reserved(ReservedDirective),
}

/// Formatting controls for inserting a block mapping entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MappingEntryStyle {
    /// Reuse the target mapping indentation and the document line ending.
    #[default]
    Inherit,
    /// Insert with an explicit indentation width, in spaces.
    Indent(usize),
}

/// A source-preserving YAML document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlDoc {
    /// Original source buffer.
    pub(crate) source: Source,
    /// CST and semantic nodes. The CST remains the source of truth.
    pub(crate) nodes: Vec<Node>,
    /// Compact semantic metadata keyed by CST node IDs.
    pub(crate) semantics: SemanticStore,
    /// Optional scalar, sequence, or mapping root used by nested typed overlays.
    pub(crate) root_override: Option<NodeId>,
    /// Pending patch edits applied from highest offset to lowest offset.
    pub(crate) edits: Vec<Edit>,
}

impl YamlDoc {
    /// Parses a YAML stream into a round-trip document.
    ///
    /// This bootstrap parser preserves the input text exactly and records a root
    /// stream node. Real lexing and parsing will replace this placeholder.
    ///
    /// # Errors
    ///
    /// Returns an error when source validation, CST parsing, or semantic view
    /// composition fails.
    pub fn parse(input: &str) -> Result<Self, YamlError> {
        Self::parse_owned(input.to_owned())
    }

    /// Parses an owned YAML stream without copying its source buffer.
    ///
    /// # Errors
    ///
    /// Returns an error when source validation, CST parsing, or semantic view
    /// composition fails.
    pub fn parse_owned(input: String) -> Result<Self, YamlError> {
        let source = Source::new(input)?;
        let parsed = Parser::new(&source)
            .parse()
            .map_err(|error| error.with_position_from(&source))?;
        Ok(Self {
            source,
            nodes: parsed.nodes,
            semantics: parsed.semantics,
            root_override: None,
            edits: Vec::new(),
        })
    }

    /// Applies pending edits, reparses the rendered YAML, and clears the edit queue.
    ///
    /// `YamlDoc::to_string` only previews the original source with queued byte
    /// patches applied. It does not prove that the patched stream is still
    /// valid YAML, because low-level edit APIs can replace arbitrary node spans
    /// or insert conservative-but-raw fragments. Reparse on commit is the point
    /// where the document regains a validated CST and semantic view.
    ///
    /// # Errors
    ///
    /// Returns an error when the patched YAML cannot be parsed.
    pub fn commit_edits(&mut self) -> Result<(), YamlError> {
        if self.edits.is_empty() {
            return Ok(());
        }

        let edited = self.to_string();
        *self = Self::parse(&edited)?;
        Ok(())
    }

    /// Returns the original source text.
    #[must_use]
    pub fn as_source(&self) -> &str {
        self.source.as_str()
    }

    /// Returns the original source buffer and its line index.
    #[must_use]
    pub const fn source(&self) -> &Source {
        &self.source
    }

    /// Returns a freshly owned copy of the lossless token stream.
    ///
    /// The owned return type keeps this API stable when tokenization becomes
    /// on demand rather than retained by every document.
    ///
    /// # Errors
    ///
    /// Returns a lexer diagnostic if the source cannot be tokenized.
    pub fn tokens(&self) -> Result<Vec<Token>, YamlError> {
        lex(&self.source).map_err(|error| error.with_position_from(&self.source))
    }

    /// Returns the root node identifier when present.
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        (!self.nodes.is_empty()).then_some(NodeId(0))
    }

    /// Derives the semantic event stream from CST-linked metadata.
    #[must_use]
    pub fn events(&self) -> YamlEvents<'_> {
        let mut tasks = Vec::with_capacity(8);
        tasks.push(EventTask::StreamEnd);
        tasks.push(EventTask::Documents(0));
        tasks.push(EventTask::StreamStart);
        YamlEvents { doc: self, tasks }
    }

    /// Renders semantic events in the YAML Test Suite `test.event` format.
    #[must_use]
    pub fn events_to_test_string(&self) -> String {
        events_to_test_string(self.events())
    }

    /// Returns the number of documents in this YAML stream.
    #[must_use]
    pub fn document_count(&self) -> usize {
        self.root_override
            .map_or(self.semantics.documents.len(), |_| 1)
    }

    /// Queues an explicit document append at the end of this YAML stream.
    ///
    /// The appended document becomes visible to document-indexed lookup after
    /// [`YamlDoc::commit_edits`] reparses the stream.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` cannot be formatted as a YAML fragment or
    /// the append conflicts with another pending edit at the stream end.
    pub fn append_document<T>(&mut self, value: &T) -> Result<(), YamlError>
    where
        T: ToYamlFragment,
    {
        let line_ending = self.preferred_line_ending();
        let mut replacement = self.document_append_prefix(line_ending);
        replacement.push_str("---");
        replacement.push_str(line_ending);
        let fragment = value.to_yaml_fragment(0, line_ending)?;
        replacement.push_str(&fragment);
        replacement.push_str(line_ending);
        self.queue_edit(Span::empty_from_usize(self.source.len()), replacement)
    }

    /// Queues an explicit empty mapping document append.
    ///
    /// # Errors
    ///
    /// Returns an error when the append conflicts with another pending edit at
    /// the stream end.
    pub fn append_empty_mapping_document(&mut self) -> Result<(), YamlError> {
        self.append_document(&std::collections::BTreeMap::<String, String>::new())
    }

    /// Returns the `%YAML` directive when one is present in the stream prologue.
    #[must_use]
    pub fn yaml_directive(&self) -> Option<YamlDirective> {
        self.directive_nodes()
            .filter_map(|node| self.parse_directive_node(node).ok())
            .find_map(|directive| match directive {
                ParsedDirective::Yaml(directive) => Some(directive),
                ParsedDirective::Tag(_) | ParsedDirective::Reserved(_) => None,
            })
    }

    /// Returns `%TAG` directives from the stream prologue in source order.
    #[must_use]
    pub fn tag_directives(&self) -> Vec<TagDirective> {
        self.directive_nodes()
            .filter_map(|node| self.parse_directive_node(node).ok())
            .filter_map(|directive| match directive {
                ParsedDirective::Tag(directive) => Some(directive),
                ParsedDirective::Yaml(_) | ParsedDirective::Reserved(_) => None,
            })
            .collect()
    }

    /// Returns reserved directives from the stream prologue in source order.
    #[must_use]
    pub fn reserved_directives(&self) -> Vec<ReservedDirective> {
        self.directive_nodes()
            .filter_map(|node| self.parse_directive_node(node).ok())
            .filter_map(|directive| match directive {
                ParsedDirective::Reserved(directive) => Some(directive),
                ParsedDirective::Yaml(_) | ParsedDirective::Tag(_) => None,
            })
            .collect()
    }

    /// Queues insertion or update of the stream `%YAML` directive.
    ///
    /// # Errors
    ///
    /// Returns an error when `version` is not valid YAML directive version
    /// syntax or when the edit overlaps an existing pending edit.
    pub fn set_yaml_directive(&mut self, version: &str) -> Result<(), YamlError> {
        validate_yaml_directive_version_for_emit(version)?;
        let replacement = format!("%YAML {version}");
        if let Some(directive) = self.yaml_directive() {
            let span = self.directive_content_span(directive.node)?;
            self.queue_edit(span, replacement)
        } else {
            self.insert_directive_line(replacement)
        }
    }

    /// Queues insertion or update of a stream `%TAG` directive.
    ///
    /// # Errors
    ///
    /// Returns an error when `handle` or `prefix` is invalid or when the edit
    /// overlaps an existing pending edit.
    pub fn set_tag_directive(&mut self, handle: &str, prefix: &str) -> Result<(), YamlError> {
        validate_tag_directive_parts_for_emit(handle, prefix)?;
        let replacement = format!("%TAG {handle} {prefix}");
        if let Some(directive) = self
            .tag_directives()
            .into_iter()
            .find(|directive| directive.handle == handle)
        {
            let span = self.directive_content_span(directive.node)?;
            self.queue_edit(span, replacement)
        } else {
            self.insert_directive_line(replacement)
        }
    }

    /// Queues removal of the stream `%YAML` directive when present.
    ///
    /// # Errors
    ///
    /// Returns an error when the removal edit overlaps an existing pending edit.
    pub fn remove_yaml_directive(&mut self) -> Result<(), YamlError> {
        if let Some(directive) = self.yaml_directive() {
            self.remove_directive_node(directive.node)?;
        }
        Ok(())
    }

    /// Queues removal of the stream `%TAG` directive with `handle` when present.
    ///
    /// # Errors
    ///
    /// Returns an error when the removal edit overlaps an existing pending edit.
    pub fn remove_tag_directive(&mut self, handle: &str) -> Result<(), YamlError> {
        if let Some(directive) = self
            .tag_directives()
            .into_iter()
            .find(|directive| directive.handle == handle)
        {
            self.remove_directive_node(directive.node)?;
        }
        Ok(())
    }

    /// Returns a node by identifier.
    #[must_use]
    pub fn node(&self, node: NodeId) -> Option<&Node> {
        self.nodes.get(node.0 as usize)
    }

    /// Iterates over a node's children in source order.
    #[must_use]
    pub fn children(&self, node: NodeId) -> Children<'_> {
        Children::new(&self.nodes, node)
    }

    fn semantic_children(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(node)
            .filter(|child| self.semantics.get(*child).is_some())
    }

    /// Returns a node's semantic interpretation.
    #[must_use]
    pub fn semantic_kind(&self, node: NodeId) -> Option<SemanticKind> {
        self.semantics.get(node).map(|node| node.kind)
    }

    /// Returns the explicit tag spelling, including its leading `!`.
    #[must_use]
    pub fn raw_tag(&self, node: NodeId) -> Option<&str> {
        let span = self.semantics.properties(node)?.tag?;
        Some(self.source.slice(span))
    }

    /// Resolves an explicit tag through the built-in or document-local handle table.
    ///
    /// # Errors
    ///
    /// Returns an error when the tag spelling or its document-local handle is invalid.
    pub fn resolved_tag(&self, node: NodeId) -> Result<Option<Cow<'_, str>>, YamlError> {
        let Some(raw) = self.raw_tag(node) else {
            return Ok(None);
        };
        let document = self.semantics.property_document(node).unwrap_or(node);
        let handles = self
            .semantics
            .tag_directives(document)
            .map(|(handle, prefix)| {
                (
                    self.source.slice(handle).to_owned(),
                    self.source.slice(prefix).to_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let span = self
            .semantics
            .properties(node)
            .and_then(|properties| properties.tag)
            .unwrap_or_else(|| self.node(node).map_or(Span::empty(0), Node::span));
        resolve_tag(raw, &handles, span).map(|tag| Some(Cow::Owned(tag)))
    }

    /// Returns an anchor name without its leading `&`.
    #[must_use]
    pub fn anchor(&self, node: NodeId) -> Option<&str> {
        let span = self.semantics.properties(node)?.anchor?;
        Some(self.source.slice(span))
    }

    /// Returns an alias name without its leading `*`.
    #[must_use]
    pub fn alias_name(&self, node: NodeId) -> Option<&str> {
        let span = self.semantics.properties(node)?.alias?;
        Some(self.source.slice(span))
    }

    /// Resolves an alias to the most recent matching anchor in its document.
    #[must_use]
    pub fn resolve_alias(&self, node: NodeId) -> Option<NodeId> {
        let name = self.alias_name(node)?;
        let document = self.semantics.property_document(node)?;
        let alias_start = self.node(node)?.span.start;
        self.semantics
            .anchors()
            .rev()
            .find(|(span, target, anchor_document)| {
                *anchor_document == document
                    && self
                        .node(*target)
                        .is_some_and(|node| node.span.start <= alias_start)
                    && self.source.slice(*span) == name
            })
            .map(|(_, target, _)| target)
    }

    fn semantic_span(&self, node: NodeId, metadata: &crate::semantic::SemanticNode) -> Span {
        Span::new(
            metadata.span_start,
            self.node(node)
                .map_or(metadata.end_offset, |node| node.span.end),
        )
    }

    fn semantic_end_span(&self, node: NodeId, metadata: &crate::semantic::SemanticNode) -> Span {
        if metadata.explicit_end()
            && let Some(marker) = self.children(node).find_map(|child| {
                let child = self.node(child)?;
                (child.kind == NodeKind::DocumentMarker
                    && self.source.slice(child.span).starts_with("..."))
                .then_some(child.span)
            })
        {
            return marker;
        }
        if matches!(
            metadata.kind,
            SemanticKind::Mapping {
                style: CollectionStyle::Flow
            } | SemanticKind::Sequence {
                style: CollectionStyle::Flow
            }
        ) {
            return self.semantic_span(node, metadata);
        }
        Span::empty(metadata.end_offset)
    }

    /// Iterates over semantic document CST nodes in stream order.
    pub fn documents(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.semantics.documents.iter().copied()
    }

    /// Returns the semantic root value of a selected document.
    ///
    /// Empty documents have no root value and return `Ok(None)`. Unlike
    /// [`YamlDoc::document_root_mapping`], this method accepts scalar, sequence,
    /// mapping, and alias roots.
    ///
    /// # Errors
    ///
    /// Returns an error when `index` is outside the YAML stream.
    pub fn document_root(&self, index: usize) -> Result<Option<NodeId>, YamlError> {
        if let Some(root) = self.root_override {
            return (index == 0)
                .then_some(Some(root))
                .ok_or_else(|| self.document_index_error(index));
        }
        let document = self
            .semantics
            .documents
            .get(index)
            .copied()
            .ok_or_else(|| self.document_index_error(index))?;
        let root = self.semantic_children(document).next();
        Ok(root.filter(|root| {
            !matches!(self.semantic_kind(*root), Some(SemanticKind::Scalar { .. }))
                || self.node(*root).is_some_and(|node| !node.span.is_empty())
        }))
    }

    /// Iterates over mapping key/value CST node pairs in source order.
    pub fn mapping_entries(&self, mapping: NodeId) -> impl Iterator<Item = (NodeId, NodeId)> + '_ {
        let is_mapping = matches!(
            self.semantic_kind(mapping),
            Some(SemanticKind::Mapping { .. })
        );
        self.children(mapping).filter_map(move |entry| {
            if !is_mapping || self.node(entry)?.kind != NodeKind::MappingEntry {
                return None;
            }
            let mut children = self.semantic_children(entry);
            Some((children.next()?, children.next()?))
        })
    }

    /// Iterates over sequence item CST nodes in source order.
    pub fn sequence_items(&self, sequence: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let is_sequence = matches!(
            self.semantic_kind(sequence),
            Some(SemanticKind::Sequence { .. })
        );
        self.children(sequence).filter_map(move |entry| {
            if !is_sequence || self.node(entry)?.kind != NodeKind::SequenceEntry {
                return None;
            }
            self.semantic_children(entry).next()
        })
    }

    /// Returns the root-level mapping in the document.
    ///
    /// # Errors
    ///
    /// Returns an error when no root mapping exists or when the semantic
    /// root mapping is not linked back to a CST node.
    pub fn root_mapping(&self) -> Result<NodeId, YamlError> {
        self.document_root_mapping(0)
    }

    /// Returns the root-level mapping in a selected document.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected document does not exist, has no
    /// mapping root, or the root mapping is not linked back to the CST.
    pub fn document_root_mapping(&self, index: usize) -> Result<NodeId, YamlError> {
        if let Some(root) = self.root_override {
            return (index == 0)
                .then_some(root)
                .ok_or_else(|| self.document_index_error(index));
        }
        let document = self
            .semantics
            .documents
            .get(index)
            .copied()
            .ok_or_else(|| self.document_index_error(index))?;
        self.semantic_children(document)
            .find(|child| {
                self.node(*child).is_some_and(|node| {
                    matches!(node.kind, NodeKind::BlockMapping | NodeKind::FlowMapping)
                }) && matches!(
                    self.semantic_kind(*child),
                    Some(SemanticKind::Mapping { .. })
                )
            })
            .ok_or_else(|| {
                YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Semantic,
                        "document does not contain a root mapping",
                        self.node(document).map_or(Span::empty(0), |node| node.span),
                    )
                    .with_expected("a block or flow mapping node"),
                )
            })
    }

    /// Reads a typed overlay from a selected document.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected document is missing or empty, or the
    /// typed overlay cannot be read.
    pub fn read_document<T>(&self, index: usize) -> Result<T, YamlError>
    where
        T: FromYamlDoc,
    {
        let root = self
            .document_root(index)?
            .ok_or_else(|| self.empty_document_error(index))?;
        let nested = self.rerooted_at(root)?;
        T::from_yaml_doc(&nested)
    }

    /// Writes a typed overlay to a selected document.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected document is missing or empty, or the
    /// typed overlay cannot be written.
    pub fn write_document<T>(&mut self, index: usize, value: &T) -> Result<(), YamlError>
    where
        T: ToYamlDoc,
    {
        let root = self
            .document_root(index)?
            .ok_or_else(|| self.empty_document_error(index))?;
        let mut nested = self.rerooted_at(root)?;
        value.apply_to_yaml_doc(&mut nested)?;
        self.queue_edits_from(&nested)
    }

    /// Looks up a mapping entry by key inside `mapping`.
    ///
    /// # Errors
    ///
    /// Returns an error when a scalar key cannot be decoded.
    pub fn get_mapping_entry(
        &self,
        mapping: NodeId,
        key: &str,
    ) -> Result<Option<NodeId>, YamlError> {
        Ok(self
            .find_mapping_pair(mapping, key)?
            .and_then(|(key, _)| self.containing_entry(key)))
    }

    /// Looks up a mapping value by key inside `mapping`.
    ///
    /// # Errors
    ///
    /// Returns an error when a scalar key cannot be decoded.
    pub fn get_mapping_value(
        &self,
        mapping: NodeId,
        key: &str,
    ) -> Result<Option<NodeId>, YamlError> {
        Ok(self
            .find_mapping_pair(mapping, key)?
            .map(|(_, value)| value))
    }

    /// Looks up a nested path of mapping keys.
    ///
    /// # Errors
    ///
    /// Returns an error when semantic path lookup cannot decode a mapping key.
    pub fn get_path(&self, path: &[&str]) -> Result<Option<NodeId>, YamlError> {
        self.get_path_in_document(0, path)
    }

    /// Looks up a nested path of mapping keys in a selected document.
    ///
    /// # Errors
    ///
    /// Returns an error when semantic path lookup fails while resolving the
    /// graph path.
    pub fn get_path_in_document(
        &self,
        index: usize,
        path: &[&str],
    ) -> Result<Option<NodeId>, YamlError> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(None);
        };
        let Some((_, mut current)) =
            self.find_mapping_pair(self.document_root_mapping(index)?, first)?
        else {
            return Ok(None);
        };
        for segment in rest {
            let Some((_, value)) = self.find_mapping_pair(current, segment)? else {
                return Ok(None);
            };
            current = value;
        }
        Ok(Some(current))
    }

    fn document_index_error(&self, index: usize) -> YamlError {
        YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Semantic,
                format!("document index {index} is out of range"),
                Span::empty_from_usize(self.source.len()),
            )
            .with_expected("an existing document index"),
        )
    }

    fn empty_document_error(&self, index: usize) -> YamlError {
        YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Typed,
                format!("document {index} does not contain a YAML value"),
                Span::empty_from_usize(self.source.len()),
            )
            .with_expected("a scalar, sequence, or mapping document root"),
        )
    }

    fn find_mapping_pair(
        &self,
        mapping: NodeId,
        key: &str,
    ) -> Result<Option<(NodeId, NodeId)>, YamlError> {
        for (key_node, value_node) in self.mapping_entries(mapping) {
            if self.scalar_value(key_node)? == key {
                return Ok(Some((key_node, value_node)));
            }
        }
        Ok(None)
    }

    pub(crate) fn rerooted_at(&self, root: NodeId) -> Result<Self, YamlError> {
        let root_node = self.expect_node(root)?;
        if self.semantic_kind(root).is_none() {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    "typed overlay root does not have semantic metadata",
                    root_node.span,
                )
                .with_expected("a semantic YAML value"),
            )
            .with_position_from(&self.source));
        }
        let mut doc = self.clone();
        doc.root_override = Some(root);
        doc.edits.clear();
        Ok(doc)
    }

    pub(crate) fn rerooted_without_tag(&self, root: NodeId) -> Result<Self, YamlError> {
        let mut doc = self.rerooted_at(root)?;
        doc.semantics.clear_tag(root);
        Ok(doc)
    }

    pub(crate) fn rerooted_at_mapping(&self, mapping: NodeId) -> Result<Self, YamlError> {
        let mapping_node = self.expect_node(mapping)?;
        if !matches!(
            mapping_node.kind,
            NodeKind::BlockMapping | NodeKind::FlowMapping
        ) {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    format!("expected mapping, found {:?}", mapping_node.kind),
                    mapping_node.span,
                )
                .with_expected("BlockMapping or FlowMapping"),
            )
            .with_position_from(&self.source));
        }
        if !matches!(
            self.semantic_kind(mapping),
            Some(SemanticKind::Mapping { .. })
        ) {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    "mapping does not have semantic metadata",
                    self.expect_node(mapping)?.span,
                )
                .with_expected("a semantic mapping"),
            )
            .with_position_from(&self.source));
        }
        self.rerooted_at(mapping)
    }

    pub(crate) fn queue_edits_from(&mut self, other: &YamlDoc) -> Result<(), YamlError> {
        for edit in &other.edits {
            self.queue_edit(edit.span, edit.replacement.clone())?;
        }
        Ok(())
    }

    /// Returns the source text for a scalar node.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` is unknown or does not identify a plain CST
    /// scalar node.
    pub fn scalar_text(&self, node: NodeId) -> Result<&str, YamlError> {
        let node = self.expect_node_kind(node, NodeKind::Scalar)?;
        Ok(self.source.slice(node.span))
    }

    /// Returns the decoded value text for a scalar node.
    ///
    /// Plain scalars have trailing inline comments stripped, single-quoted
    /// scalars unescape doubled apostrophes, and double-quoted scalars unescape
    /// the common JSON/YAML escapes used by typed overlays.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` is unknown, is not a scalar node, has
    /// malformed node properties, or contains unsupported scalar escape syntax.
    pub fn scalar_value(&self, node: NodeId) -> Result<Cow<'_, str>, YamlError> {
        let node_ref = self.expect_node(node)?;
        if !matches!(
            node_ref.kind,
            NodeKind::Scalar | NodeKind::LiteralScalar | NodeKind::FoldedScalar
        ) {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    format!("expected scalar value, found {:?}", node_ref.kind),
                    node_ref.span,
                )
                .with_expected("Scalar, LiteralScalar, or FoldedScalar"),
            )
            .with_position_from(&self.source));
        }
        let text = self.source.slice(node_ref.span);
        let properties = parse_node_properties(text, node_ref.span)?;
        let value_text = &text[properties.value_start..];
        if matches!(
            self.semantic_kind(node),
            Some(SemanticKind::Scalar {
                style: crate::YamlScalarStyle::Plain,
                ..
            })
        ) && !value_text.contains(['\n', '\r'])
        {
            return Ok(Cow::Borrowed(&value_text[..plain_scalar_end(value_text)]));
        }
        decode_scalar_value_with_content_indent(
            value_text,
            self.semantics
                .properties(node)
                .and_then(|properties| properties.content_indent)
                .map(|indent| indent as usize),
        )
        .map(Cow::Owned)
    }

    /// Returns the source span of a scalar whose decoded value can be borrowed
    /// byte-for-byte from the original input.
    ///
    /// This currently returns a span only for single-line plain scalars. Quoted,
    /// escaped, folded, literal, and multiline scalars require decoding and
    /// return `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` is unknown, is not a scalar, or has malformed
    /// node properties.
    pub fn borrowable_scalar_span(&self, node: NodeId) -> Result<Option<Span>, YamlError> {
        let node_ref = self.expect_node(node)?;
        if !matches!(
            self.semantic_kind(node),
            Some(SemanticKind::Scalar {
                style: crate::YamlScalarStyle::Plain,
            })
        ) {
            if matches!(
                node_ref.kind,
                NodeKind::Scalar | NodeKind::LiteralScalar | NodeKind::FoldedScalar
            ) {
                return Ok(None);
            }
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    format!("expected scalar value, found {:?}", node_ref.kind),
                    node_ref.span,
                )
                .with_expected("Scalar, LiteralScalar, or FoldedScalar"),
            )
            .with_position_from(&self.source));
        }

        let text = self.source.slice(node_ref.span);
        if text.contains(['\n', '\r']) {
            return Ok(None);
        }
        let properties = parse_node_properties(text, node_ref.span)?;
        let value_text = &text[properties.value_start..];
        let value_len = plain_scalar_end(value_text);
        let start = Span::offset_from_usize(node_ref.span.start, properties.value_start);
        Ok(Some(Span::new(
            start,
            Span::offset_from_usize(start, value_len),
        )))
    }

    /// Queues a scalar value replacement at `path` while preserving the existing
    /// scalar style where the editor can do so safely.
    ///
    /// Plain scalars remain plain, single-quoted scalars remain single-quoted,
    /// and double-quoted scalars remain double-quoted. Inline comments and
    /// trailing whitespace outside the scalar spelling are left untouched.
    ///
    /// # Errors
    ///
    /// Returns an error when `path` does not resolve to an existing scalar, the
    /// current scalar style cannot be rewritten safely, `value` cannot be
    /// represented in that style, or the queued edit conflicts with another
    /// pending edit.
    pub fn set_scalar(&mut self, path: &[&str], value: &str) -> Result<(), YamlError> {
        let node = self.get_path(path)?.ok_or_else(|| {
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    format!("path `{}` does not exist", path.join(".")),
                    Span::empty(0),
                )
                .with_expected("an existing scalar node"),
            )
        })?;

        let (span, style) = self.scalar_replacement_target(node)?;
        let replacement = format_scalar_value(value, style)?;
        self.queue_edit(span, replacement)
    }

    /// Queues a patch that replaces the exact source span covered by `node`.
    ///
    /// The CST remains unchanged until the edited text is parsed again; callers
    /// can inspect the pending minimal-diff output through `doc.to_string()`.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` is unknown, `text` contains invalid YAML
    /// characters, or the replacement overlaps an existing pending edit.
    pub fn replace_node_text(
        &mut self,
        node: NodeId,
        text: impl Into<String>,
    ) -> Result<(), YamlError> {
        let span = self.expect_node(node)?.span;
        self.queue_edit(span, text.into())
    }

    /// Queues insertion of a plain `key: value` entry into a block mapping.
    ///
    /// This low-level writer accepts raw plain scalar text. Use typed values or
    /// node fragments when quoting or schema-aware formatting is required.
    ///
    /// # Errors
    ///
    /// Returns an error when `mapping` is not a block mapping, `key` or `value`
    /// is not valid as a plain mapping fragment, or the insertion conflicts with
    /// another pending edit.
    pub fn insert_mapping_entry(
        &mut self,
        mapping: NodeId,
        key: &str,
        value: &str,
        style: MappingEntryStyle,
    ) -> Result<(), YamlError> {
        self.insert_mapping_entry_with_comment(mapping, key, value, style, None)
    }

    /// Queues insertion of a plain `key: value` entry with optional preceding
    /// comment lines.
    ///
    /// Comments are emitted only for inserted entries; existing YAML comments are
    /// never overwritten by this helper.
    ///
    /// # Errors
    ///
    /// Returns an error when `mapping` is not a block mapping, `key`, `value`, or
    /// `comment` cannot be emitted as valid YAML text, or the insertion conflicts
    /// with another pending edit.
    pub fn insert_mapping_entry_with_comment(
        &mut self,
        mapping: NodeId,
        key: &str,
        value: &str,
        style: MappingEntryStyle,
        comment: Option<&str>,
    ) -> Result<(), YamlError> {
        let mapping_node = self.expect_node_kind(mapping, NodeKind::BlockMapping)?;
        let indent = match style {
            MappingEntryStyle::Inherit => self.node_indent(mapping_node),
            MappingEntryStyle::Indent(indent) => indent,
        };
        let insertion_offset = self.mapping_insertion_offset(mapping_node);
        let needs_leading_break =
            insertion_offset == self.source.len() && !self.source_ends_with_line_break();
        let preserve_paragraph_break = comment.is_some()
            && insertion_offset == self.source.len()
            && self.mapping_has_blank_line(mapping_node);
        let replacement = self.format_mapping_entry_replacement(
            indent,
            key,
            value,
            comment,
            needs_leading_break,
            preserve_paragraph_break,
        )?;

        self.queue_edit(Span::empty_from_usize(insertion_offset), replacement)
    }

    /// Queues insertion of a typed YAML value under `key` in a block mapping.
    ///
    /// # Errors
    ///
    /// Returns an error when `mapping` is not a block mapping, the value cannot
    /// be formatted as a block YAML fragment, or the insertion conflicts with an
    /// existing pending edit.
    pub fn insert_mapping_value_with_comment<T>(
        &mut self,
        mapping: NodeId,
        key: &str,
        value: &T,
        style: MappingEntryStyle,
        comment: Option<&str>,
    ) -> Result<(), YamlError>
    where
        T: ToYamlFragment,
    {
        if matches!(
            self.semantic_kind(mapping),
            Some(SemanticKind::Mapping {
                style: CollectionStyle::Flow
            })
        ) {
            let fragment = self.typed_value_fragment(value)?;
            return self
                .queue_mapping_insert(mapping, key, &fragment)
                .map_err(YamlEditError::into_yaml_error);
        }
        let mapping_node = self.expect_node_kind(mapping, NodeKind::BlockMapping)?;
        let indent = match style {
            MappingEntryStyle::Inherit => self.node_indent(mapping_node),
            MappingEntryStyle::Indent(indent) => indent,
        };
        let insertion_offset = self.mapping_insertion_offset(mapping_node);
        let needs_leading_break =
            insertion_offset == self.source.len() && !self.source_ends_with_line_break();
        let preserve_paragraph_break = comment.is_some()
            && insertion_offset == self.source.len()
            && self.mapping_has_blank_line(mapping_node);
        let replacement = self.format_mapping_value_replacement(
            indent,
            key,
            value,
            comment,
            needs_leading_break,
            preserve_paragraph_break,
        )?;

        self.queue_edit(Span::empty_from_usize(insertion_offset), replacement)
    }

    fn typed_value_fragment<T>(&self, value: &T) -> Result<YamlFragment, YamlError>
    where
        T: ToYamlFragment,
    {
        let yaml = value.to_yaml_fragment(0, self.preferred_line_ending())?;
        YamlFragment::parse(&yaml).map_err(|error| {
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Emitter,
                    format!("typed YAML fragment is invalid: {error}"),
                    Span::empty(0),
                )
                .with_expected("one valid YAML value"),
            )
        })
    }

    /// Queues insertion of a typed YAML value according to declaration order.
    ///
    /// # Errors
    ///
    /// Returns an error when mapping lookup fails or the selected insertion
    /// cannot be formatted or queued.
    pub fn insert_mapping_value_ordered_with_comment<T>(
        &mut self,
        mapping: NodeId,
        key: &str,
        value: &T,
        style: MappingEntryStyle,
        comment: Option<&str>,
        ordered_keys: &[&str],
    ) -> Result<(), YamlError>
    where
        T: ToYamlFragment,
    {
        let mut next_entry = None;
        if let Some(position) = ordered_keys.iter().position(|ordered| *ordered == key) {
            for later_key in &ordered_keys[position + 1..] {
                if let Some(entry) = self.get_mapping_entry(mapping, later_key)? {
                    next_entry = Some(entry);
                    break;
                }
            }
        }

        if let Some(next_entry) = next_entry {
            self.insert_mapping_value_before_with_comment(next_entry, key, value, style, comment)
        } else {
            self.insert_mapping_value_with_comment(mapping, key, value, style, comment)
        }
    }

    /// Queues insertion of a plain `key: value` entry before `before_entry`.
    ///
    /// # Errors
    ///
    /// Returns an error when `before_entry` is not a mapping entry, `key`,
    /// `value`, or `comment` cannot be emitted as valid YAML text, or the
    /// insertion conflicts with another pending edit.
    pub fn insert_mapping_entry_before_with_comment(
        &mut self,
        before_entry: NodeId,
        key: &str,
        value: &str,
        style: MappingEntryStyle,
        comment: Option<&str>,
    ) -> Result<(), YamlError> {
        let before_node = self.expect_node_kind(before_entry, NodeKind::MappingEntry)?;
        let indent = match style {
            MappingEntryStyle::Inherit => self.node_indent(before_node),
            MappingEntryStyle::Indent(indent) => indent,
        };
        let insertion_offset = self.line_start_for_offset(before_node.span.start as usize);
        let replacement =
            self.format_mapping_entry_replacement(indent, key, value, comment, false, false)?;

        self.queue_edit(Span::empty_from_usize(insertion_offset), replacement)
    }

    /// Queues insertion of a typed YAML value before an existing mapping entry.
    ///
    /// # Errors
    ///
    /// Returns an error when the insertion target is invalid, the value cannot
    /// be formatted, or the insertion conflicts with an existing pending edit.
    pub fn insert_mapping_value_before_with_comment<T>(
        &mut self,
        before_entry: NodeId,
        key: &str,
        value: &T,
        style: MappingEntryStyle,
        comment: Option<&str>,
    ) -> Result<(), YamlError>
    where
        T: ToYamlFragment,
    {
        let before_node = self.expect_node_kind(before_entry, NodeKind::MappingEntry)?;
        let mapping = before_node.parent().ok_or_else(|| {
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    "mapping entry has no parent mapping",
                    before_node.span,
                )
                .with_expected("a mapping parent"),
            )
        })?;
        if matches!(
            self.semantic_kind(mapping),
            Some(SemanticKind::Mapping {
                style: CollectionStyle::Flow
            })
        ) {
            let fragment = self.typed_value_fragment(value)?;
            return self
                .queue_mapping_insert_before(mapping, before_entry, key, &fragment)
                .map_err(YamlEditError::into_yaml_error);
        }
        let indent = match style {
            MappingEntryStyle::Inherit => self.node_indent(before_node),
            MappingEntryStyle::Indent(indent) => indent,
        };
        let insertion_offset = self.line_start_for_offset(before_node.span.start as usize);
        let replacement =
            self.format_mapping_value_replacement(indent, key, value, comment, false, false)?;

        self.queue_edit(Span::empty_from_usize(insertion_offset), replacement)
    }

    /// Queues insertion according to a declaration-order key list.
    ///
    /// If a later key from `ordered_keys` already exists in `mapping`, the new
    /// entry is inserted before that entry. Otherwise this falls back to append
    /// insertion. This is the primitive behind `insert_order = "struct"`.
    ///
    /// # Errors
    ///
    /// Returns an error when mapping lookup fails, the selected insertion target
    /// has the wrong node kind, inserted text is invalid YAML, or the queued edit
    /// conflicts with another pending edit.
    pub fn insert_mapping_entry_ordered_with_comment(
        &mut self,
        mapping: NodeId,
        key: &str,
        value: &str,
        style: MappingEntryStyle,
        comment: Option<&str>,
        ordered_keys: &[&str],
    ) -> Result<(), YamlError> {
        let mut next_entry = None;
        if let Some(position) = ordered_keys.iter().position(|ordered| *ordered == key) {
            for later_key in &ordered_keys[position + 1..] {
                if let Some(entry) = self.get_mapping_entry(mapping, later_key)? {
                    next_entry = Some(entry);
                    break;
                }
            }
        }

        if let Some(next_entry) = next_entry {
            self.insert_mapping_entry_before_with_comment(next_entry, key, value, style, comment)
        } else {
            self.insert_mapping_entry_with_comment(mapping, key, value, style, comment)
        }
    }

    /// Queues removal of the mapping entry with `key` from `mapping` when it exists.
    ///
    /// The removal is line-wise, so comments and fields outside the selected entry
    /// remain byte-for-byte unchanged. Missing keys are a no-op.
    ///
    /// # Errors
    ///
    /// Returns an error when mapping lookup fails, the selected entry cannot be
    /// removed, or the removal overlaps an existing pending edit.
    pub fn remove_mapping_entry(&mut self, mapping: NodeId, key: &str) -> Result<(), YamlError> {
        let Some(entry) = self.get_mapping_entry(mapping, key)? else {
            return Ok(());
        };
        self.remove_collection_entries(mapping, &[entry])
    }

    /// Queues line-wise removal edits for mapping entries whose keys are not allowed.
    ///
    /// This is the patch-emitter primitive used by typed overlays that choose to
    /// prune unknown fields. It preserves the order and bytes of retained entries.
    ///
    /// # Errors
    ///
    /// Returns an error when `mapping` is not a block mapping, a retained entry
    /// cannot be inspected as a scalar key, or a removal edit conflicts with
    /// another pending edit.
    pub fn retain_mapping_entries(
        &mut self,
        mapping: NodeId,
        allowed_keys: &[&str],
    ) -> Result<(), YamlError> {
        let mapping_node = self.expect_node(mapping)?;
        if !matches!(
            mapping_node.kind,
            NodeKind::BlockMapping | NodeKind::FlowMapping
        ) {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    format!("expected mapping, found {:?}", mapping_node.kind),
                    mapping_node.span,
                )
                .with_expected("BlockMapping or FlowMapping"),
            ));
        }
        let mut removals = Vec::new();

        for entry in self.children(mapping) {
            let entry_node = self.expect_node(entry)?;
            if entry_node.kind != NodeKind::MappingEntry {
                continue;
            }

            let Some(key_node) = self.children(entry).next() else {
                continue;
            };
            let key = self.scalar_value(key_node)?;
            if !allowed_keys.contains(&key.as_ref()) {
                removals.push(entry);
            }
        }

        self.remove_collection_entries(mapping, &removals)
    }

    pub(crate) fn remove_collection_entries(
        &mut self,
        collection: NodeId,
        removals: &[NodeId],
    ) -> Result<(), YamlError> {
        if removals.is_empty() {
            return Ok(());
        }
        let Some(style) = (match self.semantic_kind(collection) {
            Some(SemanticKind::Mapping { style } | SemanticKind::Sequence { style }) => Some(style),
            _ => None,
        }) else {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    "collection entry removal target is not a mapping or sequence",
                    self.expect_node(collection)?.span,
                )
                .with_expected("a mapping or sequence"),
            ));
        };
        if style == CollectionStyle::Block {
            for entry in removals {
                self.remove_node(*entry)?;
            }
            return Ok(());
        }

        let entries = self
            .children(collection)
            .filter(|node| {
                self.node(*node).is_some_and(|node| {
                    matches!(node.kind, NodeKind::MappingEntry | NodeKind::SequenceEntry)
                })
            })
            .collect::<Vec<_>>();
        if removals.len() == entries.len() {
            let collection_node = self.expect_node(collection)?;
            let delimiter = match collection_node.kind {
                NodeKind::FlowMapping => '}',
                NodeKind::FlowSequence => ']',
                _ => unreachable!("flow semantic collection must have a flow CST node"),
            };
            if let Some(relative) = self.source.slice(collection_node.span).rfind(delimiter) {
                let close = Span::offset_from_usize(collection_node.span.start, relative);
                if let Some(edit) = self
                    .edits
                    .iter_mut()
                    .find(|edit| edit.span == Span::empty(close))
                    && let Some(replacement) = edit.replacement.strip_prefix(", ")
                {
                    edit.replacement = replacement.to_owned();
                }
            }
        }
        let mut index = 0;
        while index < entries.len() {
            if !removals.contains(&entries[index]) {
                index += 1;
                continue;
            }
            let start = index;
            while index < entries.len() && removals.contains(&entries[index]) {
                index += 1;
            }
            let end = index;
            let first = self.expect_node(entries[start])?.span;
            let last = self.expect_node(entries[end - 1])?.span;
            let span = if let Some(next) = entries.get(end).copied() {
                Span::new(first.start, self.expect_node(next)?.span.start)
            } else if start > 0 {
                Span::new(self.expect_node(entries[start - 1])?.span.end, last.end)
            } else {
                Span::new(first.start, last.end)
            };
            self.queue_edit(span, String::new())?;
        }
        Ok(())
    }

    /// Queues removal of `node` from the rendered document.
    ///
    /// Mapping and sequence entries are removed line-wise, including their line
    /// break when one is present. Other nodes use their exact source span.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` is unknown or the removal overlaps an
    /// existing pending edit.
    pub fn remove_node(&mut self, node: NodeId) -> Result<(), YamlError> {
        let node = self.expect_node(node)?;
        let span = if matches!(node.kind, NodeKind::MappingEntry | NodeKind::SequenceEntry) {
            self.line_span_including_break(node.span)
        } else {
            node.span
        };
        self.queue_edit(span, String::new())
    }

    pub(crate) fn scalar_replacement_target(
        &self,
        node: NodeId,
    ) -> Result<(Span, ScalarStyle), YamlError> {
        let node = self.expect_node_kind(node, NodeKind::Scalar)?;
        let text = self.source.slice(node.span);
        let properties = parse_node_properties(text, node.span)?;
        let value_text = &text[properties.value_start..];
        let value_start = Span::offset_from_usize(node.span.start, properties.value_start);

        if value_text.starts_with('"') {
            let end = double_quoted_scalar_end(value_text).ok_or_else(|| {
                YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Emitter,
                        "could not find the end of the double-quoted scalar",
                        node.span,
                    )
                    .with_expected("a closed double-quoted scalar"),
                )
            })?;
            return Ok((
                Span::new(value_start, Span::offset_from_usize(value_start, end)),
                ScalarStyle::DoubleQuoted,
            ));
        }

        if value_text.starts_with('\'') {
            let end = single_quoted_scalar_end(value_text).ok_or_else(|| {
                YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Emitter,
                        "could not find the end of the single-quoted scalar",
                        node.span,
                    )
                    .with_expected("a closed single-quoted scalar"),
                )
            })?;
            return Ok((
                Span::new(value_start, Span::offset_from_usize(value_start, end)),
                ScalarStyle::SingleQuoted,
            ));
        }

        let end = plain_scalar_end(value_text);
        if end == 0 {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Emitter,
                    "could not find plain scalar text to replace",
                    node.span,
                )
                .with_expected("plain scalar text"),
            ));
        }

        Ok((
            Span::new(value_start, Span::offset_from_usize(value_start, end)),
            ScalarStyle::Plain,
        ))
    }

    fn directive_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.root()
            .into_iter()
            .flat_map(|root| self.children(root))
            .filter(|node| {
                self.node(*node)
                    .is_some_and(|node| node.kind == NodeKind::Directive)
            })
    }

    fn parse_directive_node(&self, node: NodeId) -> Result<ParsedDirective, YamlError> {
        let node_ref = self.expect_node_kind(node, NodeKind::Directive)?;
        let body = strip_inline_comment(self.source.slice(node_ref.span)).trim();
        let mut parts = body.split_whitespace();
        let Some(name) = parts.next() else {
            return Err(directive_emit_error(
                "directive is missing a name",
                node_ref.span,
                "%YAML, %TAG, or reserved directive syntax",
            )
            .with_position_from(&self.source));
        };

        Ok(match name {
            "%YAML" => ParsedDirective::Yaml(YamlDirective {
                version: parts.next().unwrap_or_default().to_owned(),
                node,
            }),
            "%TAG" => ParsedDirective::Tag(TagDirective {
                handle: parts.next().unwrap_or_default().to_owned(),
                prefix: parts.next().unwrap_or_default().to_owned(),
                node,
            }),
            _ => ParsedDirective::Reserved(ReservedDirective {
                name: name.to_owned(),
                parameters: parts.map(str::to_owned).collect(),
                node,
            }),
        })
    }

    fn directive_content_span(&self, node: NodeId) -> Result<Span, YamlError> {
        let node = self.expect_node_kind(node, NodeKind::Directive)?;
        let text = self.source.slice(node.span);
        let end = strip_inline_comment(text)
            .trim_end_matches([' ', '\t'])
            .len();
        Ok(Span::new(
            node.span.start,
            Span::offset_from_usize(node.span.start, end),
        ))
    }

    fn insert_directive_line(&mut self, replacement: String) -> Result<(), YamlError> {
        let insertion_offset = self.directive_insertion_offset();
        let mut line = replacement;
        line.push_str(self.preferred_line_ending());
        self.queue_edit(Span::empty_from_usize(insertion_offset), line)
    }

    fn remove_directive_node(&mut self, node: NodeId) -> Result<(), YamlError> {
        let node = self.expect_node_kind(node, NodeKind::Directive)?;
        self.queue_edit(self.line_span_including_break(node.span), String::new())
    }

    fn directive_insertion_offset(&self) -> usize {
        if let Some(last_directive) = self
            .directive_nodes()
            .filter_map(|node| self.node(node))
            .max_by_key(|node| node.span.start)
        {
            return self.line_span_including_break(last_directive.span).end as usize;
        }

        self.root()
            .and_then(|root| self.children(root).next())
            .and_then(|node| self.node(node))
            .map_or(0, |node| {
                self.line_start_for_offset(node.span.start as usize)
            })
    }

    pub(crate) fn expect_node(&self, node: NodeId) -> Result<&Node, YamlError> {
        self.node(node).ok_or_else(|| {
            YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                format!("unknown node id {}", node.0),
                Span::empty_from_usize(self.source.len()),
            ))
        })
    }

    pub(crate) fn expect_node_kind(
        &self,
        node: NodeId,
        expected: NodeKind,
    ) -> Result<&Node, YamlError> {
        let actual = self.expect_node(node)?;
        if actual.kind == expected {
            Ok(actual)
        } else {
            Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    format!("expected {expected:?}, found {:?}", actual.kind),
                    actual.span,
                )
                .with_expected(format!("{expected:?}")),
            )
            .with_position_from(&self.source))
        }
    }

    pub(crate) fn containing_entry(&self, value: NodeId) -> Option<NodeId> {
        self.node(value).and_then(Node::parent).filter(|parent| {
            self.node(*parent).is_some_and(|node| {
                matches!(node.kind, NodeKind::MappingEntry | NodeKind::SequenceEntry)
            })
        })
    }

    fn mapping_has_blank_line(&self, mapping: &Node) -> bool {
        let start = self.line_start_for_offset(mapping.span.start as usize);
        let end = mapping.span.end as usize;
        let text = &self.source.as_str()[start..end];
        text.contains("\n\n") || text.contains("\r\n\r\n")
    }

    fn format_mapping_entry_replacement(
        &self,
        indent: usize,
        key: &str,
        value: &str,
        comment: Option<&str>,
        needs_leading_break: bool,
        preserve_paragraph_break: bool,
    ) -> Result<String, YamlError> {
        validate_plain_mapping_fragment(key, "mapping key")?;
        validate_plain_mapping_fragment(value, "mapping value")?;
        if let Some(comment) = comment {
            validate_yaml_chars(comment)?;
        }

        let line_ending = self.preferred_line_ending();
        let indent_text = " ".repeat(indent);
        let mut replacement = String::new();
        if needs_leading_break {
            replacement.push_str(line_ending);
        }
        if preserve_paragraph_break {
            replacement.push_str(line_ending);
        }
        if let Some(comment) = comment {
            for line in comment.lines() {
                replacement.push_str(&indent_text);
                replacement.push('#');
                if !line.is_empty() {
                    replacement.push(' ');
                    replacement.push_str(line.trim());
                }
                replacement.push_str(line_ending);
            }
        }
        replacement.push_str(&indent_text);
        replacement.push_str(key);
        replacement.push_str(": ");
        replacement.push_str(value);
        replacement.push_str(line_ending);
        Ok(replacement)
    }

    fn format_mapping_value_replacement<T>(
        &self,
        indent: usize,
        key: &str,
        value: &T,
        comment: Option<&str>,
        needs_leading_break: bool,
        preserve_paragraph_break: bool,
    ) -> Result<String, YamlError>
    where
        T: ToYamlFragment,
    {
        validate_yaml_chars(key)?;
        if let Some(comment) = comment {
            validate_yaml_chars(comment)?;
        }

        let line_ending = self.preferred_line_ending();
        let indent_text = " ".repeat(indent);
        let child_indent = indent + 2;
        let fragment = value.to_yaml_fragment(child_indent, line_ending)?;
        let mut replacement = String::new();
        if needs_leading_break {
            replacement.push_str(line_ending);
        }
        if preserve_paragraph_break {
            replacement.push_str(line_ending);
        }
        if let Some(comment) = comment {
            for line in comment.lines() {
                replacement.push_str(&indent_text);
                replacement.push('#');
                if !line.is_empty() {
                    replacement.push(' ');
                    replacement.push_str(line.trim());
                }
                replacement.push_str(line_ending);
            }
        }
        replacement.push_str(&indent_text);
        replacement.push_str(&crate::edit::emit_string_key(key));
        if fragment.contains('\n') || fragment.starts_with(' ') {
            replacement.push(':');
            replacement.push_str(line_ending);
            replacement.push_str(&fragment);
        } else {
            replacement.push_str(": ");
            replacement.push_str(&fragment);
        }
        replacement.push_str(line_ending);
        Ok(replacement)
    }

    pub(crate) fn node_indent(&self, node: &Node) -> usize {
        let line_start = self.line_start_for_offset(node.span.start as usize);
        self.source.as_str()[line_start..node.span.start as usize]
            .bytes()
            .filter(|byte| *byte == b' ')
            .count()
    }

    fn line_start_for_offset(&self, offset: usize) -> usize {
        let offset = Span::usize_to_u32(offset);
        match self.source.line_starts().binary_search(&offset) {
            Ok(index) => self.source.line_starts()[index] as usize,
            Err(index) => self.source.line_starts()[index.saturating_sub(1)] as usize,
        }
    }

    pub(crate) fn find_nested_collection_after(
        &self,
        entry: &Node,
        parent_indent: usize,
    ) -> Option<NodeId> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                matches!(node.kind, NodeKind::BlockMapping | NodeKind::BlockSequence)
                    && node.span.start >= entry.span.end
                    && self.node_indent(node) > parent_indent
            })
            .min_by_key(|(_, node)| node.span.start)
            .map(|(index, _)| NodeId::from_usize(index))
    }

    pub(crate) fn block_scalar_content_indent(&self, scalar: &Node) -> Option<usize> {
        let text = self.source.slice(scalar.span);
        let header_end = text.find(['\r', '\n'])?;
        let mut rest = &text[header_end..];
        while let Some(stripped) = rest.strip_prefix('\r').or_else(|| rest.strip_prefix('\n')) {
            rest = stripped;
        }
        for line in rest.lines() {
            if line.trim().is_empty() {
                continue;
            }
            return Some(line.bytes().take_while(|byte| *byte == b' ').count());
        }
        None
    }

    pub(crate) fn queue_edit(&mut self, span: Span, replacement: String) -> Result<(), YamlError> {
        self.source.try_slice(span)?;
        validate_yaml_chars(&replacement)?;

        if span.is_empty()
            && let Some(existing) = self
                .edits
                .iter_mut()
                .find(|edit| edit.span.is_empty() && edit.span.start == span.start)
        {
            existing.replacement.push_str(&replacement);
            return Ok(());
        }

        if let Some(existing) = self
            .edits
            .iter()
            .find(|edit| edits_conflict(edit.span, span))
        {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Emitter,
                    "edit overlaps an existing pending edit",
                    span,
                )
                .with_note(format!(
                    "existing edit covers bytes {}..{}",
                    existing.span.start, existing.span.end
                )),
            )
            .with_position_from(&self.source));
        }

        self.edits.push(Edit { span, replacement });
        Ok(())
    }

    pub(crate) fn mapping_insertion_offset(&self, mapping: &Node) -> usize {
        node_link(mapping.last_child)
            .and_then(|child| self.node(child))
            .map_or(mapping.span.end as usize, |last_child| {
                self.line_span_including_break(last_child.span).end as usize
            })
    }

    pub(crate) fn sequence_insertion_offset(&self, sequence: &Node) -> usize {
        node_link(sequence.last_child)
            .and_then(|child| self.node(child))
            .map_or(sequence.span.end as usize, |last_child| {
                self.line_span_including_break(last_child.span).end as usize
            })
    }

    fn line_span_including_break(&self, span: Span) -> Span {
        let start = self.line_start_for_offset(span.start as usize);
        let mut end = span.end as usize;
        let bytes = self.source.as_str().as_bytes();

        if end < bytes.len() {
            if bytes[end] == b'\r' {
                end += 1;
                if end < bytes.len() && bytes[end] == b'\n' {
                    end += 1;
                }
            } else if bytes[end] == b'\n' {
                end += 1;
            }
        }

        Span::from_usize(start, end)
    }

    pub(crate) fn preferred_line_ending(&self) -> &str {
        let bytes = self.source.as_str().as_bytes();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\r' {
                return if bytes.get(index + 1) == Some(&b'\n') {
                    "\r\n"
                } else {
                    "\r"
                };
            }
            if *byte == b'\n' {
                return if index > 0 && bytes[index - 1] == b'\r' {
                    "\r\n"
                } else {
                    "\n"
                };
            }
        }
        "\n"
    }

    fn source_ends_with_line_break(&self) -> bool {
        self.source
            .as_str()
            .as_bytes()
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    }

    fn document_append_prefix(&self, line_ending: &str) -> String {
        if self.source.as_str().is_empty() || self.source_ends_with_line_break() {
            String::new()
        } else {
            line_ending.to_owned()
        }
    }
}
impl fmt::Display for YamlDoc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.edits.is_empty() {
            return formatter.write_str(self.source.as_str());
        }

        let mut output = self.source.as_str().to_owned();
        let mut edits = self.edits.clone();
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));

        for edit in edits {
            output.replace_range(
                edit.span.start as usize..edit.span.end as usize,
                &edit.replacement,
            );
        }

        formatter.write_str(&output)
    }
}
