use std::collections::BTreeMap;

use crate::inline_vec::InlineVec;
use crate::source::{CachedScalarStyle, LineFacts};
use crate::syntax::{
    Children, NO_NODE, NO_SEMANTIC_NODE, NODE_SCALAR_DOUBLE_QUOTED as SCALAR_STYLE_DOUBLE_QUOTED,
    NODE_SCALAR_PLAIN as SCALAR_STYLE_PLAIN,
    NODE_SCALAR_SINGLE_QUOTED as SCALAR_STYLE_SINGLE_QUOTED,
    NODE_SCALAR_STYLE_MASK as SCALAR_STYLE_MASK,
    NODE_SCALAR_SYNTAX_VALIDATED as SCALAR_SYNTAX_VALIDATED, node_link,
};
use crate::{
    CollectionStyle, Diagnostic, DiagnosticKind, Node, NodeId, NodeKind, ParsedYaml,
    SemanticBuilder, SemanticKind, SemanticProperties, Source, Span, YamlError, YamlEvent,
    YamlEventKind, YamlScalarStyle, validate_yaml_chars,
};

const MAX_FLOW_COLLECTION_DEPTH: usize = 1024;
fn scalar_style_from_first_char(character: char) -> YamlScalarStyle {
    match character {
        '"' => YamlScalarStyle::DoubleQuoted,
        '\'' => YamlScalarStyle::SingleQuoted,
        _ => YamlScalarStyle::Plain,
    }
}

fn scalar_style_from_flags(flags: u8) -> Option<YamlScalarStyle> {
    match flags & SCALAR_STYLE_MASK {
        SCALAR_STYLE_PLAIN => Some(YamlScalarStyle::Plain),
        SCALAR_STYLE_SINGLE_QUOTED => Some(YamlScalarStyle::SingleQuoted),
        SCALAR_STYLE_DOUBLE_QUOTED => Some(YamlScalarStyle::DoubleQuoted),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
enum FlowFrameState {
    SequenceExpectItem,
    SequenceAwaitItem {
        entry: NodeId,
        entry_start: u32,
        explicit: bool,
    },
    SequenceAwaitValue {
        entry: NodeId,
        mapping: NodeId,
        mapping_entry: NodeId,
    },
    SequenceAfterItem,
    MappingExpectKey,
    MappingAwaitKey {
        entry: NodeId,
        entry_start: u32,
        explicit: bool,
    },
    MappingAwaitValue {
        entry: NodeId,
        entry_start: u32,
    },
    MappingAfterPair,
}

#[derive(Debug, Clone, Copy)]
struct FlowFrame {
    node: NodeId,
    close: char,
    state: FlowFrameState,
}

#[derive(Debug, Clone, Copy)]
enum FlowNode {
    Scalar(NodeId, usize),
    Collection(FlowFrame, usize),
}

struct FlowParseState<'text> {
    text: &'text str,
    absolute_start: usize,
    root: NodeId,
    frames: Vec<FlowFrame>,
    completed: Option<(NodeId, usize)>,
    position: usize,
}

impl<'text> FlowParseState<'text> {
    fn new(
        text: &'text str,
        absolute_start: usize,
        root: NodeId,
        open: char,
        mut frames: Vec<FlowFrame>,
    ) -> Self {
        frames.clear();
        frames.push(flow_frame(root, open));
        Self {
            text,
            absolute_start,
            root,
            frames,
            completed: None,
            position: open.len_utf8(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenEventCollection {
    Mapping,
    Sequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingNodeProperties {
    indent: u32,
    span_start: u32,
    properties: NodeProperties,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockFrame {
    indent: u32,
    node: NodeId,
    collection: OpenEventCollection,
    previous_same_kind: u32,
}

const NO_BLOCK_FRAME: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockScalarIndent {
    node: NodeId,
    indent: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockTransition {
    Consume(usize),
    Reprocess,
    Push(usize),
    Pop(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockEntryKind {
    Mapping,
    Sequence,
    ExplicitMapping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockEntryPhase {
    Key,
    Separator,
    Value,
    NestedChild,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockValueFrame {
    kind: BlockEntryKind,
    indent: u32,
    owner: NodeId,
    collection: NodeId,
    phase: BlockEntryPhase,
    semantic_open: bool,
    pending_properties: bool,
    last_content_end: u32,
    empty_offset: u32,
    allow_indentless_sequence: bool,
    allow_same_indent_content: bool,
}

struct BlockMachine<'source> {
    parser: Parser<'source>,
    lines: LineTable<'source>,
    cursor: LineCursor<'source>,
    frames: InlineVec<BlockValueFrame, 4>,
}

pub(crate) struct Parser<'source> {
    source: &'source Source,
    nodes: Vec<Node>,
    semantics: SemanticBuilder,
    stream: Option<NodeId>,
    document: Option<NodeId>,
    document_has_content: bool,
    document_was_explicitly_opened: bool,
    document_yaml_directive_seen: bool,
    tag_handles: BTreeMap<String, String>,
    block_frames: Vec<BlockFrame>,
    last_mapping_frame: u32,
    last_sequence_frame: u32,
    pending_properties: Vec<PendingNodeProperties>,
    block_scalar_indents: Vec<BlockScalarIndent>,
    flow_frames: Vec<FlowFrame>,
    pending_block_values: InlineVec<BlockValueFrame, 2>,
}

impl<'source> Parser<'source> {
    pub(crate) fn new(source: &'source Source) -> Self {
        let line_estimate = source.line_starts().len();
        let estimated_nodes = line_estimate.saturating_mul(3).saturating_add(4);
        let estimated_events = line_estimate.saturating_mul(2).saturating_add(8);
        Self {
            source,
            nodes: Vec::with_capacity(estimated_nodes),
            semantics: SemanticBuilder::with_capacity(estimated_nodes, estimated_events),
            stream: None,
            document: None,
            document_has_content: false,
            document_was_explicitly_opened: false,
            document_yaml_directive_seen: false,
            tag_handles: BTreeMap::new(),
            block_frames: Vec::with_capacity(16),
            last_mapping_frame: NO_BLOCK_FRAME,
            last_sequence_frame: NO_BLOCK_FRAME,
            pending_properties: Vec::new(),
            block_scalar_indents: Vec::new(),
            flow_frames: Vec::new(),
            pending_block_values: InlineVec::new(),
        }
    }

    pub(crate) fn parse(mut self) -> Result<ParsedYaml, YamlError> {
        let stream = self.push_node(NodeKind::Stream, Span::from_usize(0, self.source.len()));
        self.stream = Some(stream);
        self.push_event(
            YamlEventKind::StreamStart,
            Span::from_usize(0, self.source.len()),
        );

        let lines = LineTable::new(self.source);
        self = BlockMachine::new(self, lines).run()?;
        if self.document.is_some() {
            self.close_document(false, Span::empty_from_usize(self.source.len()))?;
        } else if self.nodes[stream.0 as usize].first_child == NO_NODE {
        } else if !Children::new(&self.nodes, stream)
            .any(|child| self.nodes[child.as_usize()].kind == NodeKind::Document)
        {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Parser,
                    "directives must be followed by document content",
                    Span::empty_from_usize(self.source.len()),
                )
                .with_expected("a document start marker or document content"),
            ));
        }
        self.push_event(
            YamlEventKind::StreamEnd,
            Span::from_usize(self.source.len(), self.source.len()),
        );

        let semantics = self.semantics.finish()?;
        Ok(ParsedYaml {
            nodes: self.nodes,
            semantics,
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "top-level YAML line dispatch must preserve grammar precedence"
    )]
    fn parse_line(
        &mut self,
        lines: LineTable<'_>,
        index: usize,
        line: SourceLine<'_>,
    ) -> Result<usize, YamlError> {
        let content = line.content_without_break;
        if let Some(indent) = line.facts.indent()
            && let Some((colon, value_start)) = line.facts.simple_mapping()
            && let Some(mapping) = self.active_simple_mapping(indent)
        {
            let body = &content[indent..];
            return self.append_simple_plain_mapping_entry(
                mapping,
                lines,
                index,
                line,
                indent,
                body,
                line.content_start + indent,
                SimpleMappingFacts::new(colon, value_start),
            );
        }
        if line.facts.is_blank() {
            return Ok(1);
        }
        let trimmed = content.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return Ok(1);
        }

        if trimmed.starts_with('%') {
            self.parse_directive_line(line)?;
            return Ok(1);
        }

        let indent = if content.starts_with('\t') {
            0
        } else {
            count_indent(content, line.content_start)?
        };
        let body = &content[indent..];
        let prepared = PreparedBlockLine::new(line, indent)?;
        if body.as_bytes().first() == Some(&b'\t')
            && (is_explicit_mapping_key(body)
                || is_explicit_mapping_value(body)
                || prepared.mapping_colon().is_some())
        {
            return Err(tab_indentation_error(line.content_start + indent));
        }

        if let Some(rest) = document_marker_rest(body, "---") {
            if indent != 0 {
                return Err(invalid_document_marker(line));
            }
            if self.document.is_some() {
                self.close_document(false, Span::empty_from_usize(line.content_start))?;
            }
            let document =
                self.open_document(true, Span::from_usize(line.content_start, line.content_end));
            let marker = self.push_node(
                NodeKind::DocumentMarker,
                Span::from_usize(line.content_start, line.content_start + 3),
            );
            self.attach_child_at(document.0 as usize, marker);
            let rest = rest.trim_start();
            if rest.is_empty() || rest.starts_with('#') {
                return Ok(1);
            }
            reject_compact_decorated_document(rest, line.content_end - rest.len())?;
            let rest_start = line.content_end - rest.len();
            let prepared = PreparedBlockLine::from_body(line, 0, rest, rest_start)?;
            return self.parse_content_body(document, lines, index, prepared);
        }

        if let Some(rest) = document_marker_rest(body, "...") {
            if indent != 0 || !rest.trim().is_empty() && !rest.trim_start().starts_with('#') {
                return Err(invalid_document_marker(line));
            }
            let has_prior_document = self.stream.is_some_and(|stream| {
                Children::new(&self.nodes, stream)
                    .any(|child| self.nodes[child.as_usize()].kind == NodeKind::Document)
            });
            if self.document.is_none() && has_prior_document {
                return Ok(1);
            }
            if self.document.is_none() && !has_prior_document && !self.document_yaml_directive_seen
            {
                return Ok(1);
            }
            let document = self.ensure_current_document(false, line);
            if !has_prior_document
                && !self.document_has_content
                && self.document_yaml_directive_seen
            {
                return Err(invalid_document_marker(line));
            }
            let marker = self.push_node(
                NodeKind::DocumentMarker,
                Span::from_usize(line.content_start, line.content_start + 3),
            );
            self.attach_child_at(document.0 as usize, marker);
            self.close_document(true, Span::from_usize(line.content_start, line.content_end))?;
            return Ok(1);
        }

        self.validate_indent(indent, line, body)?;
        self.close_collections_deeper_than(indent);
        self.reject_invalid_block_sibling(indent, line, body, prepared.mapping_colon().is_some())?;
        if self.sequence_is_open_at(indent)
            && self.mapping_is_open_at(indent)
            && !is_sequence_entry(body)
            && (is_explicit_mapping_key(body)
                || is_explicit_mapping_value(body)
                || prepared.mapping_colon().is_some())
        {
            self.close_sequence_at_indent(indent);
        }
        reject_unexpected_line_start(body, line.content_start + indent)?;

        let document = self.ensure_current_document(false, line);
        self.parse_content_body(document, lines, index, prepared)
    }

    fn parse_content_body(
        &mut self,
        document: NodeId,
        lines: LineTable<'_>,
        index: usize,
        prepared: PreparedBlockLine<'_>,
    ) -> Result<usize, YamlError> {
        let line = prepared.line;
        let indent = prepared.indent();
        let body = prepared.body;
        let absolute_start = prepared.absolute_start();
        if self.nodes[document.as_usize()].kind == NodeKind::Document
            && self.document_has_content
            && indent == 0
            && self.document_has_root_flow_collection(document)
        {
            return Err(invalid_orphaned_block_content(absolute_start));
        }
        self.document_has_content = true;
        if let Some(facts) = prepared
            .simple_mapping_facts()
            .or_else(|| simple_plain_mapping_facts(body))
        {
            return self.parse_simple_plain_mapping_entry(
                document,
                lines,
                index,
                line,
                indent,
                body,
                absolute_start,
                facts,
            );
        }
        let uncommented = prepared.uncommented().trim_end();
        if body_may_start_with_node_properties(uncommented)
            && let Some(next_indent) =
                property_only_node_indent(uncommented, lines, index, absolute_start)?
        {
            self.push_pending_node_properties(uncommented, absolute_start, next_indent)?;
            return Ok(1);
        }

        if is_sequence_entry(body) {
            self.parse_sequence_entry(document, lines, index, line, indent, body)
        } else if is_explicit_mapping_key(body) {
            self.parse_explicit_mapping_entry(document, lines, index, prepared)
        } else if body.starts_with('|') || body.starts_with('>') {
            let (node, consumed) =
                self.parse_block_scalar(lines, index, absolute_start, indent, body, true)?;
            self.attach_child_at(document.0 as usize, node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        } else if let Some(colon_byte) = prepared.mapping_colon() {
            self.parse_mapping_entry(
                document,
                lines,
                index,
                line,
                indent,
                body,
                colon_byte,
                absolute_start,
            )
        } else if prepared.starts_flow {
            let (node, _end, consumed) =
                self.parse_flow_value_lines(lines, index, absolute_start)?;
            self.attach_child_at(document.0 as usize, node);
            Ok(consumed)
        } else if body.starts_with('"') {
            let (node, consumed) =
                self.parse_quoted_scalar_lines(lines, index, absolute_start, '"', false)?;
            self.attach_child_at(document.0 as usize, node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        } else {
            let allow_same_indent_continuation = !self.parent_collection_below(indent);
            let (scalar, consumed) = self.parse_block_plain_scalar(
                lines,
                index,
                indent,
                absolute_start,
                allow_same_indent_continuation,
            )?;
            self.attach_child_at(document.0 as usize, scalar);
            self.emit_scalar_event(scalar)?;
            Ok(consumed)
        }
    }

    // These inputs describe one already-analyzed source line. Keeping them
    // explicit avoids duplicating or obscuring the parser's offset bookkeeping.
    #[expect(
        clippy::too_many_arguments,
        reason = "the analyzed source line, offsets, and mapping facts must stay synchronized"
    )]
    fn parse_simple_plain_mapping_entry(
        &mut self,
        document: NodeId,
        lines: LineTable<'_>,
        index: usize,
        line: SourceLine<'_>,
        indent: usize,
        body: &str,
        absolute_start: usize,
        facts: SimpleMappingFacts,
    ) -> Result<usize, YamlError> {
        let mapping = self.mapping_for_line(
            document,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        self.append_simple_plain_mapping_entry(
            mapping,
            lines,
            index,
            line,
            indent,
            body,
            absolute_start,
            facts,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the existing mapping and analyzed source-line context are independently meaningful"
    )]
    fn append_simple_plain_mapping_entry(
        &mut self,
        mapping: NodeId,
        lines: LineTable<'_>,
        index: usize,
        line: SourceLine<'_>,
        indent: usize,
        body: &str,
        absolute_start: usize,
        facts: SimpleMappingFacts,
    ) -> Result<usize, YamlError> {
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(line.content_start, line.content_end),
        );
        self.attach_child_at(mapping.as_usize(), entry);
        self.extend_node_span(mapping, line.content_end);

        let key = self.push_node(
            NodeKind::Scalar,
            Span::from_usize(absolute_start, absolute_start + facts.colon()),
        );
        self.attach_child_at(entry.as_usize(), key);
        self.emit_property_free_plain_scalar(key);

        let value_start = absolute_start + facts.value_start();
        debug_assert_eq!(
            &body[facts.value_start()..],
            &self.source.as_str()[value_start..line.content_end]
        );
        let (value, consumed) = if lines.next_line_starts_simple_mapping(index, indent) {
            (
                self.push_node(
                    NodeKind::Scalar,
                    Span::from_usize(value_start, line.content_end),
                ),
                1,
            )
        } else {
            self.parse_block_plain_scalar(lines, index, indent, value_start, false)?
        };
        self.attach_child_at(entry.as_usize(), value);
        self.emit_property_free_plain_scalar(value);
        if consumed > 1 {
            let end = lines.content_end(index + consumed - 1);
            self.extend_node_span(entry, end);
            self.extend_node_span(mapping, end);
        }
        Ok(consumed)
    }

    fn active_simple_mapping(&self, indent: usize) -> Option<NodeId> {
        self.block_frames.last().and_then(|frame| {
            (frame.collection == OpenEventCollection::Mapping && frame.indent as usize == indent)
                .then_some(frame.node)
        })
    }

    fn parse_quoted_scalar_lines(
        &mut self,
        lines: LineTable<'_>,
        index: usize,
        absolute_start: usize,
        quote: char,
        reject_tab_indentation: bool,
    ) -> Result<(NodeId, usize), YamlError> {
        let source_tail = &self.source.as_str()[absolute_start..];
        let end = match quote {
            '"' => scan_double_quoted_scalar(source_tail, 0)?,
            '\'' => single_quoted_scalar_end(source_tail),
            _ => unreachable!("quoted scalar parser requires a quote"),
        }
        .ok_or_else(|| {
            let scalar_name = if quote == '"' {
                "double-quoted scalar"
            } else {
                "single-quoted scalar"
            };
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Lexer,
                    format!("unterminated {scalar_name}"),
                    Span::from_usize(absolute_start, self.source.len()),
                )
                .with_expected(format!("closing {quote}")),
            )
        })?;
        let absolute_end = absolute_start + end;
        let mut consumed = 1;
        for line in lines.iter_from(index + 1) {
            if line.content_start < absolute_end {
                if reject_tab_indentation && line.content_without_break.starts_with('\t') {
                    return Err(tab_indentation_error(line.content_start));
                }
                if reject_tab_indentation
                    && !line.content_without_break.trim().is_empty()
                    && content_line_indent(line.content_without_break) == 0
                {
                    return Err(invalid_quoted_scalar_continuation_indent(
                        line.content_start,
                    ));
                }
                if quote == '"' {
                    validate_double_quoted_continuation_line(&line)?;
                }
                consumed += 1;
            } else {
                break;
            }
        }

        let node = self.push_node(
            NodeKind::Scalar,
            Span::from_usize(absolute_start, absolute_end),
        );
        let style = if quote == '"' {
            YamlScalarStyle::DoubleQuoted
        } else {
            YamlScalarStyle::SingleQuoted
        };
        self.mark_scalar_syntax(node, style, true);
        Ok((node, consumed))
    }

    fn parse_flow_value_lines(
        &mut self,
        lines: LineTable<'_>,
        index: usize,
        absolute_start: usize,
    ) -> Result<(NodeId, usize, usize), YamlError> {
        let source_tail = &self.source.as_str()[absolute_start..];
        let properties = parse_node_properties(
            source_tail,
            Span::from_usize(absolute_start, self.source.len()),
        )?;
        reject_invalid_node_property_placement(source_tail, absolute_start, &properties)?;
        let marker_offset = properties.value_start()
            + leading_flow_whitespace(&source_tail[properties.value_start()..]);
        let marker_start = absolute_start + marker_offset;
        let (node, end) = self.parse_flow_value(source_tail, absolute_start)?;
        let validate_sequence_indent = source_tail[marker_offset..]
            .chars()
            .find(|character| !character.is_whitespace())
            == Some('[')
            && marker_start > lines.content_start(index) + self.source_indent_at(marker_start);
        let absolute_end = absolute_start + end;
        let mut consumed = 1;
        // Include the line break in the validation window. The one-pass cursor may
        // stop immediately after separation at the end of a line, while trailing
        // content is still bounded by the last source line the value consumed.
        let mut validation_end = lines.line_end(index);
        let flow_indent = self.source_indent_at(marker_start);
        let line_start = lines.content_start(index);
        let marker_prefix =
            &self.source.as_str()[line_start..marker_start.min(lines.content_end(index))];
        let allow_tab_continuation = marker_prefix.as_bytes().contains(&b'\t');
        for line in lines.iter_from(index + 1) {
            if line.content_start < absolute_end {
                if validate_sequence_indent {
                    reject_invalid_flow_continuation_indent(
                        &line,
                        flow_indent,
                        allow_tab_continuation,
                    )?;
                }
                consumed += 1;
                validation_end = line.line_end;
            } else {
                break;
            }
        }
        reject_trailing_flow_content(
            &source_tail[..validation_end - absolute_start],
            end,
            absolute_start,
        )?;
        Ok((node, end, consumed))
    }

    // Mapping parsing needs both the source-line view and its absolute offsets;
    // bundling them would only move this context into a single-use wrapper.
    #[expect(
        clippy::too_many_arguments,
        reason = "mapping parsing requires the source-line view and its exact absolute offsets"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "mapping entry parsing shares one span and indentation context"
    )]
    fn parse_mapping_entry(
        &mut self,
        document: NodeId,
        lines: LineTable<'_>,
        index: usize,
        line: SourceLine<'_>,
        indent: usize,
        body: &str,
        colon_byte: usize,
        absolute_start: usize,
    ) -> Result<usize, YamlError> {
        let mapping = self.mapping_for_line(
            document,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(line.content_start, line.content_end),
        );
        self.attach_child_at(mapping.0 as usize, entry);
        self.extend_node_span(mapping, line.content_end);

        let key_start = absolute_start;
        let key_text = body[..colon_byte].trim_end();
        let key_end = key_start + key_text.len();
        if is_simple_plain_atom(key_text) {
            let key = self.push_node(NodeKind::Scalar, Span::from_usize(key_start, key_end));
            self.attach_child_at(entry.as_usize(), key);
            self.emit_property_free_plain_scalar(key);
        } else if key_start < key_end && body_starts_flow_value(key_text, key_start)? {
            let (key, end) = self.parse_flow_value(key_text, key_start)?;
            reject_trailing_flow_content(key_text, end, key_start)?;
            self.attach_child_at(entry.0 as usize, key);
        } else {
            let key_properties =
                parse_node_properties(key_text, Span::from_usize(key_start, key_end))?;
            reject_invalid_node_property_placement(key_text, key_start, &key_properties)?;
            let key = if key_start < key_end {
                self.push_node(NodeKind::Scalar, Span::from_usize(key_start, key_end))
            } else {
                self.push_empty_scalar(key_start)
            };
            self.attach_child_at(entry.0 as usize, key);
            self.emit_scalar_event(key)?;
        }

        let raw_value = &body[colon_byte + 1..];
        let scalar_facts = match source_line_scalar_mapping_facts(line, absolute_start) {
            Some((cached_colon, facts)) if cached_colon == colon_byte => Some(facts),
            _ if lines.source.has_line_facts() => single_line_scalar_facts(raw_value)?,
            _ => None,
        };
        if is_simple_plain_atom(key_text)
            && let Some(facts) = scalar_facts
            && (facts.style != YamlScalarStyle::Plain
                || lines.plain_scalar_cannot_continue(index, indent))
        {
            let raw_start = absolute_start + colon_byte + 1;
            let value = self.push_node(
                NodeKind::Scalar,
                Span::from_usize(raw_start + facts.start(), raw_start + facts.end()),
            );
            self.attach_child_at(entry.as_usize(), value);
            if facts.style != YamlScalarStyle::Plain {
                self.mark_scalar_syntax(value, facts.style, true);
            }
            self.emit_property_free_scalar(value, facts.style);
            return Ok(1);
        }
        let raw_value_trimmed = raw_value.trim_start();
        let value = strip_inline_comment(raw_value);
        let value_trimmed = value.trim_start();

        if !value_trimmed.is_empty() {
            let leading = value.len() - value_trimmed.len();
            let value_start = absolute_start + colon_byte + 1 + leading;
            let value_properties = parse_node_properties(
                value_trimmed,
                Span::from_usize(value_start, value_start + value_trimmed.len()),
            )?;
            reject_invalid_node_property_placement(value_trimmed, value_start, &value_properties)?;
            reject_invalid_block_node_property_punctuation(
                value_trimmed,
                value_start,
                &value_properties,
            )?;
            reject_invalid_compact_block_collection_value(value_trimmed, value_start)?;
            if value_trimmed.starts_with('|') || value_trimmed.starts_with('>') {
                let (node, consumed) = self.parse_block_scalar(
                    lines,
                    index,
                    value_start,
                    indent,
                    value_trimmed,
                    false,
                )?;
                self.attach_child_at(entry.0 as usize, node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else if (value_properties.anchor().is_some() || value_properties.tag().is_some())
                && let Some(header_offset) =
                    block_scalar_after_node_properties(value_trimmed, value_start)?
            {
                self.push_pending_node_properties(
                    &value_trimmed[..header_offset],
                    value_start,
                    self.source_indent_at(value_start + header_offset),
                )?;
                let (node, consumed) = self.parse_block_scalar(
                    lines,
                    index,
                    value_start + header_offset,
                    indent,
                    &value_trimmed[header_offset..],
                    false,
                )?;
                self.attach_child_at(entry.0 as usize, node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else if (value_properties.anchor().is_some() || value_properties.tag().is_some())
                && let Some(next_indent) = property_only_mapping_value_collection_indent(
                    value_trimmed,
                    lines,
                    index,
                    value_start,
                    indent,
                )?
            {
                self.push_pending_node_properties(value_trimmed, value_start, next_indent)?;
                self.defer_block_value(
                    BlockEntryKind::Mapping,
                    entry,
                    mapping,
                    indent,
                    line.content_end,
                    true,
                    true,
                );
                return Ok(1);
            } else if body_starts_flow_value(raw_value_trimmed, value_start)? {
                let (node, _end, consumed) =
                    self.parse_flow_value_lines(lines, index, value_start)?;
                self.attach_child_at(entry.0 as usize, node);
                return Ok(consumed);
            }
            validate_quoted_scalar_trailing_content(value_trimmed, value_start)?;
            reject_nested_plain_mapping_colon(value_trimmed, value_start)?;
            let (node, consumed) = match value_trimmed.chars().next() {
                Some(quote @ ('"' | '\'')) if value_properties.value_start() == 0 => {
                    self.parse_quoted_scalar_lines(lines, index, value_start, quote, true)?
                }
                _ => self.parse_block_plain_scalar(lines, index, indent, value_start, false)?,
            };
            self.attach_child_at(entry.0 as usize, node);
            self.emit_scalar_event(node)?;
            return Ok(consumed);
        }
        self.defer_block_value(
            BlockEntryKind::Mapping,
            entry,
            mapping,
            indent,
            line.content_end,
            true,
            false,
        );
        Ok(1)
    }

    fn parse_explicit_mapping_entry(
        &mut self,
        document: NodeId,
        lines: LineTable<'_>,
        index: usize,
        prepared: PreparedBlockLine<'_>,
    ) -> Result<usize, YamlError> {
        let line = prepared.line;
        let indent = prepared.indent();
        let body = prepared.body;
        let absolute_start = prepared.absolute_start();
        let mapping = self.mapping_for_line(
            document,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(line.content_start, line.content_end),
        );
        self.attach_child_at(mapping.0 as usize, entry);
        self.extend_node_span(mapping, line.content_end);

        let after_question = if body == "?" { "" } else { &body[1..] };
        reject_invalid_indicator_tab(body, absolute_start)?;
        let key_text = strip_inline_comment(after_question).trim_start();
        if key_text.is_empty() {
            self.defer_explicit_mapping(
                entry,
                mapping,
                indent,
                BlockEntryPhase::Key,
                line.content_end,
            );
            return Ok(1);
        }
        let leading = after_question.len() - after_question.trim_start().len();
        let key_start = absolute_start + 1 + leading;
        let key_properties = parse_node_properties(
            key_text,
            Span::from_usize(key_start, key_start + key_text.len()),
        )?;
        reject_invalid_node_property_placement(key_text, key_start, &key_properties)?;
        let key_consumed =
            self.parse_explicit_mapping_key_node(entry, lines, index, indent, key_text, key_start)?;
        self.defer_explicit_mapping(
            entry,
            mapping,
            indent,
            BlockEntryPhase::Separator,
            line.content_end,
        );
        Ok(key_consumed)
    }

    fn parse_explicit_mapping_key_node(
        &mut self,
        entry: NodeId,
        lines: LineTable<'_>,
        index: usize,
        parent_indent: usize,
        key_text: &str,
        key_start: usize,
    ) -> Result<usize, YamlError> {
        if key_text.starts_with('|') || key_text.starts_with('>') {
            let (node, consumed) =
                self.parse_block_scalar(lines, index, key_start, parent_indent, key_text, false)?;
            self.attach_child_at(entry.0 as usize, node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        } else if is_sequence_entry(key_text) {
            let current_line = lines
                .cursor_from(index)
                .next()
                .expect("current line index is in bounds");
            let key_indent = key_start - current_line.content_start;
            let mut consumed =
                self.parse_sequence_entry(entry, lines, index, current_line, key_indent, key_text)?;
            let mut nested_index = index + consumed;
            while nested_index < lines.len() {
                let line = lines
                    .cursor_from(nested_index)
                    .next()
                    .expect("nested line index is in bounds");
                let trimmed = line.content_without_break.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    consumed += 1;
                    nested_index += 1;
                    continue;
                }
                let indent = content_line_indent(line.content_without_break);
                let body = &line.content_without_break[indent..];
                if indent != key_indent || !is_sequence_entry(body) {
                    break;
                }
                let prepared = PreparedBlockLine::new(line, indent)?;
                let nested_consumed =
                    self.parse_content_body(entry, lines, nested_index, prepared)?;
                consumed += nested_consumed;
                nested_index += nested_consumed;
            }
            Ok(consumed)
        } else if let Some(colon_byte) = flow_collection_mapping_key_colon(key_text, key_start)? {
            self.parse_compact_block_mapping_node(entry, key_text, key_start, colon_byte)
        } else if body_starts_flow_value(key_text, key_start)? {
            let (key, _end, consumed) = self.parse_flow_value_lines(lines, index, key_start)?;
            self.attach_child_at(entry.0 as usize, key);
            Ok(consumed)
        } else if let Some(colon_byte) = find_mapping_colon(key_text) {
            self.parse_compact_block_mapping_node(entry, key_text, key_start, colon_byte)
        } else {
            validate_quoted_scalar_trailing_content(key_text, key_start)?;
            reject_nested_plain_mapping_colon(key_text, key_start)?;
            let (key, consumed) =
                self.parse_block_plain_scalar(lines, index, parent_indent, key_start, false)?;
            self.attach_child_at(entry.0 as usize, key);
            self.emit_scalar_event(key)?;
            Ok(consumed)
        }
    }

    fn parse_explicit_mapping_value(
        &mut self,
        entry: NodeId,
        mapping: NodeId,
        lines: LineTable<'_>,
        index: usize,
        indent: usize,
        body: &str,
    ) -> Result<usize, YamlError> {
        let line = lines
            .cursor_from(index)
            .next()
            .expect("current line index is in bounds");
        let raw_value = &body[1..];
        reject_invalid_indicator_tab(body, line.content_start + indent)?;
        let raw_value_trimmed = raw_value.trim_start();
        let value = strip_inline_comment(raw_value);
        let value_trimmed = value.trim_start();

        if value_trimmed.is_empty() {
            self.defer_explicit_mapping_value(entry, mapping, indent, line.content_end);
            return Ok(1);
        }

        let leading = value.len() - value_trimmed.len();
        let value_start = line.content_start + indent + 1 + leading;
        let value_properties = parse_node_properties(
            value_trimmed,
            Span::from_usize(value_start, value_start + value_trimmed.len()),
        )?;
        reject_invalid_node_property_placement(value_trimmed, value_start, &value_properties)?;
        reject_invalid_block_node_property_punctuation(
            value_trimmed,
            value_start,
            &value_properties,
        )?;
        if value_trimmed.starts_with('|') || value_trimmed.starts_with('>') {
            let (node, consumed) =
                self.parse_block_scalar(lines, index, value_start, indent, value_trimmed, false)?;
            self.attach_child_at(entry.0 as usize, node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        } else if let Some(header_offset) =
            block_scalar_after_node_properties(value_trimmed, value_start)?
        {
            self.push_pending_node_properties(
                &value_trimmed[..header_offset],
                value_start,
                self.source_indent_at(value_start + header_offset),
            )?;
            let (node, consumed) = self.parse_block_scalar(
                lines,
                index,
                value_start + header_offset,
                indent,
                &value_trimmed[header_offset..],
                false,
            )?;
            self.attach_child_at(entry.0 as usize, node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        } else if let Some(next_indent) = property_only_mapping_value_collection_indent(
            value_trimmed,
            lines,
            index,
            value_start,
            indent,
        )? {
            self.push_pending_node_properties(value_trimmed, value_start, next_indent)?;
            self.defer_explicit_mapping_value(entry, mapping, indent, line.content_end);
            Ok(1)
        } else if is_sequence_entry(value_trimmed) {
            self.parse_sequence_entry(
                entry,
                lines,
                index,
                line,
                value_start - line.content_start,
                value_trimmed,
            )
        } else if let Some(colon_byte) = find_mapping_colon(value_trimmed) {
            self.parse_compact_block_mapping_node(entry, value_trimmed, value_start, colon_byte)?;
            Ok(1)
        } else if body_starts_flow_value(raw_value_trimmed, value_start)? {
            let (value_node, _end, consumed) =
                self.parse_flow_value_lines(lines, index, value_start)?;
            self.attach_child_at(entry.0 as usize, value_node);
            Ok(consumed)
        } else {
            validate_quoted_scalar_trailing_content(value_trimmed, value_start)?;
            reject_nested_plain_mapping_colon(value_trimmed, value_start)?;
            let (node, consumed) =
                self.parse_block_plain_scalar(lines, index, indent, value_start, false)?;
            self.attach_child_at(entry.0 as usize, node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        }
    }

    fn push_pending_node_properties(
        &mut self,
        text: &str,
        absolute_start: usize,
        indent: usize,
    ) -> Result<(), YamlError> {
        let properties = parse_node_properties(
            text,
            Span::from_usize(absolute_start, absolute_start + text.len()),
        )?;
        reject_invalid_node_property_placement(text, absolute_start, &properties)?;
        self.resolve_node_properties(
            &properties,
            Span::from_usize(absolute_start, absolute_start + text.len()),
        )?;
        let pending = if self
            .pending_properties
            .last()
            .is_some_and(|pending| pending.indent as usize == indent)
        {
            self.pending_properties.last_mut()
        } else {
            self.pending_properties
                .iter_mut()
                .rev()
                .find(|pending| pending.indent as usize == indent)
        };
        if let Some(pending) = pending {
            pending.span_start = pending.span_start.min(Span::usize_to_u32(absolute_start));
            if pending.properties.anchor().is_none() {
                pending.properties.set_anchor(properties.anchor());
            }
            if pending.properties.tag().is_none() {
                pending.properties.set_tag(properties.tag());
            }
        } else {
            self.pending_properties.push(PendingNodeProperties {
                indent: Span::usize_to_u32(indent),
                span_start: Span::usize_to_u32(absolute_start),
                properties,
            });
        }
        Ok(())
    }

    fn ensure_current_document(&mut self, explicit: bool, line: SourceLine<'_>) -> NodeId {
        self.document.unwrap_or_else(|| {
            self.open_document(
                explicit,
                Span::from_usize(line.content_start, line.content_end),
            )
        })
    }

    fn open_document(&mut self, explicit: bool, span: Span) -> NodeId {
        let stream = self.stream.expect("stream node exists before documents");
        let document = self.push_node(NodeKind::Document, span);
        self.attach_child_at(stream.0 as usize, document);
        self.document = Some(document);
        self.document_has_content = false;
        self.document_was_explicitly_opened = explicit;
        self.clear_block_frames();
        self.pending_properties.clear();
        self.block_scalar_indents.clear();
        self.push_node_event(YamlEventKind::DocumentStart { explicit }, span, document);
        document
    }

    fn close_document(&mut self, explicit: bool, span: Span) -> Result<(), YamlError> {
        let Some(document) = self.document else {
            return Ok(());
        };
        self.close_event_collections_deeper_than(0);
        self.close_all_event_collections();
        if !self.document_has_content && self.document_was_explicitly_opened {
            let empty = self.push_empty_scalar(span.start as usize);
            self.attach_child_at(document.0 as usize, empty);
            self.emit_scalar_event(empty)?;
        }
        self.push_event(YamlEventKind::DocumentEnd { explicit }, span);
        self.document = None;
        self.document_has_content = false;
        self.document_was_explicitly_opened = false;
        self.document_yaml_directive_seen = false;
        self.tag_handles.clear();
        self.clear_block_frames();
        self.pending_properties.clear();
        self.block_scalar_indents.clear();
        Ok(())
    }

    fn parse_directive_line(&mut self, line: SourceLine<'_>) -> Result<(), YamlError> {
        if self.document.is_some() || self.document_has_content {
            return Err(directive_after_document_content(line).with_position_from(self.source));
        }

        let stream = self.stream.expect("stream node exists before directives");
        let directive = self.push_node(
            NodeKind::Directive,
            Span::from_usize(line.content_start, line.content_end),
        );
        self.attach_child_at(stream.0 as usize, directive);

        let body = strip_inline_comment(line.content_without_break).trim();
        let mut parts = body.split_whitespace();
        let Some(name) = parts.next() else {
            return Err(invalid_directive(line, "missing directive name"));
        };

        match name {
            "%YAML" => {
                if self.document_yaml_directive_seen {
                    return Err(invalid_directive(line, "duplicate YAML directive"));
                }
                let Some(version) = parts.next() else {
                    return Err(invalid_directive(line, "missing YAML directive version"));
                };
                if parts.next().is_some() {
                    return Err(invalid_directive(
                        line,
                        "unexpected YAML directive parameter",
                    ));
                }
                if !valid_yaml_directive_version_syntax(version) {
                    return Err(invalid_directive(line, "invalid YAML directive version"));
                }
                self.document_yaml_directive_seen = true;
                Ok(())
            }
            "%TAG" => {
                let Some(handle) = parts.next() else {
                    return Err(invalid_directive(line, "missing TAG directive handle"));
                };
                let Some(prefix) = parts.next() else {
                    return Err(invalid_directive(line, "missing TAG directive prefix"));
                };
                if parts.next().is_some() {
                    return Err(invalid_directive(
                        line,
                        "unexpected TAG directive parameter",
                    ));
                }
                validate_tag_handle(handle, line)?;
                let handle_offset = body
                    .find(handle)
                    .expect("parsed TAG handle occurs in directive text");
                let prefix_offset = handle_offset
                    + handle.len()
                    + body[handle_offset + handle.len()..]
                        .find(prefix)
                        .expect("parsed TAG prefix occurs after its handle");
                self.semantics.push_tag_directive(
                    Span::from_usize(
                        line.content_start + handle_offset,
                        line.content_start + handle_offset + handle.len(),
                    ),
                    Span::from_usize(
                        line.content_start + prefix_offset,
                        line.content_start + prefix_offset + prefix.len(),
                    ),
                );
                self.tag_handles
                    .insert(handle.to_owned(), prefix.to_owned());
                Ok(())
            }
            _ => Ok(()),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "sequence entry parsing shares one span and indentation context"
    )]
    fn parse_sequence_entry(
        &mut self,
        document: NodeId,
        lines: LineTable<'_>,
        index: usize,
        line: SourceLine<'_>,
        indent: usize,
        body: &str,
    ) -> Result<usize, YamlError> {
        let sequence = self.sequence_for_line(
            document,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        let entry = self.push_node(
            NodeKind::SequenceEntry,
            Span::from_usize(line.content_start, line.content_end),
        );
        self.attach_child_at(sequence.0 as usize, entry);
        self.extend_node_span(sequence, line.content_end);

        let after_dash = if body == "-" { "" } else { &body[1..] };
        reject_invalid_indicator_tab(body, line.content_start + indent)?;
        reject_invalid_sequence_tab_separated_nested_indicator(body, line.content_start + indent)?;
        let value = after_dash.trim_start();
        if value.is_empty() || value.starts_with('#') {
            self.defer_block_value(
                BlockEntryKind::Sequence,
                entry,
                sequence,
                indent,
                line.content_start + indent + 1,
                false,
                false,
            );
            Ok(1)
        } else {
            let leading = after_dash.len() - value.len();
            let value_start = line.content_start + indent + 1 + leading;
            if is_simple_plain_atom(value) && !is_sequence_entry(value) {
                let (value_node, consumed) =
                    self.parse_block_plain_scalar(lines, index, indent, value_start, false)?;
                self.attach_child_at(entry.as_usize(), value_node);
                self.emit_property_free_plain_scalar(value_node);
                return Ok(consumed);
            }
            if let Some(colon_byte) = simple_plain_key_mapping_colon(value) {
                return self.parse_mapping_entry(
                    entry,
                    lines,
                    index,
                    line,
                    value_start - line.content_start,
                    value,
                    colon_byte,
                    value_start,
                );
            }
            let value_properties = parse_node_properties(
                value,
                Span::from_usize(value_start, value_start + value.len()),
            )?;
            reject_invalid_node_property_placement(value, value_start, &value_properties)?;
            reject_invalid_block_node_property_punctuation(value, value_start, &value_properties)?;
            if value.starts_with('|') || value.starts_with('>') {
                let (node, consumed) =
                    self.parse_block_scalar(lines, index, value_start, indent, value, false)?;
                self.attach_child_at(entry.0 as usize, node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else if (value_properties.anchor().is_some() || value_properties.tag().is_some())
                && let Some(header_offset) = block_scalar_after_node_properties(value, value_start)?
            {
                self.push_pending_node_properties(
                    &value[..header_offset],
                    value_start,
                    self.source_indent_at(value_start + header_offset),
                )?;
                let (node, consumed) = self.parse_block_scalar(
                    lines,
                    index,
                    value_start + header_offset,
                    indent,
                    &value[header_offset..],
                    false,
                )?;
                self.attach_child_at(entry.0 as usize, node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else if (value_properties.anchor().is_some() || value_properties.tag().is_some())
                && let Some(next_indent) =
                    property_only_block_collection_indent(value, lines, index, value_start)?
                        .filter(|next_indent| *next_indent > indent)
            {
                self.push_pending_node_properties(value, value_start, next_indent)?;
                self.defer_block_value(
                    BlockEntryKind::Sequence,
                    entry,
                    sequence,
                    indent,
                    line.content_end,
                    false,
                    true,
                );
                return Ok(1);
            } else if is_sequence_entry(value) {
                return self.parse_sequence_entry(
                    entry,
                    lines,
                    index,
                    line,
                    value_start - line.content_start,
                    value,
                );
            } else if body_starts_flow_value(value, value_start)? {
                let (value_node, _end, consumed) =
                    self.parse_flow_value_lines(lines, index, value_start)?;
                self.attach_child_at(entry.0 as usize, value_node);
                return Ok(consumed);
            } else if is_explicit_mapping_key(value) {
                let prepared = PreparedBlockLine::from_body(
                    line,
                    value_start - line.content_start,
                    value,
                    value_start,
                )?;
                return self.parse_explicit_mapping_entry(entry, lines, index, prepared);
            } else if is_compact_explicit_empty_key_mapping(value) {
                self.parse_compact_explicit_empty_key_mapping(
                    entry,
                    line,
                    value_start - line.content_start,
                    value,
                    value_start,
                )?;
                return Ok(1);
            } else if let Some(colon_byte) = find_mapping_colon(value) {
                return self.parse_mapping_entry(
                    entry,
                    lines,
                    index,
                    line,
                    value_start - line.content_start,
                    value,
                    colon_byte,
                    value_start,
                );
            }
            validate_quoted_scalar_trailing_content(value, value_start)?;
            reject_nested_plain_mapping_colon(value, value_start)?;
            let (value_node, consumed) =
                self.parse_block_plain_scalar(lines, index, indent, value_start, false)?;
            self.attach_child_at(entry.0 as usize, value_node);
            self.emit_scalar_event(value_node)?;
            Ok(consumed)
        }
    }

    fn parse_compact_explicit_empty_key_mapping(
        &mut self,
        parent: NodeId,
        line: SourceLine<'_>,
        indent: usize,
        body: &str,
        absolute_start: usize,
    ) -> Result<(), YamlError> {
        let mapping = self.mapping_for_line(
            parent,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        let outer_entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(absolute_start, line.content_end),
        );
        self.attach_child_at(mapping.0 as usize, outer_entry);

        let inner_mapping = self.push_node(
            NodeKind::BlockMapping,
            Span::from_usize(absolute_start, line.content_end),
        );
        self.attach_child_at(outer_entry.0 as usize, inner_mapping);
        self.push_node_event(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Block,
                tag: None,
                anchor: None,
            },
            Span::from_usize(absolute_start, line.content_end),
            inner_mapping,
        );

        let inner_entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(absolute_start, line.content_end),
        );
        self.attach_child_at(inner_mapping.0 as usize, inner_entry);

        let colon_offset = body
            .find(':')
            .expect("compact explicit empty key mapping contains colon");
        let colon_start = absolute_start + colon_offset;
        let key = self.push_empty_scalar(colon_start);
        self.attach_child_at(inner_entry.0 as usize, key);
        self.emit_scalar_event(key)?;

        let raw_value = &body[colon_offset + 1..];
        let value_trimmed = raw_value.trim_start();
        let value = if value_trimmed.is_empty() {
            self.push_empty_scalar(line.content_end)
        } else {
            let leading = raw_value.len() - value_trimmed.len();
            self.push_node(
                NodeKind::Scalar,
                Span::from_usize(
                    absolute_start + colon_offset + 1 + leading,
                    absolute_start + body.len(),
                ),
            )
        };
        self.attach_child_at(inner_entry.0 as usize, value);
        self.emit_scalar_event(value)?;
        self.push_event(
            YamlEventKind::MappingEnd,
            Span::empty_from_usize(line.content_end),
        );

        let outer_value = self.push_empty_scalar(line.content_end);
        self.attach_child_at(outer_entry.0 as usize, outer_value);
        self.emit_scalar_event(outer_value)?;
        Ok(())
    }

    fn parse_compact_block_mapping_node(
        &mut self,
        parent: NodeId,
        body: &str,
        absolute_start: usize,
        colon_byte: usize,
    ) -> Result<usize, YamlError> {
        let mapping = self.push_node(
            NodeKind::BlockMapping,
            Span::from_usize(absolute_start, absolute_start + body.len()),
        );
        self.attach_child_at(parent.0 as usize, mapping);
        self.push_node_event(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Block,
                tag: None,
                anchor: None,
            },
            Span::from_usize(absolute_start, absolute_start + body.len()),
            mapping,
        );

        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(absolute_start, absolute_start + body.len()),
        );
        self.attach_child_at(mapping.0 as usize, entry);

        let key_text = body[..colon_byte].trim_end();
        let (key, flow_key) = if key_text.is_empty() {
            (self.push_empty_scalar(absolute_start), false)
        } else if body_starts_flow_value(key_text, absolute_start)? {
            let (key, end) = self.parse_flow_value(key_text, absolute_start)?;
            reject_trailing_flow_content(key_text, end, absolute_start)?;
            (key, true)
        } else {
            let key_end = absolute_start + key_text.len();
            (
                self.push_node(NodeKind::Scalar, Span::from_usize(absolute_start, key_end)),
                false,
            )
        };
        self.attach_child_at(entry.0 as usize, key);
        if !flow_key {
            self.emit_scalar_event(key)?;
        }

        let raw_value = &body[colon_byte + 1..];
        let value_trimmed = raw_value.trim_start();
        let value = if value_trimmed.is_empty() {
            self.push_empty_scalar(absolute_start + body.len())
        } else {
            let leading = raw_value.len() - value_trimmed.len();
            self.push_node(
                NodeKind::Scalar,
                Span::from_usize(
                    absolute_start + colon_byte + 1 + leading,
                    absolute_start + body.len(),
                ),
            )
        };
        self.attach_child_at(entry.0 as usize, value);
        self.emit_scalar_event(value)?;

        self.push_event(
            YamlEventKind::MappingEnd,
            Span::empty_from_usize(absolute_start + body.len()),
        );
        Ok(1)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "a deferred entry frame retains the complete attachment and indentation state"
    )]
    fn defer_block_value(
        &mut self,
        kind: BlockEntryKind,
        owner: NodeId,
        collection: NodeId,
        indent: usize,
        empty_offset: usize,
        allow_indentless_sequence: bool,
        pending_properties: bool,
    ) {
        self.pending_block_values.push(BlockValueFrame {
            kind,
            indent: Span::usize_to_u32(indent),
            owner,
            collection,
            phase: BlockEntryPhase::Value,
            semantic_open: true,
            pending_properties,
            last_content_end: Span::usize_to_u32(empty_offset),
            empty_offset: Span::usize_to_u32(empty_offset),
            allow_indentless_sequence,
            allow_same_indent_content: false,
        });
    }

    fn defer_explicit_mapping(
        &mut self,
        entry: NodeId,
        mapping: NodeId,
        indent: usize,
        phase: BlockEntryPhase,
        offset: usize,
    ) {
        debug_assert!(matches!(
            phase,
            BlockEntryPhase::Key | BlockEntryPhase::Separator
        ));
        self.pending_block_values.push(BlockValueFrame {
            kind: BlockEntryKind::ExplicitMapping,
            indent: Span::usize_to_u32(indent),
            owner: entry,
            collection: mapping,
            phase,
            semantic_open: true,
            pending_properties: false,
            last_content_end: Span::usize_to_u32(offset),
            empty_offset: Span::usize_to_u32(offset),
            allow_indentless_sequence: false,
            allow_same_indent_content: true,
        });
    }

    fn defer_explicit_mapping_value(
        &mut self,
        entry: NodeId,
        mapping: NodeId,
        indent: usize,
        offset: usize,
    ) {
        self.defer_block_value(
            BlockEntryKind::Mapping,
            entry,
            mapping,
            indent,
            offset,
            false,
            false,
        );
        let index = self.pending_block_values.len() - 1;
        self.pending_block_values[index].allow_same_indent_content = true;
    }

    fn finish_block_value(&mut self, mut frame: BlockValueFrame) -> Result<(), YamlError> {
        debug_assert!(frame.semantic_open);
        if frame.kind == BlockEntryKind::ExplicitMapping {
            self.close_sequence_at_indent(frame.indent as usize);
            self.close_collections_deeper_than(frame.indent as usize);
            if self.nodes[frame.owner.as_usize()].first_child == NO_NODE {
                let key = self.push_empty_scalar(frame.empty_offset as usize);
                self.attach_child_at(frame.owner.as_usize(), key);
                self.emit_scalar_event(key)?;
            }
        }
        let needs_empty_value = frame.phase == BlockEntryPhase::Value
            || matches!(
                frame.phase,
                BlockEntryPhase::Key | BlockEntryPhase::Separator
            );
        frame.phase = BlockEntryPhase::Complete;
        frame.semantic_open = false;
        if needs_empty_value {
            let empty = self.push_empty_scalar(frame.empty_offset as usize);
            self.attach_child_at(frame.owner.as_usize(), empty);
            self.emit_scalar_event(empty)?;
        }
        Ok(())
    }

    fn parse_block_plain_scalar(
        &mut self,
        lines: LineTable<'_>,
        index: usize,
        parent_indent: usize,
        value_start: usize,
        allow_same_indent_continuation: bool,
    ) -> Result<(NodeId, usize), YamlError> {
        let mut consumed = 1;
        let initial_line = lines
            .cursor_from(index)
            .next()
            .expect("current line index is in bounds");
        let mut end = initial_line.content_end;
        let mut pending_blank_lines = 0usize;
        let mut scalar_has_inline_comment = plain_scalar_line_has_inline_comment(
            &self.source.as_str()[value_start..initial_line.content_end],
        );
        let initial_text = &self.source.as_str()[value_start..initial_line.content_end];
        let scalar_has_node_properties = if initial_text.starts_with(['!', '&']) {
            let properties = parse_node_properties(
                initial_text,
                Span::from_usize(value_start, initial_line.content_end),
            )?;
            properties.anchor().is_some() || properties.tag().is_some()
        } else {
            false
        };
        let initial_properties = parse_node_properties(
            initial_text,
            Span::from_usize(value_start, initial_line.content_end),
        )?;
        let scalar_style = scalar_style_from_first_char(
            initial_text[initial_properties.value_start()..]
                .chars()
                .next()
                .unwrap_or(' '),
        );

        for line in lines.iter_from(index + 1) {
            let trimmed = line.content_without_break.trim();
            if trimmed == "---" || trimmed == "..." {
                break;
            }

            if trimmed.is_empty() {
                pending_blank_lines += 1;
                continue;
            }

            if !is_plain_scalar_continuation(
                line.content_without_break,
                parent_indent,
                allow_same_indent_continuation,
            ) {
                break;
            }
            if scalar_has_node_properties && trimmed.starts_with('%') {
                return Err(directive_after_document_content(line).with_position_from(self.source));
            }
            if self.source.as_str()[value_start..initial_line.content_end].starts_with('"')
                && line.content_without_break.starts_with('\t')
            {
                return Err(tab_indentation_error(line.content_start));
            }
            let indent = content_line_indent(line.content_without_break);
            let body = &line.content_without_break[indent..];
            if scalar_has_inline_comment || plain_continuation_has_mapping_colon(body) {
                return Err(invalid_plain_scalar_continuation(
                    line.content_start + indent,
                ));
            }

            consumed += pending_blank_lines + 1;
            pending_blank_lines = 0;
            end = line.content_end;
            scalar_has_inline_comment |= plain_scalar_line_has_inline_comment(body);
        }

        let node = self.push_node(NodeKind::Scalar, Span::from_usize(value_start, end));
        if consumed > 1 {
            self.mark_scalar_syntax(node, scalar_style, scalar_style == YamlScalarStyle::Plain);
        }
        Ok((node, consumed))
    }

    fn parse_flow_value(
        &mut self,
        text: &str,
        absolute_start: usize,
    ) -> Result<(NodeId, usize), YamlError> {
        let properties = parse_node_properties(
            text,
            Span::from_usize(absolute_start, absolute_start + text.len()),
        )?;
        reject_invalid_node_property_placement(text, absolute_start, &properties)?;
        let pending_properties = self.take_pending_node_properties_at(absolute_start);
        let value_start =
            properties.value_start() + leading_flow_whitespace(&text[properties.value_start()..]);
        let value_text = &text[value_start..];
        if value_text.starts_with(['[', '{']) {
            let collection_start = absolute_start + value_start;
            let (node, consumed) =
                self.parse_flow_collection_iterative(value_text, collection_start)?;
            self.nodes[node.as_usize()].span.start = Span::usize_to_u32(absolute_start);
            self.register_flow_collection_semantics(node, properties, pending_properties)?;
            let span = self.nodes[node.as_usize()].span;
            self.semantics.attach_flow_root(node, span);
            Ok((node, value_start + consumed))
        } else {
            let end = flow_scalar_end(text, value_start, absolute_start, &[',', ']', '}'])?;
            let scalar_start = value_start + leading_flow_whitespace(&text[value_start..end]);
            let scalar_end = end - trailing_flow_whitespace(&text[value_start..end]);
            if scalar_start >= scalar_end {
                return Err(empty_flow_value(absolute_start));
            }
            let node = self.push_node(
                NodeKind::Scalar,
                Span::from_usize(absolute_start, absolute_start + scalar_end),
            );
            self.mark_scalar_syntax(
                node,
                scalar_style_from_first_char(
                    text[scalar_start..]
                        .chars()
                        .next()
                        .expect("scalar start is inside text"),
                ),
                matches!(text.as_bytes()[scalar_start], b'"' | b'\''),
            );
            self.register_flow_scalar_semantics(node, properties, pending_properties)?;
            let span = self.nodes[node.as_usize()].span;
            self.semantics.attach_flow_root(node, span);
            Ok((node, end))
        }
    }

    fn parse_flow_collection_iterative(
        &mut self,
        text: &str,
        absolute_start: usize,
    ) -> Result<(NodeId, usize), YamlError> {
        let Some(open) = text.chars().next() else {
            return Err(empty_flow_value(absolute_start));
        };
        let root = self.push_node(
            if open == '[' {
                NodeKind::FlowSequence
            } else {
                NodeKind::FlowMapping
            },
            Span::from_usize(absolute_start, absolute_start + open.len_utf8()),
        );
        let frames = std::mem::take(&mut self.flow_frames);
        let mut flow = FlowParseState::new(text, absolute_start, root, open, frames);
        let result = loop {
            if let Some((child, child_end)) = flow.completed.take() {
                match self.resume_completed_flow_node(&mut flow, child, child_end) {
                    Ok(Some(end)) => break Ok((flow.root, end)),
                    Ok(None) => {}
                    Err(error) => break Err(error),
                }
                continue;
            }
            if let Err(error) = self.advance_flow_frame(&mut flow) {
                break Err(error);
            }
        };
        self.flow_frames = flow.frames;
        result
    }

    fn resume_completed_flow_node(
        &mut self,
        flow: &mut FlowParseState<'_>,
        child: NodeId,
        child_end: usize,
    ) -> Result<Option<usize>, YamlError> {
        flow.position = child_end;
        let Some(frame_index) = flow.frames.len().checked_sub(1) else {
            return Ok(Some(flow.position));
        };
        let frame = flow.frames[frame_index];
        match frame.state {
            FlowFrameState::SequenceAwaitItem {
                entry: _,
                entry_start: _,
                explicit: _,
            } => self.resume_flow_sequence_item(flow, frame_index, frame, child)?,
            FlowFrameState::SequenceAwaitValue {
                entry,
                mapping,
                mapping_entry,
            } => {
                self.attach_child_at(mapping_entry.as_usize(), child);
                self.finish_flow_mapping_sequence_item(
                    frame.node,
                    entry,
                    mapping,
                    mapping_entry,
                    flow.position,
                    flow.absolute_start,
                );
                flow.frames[frame_index].state = FlowFrameState::SequenceAfterItem;
            }
            FlowFrameState::MappingAwaitKey {
                entry: _,
                entry_start: _,
                explicit: _,
            } => self.resume_flow_mapping_key(flow, frame_index, frame, child)?,
            FlowFrameState::MappingAwaitValue { entry, entry_start } => {
                self.attach_child_at(entry.as_usize(), child);
                self.finish_flow_mapping_entry(
                    frame.node,
                    entry,
                    entry_start as usize,
                    flow.position,
                    flow.absolute_start,
                );
                flow.frames[frame_index].state = FlowFrameState::MappingAfterPair;
            }
            _ => {
                return Err(invalid_flow_parser_state(
                    flow.absolute_start + flow.position,
                ));
            }
        }
        Ok(None)
    }

    fn resume_flow_sequence_item(
        &mut self,
        flow: &mut FlowParseState<'_>,
        frame_index: usize,
        frame: FlowFrame,
        child: NodeId,
    ) -> Result<(), YamlError> {
        let FlowFrameState::SequenceAwaitItem {
            entry,
            entry_start,
            explicit,
        } = frame.state
        else {
            return Err(invalid_flow_parser_state(
                flow.absolute_start + flow.position,
            ));
        };
        let entry_start = entry_start as usize;
        flow.position = skip_flow_whitespace(flow.text, flow.position);
        if flow.text[flow.position..].starts_with(':') {
            if !explicit {
                reject_split_implicit_flow_mapping_key(
                    flow.text,
                    entry_start,
                    flow.position,
                    flow.absolute_start,
                )?;
            }
            let mapping = self.push_node(
                NodeKind::FlowMapping,
                Span::from_usize(
                    flow.absolute_start + entry_start,
                    flow.absolute_start + flow.position,
                ),
            );
            self.register_implicit_flow_mapping(mapping);
            let mapping_entry = self.push_node(
                NodeKind::MappingEntry,
                Span::from_usize(
                    flow.absolute_start + entry_start,
                    flow.absolute_start + flow.position,
                ),
            );
            self.attach_child_at(mapping.as_usize(), mapping_entry);
            self.attach_child_at(mapping_entry.as_usize(), child);
            flow.position = skip_flow_whitespace(flow.text, flow.position + 1);
            if flow.text[flow.position..].starts_with([',', ']']) {
                let value = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
                self.attach_child_at(mapping_entry.as_usize(), value);
                self.finish_flow_mapping_sequence_item(
                    frame.node,
                    entry,
                    mapping,
                    mapping_entry,
                    flow.position,
                    flow.absolute_start,
                );
                flow.frames[frame_index].state = FlowFrameState::SequenceAfterItem;
            } else {
                flow.frames[frame_index].state = FlowFrameState::SequenceAwaitValue {
                    entry,
                    mapping,
                    mapping_entry,
                };
                self.start_flow_node(flow, frame.close)?;
            }
        } else if explicit {
            let mapping = self.push_node(
                NodeKind::FlowMapping,
                Span::from_usize(
                    flow.absolute_start + entry_start,
                    flow.absolute_start + flow.position,
                ),
            );
            self.register_implicit_flow_mapping(mapping);
            let mapping_entry = self.push_node(
                NodeKind::MappingEntry,
                Span::from_usize(
                    flow.absolute_start + entry_start,
                    flow.absolute_start + flow.position,
                ),
            );
            let value = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
            self.attach_child_at(mapping.as_usize(), mapping_entry);
            self.attach_child_at(mapping_entry.as_usize(), child);
            self.attach_child_at(mapping_entry.as_usize(), value);
            self.finish_flow_mapping_sequence_item(
                frame.node,
                entry,
                mapping,
                mapping_entry,
                flow.position,
                flow.absolute_start,
            );
            flow.frames[frame_index].state = FlowFrameState::SequenceAfterItem;
        } else {
            self.attach_child_at(entry.as_usize(), child);
            self.nodes[entry.as_usize()].span.end =
                Span::usize_to_u32(flow.absolute_start + flow.position);
            self.attach_child_at(frame.node.as_usize(), entry);
            flow.frames[frame_index].state = FlowFrameState::SequenceAfterItem;
        }
        Ok(())
    }

    fn resume_flow_mapping_key(
        &mut self,
        flow: &mut FlowParseState<'_>,
        frame_index: usize,
        frame: FlowFrame,
        child: NodeId,
    ) -> Result<(), YamlError> {
        let FlowFrameState::MappingAwaitKey {
            entry,
            entry_start,
            explicit,
        } = frame.state
        else {
            return Err(invalid_flow_parser_state(
                flow.absolute_start + flow.position,
            ));
        };
        let entry_start = entry_start as usize;
        self.attach_child_at(entry.as_usize(), child);
        flow.position = skip_flow_whitespace(flow.text, flow.position);
        if flow.text[flow.position..].starts_with(':') {
            let value_start = flow.position + 1;
            flow.position = skip_flow_whitespace(flow.text, value_start);
            reject_unindented_split_flow_mapping_value(
                flow.text,
                value_start,
                flow.position,
                flow.absolute_start,
                self.source_indent_at(flow.absolute_start),
            )?;
            if flow.text[flow.position..].starts_with([',', '}']) {
                let value = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
                self.attach_child_at(entry.as_usize(), value);
                self.finish_flow_mapping_entry(
                    frame.node,
                    entry,
                    entry_start,
                    flow.position,
                    flow.absolute_start,
                );
                flow.frames[frame_index].state = FlowFrameState::MappingAfterPair;
            } else {
                flow.frames[frame_index].state = FlowFrameState::MappingAwaitValue {
                    entry,
                    entry_start: Span::usize_to_u32(entry_start),
                };
                self.start_flow_node(flow, frame.close)?;
            }
        } else if explicit || flow.text[flow.position..].starts_with(',') {
            let value = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
            self.attach_child_at(entry.as_usize(), value);
            self.finish_flow_mapping_entry(
                frame.node,
                entry,
                entry_start,
                flow.position,
                flow.absolute_start,
            );
            flow.frames[frame_index].state = FlowFrameState::MappingAfterPair;
        } else {
            let found = flow.text[flow.position..]
                .chars()
                .next()
                .unwrap_or(frame.close);
            return Err(missing_flow_mapping_colon(
                flow.absolute_start + flow.position,
                found,
            ));
        }
        Ok(())
    }

    fn advance_flow_frame(&mut self, flow: &mut FlowParseState<'_>) -> Result<(), YamlError> {
        let frame_index = flow.frames.len() - 1;
        let frame = flow.frames[frame_index];
        flow.position = skip_flow_whitespace(flow.text, flow.position);
        let Some(character) = flow.text[flow.position..].chars().next() else {
            return Err(if frame.close == ']' {
                missing_flow_sequence_end(flow.absolute_start, flow.text.len())
            } else {
                missing_flow_mapping_end(flow.absolute_start, flow.text.len())
            });
        };
        match frame.state {
            FlowFrameState::SequenceExpectItem => {
                self.start_flow_sequence_item(flow, frame_index, frame, character)
            }
            FlowFrameState::SequenceAfterItem => {
                self.advance_flow_sequence_separator(flow, frame_index, frame, character)
            }
            FlowFrameState::MappingExpectKey => {
                self.start_flow_mapping_entry(flow, frame_index, frame, character)
            }
            FlowFrameState::MappingAfterPair => {
                self.advance_flow_mapping_separator(flow, frame_index, frame, character)
            }
            _ => Err(invalid_flow_parser_state(
                flow.absolute_start + flow.position,
            )),
        }
    }

    fn start_flow_sequence_item(
        &mut self,
        flow: &mut FlowParseState<'_>,
        frame_index: usize,
        frame: FlowFrame,
        character: char,
    ) -> Result<(), YamlError> {
        if character == frame.close {
            self.complete_flow_collection(flow, frame, character);
            return Ok(());
        }
        if character == ',' {
            return Err(unexpected_flow_comma(flow.absolute_start + flow.position));
        }
        if matches!(character, ']' | '}') {
            return Err(expected_flow_separator(
                flow.absolute_start + flow.position,
                character,
            ));
        }

        let entry_start = flow.position;
        let entry = self.push_node(
            NodeKind::SequenceEntry,
            Span::empty_from_usize(flow.absolute_start + entry_start),
        );
        let explicit = character == '?' && is_flow_explicit_key_indicator(flow.text, flow.position);
        if explicit {
            flow.position = skip_flow_whitespace(flow.text, flow.position + 1);
        }
        if flow.text[flow.position..].starts_with(':')
            && is_flow_mapping_separator_colon(flow.text, flow.position)
        {
            let mapping = self.push_node(
                NodeKind::FlowMapping,
                Span::empty_from_usize(flow.absolute_start + entry_start),
            );
            self.register_implicit_flow_mapping(mapping);
            let mapping_entry = self.push_node(
                NodeKind::MappingEntry,
                Span::empty_from_usize(flow.absolute_start + entry_start),
            );
            let key = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
            self.attach_child_at(mapping.as_usize(), mapping_entry);
            self.attach_child_at(mapping_entry.as_usize(), key);
            flow.position = skip_flow_whitespace(flow.text, flow.position + 1);
            if flow.text[flow.position..].starts_with([',', ']']) {
                let value = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
                self.attach_child_at(mapping_entry.as_usize(), value);
                self.finish_flow_mapping_sequence_item(
                    frame.node,
                    entry,
                    mapping,
                    mapping_entry,
                    flow.position,
                    flow.absolute_start,
                );
                flow.frames[frame_index].state = FlowFrameState::SequenceAfterItem;
            } else {
                flow.frames[frame_index].state = FlowFrameState::SequenceAwaitValue {
                    entry,
                    mapping,
                    mapping_entry,
                };
                self.start_flow_node(flow, frame.close)?;
            }
        } else if explicit && flow.text[flow.position..].starts_with([',', ']']) {
            let mapping = self.push_node(
                NodeKind::FlowMapping,
                Span::empty_from_usize(flow.absolute_start + entry_start),
            );
            self.register_implicit_flow_mapping(mapping);
            let mapping_entry = self.push_node(
                NodeKind::MappingEntry,
                Span::empty_from_usize(flow.absolute_start + entry_start),
            );
            let key = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
            let value = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
            self.attach_child_at(mapping.as_usize(), mapping_entry);
            self.attach_child_at(mapping_entry.as_usize(), key);
            self.attach_child_at(mapping_entry.as_usize(), value);
            self.finish_flow_mapping_sequence_item(
                frame.node,
                entry,
                mapping,
                mapping_entry,
                flow.position,
                flow.absolute_start,
            );
            flow.frames[frame_index].state = FlowFrameState::SequenceAfterItem;
        } else {
            flow.frames[frame_index].state = FlowFrameState::SequenceAwaitItem {
                entry,
                entry_start: Span::usize_to_u32(entry_start),
                explicit,
            };
            self.start_flow_node(flow, frame.close)?;
        }
        Ok(())
    }

    fn start_flow_mapping_entry(
        &mut self,
        flow: &mut FlowParseState<'_>,
        frame_index: usize,
        frame: FlowFrame,
        character: char,
    ) -> Result<(), YamlError> {
        if character == frame.close {
            self.complete_flow_collection(flow, frame, character);
            return Ok(());
        }
        if character == ',' {
            return Err(unexpected_flow_mapping_comma(
                flow.absolute_start + flow.position,
            ));
        }
        if matches!(character, ']' | '}') {
            return Err(expected_flow_mapping_separator(
                flow.absolute_start + flow.position,
                character,
            ));
        }

        let entry_start = flow.position;
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::empty_from_usize(flow.absolute_start + entry_start),
        );
        let explicit = character == '?' && is_flow_explicit_key_indicator(flow.text, flow.position);
        if explicit {
            flow.position = skip_flow_whitespace(flow.text, flow.position + 1);
        }
        if flow.text[flow.position..].starts_with(':')
            && is_flow_mapping_separator_colon(flow.text, flow.position)
        {
            let key = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
            self.attach_child_at(entry.as_usize(), key);
            let value_start = flow.position + 1;
            flow.position = skip_flow_whitespace(flow.text, value_start);
            reject_unindented_split_flow_mapping_value(
                flow.text,
                value_start,
                flow.position,
                flow.absolute_start,
                self.source_indent_at(flow.absolute_start),
            )?;
            if flow.text[flow.position..].starts_with([',', '}']) {
                let value = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
                self.attach_child_at(entry.as_usize(), value);
                self.finish_flow_mapping_entry(
                    frame.node,
                    entry,
                    entry_start,
                    flow.position,
                    flow.absolute_start,
                );
                flow.frames[frame_index].state = FlowFrameState::MappingAfterPair;
            } else {
                flow.frames[frame_index].state = FlowFrameState::MappingAwaitValue {
                    entry,
                    entry_start: Span::usize_to_u32(entry_start),
                };
                self.start_flow_node(flow, frame.close)?;
            }
        } else if explicit && flow.text[flow.position..].starts_with([',', '}']) {
            let key = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
            let value = self.push_empty_flow_scalar(flow.absolute_start + flow.position);
            self.attach_child_at(entry.as_usize(), key);
            self.attach_child_at(entry.as_usize(), value);
            self.finish_flow_mapping_entry(
                frame.node,
                entry,
                entry_start,
                flow.position,
                flow.absolute_start,
            );
            flow.frames[frame_index].state = FlowFrameState::MappingAfterPair;
        } else {
            flow.frames[frame_index].state = FlowFrameState::MappingAwaitKey {
                entry,
                entry_start: Span::usize_to_u32(entry_start),
                explicit,
            };
            self.start_flow_node(flow, frame.close)?;
        }
        Ok(())
    }

    fn advance_flow_sequence_separator(
        &mut self,
        flow: &mut FlowParseState<'_>,
        frame_index: usize,
        frame: FlowFrame,
        character: char,
    ) -> Result<(), YamlError> {
        match character {
            ',' => {
                flow.position += 1;
                flow.frames[frame_index].state = FlowFrameState::SequenceExpectItem;
                Ok(())
            }
            character if character == frame.close => {
                self.complete_flow_collection(flow, frame, character);
                Ok(())
            }
            _ => Err(expected_flow_separator(
                flow.absolute_start + flow.position,
                character,
            )),
        }
    }

    fn advance_flow_mapping_separator(
        &mut self,
        flow: &mut FlowParseState<'_>,
        frame_index: usize,
        frame: FlowFrame,
        character: char,
    ) -> Result<(), YamlError> {
        match character {
            ',' => {
                flow.position += 1;
                flow.frames[frame_index].state = FlowFrameState::MappingExpectKey;
                Ok(())
            }
            character if character == frame.close => {
                self.complete_flow_collection(flow, frame, character);
                Ok(())
            }
            _ => Err(expected_flow_mapping_separator(
                flow.absolute_start + flow.position,
                character,
            )),
        }
    }

    fn complete_flow_collection(
        &mut self,
        flow: &mut FlowParseState<'_>,
        frame: FlowFrame,
        character: char,
    ) {
        flow.position += character.len_utf8();
        self.nodes[frame.node.as_usize()].span.end =
            Span::usize_to_u32(flow.absolute_start + flow.position);
        if frame.node != flow.root {
            let span = self.nodes[frame.node.as_usize()].span;
            self.semantics
                .finish_flow_collection(&mut self.nodes, frame.node, span);
        }
        flow.frames.pop();
        flow.completed = Some((frame.node, flow.position));
    }

    fn start_flow_node(
        &mut self,
        flow: &mut FlowParseState<'_>,
        close: char,
    ) -> Result<(), YamlError> {
        match self.prepare_flow_node(flow.text, flow.position, flow.absolute_start, close)? {
            FlowNode::Scalar(node, end) => {
                flow.completed = Some((node, end));
            }
            FlowNode::Collection(frame, after_open) => {
                if flow.frames.len() >= MAX_FLOW_COLLECTION_DEPTH {
                    return Err(flow_collection_depth_limit_exceeded(
                        flow.absolute_start + after_open - 1,
                    ));
                }
                flow.frames.push(frame);
                flow.position = after_open;
            }
        }
        Ok(())
    }

    fn prepare_flow_node(
        &mut self,
        text: &str,
        start: usize,
        absolute_start: usize,
        close: char,
    ) -> Result<FlowNode, YamlError> {
        let properties = parse_node_properties(
            &text[start..],
            Span::from_usize(absolute_start + start, absolute_start + text.len()),
        )?;
        reject_invalid_node_property_placement(
            &text[start..],
            absolute_start + start,
            &properties,
        )?;
        let marker = start
            + properties.value_start()
            + leading_flow_whitespace(&text[start + properties.value_start()..]);
        let Some(character) = text[marker..].chars().next() else {
            return Err(empty_flow_value(absolute_start + start));
        };
        if (properties.anchor().is_some() || properties.tag().is_some())
            && (character == ','
                || character == close
                || character == ':' && is_flow_mapping_separator_colon(text, marker))
        {
            let property_end = marker - trailing_flow_whitespace(&text[start..marker]);
            let node = self.push_node(
                NodeKind::Scalar,
                Span::from_usize(absolute_start + start, absolute_start + property_end),
            );
            self.register_flow_scalar_semantics(node, properties, None)?;
            return Ok(FlowNode::Scalar(node, marker));
        }
        if matches!(character, '[' | '{') {
            let node = self.push_node(
                if character == '[' {
                    NodeKind::FlowSequence
                } else {
                    NodeKind::FlowMapping
                },
                Span::from_usize(
                    absolute_start + start,
                    absolute_start + marker + character.len_utf8(),
                ),
            );
            self.register_flow_collection_semantics(node, properties, None)?;
            return Ok(FlowNode::Collection(
                flow_frame(node, character),
                marker + character.len_utf8(),
            ));
        }

        let end = flow_frame_scalar_end(text, marker, absolute_start, close)?;
        let trimmed_end = end - trailing_flow_whitespace(&text[start..end]);
        if marker >= trimmed_end {
            return Err(empty_flow_value(absolute_start + start));
        }
        let node = self.push_node(
            NodeKind::Scalar,
            Span::from_usize(absolute_start + start, absolute_start + trimmed_end),
        );
        self.mark_scalar_syntax(
            node,
            scalar_style_from_first_char(character),
            matches!(character, '"' | '\''),
        );
        self.register_flow_scalar_semantics(node, properties, None)?;
        Ok(FlowNode::Scalar(node, end))
    }

    fn finish_flow_mapping_sequence_item(
        &mut self,
        sequence: NodeId,
        entry: NodeId,
        mapping: NodeId,
        mapping_entry: NodeId,
        end: usize,
        absolute_start: usize,
    ) {
        let absolute_end = Span::usize_to_u32(absolute_start + end);
        self.nodes[mapping_entry.as_usize()].span.end = absolute_end;
        self.nodes[mapping.as_usize()].span.end = absolute_end;
        let mapping_span = self.nodes[mapping.as_usize()].span;
        self.semantics
            .finish_flow_collection(&mut self.nodes, mapping, mapping_span);
        self.attach_child_at(entry.as_usize(), mapping);
        self.nodes[entry.as_usize()].span.end = Span::usize_to_u32(absolute_start + end);
        self.attach_child_at(sequence.as_usize(), entry);
    }

    fn finish_flow_mapping_entry(
        &mut self,
        mapping: NodeId,
        entry: NodeId,
        entry_start: usize,
        end: usize,
        absolute_start: usize,
    ) {
        self.nodes[entry.as_usize()].span =
            Span::from_usize(absolute_start + entry_start, absolute_start + end);
        self.attach_child_at(mapping.as_usize(), entry);
        self.nodes[mapping.as_usize()].span.end = Span::usize_to_u32(absolute_start + end);
    }

    fn register_implicit_flow_mapping(&mut self, mapping: NodeId) {
        let span = self.nodes[mapping.as_usize()].span;
        self.semantics.register_flow_collection(
            &mut self.nodes,
            mapping,
            span,
            CollectionStyle::Flow,
            true,
            SemanticProperties::NONE,
        );
    }

    fn parse_block_scalar(
        &mut self,
        lines: LineTable<'_>,
        index: usize,
        header_start: usize,
        parent_indent: usize,
        header: &str,
        allow_same_indent_content: bool,
    ) -> Result<(NodeId, usize), YamlError> {
        let header = parse_block_scalar_header(header, header_start)?;
        let mut consumed = 1;
        let mut content_indent = header
            .indent
            .map_or(usize::MAX, |indent| parent_indent + indent);
        let initial_line = lines
            .cursor_from(index)
            .next()
            .expect("current line index is in bounds");
        let mut end = initial_line.line_end;
        let inline_header =
            header_start > initial_line.content_start + parent_indent && !allow_same_indent_content;
        let mut pending_blank_lines = 0usize;
        let mut pending_blank_end = end;
        let mut pending_blank_indent = None::<usize>;
        let mut reached_end = true;

        for line in lines.iter_from(index + 1) {
            let trimmed = line.content_without_break.trim();
            if trimmed == "---" || trimmed == "..." {
                reached_end = false;
                break;
            }

            let tab_content_line =
                trimmed.is_empty() && line.content_without_break.as_bytes().contains(&b'\t');
            if tab_content_line
                && content_indent == usize::MAX
                && count_literal_content_indent(line.content_without_break) == 0
            {
                return Err(tab_indentation_error(line.content_start));
            }
            if trimmed.is_empty() && !tab_content_line {
                if content_indent == usize::MAX && inline_header {
                    pending_blank_lines += 1;
                    pending_blank_end = line.line_end;
                    let indent = count_literal_content_indent(line.content_without_break);
                    pending_blank_indent =
                        Some(pending_blank_indent.map_or(indent, |current| current.min(indent)));
                    continue;
                }
                consumed += 1;
                end = line.line_end;
                continue;
            }

            let indent = count_literal_content_indent(line.content_without_break);
            if content_indent == usize::MAX
                && pending_blank_lines > 0
                && indent <= pending_blank_indent.unwrap_or(usize::MAX)
                && line.content_without_break[indent..].starts_with('#')
            {
                return Err(invalid_block_scalar_content(line.content_start + indent));
            }
            if content_indent == usize::MAX {
                if indent <= parent_indent && (parent_indent > 0 || inline_header) {
                    reached_end = false;
                    break;
                }
                content_indent = indent;
            }

            if indent < content_indent {
                reached_end = false;
                break;
            }

            if pending_blank_lines > 0 {
                pending_blank_lines = 0;
            }
            consumed += 1;
            end = line.line_end;
        }

        if reached_end && pending_blank_lines > 0 {
            consumed += pending_blank_lines;
            end = pending_blank_end;
            if content_indent == usize::MAX {
                content_indent = pending_blank_indent.unwrap_or_default();
            }
        }

        let scalar = self.push_node(header.kind.node_kind(), Span::from_usize(header_start, end));
        let content_indent = if content_indent == usize::MAX {
            header.indent.unwrap_or_default()
        } else {
            content_indent
        };
        self.block_scalar_indents.push(BlockScalarIndent {
            node: scalar,
            indent: Span::usize_to_u32(content_indent),
        });
        Ok((scalar, consumed))
    }

    fn mapping_for_line(&mut self, parent: NodeId, indent: usize, span: Span) -> NodeId {
        if let Some(node) = self.collection_at(self.last_mapping_frame, indent) {
            node
        } else {
            let (span, properties) = self.collection_properties(indent, span);
            let mapping = self.push_node(NodeKind::BlockMapping, span);
            self.attach_child_at(parent.0 as usize, mapping);
            self.push_block_frame(BlockFrame {
                indent: Span::usize_to_u32(indent),
                node: mapping,
                collection: OpenEventCollection::Mapping,
                previous_same_kind: NO_BLOCK_FRAME,
            });
            self.open_event_collection(
                indent,
                OpenEventCollection::Mapping,
                YamlEventKind::MappingStart {
                    style: CollectionStyle::Block,
                    tag: None,
                    anchor: None,
                },
                span,
                mapping,
                semantic_properties(&properties, None),
            );
            mapping
        }
    }

    fn document_has_root_flow_collection(&self, document: NodeId) -> bool {
        Children::new(&self.nodes, document).any(|child| {
            matches!(
                self.nodes[child.as_usize()].kind,
                NodeKind::FlowSequence | NodeKind::FlowMapping
            )
        })
    }

    fn sequence_for_line(&mut self, parent: NodeId, indent: usize, span: Span) -> NodeId {
        if let Some(node) = self.collection_at(self.last_sequence_frame, indent) {
            node
        } else {
            let (span, properties) = self.collection_properties(indent, span);
            let sequence = self.push_node(NodeKind::BlockSequence, span);
            self.attach_child_at(parent.0 as usize, sequence);
            self.push_block_frame(BlockFrame {
                indent: Span::usize_to_u32(indent),
                node: sequence,
                collection: OpenEventCollection::Sequence,
                previous_same_kind: NO_BLOCK_FRAME,
            });
            self.open_event_collection(
                indent,
                OpenEventCollection::Sequence,
                YamlEventKind::SequenceStart {
                    style: CollectionStyle::Block,
                    tag: None,
                    anchor: None,
                },
                span,
                sequence,
                semantic_properties(&properties, None),
            );
            sequence
        }
    }

    fn collection_at(&self, frame: u32, indent: usize) -> Option<NodeId> {
        let frame = self.block_frames.get(frame as usize)?;
        (frame.indent as usize == indent).then_some(frame.node)
    }

    fn push_block_frame(&mut self, mut frame: BlockFrame) {
        let index = u32::try_from(self.block_frames.len())
            .expect("block collection stack exceeds u32 capacity");
        let last = match frame.collection {
            OpenEventCollection::Mapping => &mut self.last_mapping_frame,
            OpenEventCollection::Sequence => &mut self.last_sequence_frame,
        };
        frame.previous_same_kind = *last;
        *last = index;
        self.block_frames.push(frame);
    }

    fn clear_block_frames(&mut self) {
        self.block_frames.clear();
        self.last_mapping_frame = NO_BLOCK_FRAME;
        self.last_sequence_frame = NO_BLOCK_FRAME;
    }

    fn collection_properties(&mut self, indent: usize, span: Span) -> (Span, NodeProperties) {
        let Some(pending) = self.take_pending_node_properties(indent) else {
            return (span, NodeProperties::default());
        };
        (Span::new(pending.span_start, span.end), pending.properties)
    }

    fn take_pending_node_properties(&mut self, indent: usize) -> Option<PendingNodeProperties> {
        if self
            .pending_properties
            .last()
            .is_some_and(|pending| pending.indent as usize == indent)
        {
            return self.pending_properties.pop();
        }
        let index = self
            .pending_properties
            .iter()
            .rposition(|pending| pending.indent as usize == indent)?;
        Some(self.pending_properties.remove(index))
    }

    fn take_pending_node_properties_at(&mut self, offset: usize) -> Option<PendingNodeProperties> {
        if self.pending_properties.is_empty() {
            return None;
        }
        self.take_pending_node_properties(self.source_indent_at(offset))
    }

    fn source_indent_at(&self, offset: usize) -> usize {
        let text = self.source.as_str();
        let line_start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
        content_line_indent(&text[line_start..offset])
    }

    fn validate_indent(
        &self,
        indent: usize,
        line: SourceLine<'_>,
        body: &str,
    ) -> Result<(), YamlError> {
        if indent == 0 {
            return Ok(());
        }

        let has_parent_collection = self.parent_collection_below(indent);

        let is_indented_root_collection = !has_parent_collection
            && (is_sequence_entry(body) || body_starts_flow_value_start(body));

        if has_parent_collection || is_indented_root_collection {
            Ok(())
        } else {
            Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Parser,
                    "invalid indentation without a parent collection",
                    Span::from_usize(line.content_start, line.content_start + indent),
                )
                .with_expected("a parent mapping or sequence at a lower indentation level"),
            ))
        }
    }

    fn parent_collection_below(&self, indent: usize) -> bool {
        self.block_frames
            .first()
            .is_some_and(|frame| (frame.indent as usize) < indent)
    }

    fn mapping_is_open_at(&self, indent: usize) -> bool {
        self.collection_at(self.last_mapping_frame, indent)
            .is_some()
    }

    fn sequence_is_open_at(&self, indent: usize) -> bool {
        self.collection_at(self.last_sequence_frame, indent)
            .is_some()
    }

    fn reject_invalid_block_sibling(
        &self,
        indent: usize,
        line: SourceLine<'_>,
        body: &str,
        known_mapping: bool,
    ) -> Result<(), YamlError> {
        if self.sequence_is_open_at(indent) && !is_sequence_entry(body) {
            let mapping_is_open_at_indent = self.mapping_is_open_at(indent);
            let is_mapping_sibling = is_explicit_mapping_key(body)
                || is_explicit_mapping_value(body)
                || known_mapping
                || flow_collection_mapping_key_colon(body, line.content_start + indent)?.is_some()
                || find_mapping_colon(body).is_some();
            if !mapping_is_open_at_indent || !is_mapping_sibling {
                return Err(invalid_nested_block_sequence_sibling(
                    line.content_start + indent,
                ));
            }
        }
        if self.mapping_is_open_at(indent) && comment_text_contains_mapping_colon(body) {
            return Err(invalid_orphaned_block_content(
                line.content_start + indent + separated_comment_offset(body).unwrap_or(0),
            ));
        }

        let has_collection_at_indent =
            self.mapping_is_open_at(indent) || self.sequence_is_open_at(indent);
        let is_indented_root_collection = indent > 0
            && !self.parent_collection_below(indent)
            && (is_sequence_entry(body) || body_starts_flow_value_start(body));
        if indent > 0
            && !has_collection_at_indent
            && !is_indented_root_collection
            && (is_sequence_entry(body)
                || is_explicit_mapping_key(body)
                || is_explicit_mapping_value(body)
                || known_mapping
                || find_mapping_colon(body).is_some())
        {
            return Err(invalid_orphaned_block_content(line.content_start + indent));
        }

        Ok(())
    }

    fn close_collections_deeper_than(&mut self, indent: usize) {
        while self
            .block_frames
            .last()
            .is_some_and(|frame| frame.indent as usize > indent)
        {
            self.close_last_event_collection();
        }
    }

    fn close_sequence_at_indent(&mut self, indent: usize) {
        if self.block_frames.last().is_some_and(|frame| {
            frame.indent as usize == indent && frame.collection == OpenEventCollection::Sequence
        }) {
            self.close_last_event_collection();
        }
    }

    fn push_node(&mut self, kind: NodeKind, span: Span) -> NodeId {
        let id = NodeId::from_usize(self.nodes.len());
        self.nodes.push(Node {
            kind,
            syntax_flags: 0,
            span,
            parent: NO_NODE,
            first_child: NO_NODE,
            last_child: NO_NODE,
            next_sibling: NO_NODE,
            semantic: NO_SEMANTIC_NODE,
        });
        id
    }

    fn mark_scalar_syntax(&mut self, node: NodeId, style: YamlScalarStyle, validated: bool) {
        let style = match style {
            YamlScalarStyle::Plain => SCALAR_STYLE_PLAIN,
            YamlScalarStyle::SingleQuoted => SCALAR_STYLE_SINGLE_QUOTED,
            YamlScalarStyle::DoubleQuoted => SCALAR_STYLE_DOUBLE_QUOTED,
            YamlScalarStyle::Literal | YamlScalarStyle::Folded => 0,
        };
        self.nodes[node.as_usize()].syntax_flags =
            style | (u8::from(validated) * SCALAR_SYNTAX_VALIDATED);
    }

    fn attach_child_at(&mut self, parent_index: usize, child: NodeId) {
        debug_assert_eq!(self.nodes[child.as_usize()].parent, NO_NODE);
        let parent = NodeId::from_usize(parent_index);
        let previous = node_link(self.nodes[parent_index].last_child);
        self.nodes[child.as_usize()].parent = parent.0;
        if let Some(previous) = previous {
            self.nodes[previous.as_usize()].next_sibling = child.0;
        } else {
            self.nodes[parent_index].first_child = child.0;
        }
        self.nodes[parent_index].last_child = child.0;
    }

    fn push_empty_scalar(&mut self, offset: usize) -> NodeId {
        self.push_node(NodeKind::Scalar, Span::empty_from_usize(offset))
    }

    fn push_empty_flow_scalar(&mut self, offset: usize) -> NodeId {
        let node = self.push_empty_scalar(offset);
        self.semantics.register_flow_scalar(
            &mut self.nodes,
            node,
            Span::empty_from_usize(offset),
            SemanticKind::Scalar {
                style: YamlScalarStyle::Plain,
            },
            SemanticProperties::NONE,
        );
        node
    }

    fn extend_node_span(&mut self, node: NodeId, end: usize) {
        let end = Span::usize_to_u32(end);
        let mut current = Some(node);
        while let Some(node) = current {
            let syntax = &mut self.nodes[node.as_usize()];
            syntax.span.end = syntax.span.end.max(end);
            current = node_link(syntax.parent);
        }
    }

    fn push_event(&mut self, kind: YamlEventKind, span: Span) {
        self.semantics
            .push(&mut self.nodes, kind, span, None, SemanticProperties::NONE);
    }

    fn push_node_event(&mut self, kind: YamlEventKind, span: Span, cst: NodeId) {
        self.semantics.push(
            &mut self.nodes,
            kind,
            span,
            Some(cst),
            SemanticProperties::NONE,
        );
    }

    fn push_node_event_with_properties(
        &mut self,
        kind: YamlEventKind,
        span: Span,
        cst: NodeId,
        properties: SemanticProperties,
    ) {
        self.semantics
            .push(&mut self.nodes, kind, span, Some(cst), properties);
    }

    fn open_event_collection(
        &mut self,
        _indent: usize,
        _collection: OpenEventCollection,
        kind: YamlEventKind,
        span: Span,
        cst: NodeId,
        properties: SemanticProperties,
    ) {
        self.push_node_event_with_properties(kind, span, cst, properties);
    }

    fn close_event_collections_deeper_than(&mut self, indent: usize) {
        while self
            .last_event_collection()
            .is_some_and(|(level, _)| level > indent)
        {
            self.close_last_event_collection();
        }
    }

    fn close_all_event_collections(&mut self) {
        while self.last_event_collection().is_some() {
            self.close_last_event_collection();
        }
    }

    fn close_last_event_collection(&mut self) {
        let Some(frame) = self.block_frames.pop() else {
            return;
        };
        match frame.collection {
            OpenEventCollection::Mapping => {
                debug_assert_eq!(self.last_mapping_frame as usize, self.block_frames.len());
                self.last_mapping_frame = frame.previous_same_kind;
            }
            OpenEventCollection::Sequence => {
                debug_assert_eq!(self.last_sequence_frame as usize, self.block_frames.len());
                self.last_sequence_frame = frame.previous_same_kind;
            }
        }
        let offset = Span::usize_to_u32(self.source.len());
        let kind = match frame.collection {
            OpenEventCollection::Mapping => YamlEventKind::MappingEnd,
            OpenEventCollection::Sequence => YamlEventKind::SequenceEnd,
        };
        self.push_event(kind, Span::empty(offset));
    }

    fn last_event_collection(&self) -> Option<(usize, OpenEventCollection)> {
        self.block_frames
            .last()
            .map(|frame| (frame.indent as usize, frame.collection))
    }

    fn register_flow_collection_semantics(
        &mut self,
        node: NodeId,
        mut properties: NodeProperties,
        pending: Option<PendingNodeProperties>,
    ) -> Result<(), YamlError> {
        let span = self.nodes[node.as_usize()].span;
        merge_pending_properties(&mut properties, pending);
        self.resolve_node_properties(&properties, span)?;
        let semantic_properties = semantic_properties(&properties, None);
        let mapping = self.nodes[node.as_usize()].kind == NodeKind::FlowMapping;
        self.semantics.register_flow_collection(
            &mut self.nodes,
            node,
            span,
            CollectionStyle::Flow,
            mapping,
            semantic_properties,
        );
        Ok(())
    }

    fn register_flow_scalar_semantics(
        &mut self,
        node: NodeId,
        mut properties: NodeProperties,
        pending: Option<PendingNodeProperties>,
    ) -> Result<(), YamlError> {
        let node_span = self.nodes[node.as_usize()].span;
        let syntax_flags = self.nodes[node.as_usize()].syntax_flags;
        let text = self.source.slice(node_span);
        merge_pending_properties(&mut properties, pending);
        self.resolve_node_properties(&properties, node_span)?;
        let value_start = properties.value_start().min(text.len());
        let value_text = &text[value_start..];
        let style = scalar_style_from_flags(syntax_flags).unwrap_or_else(|| {
            scalar_style_from_first_char(value_text.chars().next().unwrap_or(' '))
        });
        let trimmed = strip_inline_comment(value_text).trim();
        if style == YamlScalarStyle::Plain
            && let Some(alias) = trimmed.strip_prefix('*')
            && !alias.is_empty()
            && !alias.chars().any(char::is_whitespace)
        {
            let alias_start = node_span.start as usize
                + value_start
                + value_text
                    .len()
                    .saturating_sub(value_text.trim_start().len())
                + 1;
            self.semantics.register_flow_scalar(
                &mut self.nodes,
                node,
                node_span,
                SemanticKind::Alias,
                SemanticProperties {
                    alias: Some(Span::from_usize(alias_start, alias_start + alias.len())),
                    ..semantic_properties(&properties, None)
                },
            );
            return Ok(());
        }
        self.semantics.register_flow_scalar(
            &mut self.nodes,
            node,
            node_span,
            SemanticKind::Scalar { style },
            semantic_properties(&properties, None),
        );
        Ok(())
    }

    fn emit_scalar_event(&mut self, node: NodeId) -> Result<(), YamlError> {
        let node_id = node;
        let block_scalar_content_indent = self.take_block_scalar_indent(node_id);
        let node_kind = self.nodes[node.0 as usize].kind;
        let node_span = self.nodes[node.0 as usize].span;
        let syntax_flags = self.nodes[node.0 as usize].syntax_flags;
        let text = self.source.slice(node_span);
        let mut properties = parse_node_properties(text, node_span)?;
        self.resolve_node_properties(&properties, node_span)?;
        let span =
            if let Some(pending) = self.take_pending_node_properties_at(node_span.start as usize) {
                if properties.anchor().is_none() {
                    properties.set_anchor(pending.properties.anchor());
                }
                if properties.tag().is_none() {
                    properties.set_tag(pending.properties.tag());
                }
                Span::new(pending.span_start, node_span.end)
            } else {
                node_span
            };
        let value_text = &text[properties.value_start()..];
        let style = match node_kind {
            NodeKind::LiteralScalar => YamlScalarStyle::Literal,
            NodeKind::FoldedScalar => YamlScalarStyle::Folded,
            NodeKind::Scalar => scalar_style_from_flags(syntax_flags).unwrap_or_else(|| {
                scalar_style_from_first_char(value_text.chars().next().unwrap_or(' '))
            }),
            _ => unreachable!("emit_scalar_event only receives scalar nodes"),
        };
        let syntax_validated = syntax_flags & SCALAR_SYNTAX_VALIDATED != 0;
        let quoted_end = if syntax_validated {
            None
        } else if style == YamlScalarStyle::DoubleQuoted {
            Some(double_quoted_scalar_end(value_text).ok_or_else(|| {
                unterminated_quoted_scalar_error(
                    '"',
                    Span::from_usize(
                        node_span.start as usize + properties.value_start(),
                        node_span.end as usize,
                    ),
                )
            })?)
        } else if style == YamlScalarStyle::SingleQuoted {
            let _ = single_quoted_scalar_end(value_text).ok_or_else(|| {
                unterminated_quoted_scalar_error(
                    '\'',
                    Span::from_usize(
                        node_span.start as usize + properties.value_start(),
                        node_span.end as usize,
                    ),
                )
            })?;
            None
        } else {
            None
        };
        if style == YamlScalarStyle::Plain && !syntax_validated {
            let trimmed = strip_inline_comment(value_text).trim();
            if let Some(alias) = trimmed.strip_prefix('*')
                && !alias.is_empty()
                && !alias.chars().any(char::is_whitespace)
            {
                let alias_start = node_span.start as usize
                    + properties.value_start()
                    + value_text
                        .len()
                        .saturating_sub(value_text.trim_start().len())
                    + 1;
                let semantic_properties = SemanticProperties {
                    alias: Some(Span::from_usize(alias_start, alias_start + alias.len())),
                    ..semantic_properties(&properties, None)
                };
                self.push_node_event_with_properties(
                    YamlEventKind::Alias {
                        name: alias.to_owned(),
                    },
                    span,
                    node,
                    semantic_properties,
                );
                return Ok(());
            }
        }
        if let Some(end) = quoted_end {
            validate_double_quoted_scalar_content(&value_text[1..end - 1])?;
            validate_double_quoted_scalar_escapes(&value_text[1..end - 1])?;
        }
        let content_indent = block_scalar_content_indent.map(Span::usize_to_u32);
        let semantic_properties = semantic_properties(&properties, content_indent);
        self.push_node_event_with_properties(
            YamlEventKind::Scalar {
                style,
                value: String::new(),
                tag: None,
                anchor: None,
            },
            span,
            node,
            semantic_properties,
        );
        Ok(())
    }

    fn emit_property_free_plain_scalar(&mut self, node: NodeId) {
        self.emit_property_free_scalar(node, YamlScalarStyle::Plain);
    }

    fn emit_property_free_scalar(&mut self, node: NodeId, style: YamlScalarStyle) {
        let span = self.nodes[node.as_usize()].span;
        self.semantics
            .push_property_free_scalar(&mut self.nodes, node, span, style);
    }

    fn take_block_scalar_indent(&mut self, node: NodeId) -> Option<usize> {
        if self
            .block_scalar_indents
            .last()
            .is_some_and(|record| record.node == node)
        {
            return self
                .block_scalar_indents
                .pop()
                .map(|record| record.indent as usize);
        }
        let index = self
            .block_scalar_indents
            .iter()
            .rposition(|record| record.node == node)?;
        Some(self.block_scalar_indents.remove(index).indent as usize)
    }

    fn resolve_node_properties(
        &self,
        properties: &NodeProperties,
        span: Span,
    ) -> Result<(), YamlError> {
        if let Some(tag) = properties.tag() {
            let _ = resolve_tag(self.source.slice(tag), &self.tag_handles, span)?;
        }
        Ok(())
    }
}

fn unterminated_quoted_scalar_error(quote: char, span: Span) -> YamlError {
    let scalar_name = if quote == '"' {
        "double-quoted scalar"
    } else {
        "single-quoted scalar"
    };
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Lexer,
            format!("unterminated {scalar_name}"),
            span,
        )
        .with_expected(format!("closing {quote}")),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct NodeProperties {
    tag: Option<Span>,
    anchor: Option<Span>,
    value_start: usize,
}

impl NodeProperties {
    pub(crate) fn tag(self) -> Option<Span> {
        self.tag
    }

    pub(crate) fn anchor(self) -> Option<Span> {
        self.anchor
    }

    pub(crate) const fn value_start(self) -> usize {
        self.value_start
    }

    fn set_tag(&mut self, tag: Option<Span>) {
        self.tag = tag;
    }

    fn set_anchor(&mut self, anchor: Option<Span>) {
        self.anchor = anchor;
    }

    fn set_value_start(&mut self, value_start: usize) {
        self.value_start = value_start;
    }
}

fn semantic_properties(
    properties: &NodeProperties,
    content_indent: Option<u32>,
) -> SemanticProperties {
    SemanticProperties {
        tag: properties.tag(),
        anchor: properties.anchor(),
        alias: None,
        content_indent,
    }
}

fn merge_pending_properties(
    properties: &mut NodeProperties,
    pending: Option<PendingNodeProperties>,
) {
    let Some(pending) = pending else {
        return;
    };
    if properties.anchor().is_none() {
        properties.set_anchor(pending.properties.anchor());
    }
    if properties.tag().is_none() {
        properties.set_tag(pending.properties.tag());
    }
}

fn reject_invalid_node_property_placement(
    text: &str,
    absolute_start: usize,
    properties: &NodeProperties,
) -> Result<(), YamlError> {
    if properties.anchor().is_none() && properties.tag().is_none() {
        return Ok(());
    }

    let value_start =
        properties.value_start() + leading_flow_whitespace(&text[properties.value_start()..]);
    let value_text = &text[value_start..];
    let Some(first) = value_text.chars().next() else {
        return Ok(());
    };

    if first == '*' || (first == '-' && is_sequence_entry(value_text)) {
        return Err(invalid_node_property_placement(
            absolute_start + value_start,
            first,
        ));
    }

    Ok(())
}

fn reject_invalid_block_node_property_punctuation(
    text: &str,
    absolute_start: usize,
    properties: &NodeProperties,
) -> Result<(), YamlError> {
    if properties.anchor().is_none() && properties.tag().is_none() {
        return Ok(());
    }
    let value_start =
        properties.value_start() + leading_flow_whitespace(&text[properties.value_start()..]);
    if text[value_start..].starts_with(',') {
        return Err(invalid_node_property_placement(
            absolute_start + value_start,
            ',',
        ));
    }
    Ok(())
}

fn invalid_node_property_placement(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("invalid node property placement before `{found}`"),
            Span::from_usize(offset, offset + found.len_utf8()),
        )
        .with_expected("a scalar or collection node"),
    )
}

fn body_starts_flow_value(body: &str, absolute_start: usize) -> Result<bool, YamlError> {
    if !body_may_start_flow_value(body) {
        return Ok(false);
    }
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
    reject_invalid_node_property_placement(body, absolute_start, &properties)?;
    let value_start =
        properties.value_start() + leading_flow_whitespace(&body[properties.value_start()..]);
    Ok(matches!(
        body[value_start..].chars().next(),
        Some('[' | '{')
    ))
}

fn body_may_start_flow_value(body: &str) -> bool {
    matches!(
        body.trim_start_matches([' ', '\t']).as_bytes().first(),
        Some(b'[' | b'{' | b'&' | b'!')
    )
}

fn body_may_start_with_node_properties(body: &str) -> bool {
    matches!(
        body.trim_start_matches([' ', '\t']).as_bytes().first(),
        Some(b'&' | b'!')
    )
}

fn flow_collection_mapping_key_colon(
    body: &str,
    absolute_start: usize,
) -> Result<Option<usize>, YamlError> {
    if !body_starts_flow_value(body, absolute_start)? {
        return Ok(None);
    }
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
    let marker_offset =
        properties.value_start() + leading_flow_whitespace(&body[properties.value_start()..]);
    let Ok(flow_end) =
        flow_collection_source_end(&body[marker_offset..], absolute_start + marker_offset)
    else {
        return Ok(None);
    };
    let end = marker_offset + flow_end;
    let separator = skip_flow_whitespace(body, end);
    if body[separator..].starts_with(':') {
        Ok(Some(separator))
    } else {
        Ok(None)
    }
}

fn block_scalar_after_node_properties(
    body: &str,
    absolute_start: usize,
) -> Result<Option<usize>, YamlError> {
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
    if properties.anchor().is_none() && properties.tag().is_none() {
        return Ok(None);
    }
    let header_offset =
        properties.value_start() + leading_flow_whitespace(&body[properties.value_start()..]);
    if matches!(body[header_offset..].chars().next(), Some('|' | '>')) {
        Ok(Some(header_offset))
    } else {
        Ok(None)
    }
}

fn property_only_block_collection_indent(
    body: &str,
    lines: LineTable<'_>,
    index: usize,
    absolute_start: usize,
) -> Result<Option<usize>, YamlError> {
    let Some(indent) = property_only_node_indent(body, lines, index, absolute_start)? else {
        return Ok(None);
    };
    let Some((_, nested_body)) = first_non_property_node_after(lines, index, absolute_start)?
    else {
        return Ok(None);
    };
    if is_sequence_entry(nested_body)
        || is_explicit_mapping_key(nested_body)
        || find_mapping_colon(nested_body).is_some()
    {
        Ok(Some(indent))
    } else {
        Ok(None)
    }
}

fn property_only_mapping_value_collection_indent(
    body: &str,
    lines: LineTable<'_>,
    index: usize,
    absolute_start: usize,
    parent_indent: usize,
) -> Result<Option<usize>, YamlError> {
    reject_invalid_anchor_only_nested_property_mapping(body, lines, index, absolute_start)?;
    let Some(indent) = property_only_block_collection_indent(body, lines, index, absolute_start)?
    else {
        return Ok(None);
    };
    if indent > parent_indent {
        return Ok(Some(indent));
    }

    let Some((_, nested_indent, nested_body)) =
        first_non_property_node_after_with_index(lines, index, absolute_start)?
    else {
        return Ok(None);
    };
    if nested_indent == parent_indent && is_sequence_entry(nested_body) {
        Ok(Some(indent))
    } else {
        Ok(None)
    }
}

fn reject_invalid_anchor_only_nested_property_mapping(
    body: &str,
    lines: LineTable<'_>,
    index: usize,
    absolute_start: usize,
) -> Result<(), YamlError> {
    let body = strip_inline_comment(body).trim_end();
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
    if properties.anchor().is_none()
        || properties.tag().is_some()
        || properties.value_start() < body.len()
    {
        return Ok(());
    }

    let Some((_, _, nested_body)) =
        first_non_property_node_after_with_index(lines, index, absolute_start)?
    else {
        return Ok(());
    };
    let nested_body = strip_inline_comment(nested_body).trim_end();
    let nested_properties = parse_node_properties(
        nested_body,
        Span::from_usize(absolute_start, absolute_start + nested_body.len()),
    )?;
    if (nested_properties.anchor().is_some() || nested_properties.tag().is_some())
        && nested_properties.value_start() < nested_body.len()
        && find_mapping_colon(nested_body).is_none()
    {
        return Err(invalid_node_property_placement(
            absolute_start + nested_properties.value_start(),
            nested_body[nested_properties.value_start()..]
                .chars()
                .next()
                .unwrap_or(':'),
        ));
    }

    Ok(())
}

fn first_non_property_node_after(
    lines: LineTable<'_>,
    index: usize,
    absolute_start: usize,
) -> Result<Option<(usize, &str)>, YamlError> {
    Ok(
        first_non_property_node_after_with_index(lines, index, absolute_start)?
            .map(|(_, indent, body)| (indent, body)),
    )
}

fn first_non_property_node_after_with_index(
    lines: LineTable<'_>,
    index: usize,
    absolute_start: usize,
) -> Result<Option<(usize, usize, &str)>, YamlError> {
    let mut scan_index = index;
    while let Some((next_index, indent, nested_body)) =
        next_significant_body_with_index(lines, scan_index)
    {
        let nested_body = strip_inline_comment(nested_body).trim_end();
        let nested_properties = parse_node_properties(
            nested_body,
            Span::from_usize(absolute_start, absolute_start + nested_body.len()),
        )?;
        if (nested_properties.anchor().is_some() || nested_properties.tag().is_some())
            && nested_properties.value_start() == nested_body.len()
        {
            scan_index = next_index;
            continue;
        }
        return Ok(Some((next_index, indent, nested_body)));
    }
    Ok(None)
}

fn property_only_node_indent(
    body: &str,
    lines: LineTable<'_>,
    index: usize,
    absolute_start: usize,
) -> Result<Option<usize>, YamlError> {
    if !body_may_start_with_node_properties(body) {
        return Ok(None);
    }
    let body = strip_inline_comment(body).trim_end();
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
    if properties.anchor().is_none() && properties.tag().is_none() {
        return Ok(None);
    }
    if properties.value_start() < body.len() {
        return Ok(None);
    }

    Ok(first_non_property_node_after(lines, index, absolute_start)?.map(|(indent, _)| indent))
}

fn next_significant_body_with_index(
    lines: LineTable<'_>,
    current_index: usize,
) -> Option<(usize, usize, &str)> {
    if let Some(next_index) = lines.source.cached_next_significant_line(current_index) {
        return next_index.map(|index| {
            let line = lines
                .cursor_from(index)
                .next()
                .expect("line index from source table is in bounds");
            let indent = line
                .facts
                .indent()
                .unwrap_or_else(|| content_line_indent(line.content_without_break));
            (index, indent, &line.content_without_break[indent..])
        });
    }

    for (relative_index, line) in lines.iter_from(current_index + 1).enumerate() {
        let index = current_index + 1 + relative_index;
        let trimmed = line.content_without_break.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = content_line_indent(line.content_without_break);
        return Some((index, indent, &line.content_without_break[indent..]));
    }

    None
}

pub(crate) fn resolve_tag(
    tag: &str,
    handles: &BTreeMap<String, String>,
    span: Span,
) -> Result<String, YamlError> {
    if let Some(verbatim) = tag.strip_prefix("!<").and_then(|tag| tag.strip_suffix('>')) {
        return Ok(verbatim.to_owned());
    }

    let (handle, suffix) = tag_handle_and_suffix(tag);
    let prefix = handles.get(handle).map(String::as_str).or(match handle {
        "!" => Some("!"),
        "!!" => Some("tag:yaml.org,2002:"),
        _ => None,
    });
    let Some(prefix) = prefix else {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                format!("unresolved tag handle `{handle}`"),
                span,
            )
            .with_expected("a matching %TAG directive"),
        ));
    };
    let suffix = percent_decode_tag_suffix(suffix, span)?;
    Ok(format!("{prefix}{suffix}"))
}

fn tag_handle_and_suffix(tag: &str) -> (&str, &str) {
    if let Some(suffix) = tag.strip_prefix("!!") {
        return ("!!", suffix);
    }

    if let Some(rest) = tag.strip_prefix('!')
        && let Some(end) = rest.find('!')
    {
        let handle_end = 1 + end + 1;
        return (&tag[..handle_end], &tag[handle_end..]);
    }

    if let Some(suffix) = tag.strip_prefix('!') {
        ("!", suffix)
    } else {
        ("!", tag)
    }
}

pub(crate) fn parse_node_properties(text: &str, span: Span) -> Result<NodeProperties, YamlError> {
    let mut properties = NodeProperties::default();
    let mut position = 0;

    loop {
        position = skip_property_whitespace(text, position);
        let Some(character) = text[position..].chars().next() else {
            properties.set_value_start(position);
            return Ok(properties);
        };

        match character {
            '&' => {
                if properties.anchor().is_some() {
                    return Err(node_property_error(
                        "duplicate anchor property",
                        span,
                        position,
                    ));
                }
                let next = parse_anchor_property(text, position, span)?;
                properties.set_anchor(Some(Span::new(
                    Span::offset_from_usize(span.start, position + 1),
                    Span::offset_from_usize(span.start, next),
                )));
                position = next;
            }
            '!' => {
                if properties.tag().is_some() {
                    return Err(node_property_error(
                        "duplicate tag property",
                        span,
                        position,
                    ));
                }
                let next = parse_tag_property(text, position, span)?;
                properties.set_tag(Some(Span::new(
                    Span::offset_from_usize(span.start, position),
                    Span::offset_from_usize(span.start, next),
                )));
                position = next;
            }
            _ => {
                properties.set_value_start(position);
                return Ok(properties);
            }
        }

        if next_property_character_is_not_whitespace(text, position) {
            properties.set_value_start(position);
            return Ok(properties);
        }
    }
}

fn skip_property_whitespace(text: &str, mut position: usize) -> usize {
    let bytes = text.as_bytes();
    while matches!(bytes.get(position), Some(b' ' | b'\t')) {
        position += 1;
    }
    position
}

fn next_property_character_is_not_whitespace(text: &str, position: usize) -> bool {
    match text.as_bytes().get(position) {
        Some(b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C) | None => false,
        Some(byte) if byte.is_ascii() => true,
        Some(_) => text[position..]
            .chars()
            .next()
            .is_some_and(|next| !next.is_whitespace()),
    }
}

fn parse_anchor_property(text: &str, position: usize, span: Span) -> Result<usize, YamlError> {
    let start = position + 1;
    let end = property_token_end(text, start);
    if start == end {
        return Err(node_property_error_with_expected(
            "missing anchor name",
            span,
            position,
            "an anchor name after `&`",
        ));
    }
    Ok(end)
}

fn parse_tag_property(text: &str, position: usize, span: Span) -> Result<usize, YamlError> {
    if text[position..].starts_with("!<") {
        let start = position + 2;
        let Some(relative_end) = text[start..].find('>') else {
            return Err(node_property_error_with_expected(
                "unterminated verbatim tag",
                span,
                position,
                "a closing `>`",
            ));
        };
        let end = start + relative_end;
        if start == end {
            return Err(node_property_error_with_expected(
                "empty verbatim tag",
                span,
                position,
                "a tag URI inside `!<...>`",
            ));
        }
        return Ok(end + 1);
    }

    let end = property_token_end(text, position);
    if end == position + 1 {
        return Ok(end);
    }
    Ok(end)
}

fn percent_decode_tag_suffix(suffix: &str, span: Span) -> Result<String, YamlError> {
    let mut output = String::new();
    let mut position = 0;
    while position < suffix.len() {
        let character = suffix[position..]
            .chars()
            .next()
            .expect("position is inside suffix");
        if character != '%' {
            output.push(character);
            position += character.len_utf8();
            continue;
        }

        let hex_start = position + 1;
        let hex_end = hex_start + 2;
        let bytes = suffix.as_bytes();
        let hex_digits = bytes.get(hex_start..hex_end);
        if !hex_digits.is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit)) {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Parser,
                    "malformed tag URI escape",
                    Span::empty(Span::offset_from_usize(span.start, position)),
                )
                .with_expected("two hexadecimal digits after `%`"),
            ));
        }
        let hex = std::str::from_utf8(hex_digits.expect("hex digits were validated"))
            .expect("hex digits are ASCII");
        let byte = u8::from_str_radix(hex, 16).expect("hex digits were validated");
        output.push(char::from(byte));
        position = hex_end;
    }
    Ok(output)
}

fn property_token_end(text: &str, mut position: usize) -> usize {
    if text.is_ascii() {
        let bytes = text.as_bytes();
        while let Some(byte) = bytes.get(position) {
            if matches!(
                *byte,
                b' ' | b'\t' | b'\r' | b'\n' | b'[' | b']' | b'{' | b'}' | b','
            ) {
                break;
            }
            position += 1;
        }
        return position;
    }

    while let Some(character) = text[position..].chars().next() {
        if character.is_whitespace() || matches!(character, '[' | ']' | '{' | '}' | ',') {
            break;
        }
        position += character.len_utf8();
    }
    position
}

fn node_property_error(message: impl Into<String>, span: Span, offset: usize) -> YamlError {
    YamlError::new(Diagnostic::new(
        DiagnosticKind::Parser,
        message,
        Span::empty(Span::offset_from_usize(span.start, offset)),
    ))
}

fn node_property_error_with_expected(
    message: impl Into<String>,
    span: Span,
    offset: usize,
    expected: impl Into<String>,
) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            message,
            Span::empty(Span::offset_from_usize(span.start, offset)),
        )
        .with_expected(expected),
    )
}

pub(crate) fn document_marker_rest<'text>(body: &'text str, marker: &str) -> Option<&'text str> {
    let rest = body.strip_prefix(marker)?;
    if rest.chars().next().is_none_or(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

pub(crate) fn strip_inline_comment(text: &str) -> &str {
    if text.is_ascii() {
        return strip_inline_comment_ascii(text);
    }

    let mut quoted = None;
    let mut previous_was_space = true;
    for (offset, character) in text.char_indices() {
        match quoted {
            Some('"') if character == '"' => quoted = None,
            Some('\'') if character == '\'' => quoted = None,
            None if character == '"' || character == '\'' => quoted = Some(character),
            None if character == '#' && previous_was_space => return &text[..offset],
            Some(_) | None => {}
        }
        previous_was_space = character.is_whitespace();
    }
    text
}

fn strip_inline_comment_ascii(text: &str) -> &str {
    let mut quoted = None;
    let mut previous_was_space = true;
    for (offset, byte) in text.bytes().enumerate() {
        match quoted {
            Some(b'"') if byte == b'"' => quoted = None,
            Some(b'\'') if byte == b'\'' => quoted = None,
            None if byte == b'"' || byte == b'\'' => quoted = Some(byte),
            None if byte == b'#' && previous_was_space => return &text[..offset],
            Some(_) | None => {}
        }
        previous_was_space = matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C);
    }
    text
}

fn invalid_directive(line: SourceLine<'_>, message: &'static str) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            message,
            Span::from_usize(line.content_start, line.content_end),
        )
        .with_expected("%YAML or %TAG directive syntax"),
    )
}

fn directive_after_document_content(line: SourceLine<'_>) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "directives must appear before document content",
            Span::from_usize(line.content_start, line.content_end),
        )
        .with_expected("a directive before the document start marker or content"),
    )
}

fn valid_yaml_directive_version_syntax(version: &str) -> bool {
    let Some((major, minor)) = version.split_once('.') else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.chars().all(|character| character.is_ascii_digit())
        && minor.chars().all(|character| character.is_ascii_digit())
}

pub(crate) fn validate_yaml_directive_version_for_emit(version: &str) -> Result<(), YamlError> {
    validate_yaml_chars(version)?;
    if version.chars().any(char::is_whitespace) || !valid_yaml_directive_version_syntax(version) {
        return Err(directive_emit_error(
            "invalid YAML directive version",
            Span::empty(0),
            "major.minor version digits",
        ));
    }
    Ok(())
}

fn valid_tag_handle(handle: &str) -> bool {
    handle == "!"
        || handle == "!!"
        || (handle.starts_with('!')
            && handle.ends_with('!')
            && handle.len() > 2
            && handle[1..handle.len() - 1].chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '-' || character == '_'
            }))
}

fn validate_tag_handle(handle: &str, line: SourceLine<'_>) -> Result<(), YamlError> {
    if valid_tag_handle(handle) {
        Ok(())
    } else {
        Err(invalid_directive(line, "invalid TAG directive handle"))
    }
}

pub(crate) fn validate_tag_directive_parts_for_emit(
    handle: &str,
    prefix: &str,
) -> Result<(), YamlError> {
    validate_yaml_chars(handle)?;
    validate_yaml_chars(prefix)?;
    if !valid_tag_handle(handle) {
        return Err(directive_emit_error(
            "invalid TAG directive handle",
            Span::empty(0),
            "!, !!, or !name!",
        ));
    }
    if prefix.is_empty()
        || prefix
            .chars()
            .any(|character| matches!(character, '\r' | '\n') || character.is_whitespace())
    {
        return Err(directive_emit_error(
            "invalid TAG directive prefix",
            Span::empty(0),
            "non-empty single-line tag prefix without whitespace",
        ));
    }
    Ok(())
}

pub(crate) fn directive_emit_error(
    message: impl Into<String>,
    span: Span,
    expected: impl Into<String>,
) -> YamlError {
    YamlError::new(Diagnostic::new(DiagnosticKind::Emitter, message, span).with_expected(expected))
}

#[derive(Clone, Copy)]
struct SourceLine<'source> {
    content_start: usize,
    content_end: usize,
    line_end: usize,
    content_without_break: &'source str,
    facts: LineFacts,
}

#[derive(Clone, Copy)]
struct PreparedBlockLine<'source> {
    line: SourceLine<'source>,
    indent: u32,
    body: &'source str,
    absolute_start: u32,
    comment_end: u32,
    mapping_colon: Option<u32>,
    starts_flow: bool,
}

impl<'source> PreparedBlockLine<'source> {
    fn new(line: SourceLine<'source>, indent: usize) -> Result<Self, YamlError> {
        Self::from_body(
            line,
            indent,
            &line.content_without_break[indent..],
            line.content_start + indent,
        )
    }

    fn from_body(
        line: SourceLine<'source>,
        indent: usize,
        body: &'source str,
        absolute_start: usize,
    ) -> Result<Self, YamlError> {
        let cached_mapping_colon = line
            .facts
            .mapping_colon()
            .filter(|_| absolute_start == line.content_start + indent);
        let (starts_flow, mapping_colon) = if let Some(colon) = cached_mapping_colon {
            (false, Some(colon))
        } else {
            let starts_flow = body_starts_flow_value(body, absolute_start)?;
            let mapping_colon = if starts_flow {
                flow_collection_mapping_key_colon(body, absolute_start)?
            } else {
                find_mapping_colon(body)
            };
            (starts_flow, mapping_colon)
        };
        let comment_end = if body_may_start_with_node_properties(body) {
            strip_inline_comment(body).len()
        } else {
            body.len()
        };
        Ok(Self {
            line,
            indent: Span::usize_to_u32(indent),
            body,
            absolute_start: Span::usize_to_u32(absolute_start),
            comment_end: Span::usize_to_u32(comment_end),
            mapping_colon: mapping_colon.map(Span::usize_to_u32),
            starts_flow,
        })
    }

    const fn indent(self) -> usize {
        self.indent as usize
    }

    const fn absolute_start(self) -> usize {
        self.absolute_start as usize
    }

    fn uncommented(self) -> &'source str {
        &self.body[..self.comment_end as usize]
    }

    const fn mapping_colon(self) -> Option<usize> {
        match self.mapping_colon {
            Some(colon) => Some(colon as usize),
            None => None,
        }
    }

    fn simple_mapping_facts(self) -> Option<SimpleMappingFacts> {
        source_line_simple_mapping_facts(self.line, self.absolute_start())
    }
}

fn source_line_simple_mapping_facts(
    line: SourceLine<'_>,
    absolute_start: usize,
) -> Option<SimpleMappingFacts> {
    let indent = line.facts.indent()?;
    if absolute_start != line.content_start + indent {
        return None;
    }
    let (colon, value_start) = line.facts.simple_mapping()?;
    Some(SimpleMappingFacts::new(colon, value_start))
}

fn source_line_scalar_mapping_facts(
    line: SourceLine<'_>,
    absolute_start: usize,
) -> Option<(usize, SingleLineScalarFacts)> {
    let indent = line.facts.indent()?;
    if absolute_start != line.content_start + indent {
        return None;
    }
    let (colon, start, end, style) = line.facts.scalar_mapping()?;
    Some((
        colon,
        SingleLineScalarFacts {
            start: Span::usize_to_u32(start - colon - 1),
            end: Span::usize_to_u32(end - colon - 1),
            style: match style {
                CachedScalarStyle::Plain => YamlScalarStyle::Plain,
                CachedScalarStyle::SingleQuoted => YamlScalarStyle::SingleQuoted,
                CachedScalarStyle::DoubleQuoted => YamlScalarStyle::DoubleQuoted,
            },
        },
    ))
}

#[derive(Clone, Copy)]
struct LineTable<'source> {
    source: &'source Source,
}

#[derive(Clone)]
struct LineCursor<'source> {
    lines: LineTable<'source>,
    next_index: usize,
    next_start: usize,
}

impl<'source> LineTable<'source> {
    pub(crate) fn new(source: &'source Source) -> Self {
        Self { source }
    }

    fn len(self) -> usize {
        let starts = self.source.line_starts();
        starts.len()
            - usize::from(starts.last().copied() == Some(Span::usize_to_u32(self.source.len())))
    }

    fn content_start(self, index: usize) -> usize {
        self.source.line_starts()[index] as usize
    }

    fn line_end(self, index: usize) -> usize {
        self.source
            .line_starts()
            .get(index + 1)
            .copied()
            .map_or(self.source.len(), |start| start as usize)
    }

    fn content_end(self, index: usize) -> usize {
        let start = self.content_start(index);
        let mut end = self.line_end(index);
        let bytes = self.source.as_str().as_bytes();
        if end > start && bytes[end - 1] == b'\n' {
            end -= 1;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
        } else if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        end
    }

    fn next_line_starts_simple_mapping(self, index: usize, parent_indent: usize) -> bool {
        let next = index + 1;
        if next >= self.len() {
            return true;
        }
        let facts = self.source.line_facts(next);
        facts
            .simple_mapping()
            .is_some_and(|_| facts.indent().is_some_and(|indent| indent <= parent_indent))
    }

    fn plain_scalar_cannot_continue(self, index: usize, parent_indent: usize) -> bool {
        next_significant_body_with_index(self, index)
            .is_none_or(|(_, indent, _)| indent <= parent_indent)
    }

    fn iter_from(self, start: usize) -> impl Iterator<Item = SourceLine<'source>> {
        self.cursor_from(start)
    }

    fn cursor_from(self, start: usize) -> LineCursor<'source> {
        let next_start = self
            .source
            .line_starts()
            .get(start)
            .copied()
            .map_or(self.source.len(), |offset| offset as usize);
        LineCursor {
            lines: self,
            next_index: start,
            next_start,
        }
    }
}

impl<'source> LineCursor<'source> {
    fn next_indexed(&mut self) -> Option<(usize, SourceLine<'source>)> {
        let index = self.next_index;
        self.next().map(|line| (index, line))
    }

    fn advance_by(&mut self, count: usize) {
        for _ in 0..count {
            if self.next().is_none() {
                break;
            }
        }
    }
}

impl<'source> Iterator for LineCursor<'source> {
    type Item = SourceLine<'source>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_index >= self.lines.len() {
            return None;
        }

        let index = self.next_index;
        let start = self.next_start;
        let line_end = self
            .lines
            .source
            .line_starts()
            .get(index + 1)
            .copied()
            .map_or(self.lines.source.len(), |offset| offset as usize);
        self.next_index += 1;
        self.next_start = line_end;

        let text = self.lines.source.as_str();
        let mut content_end = line_end;
        if content_end > start && text.as_bytes()[content_end - 1] == b'\n' {
            content_end -= 1;
            if content_end > start && text.as_bytes()[content_end - 1] == b'\r' {
                content_end -= 1;
            }
        } else if content_end > start && text.as_bytes()[content_end - 1] == b'\r' {
            content_end -= 1;
        }

        Some(SourceLine {
            content_start: start,
            content_end,
            line_end,
            content_without_break: self.lines.source.validated_line_slice(start, content_end),
            facts: self.lines.source.line_facts(index),
        })
    }
}

impl<'source> BlockMachine<'source> {
    fn new(parser: Parser<'source>, lines: LineTable<'source>) -> Self {
        Self {
            parser,
            lines,
            cursor: lines.cursor_from(0),
            frames: InlineVec::new(),
        }
    }

    fn run(mut self) -> Result<Parser<'source>, YamlError> {
        while let Some((index, line)) = self.cursor.next_indexed() {
            let transition = if self.frames.is_empty() {
                self.transition_line(index, line)?
            } else {
                loop {
                    let transition = if line_is_blank_or_comment(line) {
                        let transition = self.transition_line(index, line)?;
                        if let Some(frame) = self
                            .frames
                            .last_mut()
                            .filter(|frame| frame.allow_same_indent_content)
                        {
                            frame.last_content_end = Span::usize_to_u32(line.content_end);
                        }
                        transition
                    } else if self
                        .frames
                        .last()
                        .is_some_and(|frame| frame.kind == BlockEntryKind::ExplicitMapping)
                    {
                        self.transition_explicit_mapping(index, line)?
                    } else if self.frame_rejects_line(line)? {
                        let frame = self.frames.pop().expect("a rejected entry frame exists");
                        self.finish_frame(frame)?;
                        BlockTransition::Reprocess
                    } else if self.frames.is_empty() {
                        self.transition_line(index, line)?
                    } else {
                        self.transition_nested_line(index, line)?
                    };

                    if transition != BlockTransition::Reprocess {
                        break transition;
                    }
                }
            };
            let consumed = match transition {
                BlockTransition::Consume(consumed)
                | BlockTransition::Push(consumed)
                | BlockTransition::Pop(consumed) => consumed,
                BlockTransition::Reprocess => unreachable!("reprocessing completes in the loop"),
            };
            self.cursor.advance_by(consumed.saturating_sub(1));
        }
        while let Some(frame) = self.frames.pop() {
            self.finish_frame(frame)?;
        }
        Ok(self.parser)
    }

    fn frame_rejects_line(&self, line: SourceLine<'source>) -> Result<bool, YamlError> {
        let Some(frame) = self.frames.last() else {
            return Ok(false);
        };
        let indent = content_line_indent(line.content_without_break);
        let body = &line.content_without_break[indent..];
        Ok(indent < frame.indent as usize
            || indent == frame.indent as usize
                && frame.allow_same_indent_content
                && (is_explicit_mapping_key(body) || is_explicit_mapping_value(body))
            || indent == frame.indent as usize
                && !(frame.allow_same_indent_content
                    || frame.allow_indentless_sequence && is_sequence_entry(body)))
    }

    fn transition_explicit_mapping(
        &mut self,
        index: usize,
        line: SourceLine<'source>,
    ) -> Result<BlockTransition, YamlError> {
        let frame_index = self.frames.len() - 1;
        let frame = self.frames[frame_index];
        let indent = content_line_indent(line.content_without_break);
        let body = &line.content_without_break[indent..];
        if frame.phase == BlockEntryPhase::Key
            && !(indent == frame.indent as usize && is_explicit_mapping_value(body))
        {
            if indent < frame.indent as usize {
                let frame = self.frames.pop().expect("explicit mapping frame exists");
                self.finish_frame(frame)?;
                return Ok(BlockTransition::Reprocess);
            }
            self.parser.close_collections_deeper_than(indent);
            reject_unexpected_line_start(body, line.content_start + indent)?;
            let previous_depth = self.parser.block_frames.len();
            let prepared = PreparedBlockLine::new(line, indent)?;
            let consumed =
                self.parser
                    .parse_content_body(frame.owner, self.lines, index, prepared)?;
            let end = self.lines.content_end(index + consumed - 1);
            let active = &mut self.frames[frame_index];
            active.last_content_end = Span::usize_to_u32(end);
            self.capture_deferred_frame();
            return Ok(self.depth_transition(previous_depth, consumed));
        }

        if indent != frame.indent as usize || !is_explicit_mapping_value(body) {
            let frame = self.frames.pop().expect("explicit mapping frame exists");
            self.finish_frame(frame)?;
            return Ok(BlockTransition::Reprocess);
        }
        if self.parser.nodes[frame.owner.as_usize()].first_child == NO_NODE {
            let key = self.parser.push_empty_scalar(frame.empty_offset as usize);
            self.parser.attach_child_at(frame.owner.as_usize(), key);
            self.parser.emit_scalar_event(key)?;
        }
        self.parser.close_sequence_at_indent(frame.indent as usize);
        self.parser
            .close_collections_deeper_than(frame.indent as usize);
        let consumed = self.parser.parse_explicit_mapping_value(
            frame.owner,
            frame.collection,
            self.lines,
            index,
            frame.indent as usize,
            body,
        )?;
        let end = self.lines.content_end(index + consumed - 1);
        self.parser.extend_node_span(frame.owner, end);
        self.parser.extend_node_span(frame.collection, end);
        self.frames.pop();
        if let Some(parent) = self.frames.last_mut() {
            parent.last_content_end = parent.last_content_end.max(Span::usize_to_u32(end));
        }
        let pushed = !self.parser.pending_block_values.is_empty();
        self.capture_deferred_frame();
        Ok(if pushed {
            BlockTransition::Push(consumed)
        } else {
            BlockTransition::Consume(consumed)
        })
    }

    fn transition_nested_line(
        &mut self,
        index: usize,
        line: SourceLine<'source>,
    ) -> Result<BlockTransition, YamlError> {
        let frame_index = self.frames.len() - 1;
        let owner = self.frames[frame_index].owner;
        let indent = content_line_indent(line.content_without_break);
        let body = &line.content_without_break[indent..];
        let absolute_start = line.content_start + indent;
        self.parser.close_collections_deeper_than(indent);
        reject_unexpected_line_start(body, line.content_start + indent)?;
        if self.parser.sequence_is_open_at(indent) && !is_sequence_entry(body) {
            return Err(invalid_nested_block_sequence_sibling(
                line.content_start + indent,
            ));
        }
        let previous_depth = self.parser.block_frames.len();
        if let Some(facts) = source_line_simple_mapping_facts(line, absolute_start) {
            let consumed = if let Some(mapping) = self.parser.active_simple_mapping(indent) {
                self.parser.append_simple_plain_mapping_entry(
                    mapping,
                    self.lines,
                    index,
                    line,
                    indent,
                    body,
                    absolute_start,
                    facts,
                )?
            } else {
                self.parser.parse_simple_plain_mapping_entry(
                    owner,
                    self.lines,
                    index,
                    line,
                    indent,
                    body,
                    absolute_start,
                    facts,
                )?
            };
            self.finish_nested_transition(frame_index, index, consumed, false);
            self.capture_deferred_frame();
            return Ok(self.depth_transition(previous_depth, consumed));
        }
        let prepared = PreparedBlockLine::new(line, indent)?;
        let structural = is_sequence_entry(body)
            || is_explicit_mapping_key(body)
            || prepared.mapping_colon().is_some()
            || body_starts_flow_value(body, absolute_start)?;
        let property_only = !structural
            && body_may_start_with_node_properties(body)
            && (property_only_block_collection_indent(body, self.lines, index, absolute_start)?
                .is_some()
                || property_only_node_indent(body, self.lines, index, absolute_start)?.is_some());
        let mapping_scalar = self.frames[frame_index].kind == BlockEntryKind::Mapping
            && self.frames[frame_index].phase == BlockEntryPhase::Value
            && !property_only
            && !structural;
        let split_block_scalar = self.frames[frame_index].kind == BlockEntryKind::Mapping
            && self.frames[frame_index].pending_properties
            && (body.starts_with('|') || body.starts_with('>'));
        let consumed = if split_block_scalar {
            let parent_indent = self.frames[frame_index].indent as usize;
            let (scalar, consumed) = self.parser.parse_block_scalar(
                self.lines,
                index,
                absolute_start,
                parent_indent,
                body,
                true,
            )?;
            self.parser.attach_child_at(owner.as_usize(), scalar);
            self.parser.emit_scalar_event(scalar)?;
            consumed
        } else if mapping_scalar {
            let parent_indent = self.frames[frame_index].indent as usize;
            let (scalar, consumed) = self.parser.parse_block_plain_scalar(
                self.lines,
                index,
                parent_indent,
                absolute_start,
                false,
            )?;
            self.parser.attach_child_at(owner.as_usize(), scalar);
            self.parser.emit_scalar_event(scalar)?;
            consumed
        } else {
            self.parser
                .parse_content_body(owner, self.lines, index, prepared)?
        };
        self.finish_nested_transition(frame_index, index, consumed, property_only);
        self.capture_deferred_frame();
        Ok(self.depth_transition(previous_depth, consumed))
    }

    fn finish_nested_transition(
        &mut self,
        frame_index: usize,
        index: usize,
        consumed: usize,
        pending_properties: bool,
    ) {
        let end = self.lines.content_end(index + consumed - 1);
        let frame = &mut self.frames[frame_index];
        frame.last_content_end = Span::usize_to_u32(end);
        frame.phase = BlockEntryPhase::NestedChild;
        frame.pending_properties = pending_properties;
    }

    fn transition_line(
        &mut self,
        index: usize,
        line: SourceLine<'source>,
    ) -> Result<BlockTransition, YamlError> {
        let consumed = self.parser.parse_line(self.lines, index, line)?;
        if consumed == 0 {
            return Ok(BlockTransition::Reprocess);
        }
        let pushed = !self.parser.pending_block_values.is_empty();
        self.capture_deferred_frame();
        Ok(if pushed {
            BlockTransition::Push(consumed)
        } else {
            BlockTransition::Consume(consumed)
        })
    }

    fn capture_deferred_frame(&mut self) {
        while let Some(frame) = self.parser.pending_block_values.pop() {
            self.frames.push(frame);
        }
    }

    fn finish_frame(&mut self, frame: BlockValueFrame) -> Result<(), YamlError> {
        let end = frame.last_content_end as usize;
        if frame.kind != BlockEntryKind::ExplicitMapping {
            self.parser.extend_node_span(frame.owner, end);
            self.parser.extend_node_span(frame.collection, end);
        }
        self.parser.finish_block_value(frame)?;
        if let Some(parent) = self.frames.last_mut() {
            parent.last_content_end = parent.last_content_end.max(Span::usize_to_u32(end));
        }
        Ok(())
    }

    fn depth_transition(&self, previous_depth: usize, consumed: usize) -> BlockTransition {
        match self.parser.block_frames.len().cmp(&previous_depth) {
            std::cmp::Ordering::Greater => BlockTransition::Push(consumed),
            std::cmp::Ordering::Less => BlockTransition::Pop(consumed),
            std::cmp::Ordering::Equal => BlockTransition::Consume(consumed),
        }
    }
}

fn line_is_blank_or_comment(line: SourceLine<'_>) -> bool {
    if line.facts.is_blank() {
        return true;
    }
    let trimmed = line.content_without_break.trim_start();
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn invalid_document_marker(line: SourceLine<'_>) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "invalid document marker",
            Span::from_usize(line.content_start, line.content_end),
        )
        .with_expected("--- or ... followed by separation or line break"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockChomp {
    Strip,
    Clip,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockScalarKind {
    Literal,
    Folded,
}

impl BlockScalarKind {
    const fn node_kind(self) -> NodeKind {
        match self {
            Self::Literal => NodeKind::LiteralScalar,
            Self::Folded => NodeKind::FoldedScalar,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockScalarHeader {
    kind: BlockScalarKind,
    pub(crate) chomp: BlockChomp,
    pub(crate) indent: Option<usize>,
}

pub(crate) fn parse_block_scalar_header(
    header: &str,
    header_start: usize,
) -> Result<BlockScalarHeader, YamlError> {
    let kind = if header.starts_with('|') {
        BlockScalarKind::Literal
    } else if header.starts_with('>') {
        BlockScalarKind::Folded
    } else {
        unreachable!("block scalar parser only receives | or > headers");
    };

    let mut chomp = BlockChomp::Clip;
    let mut indent = None;
    let mut seen_chomp = false;
    let mut seen_indent = false;
    let mut offset = 1;

    while offset < header.len() {
        let character = header[offset..]
            .chars()
            .next()
            .expect("offset is inside header");
        match character {
            '-' | '+' if !seen_chomp => {
                chomp = if character == '-' {
                    BlockChomp::Strip
                } else {
                    BlockChomp::Keep
                };
                seen_chomp = true;
                offset += character.len_utf8();
            }
            '1'..='9' if !seen_indent => {
                indent = Some(character.to_digit(10).expect("digit") as usize);
                seen_indent = true;
                offset += character.len_utf8();
            }
            ' ' | '\t' => {
                let rest = &header[offset..];
                let comment_start = rest.find('#').unwrap_or(rest.len());
                if rest[..comment_start].trim().is_empty() {
                    return Ok(BlockScalarHeader {
                        kind,
                        chomp,
                        indent,
                    });
                }
                return Err(invalid_block_scalar_header(
                    header_start + offset,
                    character,
                ));
            }
            '#' => {
                if offset == 1 {
                    return Err(invalid_block_scalar_header(
                        header_start + offset,
                        character,
                    ));
                }
                return Ok(BlockScalarHeader {
                    kind,
                    chomp,
                    indent,
                });
            }
            _ => {
                return Err(invalid_block_scalar_header(
                    header_start + offset,
                    character,
                ));
            }
        }
    }

    Ok(BlockScalarHeader {
        kind,
        chomp,
        indent,
    })
}

fn invalid_block_scalar_header(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("invalid block scalar header before `{found}`"),
            Span::from_usize(offset, offset + found.len_utf8()),
        )
        .with_expected("|, >, chomping indicator, or a one-digit indentation indicator"),
    )
}

fn invalid_block_scalar_content(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "invalid block scalar content indentation",
            Span::from_usize(offset, offset + 1),
        )
        .with_expected("an indented scalar content line"),
    )
}

fn reject_unexpected_line_start(body: &str, body_start: usize) -> Result<(), YamlError> {
    let Some(first) = body.chars().next() else {
        return Ok(());
    };

    if matches!(first, ',' | ']' | '}') {
        Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                format!("unexpected token `{first}`"),
                Span::from_usize(body_start, body_start + first.len_utf8()),
            )
            .with_expected("mapping entry, sequence entry, or scalar"),
        ))
    } else {
        Ok(())
    }
}

fn count_indent(content: &str, content_start: usize) -> Result<usize, YamlError> {
    let mut indent = 0;
    for (offset, byte) in content.bytes().enumerate() {
        match byte {
            b' ' => indent += 1,
            b'\t' => {
                return Err(tab_indentation_error(content_start + offset));
            }
            _ => break,
        }
    }
    Ok(indent)
}

fn tab_indentation_error(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "tab character is not allowed in indentation",
            Span::from_usize(offset, offset + 1),
        )
        .with_expected("spaces for indentation"),
    )
}

fn invalid_quoted_scalar_continuation_indent(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "quoted scalar continuation is not indented",
            Span::empty_from_usize(offset),
        )
        .with_expected("quoted scalar content indented deeper than its parent mapping"),
    )
}

fn content_line_indent(content: &str) -> usize {
    content.bytes().take_while(|byte| *byte == b' ').count()
}

fn is_plain_scalar_continuation(
    content: &str,
    parent_indent: usize,
    allow_same_indent: bool,
) -> bool {
    let indent = content_line_indent(content);
    if indent > parent_indent || content.as_bytes().get(indent) == Some(&b'\t') {
        return true;
    }
    allow_same_indent
        && indent == parent_indent
        && !starts_new_same_indent_collection(&content[indent..])
}

fn starts_new_same_indent_collection(body: &str) -> bool {
    body.starts_with("---")
        || body.starts_with("...")
        || is_sequence_entry(body)
        || is_explicit_mapping_key(body)
        || is_explicit_mapping_value(body)
        || find_mapping_colon(body).is_some()
        || body_starts_flow_value_start(body)
}

fn body_starts_flow_value_start(body: &str) -> bool {
    let body = body.trim_start_matches([' ', '\t']);
    matches!(body.chars().next(), Some('[' | '{'))
}

fn count_literal_content_indent(content: &str) -> usize {
    content.bytes().take_while(|byte| *byte == b' ').count()
}

fn is_sequence_entry(body: &str) -> bool {
    body == "-" || body.starts_with("- ") || body.starts_with("-\t")
}

fn is_explicit_mapping_key(body: &str) -> bool {
    body == "?" || body.starts_with("? ") || body.starts_with("?\t")
}

fn is_explicit_mapping_value(body: &str) -> bool {
    body == ":" || body.starts_with(": ") || body.starts_with(":\t")
}

fn is_compact_explicit_empty_key_mapping(body: &str) -> bool {
    let Some(after_question) = body.strip_prefix('?') else {
        return false;
    };
    is_explicit_mapping_value(after_question.trim_start())
}

fn reject_invalid_indicator_tab(body: &str, absolute_start: usize) -> Result<(), YamlError> {
    let Some(indicator) = body.as_bytes().first().copied() else {
        return Ok(());
    };
    if !matches!(indicator, b'-' | b'?' | b':') || body.as_bytes().get(1) != Some(&b'\t') {
        return Ok(());
    }

    let invalid = match indicator {
        b'-' => {
            let after_tab = &body[2..];
            after_tab == "-" || after_tab.starts_with("- ") || after_tab.starts_with("-\t")
        }
        b'?' | b':' => true,
        _ => false,
    };
    if !invalid {
        return Ok(());
    }

    Err(YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "tab character is not allowed after a block indicator",
            Span::from_usize(absolute_start + 1, absolute_start + 2),
        )
        .with_expected("a space for block indicator separation"),
    ))
}

fn reject_invalid_sequence_tab_separated_nested_indicator(
    body: &str,
    absolute_start: usize,
) -> Result<(), YamlError> {
    let Some(after_dash) = body.strip_prefix('-') else {
        return Ok(());
    };
    let spaces = after_dash.bytes().take_while(|byte| *byte == b' ').count();
    if spaces == 0 || after_dash.as_bytes().get(spaces) != Some(&b'\t') {
        return Ok(());
    }
    let after_tab = &after_dash[spaces + 1..];
    if after_tab == "-" || after_tab.starts_with("- ") || after_tab.starts_with("-\t") {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "tab character is not allowed before a nested block sequence indicator",
                Span::from_usize(absolute_start + 1 + spaces, absolute_start + 2 + spaces),
            )
            .with_expected("spaces before a nested block sequence indicator"),
        ));
    }
    Ok(())
}

fn reject_invalid_compact_block_collection_value(
    value: &str,
    absolute_start: usize,
) -> Result<(), YamlError> {
    if is_sequence_entry(value)
        || is_explicit_mapping_key(value)
        || is_explicit_mapping_value(value)
    {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "compact block collection value is not allowed",
                Span::from_usize(absolute_start, absolute_start + 1),
            )
            .with_expected("a nested block collection on the following indented line"),
        ));
    }
    Ok(())
}

fn reject_invalid_flow_continuation_indent(
    line: &SourceLine<'_>,
    flow_indent: usize,
    allow_tab_continuation: bool,
) -> Result<(), YamlError> {
    let trimmed = line.content_without_break.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(());
    }
    let indent = content_line_indent(line.content_without_break);
    if line.content_without_break[indent..]
        .chars()
        .next()
        .is_some_and(|character| matches!(character, ',' | '?' | ']' | '}'))
    {
        return Ok(());
    }
    if indent > flow_indent {
        return Ok(());
    }
    if line.content_without_break.as_bytes().get(indent) == Some(&b'\t') {
        let tabbed = line.content_without_break[indent..].trim_start_matches('\t');
        if tabbed
            .chars()
            .next()
            .is_some_and(|character| matches!(character, ',' | '?' | ']' | '}'))
        {
            return Ok(());
        }
        if allow_tab_continuation {
            return Ok(());
        }
        return Err(tab_indentation_error(line.content_start + indent));
    }
    Err(YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "flow collection continuation is not indented",
            Span::from_usize(line.content_start + indent, line.content_start + indent + 1),
        )
        .with_expected("flow content indented deeper than the parent block"),
    ))
}

fn reject_split_implicit_flow_mapping_key(
    text: &str,
    start: usize,
    colon: usize,
    absolute_start: usize,
) -> Result<(), YamlError> {
    if text[start..colon].contains('\n') {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "implicit flow mapping key cannot be split before `:`",
                Span::from_usize(absolute_start + colon, absolute_start + colon + 1),
            )
            .with_expected("a mapping separator on the same line as the implicit key"),
        ));
    }
    Ok(())
}

fn reject_unindented_split_flow_mapping_value(
    text: &str,
    whitespace_start: usize,
    value_start: usize,
    absolute_start: usize,
    flow_indent: usize,
) -> Result<(), YamlError> {
    if !text[whitespace_start..value_start].contains('\n')
        && !text[whitespace_start..value_start].contains('\r')
    {
        return Ok(());
    }
    if text[value_start..]
        .chars()
        .next()
        .is_none_or(|character| matches!(character, ',' | '}'))
    {
        return Ok(());
    }
    let line_start = text[..value_start]
        .rfind(['\n', '\r'])
        .map_or(0, |offset| offset + 1);
    let indent = content_line_indent(&text[line_start..]);
    if indent > flow_indent {
        return Ok(());
    }
    Err(YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "flow mapping value is not indented",
            Span::from_usize(
                absolute_start + line_start + indent,
                absolute_start + line_start + indent + 1,
            ),
        )
        .with_expected("flow mapping value indented deeper than the parent block"),
    ))
}

fn invalid_nested_block_sequence_sibling(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "invalid content after nested block sequence",
            Span::from_usize(offset, offset + 1),
        )
        .with_expected("another sequence entry or the end of the nested sequence"),
    )
}

fn invalid_orphaned_block_content(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "invalid orphaned block content",
            Span::from_usize(offset, offset + 1),
        )
        .with_expected("a valid sibling collection entry or document boundary"),
    )
}

fn invalid_plain_scalar_continuation(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "invalid plain scalar continuation",
            Span::from_usize(offset, offset + 1),
        )
        .with_expected("plain scalar content without a mapping separator or prior comment"),
    )
}

fn plain_scalar_line_has_inline_comment(text: &str) -> bool {
    separated_comment_offset(text).is_some()
}

fn separated_comment_offset(text: &str) -> Option<usize> {
    let mut previous_was_whitespace = false;
    for (offset, character) in text.char_indices() {
        if character == '#' && previous_was_whitespace {
            return Some(offset);
        }
        previous_was_whitespace = matches!(character, ' ' | '\t');
    }
    None
}

fn comment_text_contains_mapping_colon(text: &str) -> bool {
    let Some(comment) = separated_comment_offset(text) else {
        return false;
    };
    text[comment..].char_indices().any(|(offset, character)| {
        character == ':' && is_block_mapping_separator_colon(&text[comment..], offset)
    })
}

fn validate_quoted_scalar_trailing_content(
    text: &str,
    absolute_start: usize,
) -> Result<(), YamlError> {
    let Some(quote) = text
        .chars()
        .next()
        .filter(|quote| matches!(quote, '"' | '\''))
    else {
        return Ok(());
    };
    let end = match quote {
        '"' => double_quoted_scalar_end(text),
        '\'' => single_quoted_scalar_end(text),
        _ => None,
    }
    .unwrap_or(text.len());
    if end == text.len() {
        return Ok(());
    }
    let trailing = &text[end..];
    if trailing.is_empty() {
        return Ok(());
    }
    let whitespace = trailing
        .bytes()
        .take_while(|byte| matches!(*byte, b' ' | b'\t'))
        .count();
    if whitespace == trailing.len() {
        return Ok(());
    }
    if whitespace == 0 || !trailing[whitespace..].starts_with('#') {
        let offset = absolute_start + end + whitespace;
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "unexpected content after quoted scalar",
                Span::from_usize(offset, offset + 1),
            )
            .with_expected("line break or separated comment"),
        ));
    }
    Ok(())
}

fn reject_nested_plain_mapping_colon(text: &str, absolute_start: usize) -> Result<(), YamlError> {
    if text.starts_with(['"', '\'']) {
        return Ok(());
    }
    if let Some(colon) = find_mapping_colon(text) {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "plain scalar contains a nested mapping separator",
                Span::from_usize(absolute_start + colon, absolute_start + colon + 1),
            )
            .with_expected("quoted scalar content or a nested mapping on the following line"),
        ));
    }
    Ok(())
}

fn reject_compact_decorated_document(rest: &str, absolute_start: usize) -> Result<(), YamlError> {
    if body_starts_flow_value(rest, absolute_start)? {
        return Ok(());
    }

    if find_mapping_colon(rest).is_some() {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "compact document mapping is not allowed",
                Span::from_usize(absolute_start, absolute_start + rest.len()),
            )
            .with_expected("a document marker followed by a separate block node"),
        ));
    }

    let properties = parse_node_properties(
        rest,
        Span::from_usize(absolute_start, absolute_start + rest.len()),
    )?;
    if properties.anchor().is_none() && properties.tag().is_none() {
        return Ok(());
    }
    let value = rest[properties.value_start()..].trim_start();
    if find_mapping_colon(value).is_some() {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "compact decorated document mapping is not allowed",
                Span::from_usize(absolute_start, absolute_start + rest.len()),
            )
            .with_expected("a document marker followed by a separate block node"),
        ));
    }
    Ok(())
}

fn reject_trailing_flow_content(
    text: &str,
    parsed_end: usize,
    absolute_start: usize,
) -> Result<(), YamlError> {
    let trailing = text
        .get(parsed_end..)
        .ok_or_else(|| invalid_flow_parser_state(absolute_start + text.len()))?;
    let trailing_whitespace = trailing
        .chars()
        .take_while(|character| character.is_whitespace())
        .map(char::len_utf8)
        .sum::<usize>();
    let offset = parsed_end + trailing_whitespace;
    let Some(character) = text[offset..].chars().next() else {
        return Ok(());
    };

    if character == '#' && is_flow_comment_start(text, offset) {
        return Ok(());
    }

    Err(YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("unexpected token `{character}` after flow collection"),
            Span::from_usize(
                absolute_start + offset,
                absolute_start + offset + character.len_utf8(),
            ),
        )
        .with_expected("line break or comment"),
    ))
}

fn flow_collection_source_end(text: &str, absolute_start: usize) -> Result<usize, YamlError> {
    let start = leading_flow_whitespace(text);
    let Some(open) = text[start..].chars().next() else {
        return Err(empty_flow_value(absolute_start));
    };
    let close = match open {
        '[' => ']',
        '{' => '}',
        _ => return Err(empty_flow_value(absolute_start)),
    };
    let mut stack = vec![close];
    let mut position = start + open.len_utf8();

    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        match character {
            '"' => position = double_quoted_flow_end(text, position, absolute_start)?,
            '\'' => position = single_quoted_flow_end(text, position, absolute_start)?,
            '#' if is_flow_comment_start(text, position) => {
                position = flow_comment_end(text, position);
            }
            '[' => {
                stack.push(']');
                position += 1;
            }
            '{' => {
                stack.push('}');
                position += 1;
            }
            ']' | '}' => {
                if stack.pop() != Some(character) {
                    return Err(expected_flow_separator(
                        absolute_start + position,
                        character,
                    ));
                }
                position += 1;
                if stack.is_empty() {
                    return Ok(position);
                }
            }
            _ => position += character.len_utf8(),
        }
    }

    if close == ']' {
        Err(missing_flow_sequence_end(absolute_start, text.len()))
    } else {
        Err(missing_flow_mapping_end(absolute_start, text.len()))
    }
}

fn skip_flow_whitespace(text: &str, mut position: usize) -> usize {
    while let Some(character) = text[position..].chars().next() {
        if character.is_whitespace() {
            position += character.len_utf8();
        } else if character == '#' && is_flow_comment_start(text, position) {
            position = flow_comment_end(text, position);
        } else {
            break;
        }
    }
    position
}

fn leading_flow_whitespace(text: &str) -> usize {
    skip_flow_whitespace(text, 0)
}

fn trailing_flow_whitespace(text: &str) -> usize {
    let mut length = 0;
    for character in text.chars().rev() {
        if character.is_whitespace() {
            length += character.len_utf8();
        } else {
            break;
        }
    }
    length
}

fn is_flow_mapping_separator_colon(text: &str, position: usize) -> bool {
    let next_position = position + ':'.len_utf8();
    let Some(next) = text[next_position..].chars().next() else {
        return true;
    };

    next.is_whitespace()
        || matches!(next, ',' | ']' | '}')
        || previous_flow_token_can_end_key(text, position)
}

fn is_flow_explicit_key_indicator(text: &str, position: usize) -> bool {
    debug_assert!(text[position..].starts_with('?'));
    let next_position = position + '?'.len_utf8();
    text[next_position..]
        .chars()
        .next()
        .is_none_or(|character| character.is_whitespace() || matches!(character, ',' | ']' | '}'))
}

fn previous_flow_token_can_end_key(text: &str, position: usize) -> bool {
    text[..position]
        .chars()
        .rev()
        .find(|character| !character.is_whitespace())
        .is_some_and(|character| matches!(character, '"' | '\'' | ']' | '}'))
}

fn flow_scalar_end(
    text: &str,
    start: usize,
    absolute_start: usize,
    terminators: &[char],
) -> Result<usize, YamlError> {
    let mut position = start;
    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        if terminators.contains(&character) {
            return Ok(position);
        }
        match character {
            '-' if is_forbidden_flow_plain_indicator(text, position, "---") => {
                return Err(forbidden_flow_plain_indicator(
                    absolute_start + position,
                    "---",
                ));
            }
            '.' if is_forbidden_flow_plain_indicator(text, position, "...") => {
                return Err(forbidden_flow_plain_indicator(
                    absolute_start + position,
                    "...",
                ));
            }
            '-' if is_bare_flow_dash(text, position) => {
                return Err(forbidden_flow_plain_indicator(
                    absolute_start + position,
                    "-",
                ));
            }
            '#' if is_flow_comment_start(text, position) => return Ok(position),
            '"' => position = double_quoted_flow_end(text, position, absolute_start)?,
            '\'' => position = single_quoted_flow_end(text, position, absolute_start)?,
            _ => position += character.len_utf8(),
        }
    }

    Ok(position)
}

fn flow_frame(node: NodeId, open: char) -> FlowFrame {
    if open == '[' {
        FlowFrame {
            node,
            close: ']',
            state: FlowFrameState::SequenceExpectItem,
        }
    } else {
        FlowFrame {
            node,
            close: '}',
            state: FlowFrameState::MappingExpectKey,
        }
    }
}

fn flow_frame_scalar_end(
    text: &str,
    start: usize,
    absolute_start: usize,
    close: char,
) -> Result<usize, YamlError> {
    let mut position = start;
    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        if character == ',' || character == close {
            return Ok(position);
        }
        match character {
            ':' if is_flow_mapping_separator_colon(text, position) => return Ok(position),
            '[' | '{' => {
                return Err(unexpected_flow_collection_after_scalar(
                    absolute_start + position,
                    character,
                ));
            }
            ']' | '}' => {
                return Err(expected_flow_separator(
                    absolute_start + position,
                    character,
                ));
            }
            '-' if is_forbidden_flow_plain_indicator(text, position, "---") => {
                return Err(forbidden_flow_plain_indicator(
                    absolute_start + position,
                    "---",
                ));
            }
            '.' if is_forbidden_flow_plain_indicator(text, position, "...") => {
                return Err(forbidden_flow_plain_indicator(
                    absolute_start + position,
                    "...",
                ));
            }
            '-' if is_bare_flow_dash(text, position) => {
                return Err(forbidden_flow_plain_indicator(
                    absolute_start + position,
                    "-",
                ));
            }
            '#' if is_flow_comment_start(text, position) => return Ok(position),
            '"' => position = double_quoted_flow_end(text, position, absolute_start)?,
            '\'' => position = single_quoted_flow_end(text, position, absolute_start)?,
            _ => position += character.len_utf8(),
        }
    }
    Ok(position)
}

fn is_forbidden_flow_plain_indicator(text: &str, position: usize, indicator: &str) -> bool {
    text[position..].starts_with(indicator)
        && previous_flow_character_allows_plain_indicator(text, position)
        && following_flow_character_terminates_indicator(text, position + indicator.len())
}

fn is_bare_flow_dash(text: &str, position: usize) -> bool {
    text[position..].starts_with('-')
        && previous_flow_character_allows_plain_indicator(text, position)
        && following_flow_character_terminates_indicator(text, position + '-'.len_utf8())
}

fn previous_flow_character_allows_plain_indicator(text: &str, position: usize) -> bool {
    text[..position]
        .chars()
        .next_back()
        .is_none_or(|character| character.is_whitespace() || matches!(character, '[' | '{' | ','))
}

fn following_flow_character_terminates_indicator(text: &str, position: usize) -> bool {
    text[position..]
        .chars()
        .next()
        .is_none_or(|character| character.is_whitespace() || matches!(character, ',' | ']' | '}'))
}

fn forbidden_flow_plain_indicator(offset: usize, indicator: &str) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("block indicator `{indicator}` is not allowed as a flow scalar"),
            Span::from_usize(offset, offset + indicator.len()),
        )
        .with_expected("quoted scalar content or a non-indicator plain scalar"),
    )
}

fn is_flow_comment_start(text: &str, position: usize) -> bool {
    debug_assert!(text[position..].starts_with('#'));
    text[..position]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
}

fn flow_comment_end(text: &str, position: usize) -> usize {
    let after_hash = position + '#'.len_utf8();
    match text[after_hash..].find(['\n', '\r']) {
        Some(offset) => after_hash + offset,
        None => text.len(),
    }
}

fn double_quoted_flow_end(
    text: &str,
    start: usize,
    absolute_start: usize,
) -> Result<usize, YamlError> {
    scan_double_quoted_scalar(text, start)?.ok_or_else(|| {
        YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "unterminated double-quoted scalar in flow sequence",
                Span::from_usize(absolute_start + start, absolute_start + text.len()),
            )
            .with_expected("closing \""),
        )
    })
}

fn scan_double_quoted_scalar(text: &str, start: usize) -> Result<Option<usize>, YamlError> {
    let bytes = text.as_bytes();
    let mut position = start + 1;
    while position < bytes.len() {
        let byte = bytes[position];
        if byte == b'"' {
            return Ok(Some(position + 1));
        }
        if byte != b'\\' {
            if byte == b'\r' && bytes.get(position + 1) == Some(&b'\n') {
                position += 2;
                reject_double_quoted_document_marker(&text[position..])?;
            } else {
                position += 1;
                if byte == b'\n' || byte == b'\r' {
                    reject_double_quoted_document_marker(&text[position..])?;
                }
            }
            continue;
        }

        position += 1;
        let whitespace_end = position
            + bytes[position..]
                .iter()
                .take_while(|byte| matches!(**byte, b' ' | b'\t'))
                .count();
        if matches!(bytes.get(whitespace_end), Some(b'\n' | b'\r')) {
            position = skip_escaped_line_break(text, whitespace_end);
            reject_double_quoted_document_marker(&text[position..])?;
            continue;
        }

        let Some(&escaped) = bytes.get(position) else {
            return Ok(None);
        };
        position += 1;
        match escaped {
            b'"' | b'\\' | b'/' | b' ' | b'0' | b'a' | b'b' | b't' | b'\t' | b'n' | b'v' | b'f'
            | b'r' | b'e' => {}
            b'x' => position = decode_hex_escape(text, position, 2)?.1,
            b'u' => position = decode_hex_escape(text, position, 4)?.1,
            b'U' => position = decode_hex_escape(text, position, 8)?.1,
            b'\n' | b'\r' => {
                position = skip_escaped_line_break(text, position - 1);
                reject_double_quoted_document_marker(&text[position..])?;
            }
            _ => return Err(invalid_double_quoted_escape("invalid double-quoted escape")),
        }
    }

    Ok(None)
}

fn reject_double_quoted_document_marker(line: &str) -> Result<(), YamlError> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if document_marker_rest(trimmed, "---").is_some()
        || document_marker_rest(trimmed, "...").is_some()
    {
        return Err(invalid_double_quoted_escape(
            "document marker is not allowed inside a double-quoted scalar",
        ));
    }
    Ok(())
}

fn single_quoted_flow_end(
    text: &str,
    start: usize,
    absolute_start: usize,
) -> Result<usize, YamlError> {
    let mut position = start + 1;

    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        position += character.len_utf8();
        if character == '\'' {
            if text[position..].starts_with('\'') {
                position += 1;
            } else {
                return Ok(position);
            }
        }
    }

    Err(YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "unterminated single-quoted scalar in flow sequence",
            Span::from_usize(absolute_start + start, absolute_start + text.len()),
        )
        .with_expected("closing '"),
    ))
}

fn missing_flow_sequence_end(absolute_start: usize, text_len: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "missing flow sequence closing bracket",
            Span::empty_from_usize(absolute_start + text_len),
        )
        .with_expected("]"),
    )
}

fn empty_flow_value(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "empty flow value",
            Span::empty_from_usize(offset),
        )
        .with_expected("a scalar or nested flow collection"),
    )
}

fn unexpected_flow_collection_after_scalar(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("unexpected token `{found}` after flow scalar"),
            Span::from_usize(offset, offset + found.len_utf8()),
        )
        .with_expected("a comma or the end of the flow collection"),
    )
}

fn invalid_flow_parser_state(offset: usize) -> YamlError {
    YamlError::new(Diagnostic::new(
        DiagnosticKind::Parser,
        "invalid flow parser state",
        Span::empty_from_usize(offset),
    ))
}

fn flow_collection_depth_limit_exceeded(offset: usize) -> YamlError {
    YamlError::new(Diagnostic::new(
        DiagnosticKind::Parser,
        format!("flow collection nesting limit of {MAX_FLOW_COLLECTION_DEPTH} exceeded"),
        Span::from_usize(offset, offset + 1),
    ))
}

fn unexpected_flow_comma(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "unexpected comma in flow sequence",
            Span::from_usize(offset, offset + 1),
        )
        .with_expected("a scalar, nested flow sequence, or ]"),
    )
}

fn expected_flow_separator(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("unexpected token `{found}` in flow sequence"),
            Span::from_usize(offset, offset + found.len_utf8()),
        )
        .with_expected(", or ]"),
    )
}

fn missing_flow_mapping_end(absolute_start: usize, text_len: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "missing flow mapping closing brace",
            Span::empty_from_usize(absolute_start + text_len),
        )
        .with_expected("}"),
    )
}

fn unexpected_flow_mapping_comma(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "unexpected comma in flow mapping",
            Span::from_usize(offset, offset + 1),
        )
        .with_expected("a mapping key or }"),
    )
}

fn missing_flow_mapping_colon(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("missing colon after flow mapping key before `{found}`"),
            Span::from_usize(offset, offset + found.len_utf8()),
        )
        .with_expected(":"),
    )
}

fn expected_flow_mapping_separator(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("unexpected token `{found}` in flow mapping"),
            Span::from_usize(offset, offset + found.len_utf8()),
        )
        .with_expected(", or }"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SimpleMappingFacts {
    colon: u32,
    value_start: u32,
}

impl SimpleMappingFacts {
    fn new(colon: usize, value_start: usize) -> Self {
        Self {
            colon: Span::usize_to_u32(colon),
            value_start: Span::usize_to_u32(value_start),
        }
    }

    const fn colon(self) -> usize {
        self.colon as usize
    }

    const fn value_start(self) -> usize {
        self.value_start as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SingleLineScalarFacts {
    start: u32,
    end: u32,
    style: YamlScalarStyle,
}

impl SingleLineScalarFacts {
    fn new(start: usize, end: usize, style: YamlScalarStyle) -> Self {
        Self {
            start: Span::usize_to_u32(start),
            end: Span::usize_to_u32(end),
            style,
        }
    }

    const fn start(self) -> usize {
        self.start as usize
    }

    const fn end(self) -> usize {
        self.end as usize
    }
}

fn single_line_scalar_facts(raw: &str) -> Result<Option<SingleLineScalarFacts>, YamlError> {
    let start = raw.bytes().take_while(|byte| *byte == b' ').count();
    let text = &raw[start..];
    let Some(first) = text.as_bytes().first().copied() else {
        return Ok(None);
    };

    if first == b'"' {
        let Some(end) = scan_double_quoted_scalar(text, 0)? else {
            return Ok(None);
        };
        if !valid_quoted_scalar_trailing_text(&text[end..]) {
            return Ok(None);
        }
        return Ok(Some(SingleLineScalarFacts::new(
            start,
            start + end,
            YamlScalarStyle::DoubleQuoted,
        )));
    }
    if first == b'\'' {
        let Some(end) = single_quoted_scalar_end(text) else {
            return Ok(None);
        };
        if !valid_quoted_scalar_trailing_text(&text[end..]) {
            return Ok(None);
        }
        return Ok(Some(SingleLineScalarFacts::new(
            start,
            start + end,
            YamlScalarStyle::SingleQuoted,
        )));
    }
    if matches!(
        first,
        b'-' | b'?'
            | b':'
            | b','
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'#'
            | b'&'
            | b'*'
            | b'!'
            | b'|'
            | b'>'
            | b'%'
            | b'@'
            | b'`'
    ) {
        return Ok(None);
    }

    let bytes = text.as_bytes();
    let mut end = bytes.len();
    let mut previous_was_whitespace = false;
    for (offset, &byte) in bytes.iter().enumerate() {
        if byte == b'#' && previous_was_whitespace {
            end = offset;
            break;
        }
        if byte == b':'
            && bytes
                .get(offset + 1)
                .is_none_or(|next| matches!(*next, b' ' | b'\t'))
        {
            return Ok(None);
        }
        if matches!(byte, b'\t' | b'[' | b']' | b'{' | b'}' | b',') {
            return Ok(None);
        }
        previous_was_whitespace = byte == b' ';
    }
    end = text[..end].trim_end_matches(' ').len();
    if end == 0 {
        return Ok(None);
    }
    Ok(Some(SingleLineScalarFacts::new(
        start,
        start + end,
        YamlScalarStyle::Plain,
    )))
}

fn valid_quoted_scalar_trailing_text(trailing: &str) -> bool {
    if trailing.is_empty() || trailing.bytes().all(|byte| byte == b' ') {
        return true;
    }
    let whitespace = trailing.bytes().take_while(|byte| *byte == b' ').count();
    whitespace > 0 && trailing[whitespace..].starts_with('#')
}

fn simple_plain_mapping_facts(body: &str) -> Option<SimpleMappingFacts> {
    let bytes = body.as_bytes();
    let colon = bytes.iter().position(|byte| *byte == b':')?;
    if !is_simple_plain_atom(&body[..colon]) {
        return None;
    }

    let mut value_start = colon + 1;
    if !matches!(bytes.get(value_start), Some(b' ')) {
        return None;
    }
    while matches!(bytes.get(value_start), Some(b' ')) {
        value_start += 1;
    }
    if value_start == bytes.len()
        || &body[value_start..] == "-"
        || !is_simple_plain_atom(&body[value_start..])
    {
        return None;
    }

    Some(SimpleMappingFacts::new(colon, value_start))
}

fn simple_plain_key_mapping_colon(body: &str) -> Option<usize> {
    let colon = body.as_bytes().iter().position(|byte| *byte == b':')?;
    if !is_simple_plain_atom(&body[..colon]) || !is_block_mapping_separator_colon(body, colon) {
        return None;
    }
    Some(colon)
}

fn is_simple_plain_atom(text: &str) -> bool {
    !text.is_empty() && text.as_bytes().iter().copied().all(is_simple_plain_byte)
}

fn is_simple_plain_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/')
}

#[cfg(test)]
mod layout_tests {
    use std::mem::size_of;

    use super::{
        BlockFrame, FlowFrame, NodeProperties, PendingNodeProperties, SimpleMappingFacts,
        SingleLineScalarFacts,
    };

    #[test]
    fn transient_parser_records_stay_compact() {
        assert_eq!(size_of::<FlowFrame>(), 24);
        assert_eq!(size_of::<NodeProperties>(), 32);
        assert_eq!(size_of::<PendingNodeProperties>(), 40);
        assert_eq!(size_of::<BlockFrame>(), 16);
        assert_eq!(size_of::<SimpleMappingFacts>(), 8);
        assert_eq!(size_of::<SingleLineScalarFacts>(), 12);
    }
}

pub(crate) fn find_mapping_colon(body: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut previous_was_whitespace = false;
    let start = if body.starts_with(['!', '&']) {
        parse_node_properties(body, Span::empty(0))
            .ok()
            .filter(|properties| properties.anchor().is_some() || properties.tag().is_some())
            .map_or(0, NodeProperties::value_start)
    } else {
        0
    };

    for (offset, character) in body[start..].char_indices() {
        let offset = start + offset;
        if in_double {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_double = false;
            }
        } else if in_single {
            if character == '\'' {
                in_single = false;
            }
        } else if character == '"' && offset == start {
            in_double = true;
        } else if character == '\'' && offset == start {
            in_single = true;
        } else if character == '#' && previous_was_whitespace {
            return None;
        } else if character == ':'
            && is_block_mapping_separator_colon(body, offset)
            && !colon_is_inside_leading_alias(body, offset)
        {
            return Some(offset);
        }
        previous_was_whitespace = matches!(character, ' ' | '\t');
    }

    None
}

fn plain_continuation_has_mapping_colon(body: &str) -> bool {
    if body.starts_with(['!', '&']) {
        return find_mapping_colon(body).is_some();
    }

    let bytes = body.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut previous_was_whitespace = false;
    for (offset, &byte) in bytes.iter().enumerate() {
        if in_double {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_double = false;
            }
        } else if in_single {
            if byte == b'\'' {
                in_single = false;
            }
        } else if byte == b'"' && offset == 0 {
            in_double = true;
        } else if byte == b'\'' && offset == 0 {
            in_single = true;
        } else if byte == b'#' && previous_was_whitespace {
            return false;
        } else if byte == b':'
            && bytes
                .get(offset + 1)
                .is_none_or(|next| matches!(*next, b' ' | b'\t'))
            && !colon_is_inside_leading_alias(body, offset)
        {
            return true;
        }
        previous_was_whitespace = matches!(byte, b' ' | b'\t');
    }
    false
}

fn is_block_mapping_separator_colon(text: &str, position: usize) -> bool {
    let next_position = position + ':'.len_utf8();
    text[next_position..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
}

fn colon_is_inside_leading_alias(text: &str, position: usize) -> bool {
    text.starts_with('*') && !text[..position].chars().any(char::is_whitespace)
}

pub(crate) fn validate_plain_mapping_fragment(text: &str, role: &str) -> Result<(), YamlError> {
    validate_yaml_chars(text)?;

    if text.is_empty()
        || text
            .chars()
            .any(|character| matches!(character, '\r' | '\n'))
    {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Emitter,
                format!("{role} must be non-empty single-line plain text"),
                Span::empty(0),
            )
            .with_expected("single-line plain text"),
        ));
    }

    Ok(())
}

pub(crate) fn edits_conflict(left: Span, right: Span) -> bool {
    if left.is_empty() && right.is_empty() {
        left.start == right.start
    } else {
        left.start < right.end && right.start < left.end
    }
}

pub(crate) fn double_quoted_scalar_end(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (offset, character) in text.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Some(offset + character.len_utf8());
        }
    }
    None
}

fn validate_double_quoted_continuation_line(line: &SourceLine<'_>) -> Result<(), YamlError> {
    let trimmed = line.content_without_break.trim();
    if document_marker_rest(trimmed, "---").is_some()
        || document_marker_rest(trimmed, "...").is_some()
    {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "document marker is not allowed inside a double-quoted scalar",
                Span::from_usize(line.content_start, line.content_end),
            )
            .with_expected("double-quoted scalar content"),
        ));
    }

    Ok(())
}

pub(crate) fn single_quoted_scalar_end(text: &str) -> Option<usize> {
    let mut chars = text.char_indices().skip(1).peekable();
    while let Some((offset, character)) = chars.next() {
        if character != '\'' {
            continue;
        }

        if chars.peek().is_some_and(|(_, next)| *next == '\'') {
            chars.next();
        } else {
            return Some(offset + character.len_utf8());
        }
    }
    None
}

pub(crate) fn plain_scalar_end(text: &str) -> usize {
    let mut end = text.len();
    let mut previous_was_whitespace = false;

    for (offset, character) in text.char_indices() {
        if character == '#' && previous_was_whitespace {
            end = offset;
            break;
        }
        previous_was_whitespace = matches!(character, ' ' | '\t');
    }

    text[..end].trim_end_matches([' ', '\t']).len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
}

pub(crate) fn format_scalar_value(value: &str, style: ScalarStyle) -> Result<String, YamlError> {
    validate_yaml_chars(value)?;

    match style {
        ScalarStyle::Plain => format_plain_scalar_value(value),
        ScalarStyle::SingleQuoted => format_single_quoted_scalar_value(value),
        ScalarStyle::DoubleQuoted => Ok(format_double_quoted_scalar_value(value)),
    }
}

fn format_plain_scalar_value(value: &str) -> Result<String, YamlError> {
    if value.is_empty()
        || value
            .chars()
            .any(|character| matches!(character, '\r' | '\n'))
        || value.starts_with([' ', '\t', '#', ':', '-', '?', ',', ']', '}'])
        || value.ends_with([' ', '\t'])
        || value.contains(": ")
        || value.contains(" #")
    {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Emitter,
                "plain scalar replacement cannot preserve plain style",
                Span::empty(0),
            )
            .with_expected("non-empty single-line plain scalar text without YAML indicators"),
        ));
    }

    Ok(value.to_owned())
}

fn format_single_quoted_scalar_value(value: &str) -> Result<String, YamlError> {
    if value
        .chars()
        .any(|character| matches!(character, '\r' | '\n'))
    {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Emitter,
                "single-quoted scalar replacement cannot contain line breaks",
                Span::empty(0),
            )
            .with_expected("single-line scalar text"),
        ));
    }

    let mut output = String::from("'");
    for character in value.chars() {
        if character == '\'' {
            output.push_str("''");
        } else {
            output.push(character);
        }
    }
    output.push('\'');
    Ok(output)
}

fn format_double_quoted_scalar_value(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            _ => output.push(character),
        }
    }
    output.push('"');
    output
}

pub(crate) fn decode_scalar_value_with_content_indent(
    text: &str,
    content_indent: Option<usize>,
) -> Result<String, YamlError> {
    if text.starts_with('|') {
        return decode_literal_scalar_value(text, content_indent);
    }
    if text.starts_with('>') {
        return decode_folded_scalar_value(text, content_indent);
    }

    if text.starts_with('"') {
        let end = double_quoted_scalar_end(text).ok_or_else(|| {
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    "could not decode double-quoted scalar",
                    Span::empty(0),
                )
                .with_expected("a closed double-quoted scalar"),
            )
        })?;
        let raw_content = &text[1..end - 1];
        validate_double_quoted_scalar_content(raw_content)?;
        let continued = strip_double_quoted_line_continuations(raw_content);
        let folded = fold_quoted_scalar_lines(&continued);
        return decode_double_quoted_scalar(&folded);
    }

    if text.starts_with('\'') {
        let end = single_quoted_scalar_end(text).ok_or_else(|| {
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    "could not decode single-quoted scalar",
                    Span::empty(0),
                )
                .with_expected("a closed single-quoted scalar"),
            )
        })?;
        return Ok(fold_quoted_scalar_lines(&text[1..end - 1]).replace("''", "'"));
    }

    Ok(decode_plain_scalar_value(text))
}

fn fold_quoted_scalar_lines(text: &str) -> String {
    if !text.contains(['\n', '\r']) {
        return text.to_owned();
    }

    let mut folded = String::new();
    let mut position = 0;
    let mut pending_breaks = 0usize;
    let mut saw_content_line = false;
    let mut first_line = true;

    while position < text.len() {
        let (line, next_position) = next_literal_content_line(text, position);
        let (body, break_text) = split_line_break(line);
        let body = if first_line {
            body
        } else {
            body.trim_start_matches([' ', '\t'])
        };
        let body = if break_text.is_empty() {
            body
        } else {
            body.trim_end_matches([' ', '\t'])
        };

        if body.is_empty() {
            if !break_text.is_empty() {
                pending_breaks += 1;
            }
        } else {
            if saw_content_line || pending_breaks > 0 {
                push_folded_quoted_breaks(&mut folded, pending_breaks);
            }
            folded.push_str(body);
            pending_breaks = usize::from(!break_text.is_empty());
            saw_content_line = true;
        }

        first_line = false;
        position = next_position;
    }

    if pending_breaks > 0 {
        push_folded_quoted_breaks(&mut folded, pending_breaks);
    }

    folded
}

fn validate_double_quoted_scalar_content(text: &str) -> Result<(), YamlError> {
    let mut position = 0;
    let mut first_line = true;
    while position < text.len() {
        let (line, next_position) = next_literal_content_line(text, position);
        let (body, _) = split_line_break(line);
        if !first_line {
            let trimmed = body.trim();
            if document_marker_rest(trimmed, "---").is_some()
                || document_marker_rest(trimmed, "...").is_some()
            {
                return Err(invalid_double_quoted_escape(
                    "document marker is not allowed inside a double-quoted scalar",
                ));
            }
        }
        first_line = false;
        position = next_position;
    }
    Ok(())
}

fn validate_double_quoted_scalar_escapes(text: &str) -> Result<(), YamlError> {
    let mut position = 0;
    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        if character != '\\' {
            position += character.len_utf8();
            continue;
        }

        position += '\\'.len_utf8();
        let whitespace_end = position
            + text[position..]
                .bytes()
                .take_while(|byte| matches!(*byte, b' ' | b'\t'))
                .count();
        if text[whitespace_end..]
            .chars()
            .next()
            .is_some_and(|next| matches!(next, '\n' | '\r'))
        {
            position = skip_escaped_line_break(text, whitespace_end);
            continue;
        }

        let Some(escaped) = text[position..].chars().next() else {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    "unterminated double-quoted escape",
                    Span::empty(0),
                )
                .with_expected("an escaped character"),
            ));
        };
        position += escaped.len_utf8();
        match escaped {
            '"' | '\\' | '/' | ' ' | '0' | 'a' | 'b' | 't' | '\t' | 'n' | 'v' | 'f' | 'r' | 'e' => {
            }
            'x' => position = decode_hex_escape(text, position, 2)?.1,
            'u' => position = decode_hex_escape(text, position, 4)?.1,
            'U' => position = decode_hex_escape(text, position, 8)?.1,
            '\n' | '\r' => position = skip_escaped_line_break(text, position - escaped.len_utf8()),
            _ => return Err(invalid_double_quoted_escape("invalid double-quoted escape")),
        }
    }
    Ok(())
}

fn strip_double_quoted_line_continuations(text: &str) -> String {
    let mut output = String::new();
    let mut position = 0;

    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        if character == '\\' {
            let after_backslash = position + character.len_utf8();
            let whitespace_end = after_backslash
                + text[after_backslash..]
                    .bytes()
                    .take_while(|byte| matches!(*byte, b' ' | b'\t'))
                    .count();
            if text[whitespace_end..]
                .chars()
                .next()
                .is_some_and(|next| matches!(next, '\n' | '\r'))
            {
                for whitespace in text[after_backslash..whitespace_end].chars() {
                    if whitespace == '\t' {
                        output.push(whitespace);
                    }
                }
                if whitespace_end > after_backslash {
                    output.push(' ');
                }
                position = skip_escaped_line_break(text, whitespace_end);
                continue;
            }
            output.push(character);
            position += character.len_utf8();
        } else {
            output.push(character);
            position += character.len_utf8();
        }
    }

    output
}

fn push_folded_quoted_breaks(output: &mut String, breaks: usize) {
    match breaks {
        0 => {}
        1 => output.push(' '),
        count => {
            for _ in 1..count {
                output.push('\n');
            }
        }
    }
}

fn decode_plain_scalar_value(text: &str) -> String {
    if !text.contains(['\n', '\r']) {
        return text[..plain_scalar_end(text)].to_owned();
    }

    let mut decoded = String::new();
    let mut position = 0;
    let mut pending_breaks = 0usize;
    let mut saw_value_line = false;

    while position < text.len() {
        let (line, next_position) = next_literal_content_line(text, position);
        let (body, _) = split_line_break(line);
        let body = strip_inline_comment(body).trim();

        if body.is_empty() {
            pending_breaks += 1;
        } else {
            if saw_value_line {
                if pending_breaks == 0 {
                    decoded.push(' ');
                } else {
                    for _ in 0..pending_breaks {
                        decoded.push('\n');
                    }
                }
            }
            decoded.push_str(body);
            pending_breaks = 0;
            saw_value_line = true;
        }

        position = next_position;
    }

    decoded
}

fn decode_literal_scalar_value(
    text: &str,
    content_indent: Option<usize>,
) -> Result<String, YamlError> {
    let (header_text, content_start) = split_first_line(text);
    let header = parse_block_scalar_header(header_text, 0)?;
    let content = &text[content_start..];
    let content_indent = content_indent.unwrap_or_else(|| {
        header
            .indent
            .unwrap_or_else(|| detect_literal_content_indent(content))
    });
    if content.is_empty() && header.chomp == BlockChomp::Keep && content_start > header_text.len() {
        return Ok("\n".to_owned());
    }
    let mut decoded = String::new();
    let mut position = 0;

    while position < content.len() {
        let (line, next_position) = next_literal_content_line(content, position);
        let (body, break_text) = split_line_break(line);
        let stripped = strip_literal_indent(body, content_indent);
        decoded.push_str(stripped);
        decoded.push_str(break_text);
        position = next_position;
    }

    if decoded.is_empty() && !content.is_empty() && header.chomp == BlockChomp::Keep {
        return Ok("\n".to_owned());
    }
    Ok(apply_block_chomp(decoded, header.chomp))
}

fn decode_folded_scalar_value(
    text: &str,
    content_indent: Option<usize>,
) -> Result<String, YamlError> {
    let (header_text, content_start) = split_first_line(text);
    let header = parse_block_scalar_header(header_text, 0)?;
    let content = &text[content_start..];
    let content_indent = content_indent.unwrap_or_else(|| {
        header
            .indent
            .unwrap_or_else(|| detect_literal_content_indent(content))
    });
    if content.is_empty() && header.chomp == BlockChomp::Keep && content_start > header_text.len() {
        return Ok("\n".to_owned());
    }
    let literal = decode_block_scalar_content(content, content_indent);
    if literal.is_empty() && !content.is_empty() && header.chomp == BlockChomp::Keep {
        return Ok("\n".to_owned());
    }

    Ok(apply_block_chomp(
        fold_block_scalar_lines(&literal),
        header.chomp,
    ))
}

fn decode_block_scalar_content(content: &str, content_indent: usize) -> String {
    let mut decoded = String::new();
    let mut position = 0;

    while position < content.len() {
        let (line, next_position) = next_literal_content_line(content, position);
        let (body, break_text) = split_line_break(line);
        let stripped = strip_literal_indent(body, content_indent);
        decoded.push_str(stripped);
        decoded.push_str(break_text);
        position = next_position;
    }

    decoded
}

fn fold_block_scalar_lines(literal: &str) -> String {
    let lines = literal_lines(literal);
    let mut output = String::new();
    let mut saw_content_line = false;
    let mut previous_more_indented = false;
    let mut pending_blank_lines = 0usize;
    let mut last_content_had_break = false;

    for (body, break_text) in lines {
        let more_indented = line_is_more_indented(body);
        if body.is_empty() {
            if saw_content_line && !break_text.is_empty() {
                pending_blank_lines += 1;
                last_content_had_break = true;
            } else {
                output.push_str(break_text);
            }
            continue;
        }

        if saw_content_line {
            if pending_blank_lines > 0 {
                let breaks = if previous_more_indented || more_indented {
                    pending_blank_lines + 1
                } else {
                    pending_blank_lines
                };
                for _ in 0..breaks {
                    output.push('\n');
                }
            } else if previous_more_indented || more_indented {
                output.push('\n');
            } else if last_content_had_break {
                output.push(' ');
            }
        }

        output.push_str(body);
        saw_content_line = true;
        previous_more_indented = more_indented;
        pending_blank_lines = 0;
        last_content_had_break = !break_text.is_empty();
    }

    if saw_content_line && last_content_had_break {
        for _ in 0..=pending_blank_lines {
            output.push('\n');
        }
    }

    output
}

fn literal_lines(mut text: &str) -> Vec<(&str, &str)> {
    let mut lines = Vec::new();
    while !text.is_empty() {
        let (line, next) = next_literal_content_line(text, 0);
        let (body, break_text) = split_line_break(line);
        lines.push((body, break_text));
        text = &text[next..];
    }
    lines
}

fn line_is_more_indented(line: &str) -> bool {
    line.starts_with(' ') || line.starts_with('\t')
}

fn split_first_line(text: &str) -> (&str, usize) {
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            return (&text[..index], index + 1);
        }
        if *byte == b'\r' {
            let end = if bytes.get(index + 1) == Some(&b'\n') {
                index + 2
            } else {
                index + 1
            };
            return (&text[..index], end);
        }
    }
    (text, text.len())
}

fn next_literal_content_line(text: &str, start: usize) -> (&str, usize) {
    let bytes = text.as_bytes();
    let mut index = start;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            return (&text[start..=index], index + 1);
        }
        if bytes[index] == b'\r' {
            let end = if bytes.get(index + 1) == Some(&b'\n') {
                index + 2
            } else {
                index + 1
            };
            return (&text[start..end], end);
        }
        index += 1;
    }
    (&text[start..], text.len())
}

fn split_line_break(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else if let Some(body) = line.strip_suffix('\r') {
        (body, "\r")
    } else {
        (line, "")
    }
}

fn detect_literal_content_indent(content: &str) -> usize {
    let mut position = 0;
    while position < content.len() {
        let (line, next_position) = next_literal_content_line(content, position);
        let (body, _) = split_line_break(line);
        if !body.trim().is_empty() {
            return body.bytes().take_while(|byte| *byte == b' ').count();
        }
        position = next_position;
    }
    0
}

fn strip_literal_indent(line: &str, indent: usize) -> &str {
    if !line.is_empty() && line.bytes().all(|byte| byte == b' ') && line.len() > indent {
        return &line[indent..];
    }
    for (stripped, (offset, byte)) in line.bytes().enumerate().enumerate() {
        if stripped == indent || byte != b' ' {
            return &line[offset..];
        }
    }
    ""
}

fn apply_block_chomp(mut value: String, chomp: BlockChomp) -> String {
    match chomp {
        BlockChomp::Keep => value,
        BlockChomp::Strip => {
            trim_trailing_line_breaks(&mut value);
            value
        }
        BlockChomp::Clip => {
            let had_line_break = ends_with_line_break(&value);
            trim_trailing_line_breaks(&mut value);
            if had_line_break || !value.is_empty() {
                value.push('\n');
            }
            value
        }
    }
}

fn trim_trailing_line_breaks(value: &mut String) {
    while value.ends_with('\n') || value.ends_with('\r') {
        value.pop();
    }
}

fn ends_with_line_break(value: &str) -> bool {
    value.ends_with('\n') || value.ends_with('\r')
}

/// Renders YAML events in the YAML Test Suite `test.event` format.
#[must_use]
pub fn events_to_test_string<I, E>(events: I) -> String
where
    I: IntoIterator<Item = E>,
    E: std::borrow::Borrow<YamlEvent>,
{
    let mut output = String::new();
    for event in events {
        let event = event.borrow();
        match &event.kind {
            YamlEventKind::StreamStart => output.push_str("+STR\n"),
            YamlEventKind::StreamEnd => output.push_str("-STR\n"),
            YamlEventKind::DocumentStart { explicit } => {
                if *explicit {
                    output.push_str("+DOC ---\n");
                } else {
                    output.push_str("+DOC\n");
                }
            }
            YamlEventKind::DocumentEnd { explicit } => {
                if *explicit {
                    output.push_str("-DOC ...\n");
                } else {
                    output.push_str("-DOC\n");
                }
            }
            YamlEventKind::SequenceStart { style, tag, anchor } => {
                output.push_str("+SEQ");
                match style {
                    CollectionStyle::Block => {
                        push_event_properties(&mut output, tag.as_deref(), anchor.as_deref());
                        output.push('\n');
                    }
                    CollectionStyle::Flow => {
                        output.push_str(" []");
                        push_event_properties(&mut output, tag.as_deref(), anchor.as_deref());
                        output.push('\n');
                    }
                }
            }
            YamlEventKind::SequenceEnd => output.push_str("-SEQ\n"),
            YamlEventKind::MappingStart { style, tag, anchor } => {
                output.push_str("+MAP");
                match style {
                    CollectionStyle::Block => {
                        push_event_properties(&mut output, tag.as_deref(), anchor.as_deref());
                        output.push('\n');
                    }
                    CollectionStyle::Flow => {
                        output.push_str(" {}");
                        push_event_properties(&mut output, tag.as_deref(), anchor.as_deref());
                        output.push('\n');
                    }
                }
            }
            YamlEventKind::MappingEnd => output.push_str("-MAP\n"),
            YamlEventKind::Scalar {
                style,
                value,
                tag,
                anchor,
            } => {
                output.push_str("=VAL");
                push_event_properties(&mut output, tag.as_deref(), anchor.as_deref());
                output.push(' ');
                output.push(match style {
                    YamlScalarStyle::Plain => ':',
                    YamlScalarStyle::SingleQuoted => '\'',
                    YamlScalarStyle::DoubleQuoted => '"',
                    YamlScalarStyle::Literal => '|',
                    YamlScalarStyle::Folded => '>',
                });
                output.push_str(&escape_event_value(value));
                output.push('\n');
            }
            YamlEventKind::Alias { name } => {
                output.push_str("=ALI *");
                output.push_str(name);
                output.push('\n');
            }
        }
    }
    output
}

fn push_event_properties(output: &mut String, tag: Option<&str>, anchor: Option<&str>) {
    if let Some(anchor) = anchor {
        output.push(' ');
        output.push('&');
        output.push_str(anchor);
    }
    if let Some(tag) = tag {
        output.push(' ');
        output.push_str(&event_tag_spelling(tag));
    }
}

fn event_tag_spelling(tag: &str) -> String {
    if let Some(verbatim) = tag.strip_prefix("!<").and_then(|tag| tag.strip_suffix('>')) {
        format!("<{verbatim}>")
    } else if let Some(suffix) = tag.strip_prefix("!!") {
        format!("<tag:yaml.org,2002:{suffix}>")
    } else if let Some(local) = tag.strip_prefix('!') {
        format!("<!{local}>")
    } else {
        format!("<{tag}>")
    }
}

fn escape_event_value(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            _ => output.push(character),
        }
    }
    output
}

fn decode_double_quoted_scalar(text: &str) -> Result<String, YamlError> {
    let mut output = String::new();
    let mut position = 0;

    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        if character != '\\' {
            output.push(character);
            position += character.len_utf8();
            continue;
        }

        position += '\\'.len_utf8();
        let Some(escaped) = text[position..].chars().next() else {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    "unterminated double-quoted escape",
                    Span::empty(0),
                )
                .with_expected("an escaped character"),
            ));
        };

        match escaped {
            '"' => {
                output.push('"');
                position += escaped.len_utf8();
            }
            '\\' => {
                output.push('\\');
                position += escaped.len_utf8();
            }
            '/' => {
                output.push('/');
                position += escaped.len_utf8();
            }
            ' ' => {
                output.push(' ');
                position += escaped.len_utf8();
            }
            '0' => {
                output.push('\0');
                position += escaped.len_utf8();
            }
            'a' => {
                output.push('\u{0007}');
                position += escaped.len_utf8();
            }
            'b' => {
                output.push('\u{0008}');
                position += escaped.len_utf8();
            }
            't' | '\t' => {
                output.push('\t');
                position += escaped.len_utf8();
            }
            'n' => {
                output.push('\n');
                position += escaped.len_utf8();
            }
            'v' => {
                output.push('\u{000B}');
                position += escaped.len_utf8();
            }
            'f' => {
                output.push('\u{000C}');
                position += escaped.len_utf8();
            }
            'r' => {
                output.push('\r');
                position += escaped.len_utf8();
            }
            'e' => {
                output.push('\u{001B}');
                position += escaped.len_utf8();
            }
            'x' => {
                let (character, next) = decode_hex_escape(text, position + escaped.len_utf8(), 2)?;
                output.push(character);
                position = next;
            }
            'u' => {
                let (character, next) = decode_hex_escape(text, position + escaped.len_utf8(), 4)?;
                output.push(character);
                position = next;
            }
            'U' => {
                let (character, next) = decode_hex_escape(text, position + escaped.len_utf8(), 8)?;
                output.push(character);
                position = next;
            }
            '\n' | '\r' => {
                position = skip_escaped_line_break(text, position);
            }
            _ => {
                return Err(invalid_double_quoted_escape("invalid double-quoted escape"));
            }
        }
    }

    Ok(output)
}

fn decode_hex_escape(text: &str, start: usize, digits: usize) -> Result<(char, usize), YamlError> {
    let end = start + digits;
    let Some(hex) = text.get(start..end) else {
        return Err(invalid_double_quoted_escape(
            "truncated double-quoted hex escape",
        ));
    };
    if !hex.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return Err(invalid_double_quoted_escape(
            "invalid double-quoted hex escape",
        ));
    }
    let value = u32::from_str_radix(hex, 16)
        .map_err(|_| invalid_double_quoted_escape("invalid double-quoted hex escape"))?;
    let Some(character) = char::from_u32(value) else {
        return Err(invalid_double_quoted_escape(
            "invalid Unicode scalar value in double-quoted escape",
        ));
    };

    Ok((character, end))
}

fn skip_escaped_line_break(text: &str, position: usize) -> usize {
    let mut next = if text[position..].starts_with("\r\n") {
        position + 2
    } else {
        position
            + text[position..]
                .chars()
                .next()
                .expect("position is inside text")
                .len_utf8()
    };

    while let Some(character) = text[next..].chars().next() {
        if matches!(character, ' ' | '\t') {
            next += character.len_utf8();
        } else {
            break;
        }
    }

    next
}

fn invalid_double_quoted_escape(message: &'static str) -> YamlError {
    YamlError::new(
        Diagnostic::new(DiagnosticKind::Typed, message, Span::empty(0))
            .with_expected("a valid double-quoted escape"),
    )
}
