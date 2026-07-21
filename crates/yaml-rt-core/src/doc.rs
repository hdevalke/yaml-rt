use std::fmt;

use crate::{
    CollectionStyle, CollectionTarget, Diagnostic, DiagnosticKind, FromYamlDoc, GraphKind,
    GraphNode, GraphNodeId, Node, NodeId, NodeKind, Parser, ScalarStyle, SemanticGraph, Source,
    Span, ToYamlDoc, ToYamlFragment, Token, YamlError, YamlEvent, compose_graph,
    decode_scalar_value, directive_emit_error, double_quoted_scalar_end, edits_conflict,
    events_to_test_string, format_scalar_value, lex, next_line_content_start,
    parse_node_properties, plain_scalar_end, single_quoted_scalar_end, strip_inline_comment,
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

/// Formatting controls for inserting an MVP block mapping entry.
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
    /// Lossless token stream in source order.
    pub(crate) tokens: Vec<Token>,
    /// CST and semantic nodes. The CST remains the source of truth.
    pub(crate) nodes: Vec<Node>,
    /// Semantic YAML event stream produced by the parser.
    pub(crate) events: Vec<YamlEvent>,
    /// CST-linked semantic graph composed from parser events.
    pub(crate) graph: SemanticGraph,
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
    /// Returns an error when source validation, lexing, CST parsing, or semantic
    /// graph composition fails.
    pub fn parse(input: &str) -> Result<Self, YamlError> {
        let source = Source::new(input.to_owned())?;
        let tokens = lex(&source).map_err(|error| error.with_position_from(&source))?;
        let parsed = Parser::new(&source, &tokens)
            .parse()
            .map_err(|error| error.with_position_from(&source))?;
        let graph = compose_graph(&parsed.events, &parsed.nodes)
            .map_err(|error| error.with_position_from(&source))?;

        Ok(Self {
            source,
            tokens,
            nodes: parsed.nodes,
            events: parsed.events,
            graph,
            edits: Vec::new(),
        })
    }

    /// Applies pending edits, reparses the rendered YAML, and clears the edit queue.
    ///
    /// `YamlDoc::to_string` only previews the original source with queued byte
    /// patches applied. It does not prove that the patched stream is still
    /// valid YAML, because low-level edit APIs can replace arbitrary node spans
    /// or insert conservative-but-raw fragments. Reparse on commit is the point
    /// where the document regains a validated CST and semantic graph.
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
        Ok(self.tokens.clone())
    }

    /// Returns the root node identifier when present.
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        (!self.nodes.is_empty()).then_some(NodeId(0))
    }

    /// Returns the semantic event stream produced by the parser.
    #[must_use]
    pub fn events(&self) -> &[YamlEvent] {
        &self.events
    }

    /// Renders semantic events in the YAML Test Suite `test.event` format.
    #[must_use]
    pub fn events_to_test_string(&self) -> String {
        events_to_test_string(&self.events)
    }

    /// Returns the CST-linked semantic graph.
    #[must_use]
    pub fn graph(&self) -> &SemanticGraph {
        &self.graph
    }

    /// Returns the number of documents in this YAML stream.
    #[must_use]
    pub fn document_count(&self) -> usize {
        self.graph.documents.len()
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

    /// Returns a semantic graph node by identifier.
    #[must_use]
    pub fn graph_node(&self, node: GraphNodeId) -> Option<&GraphNode> {
        self.graph.nodes.get(node.0 as usize)
    }

    /// Returns the CST node linked to `node`, when one exists.
    #[must_use]
    pub fn graph_node_cst(&self, node: GraphNodeId) -> Option<NodeId> {
        self.graph_node(node).and_then(|node| node.cst)
    }

    /// Returns a node by identifier.
    #[must_use]
    pub fn node(&self, node: NodeId) -> Option<&Node> {
        self.nodes.get(node.0 as usize)
    }

    /// Returns the root mapping graph node.
    ///
    /// # Errors
    ///
    /// Returns an error when the document has no semantic root, the root is not
    /// a document, or the document has no root block mapping.
    pub fn root_graph_mapping(&self) -> Result<GraphNodeId, YamlError> {
        self.document_root_mapping_graph(0)
    }

    /// Returns a document graph node by zero-based document index.
    ///
    /// # Errors
    ///
    /// Returns an error when `index` does not identify an existing document.
    pub fn document_graph(&self, index: usize) -> Result<GraphNodeId, YamlError> {
        self.graph
            .documents
            .get(index)
            .copied()
            .ok_or_else(|| self.document_index_error(index))
    }

    /// Returns the first root-level block mapping in the document.
    ///
    /// # Errors
    ///
    /// Returns an error when no root block mapping exists or when the semantic
    /// root mapping is not linked back to a CST node.
    pub fn root_mapping(&self) -> Result<NodeId, YamlError> {
        self.document_root_mapping(0)
    }

    /// Returns the root-level block mapping in a selected document.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected document does not exist, has no block
    /// mapping root, or the root mapping is not linked back to the CST.
    pub fn document_root_mapping(&self, index: usize) -> Result<NodeId, YamlError> {
        let mapping = self.document_root_mapping_graph(index)?;
        self.graph_node_cst(mapping).ok_or_else(|| {
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    "root mapping is not linked to the CST",
                    self.graph_node(mapping)
                        .map_or(Span::empty(0), |node| node.span),
                )
                .with_expected("a CST-backed block mapping"),
            )
        })
    }

    /// Reads a typed overlay from a selected document.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected document is missing, does not contain
    /// a root block mapping, or the typed overlay cannot be read.
    pub fn read_document<T>(&self, index: usize) -> Result<T, YamlError>
    where
        T: FromYamlDoc,
    {
        let root = self.document_root_mapping(index)?;
        let nested = self.rerooted_at_mapping(root)?;
        T::from_yaml_doc(&nested)
    }

    /// Writes a typed overlay to a selected document.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected document is missing, does not contain
    /// a root block mapping, or the typed overlay cannot be written.
    pub fn write_document<T>(&mut self, index: usize, value: &T) -> Result<(), YamlError>
    where
        T: ToYamlDoc,
    {
        let root = match self.document_root_mapping(index) {
            Ok(root) => root,
            Err(error) => {
                if let Some(root) = self.empty_flow_mapping_document_root(index)? {
                    let replacement = value.to_yaml_fragment(0, self.preferred_line_ending())?;
                    return self.replace_node_text(root, replacement);
                }
                return Err(error);
            }
        };
        let mut nested = self.rerooted_at_mapping(root)?;
        value.apply_to_yaml_doc(&mut nested)?;
        self.queue_edits_from(&nested)
    }

    /// Looks up a mapping entry by key inside `mapping`.
    ///
    /// # Errors
    ///
    /// Returns an error when the semantic graph contains an unknown graph node
    /// while resolving the mapping entry.
    pub fn get_mapping_entry(
        &self,
        mapping: NodeId,
        key: &str,
    ) -> Result<Option<NodeId>, YamlError> {
        let Some(mapping_graph) = self.graph_for_cst(mapping) else {
            return Ok(None);
        };
        let Some((key_graph, _)) = self.get_graph_mapping_entry(mapping_graph, key)? else {
            return Ok(None);
        };
        let Some(key_node) = self.graph_node_cst(key_graph) else {
            return Ok(None);
        };
        Ok(self.containing_entry(key_node))
    }

    /// Looks up a mapping value by key inside `mapping`.
    ///
    /// # Errors
    ///
    /// Returns an error when the semantic graph contains an unknown graph node
    /// while resolving the mapping value.
    pub fn get_mapping_value(
        &self,
        mapping: NodeId,
        key: &str,
    ) -> Result<Option<NodeId>, YamlError> {
        let Some(mapping_graph) = self.graph_for_cst(mapping) else {
            return Ok(None);
        };
        let Some((_, value_graph)) = self.get_graph_mapping_entry(mapping_graph, key)? else {
            return Ok(None);
        };
        Ok(self.graph_node_cst(value_graph))
    }

    /// Looks up a nested path of mapping keys and returns the semantic graph node.
    ///
    /// # Errors
    ///
    /// Returns an error when the document has no root mapping or when graph
    /// traversal encounters an unknown graph node.
    pub fn get_graph_path(&self, path: &[&str]) -> Result<Option<GraphNodeId>, YamlError> {
        self.get_graph_path_in_document(0, path)
    }

    /// Looks up a nested path of mapping keys in a selected document and returns
    /// the semantic graph node.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected document does not exist, has no root
    /// mapping, or graph traversal encounters an unknown graph node.
    pub fn get_graph_path_in_document(
        &self,
        index: usize,
        path: &[&str],
    ) -> Result<Option<GraphNodeId>, YamlError> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(None);
        };

        let Some((_, mut current)) =
            self.get_graph_mapping_entry(self.document_root_mapping_graph(index)?, first)?
        else {
            return Ok(None);
        };

        for segment in rest {
            current = match self.get_graph_mapping_entry(current, segment)? {
                Some((_, value)) => value,
                None => return Ok(None),
            };
        }

        Ok(Some(current))
    }

    /// Looks up a nested path of mapping keys.
    ///
    /// # Errors
    ///
    /// Returns an error when semantic path lookup fails while resolving the
    /// graph path.
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
        Ok(self
            .get_graph_path_in_document(index, path)?
            .and_then(|node| self.graph_node_cst(node)))
    }

    fn document_root_mapping_graph(&self, index: usize) -> Result<GraphNodeId, YamlError> {
        let root = self.document_graph(index)?;
        self.document_mapping_child(root)
    }

    fn document_mapping_child(&self, document: GraphNodeId) -> Result<GraphNodeId, YamlError> {
        let root = self.expect_graph_node(document)?;
        let GraphKind::Document { children } = &root.kind else {
            return Err(YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                "semantic root is not a document",
                root.span,
            )));
        };
        children
            .iter()
            .copied()
            .find(|child| {
                self.graph_node(*child).is_some_and(|node| {
                    matches!(node.kind, GraphKind::Mapping { .. })
                        && node
                            .cst
                            .and_then(|cst| self.node(cst))
                            .is_some_and(|node| node.kind == NodeKind::BlockMapping)
                })
            })
            .ok_or_else(|| {
                YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Semantic,
                        "document does not contain a root mapping",
                        root.span,
                    )
                    .with_expected("a mapping graph node"),
                )
            })
    }

    fn empty_flow_mapping_document_root(&self, index: usize) -> Result<Option<NodeId>, YamlError> {
        let root = self.document_graph(index)?;
        let root = self.expect_graph_node(root)?;
        let GraphKind::Document { children } = &root.kind else {
            return Ok(None);
        };
        let Some(child) = children.first().copied() else {
            return Ok(None);
        };
        let Some(graph) = self.graph_node(child) else {
            return Ok(None);
        };
        let GraphKind::Mapping { style, entries, .. } = &graph.kind else {
            return Ok(None);
        };
        if *style != CollectionStyle::Flow || !entries.is_empty() {
            return Ok(None);
        }
        Ok(graph.cst.filter(|cst| {
            self.node(*cst)
                .is_some_and(|node| node.kind == NodeKind::FlowMapping)
        }))
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

    fn get_graph_mapping_entry(
        &self,
        mapping: GraphNodeId,
        key: &str,
    ) -> Result<Option<(GraphNodeId, GraphNodeId)>, YamlError> {
        let mapping_node = self.expect_graph_node(mapping)?;
        let GraphKind::Mapping { entries, .. } = &mapping_node.kind else {
            return Ok(None);
        };

        for (key_node, value_node) in entries {
            let key_graph = self.expect_graph_node(*key_node)?;
            if let GraphKind::Scalar { value, .. } = &key_graph.kind
                && value == key
            {
                return Ok(Some((*key_node, *value_node)));
            }
        }

        Ok(None)
    }

    fn expect_graph_node(&self, node: GraphNodeId) -> Result<&GraphNode, YamlError> {
        self.graph_node(node).ok_or_else(|| {
            YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                format!("unknown graph node id {}", node.0),
                Span::empty_from_usize(self.source.len()),
            ))
        })
    }

    pub(crate) fn graph_for_cst(&self, cst: NodeId) -> Option<GraphNodeId> {
        self.graph
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.cst == Some(cst))
            .map(|(index, _)| GraphNodeId::from_usize(index))
    }

    fn graph_scalar_value(&self, node: NodeId) -> Option<&str> {
        let graph = self.graph_for_cst(node)?;
        let graph = self.graph_node(graph)?;
        match &graph.kind {
            GraphKind::Scalar { value, .. } => Some(value),
            _ => None,
        }
    }

    pub(crate) fn graph_sequence_items(&self, node: NodeId) -> Option<Vec<NodeId>> {
        let graph = self.graph_for_cst(node)?;
        let graph = self.graph_node(graph)?;
        let GraphKind::Sequence { items, .. } = &graph.kind else {
            return None;
        };
        Some(
            items
                .iter()
                .filter_map(|item| self.graph_node_cst(*item))
                .collect(),
        )
    }

    pub(crate) fn graph_mapping_entries(&self, node: NodeId) -> Option<Vec<(NodeId, NodeId)>> {
        let graph = self.graph_for_cst(node)?;
        let graph = self.graph_node(graph)?;
        let GraphKind::Mapping { entries, .. } = &graph.kind else {
            return None;
        };
        Some(
            entries
                .iter()
                .filter_map(|(key, value)| {
                    Some((self.graph_node_cst(*key)?, self.graph_node_cst(*value)?))
                })
                .collect(),
        )
    }

    pub(crate) fn rerooted_at_mapping(&self, mapping: NodeId) -> Result<Self, YamlError> {
        self.expect_node_kind(mapping, NodeKind::BlockMapping)?;
        let Some(mapping_graph) = self.graph_for_cst(mapping) else {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    "mapping is not linked to the semantic graph",
                    self.expect_node(mapping)?.span,
                )
                .with_expected("a graph-backed mapping"),
            )
            .with_position_from(&self.source));
        };
        let mut doc = self.clone();
        let root = doc.graph.root.ok_or_else(|| {
            YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                "document does not contain a semantic root",
                Span::empty(0),
            ))
        })?;
        let GraphKind::Document { children } = &mut doc.graph.nodes[root.as_usize()].kind else {
            return Err(YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                "semantic root is not a document",
                Span::empty(0),
            )));
        };
        *children = vec![mapping_graph];
        doc.graph.documents = vec![root];
        doc.edits.clear();
        Ok(doc)
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

    /// Returns the decoded value text for a scalar node in the MVP scalar subset.
    ///
    /// Plain scalars have trailing inline comments stripped, single-quoted
    /// scalars unescape doubled apostrophes, and double-quoted scalars unescape
    /// the common JSON/YAML escapes currently used by the typed overlay MVP.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` is unknown, is not a scalar node, has
    /// malformed node properties, or contains unsupported scalar escape syntax.
    pub fn scalar_value(&self, node: NodeId) -> Result<String, YamlError> {
        if let Some(value) = self.graph_scalar_value(node) {
            return Ok(value.to_owned());
        }

        let node = self.expect_node(node)?;
        if !matches!(
            node.kind,
            NodeKind::Scalar | NodeKind::LiteralScalar | NodeKind::FoldedScalar
        ) {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Semantic,
                    format!("expected scalar value, found {:?}", node.kind),
                    node.span,
                )
                .with_expected("Scalar, LiteralScalar, or FoldedScalar"),
            )
            .with_position_from(&self.source));
        }
        let text = self.source.slice(node.span);
        let properties = parse_node_properties(text, node.span)?;
        decode_scalar_value(&text[properties.value_start..])
    }

    /// Queues a scalar value replacement at `path` while preserving the existing
    /// scalar style where the MVP writer can do so safely.
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
    /// can inspect the pending minimal-diff output through [`YamlDoc::to_string`].
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
    /// This MVP writer intentionally accepts raw plain scalar text. Later scalar
    /// writers will own quoting and schema-aware formatting.
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
    /// insertion. This is the MVP primitive behind `insert_order = "struct"`.
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
        self.remove_node(entry)
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
        let mapping_node = self.expect_node_kind(mapping, NodeKind::BlockMapping)?;
        let mut removals = Vec::new();

        for entry in &mapping_node.children {
            let entry_node = self.expect_node(*entry)?;
            if entry_node.kind != NodeKind::MappingEntry {
                continue;
            }

            let Some(key_node) = entry_node.children.first().copied() else {
                continue;
            };
            let key = self.scalar_text(key_node)?;
            if !allowed_keys.contains(&key) {
                removals.push(*entry);
            }
        }

        for entry in removals {
            self.remove_node(entry)?;
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

    pub(crate) fn collection_replacement_target(
        &self,
        node: NodeId,
    ) -> Result<CollectionTarget, YamlError> {
        let node = self.expect_node(node)?;
        let text = self.source.slice(node.span);
        let properties = parse_node_properties(text, node.span)?;
        let mut body_start = properties.value_start;

        if text[body_start..].starts_with(['\r', '\n']) {
            body_start = next_line_content_start(text, body_start);
        }

        let start = Span::offset_from_usize(node.span.start, body_start);
        Ok(CollectionTarget {
            span: Span::new(start, node.span.end),
            indent: self.line_indent_for_offset(start as usize),
        })
    }

    fn directive_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.root()
            .and_then(|root| self.node(root))
            .into_iter()
            .flat_map(|stream| stream.children.iter().copied())
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
            .and_then(|root| self.node(root))
            .and_then(|stream| stream.children.first().copied())
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
        self.nodes.iter().enumerate().find_map(|(index, node)| {
            matches!(node.kind, NodeKind::MappingEntry | NodeKind::SequenceEntry)
                .then_some(())
                .filter(|()| node.children.contains(&value))
                .map(|()| NodeId::from_usize(index))
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
        validate_plain_mapping_fragment(key, "mapping key")?;
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
        replacement.push_str(key);
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

    fn line_indent_for_offset(&self, offset: usize) -> usize {
        let line_start = self.line_start_for_offset(offset);
        self.source.as_str()[line_start..offset]
            .bytes()
            .filter(|byte| *byte == b' ')
            .count()
    }

    fn line_start_for_offset(&self, offset: usize) -> usize {
        match self.source.line_starts().binary_search(&offset) {
            Ok(index) => self.source.line_starts()[index],
            Err(index) => self.source.line_starts()[index.saturating_sub(1)],
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

    fn mapping_insertion_offset(&self, mapping: &Node) -> usize {
        mapping
            .children
            .last()
            .and_then(|child| self.node(*child))
            .map_or(mapping.span.end as usize, |last_child| {
                self.line_span_including_break(last_child.span).end as usize
            })
    }

    pub(crate) fn sequence_insertion_offset(&self, sequence: &Node) -> usize {
        sequence
            .children
            .last()
            .and_then(|child| self.node(*child))
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
