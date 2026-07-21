use std::collections::BTreeMap;

use crate::{
    CollectionStyle, Diagnostic, DiagnosticKind, Node, NodeId, NodeKind, ParsedYaml, Source, Span,
    Token, YamlError, YamlEvent, YamlEventKind, YamlScalarStyle, validate_yaml_chars,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenEventCollection {
    Mapping,
    Sequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingNodeProperties {
    indent: usize,
    span_start: usize,
    properties: NodeProperties,
}

pub(crate) struct Parser<'source> {
    source: &'source Source,
    nodes: Vec<Node>,
    events: Vec<YamlEvent>,
    stream: Option<NodeId>,
    document: Option<NodeId>,
    document_has_content: bool,
    document_was_explicitly_opened: bool,
    document_yaml_directive_seen: bool,
    tag_handles: BTreeMap<String, String>,
    mappings: Vec<(usize, NodeId)>,
    sequences: Vec<(usize, NodeId)>,
    event_collections: Vec<(usize, OpenEventCollection)>,
    pending_node_properties: Vec<PendingNodeProperties>,
    block_scalar_content_indents: BTreeMap<NodeId, usize>,
}

impl<'source> Parser<'source> {
    pub(crate) fn new(source: &'source Source, tokens: &'source [Token]) -> Self {
        let estimated_nodes = tokens.len().saturating_div(2).saturating_add(4);
        let estimated_events = estimated_nodes.saturating_mul(2).saturating_add(2);
        Self {
            source,
            nodes: Vec::with_capacity(estimated_nodes),
            events: Vec::with_capacity(estimated_events),
            stream: None,
            document: None,
            document_has_content: false,
            document_was_explicitly_opened: false,
            document_yaml_directive_seen: false,
            tag_handles: default_tag_handles(),
            mappings: Vec::with_capacity(8),
            sequences: Vec::with_capacity(8),
            event_collections: Vec::with_capacity(8),
            pending_node_properties: Vec::with_capacity(4),
            block_scalar_content_indents: BTreeMap::new(),
        }
    }

    pub(crate) fn parse(mut self) -> Result<ParsedYaml, YamlError> {
        let stream = self.push_node(NodeKind::Stream, Span::from_usize(0, self.source.len()));
        self.stream = Some(stream);
        self.push_event(
            YamlEventKind::StreamStart,
            Span::from_usize(0, self.source.len()),
        );

        let lines = SourceLines::new(self.source).collect::<Result<Vec<_>, _>>()?;
        let mut index = 0;
        while index < lines.len() {
            index += self.parse_line(&lines, index)?;
        }
        if self.document.is_some() {
            self.close_document(false, Span::empty_from_usize(self.source.len()))?;
        } else if self.nodes[stream.0 as usize].children.is_empty() {
        } else if !self.nodes[stream.0 as usize]
            .children
            .iter()
            .any(|child| self.nodes[child.0 as usize].kind == NodeKind::Document)
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

        Ok(ParsedYaml {
            nodes: self.nodes,
            events: self.events,
        })
    }

    fn parse_line(&mut self, lines: &[SourceLine<'_>], index: usize) -> Result<usize, YamlError> {
        let line = lines[index];
        let content = line.content_without_break;
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
        if body.as_bytes().first() == Some(&b'\t')
            && (is_explicit_mapping_key(body)
                || is_explicit_mapping_value(body)
                || flow_collection_mapping_key_colon(body, line.content_start + indent)?.is_some()
                || find_mapping_colon(body).is_some())
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
            self.nodes[document.0 as usize].children.push(marker);
            let rest = rest.trim_start();
            if rest.is_empty() || rest.starts_with('#') {
                return Ok(1);
            }
            reject_compact_decorated_document(rest, line.content_end - rest.len())?;
            let rest_start = line.content_end - rest.len();
            return self.parse_content_body(document, lines, index, 0, rest, rest_start);
        }

        if let Some(rest) = document_marker_rest(body, "...") {
            if indent != 0 || !rest.trim().is_empty() && !rest.trim_start().starts_with('#') {
                return Err(invalid_document_marker(line));
            }
            let has_prior_document = self.stream.is_some_and(|stream| {
                self.nodes[stream.0 as usize]
                    .children
                    .iter()
                    .any(|child| self.nodes[child.0 as usize].kind == NodeKind::Document)
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
            self.nodes[document.0 as usize].children.push(marker);
            self.close_document(true, Span::from_usize(line.content_start, line.content_end))?;
            return Ok(1);
        }

        self.validate_indent(indent, line, body)?;
        self.close_collections_deeper_than(indent);
        self.reject_invalid_block_sibling(indent, line, body)?;
        if self.sequences.iter().any(|(level, _)| *level == indent)
            && self.mappings.iter().any(|(level, _)| *level == indent)
            && !is_sequence_entry(body)
            && (is_explicit_mapping_key(body)
                || is_explicit_mapping_value(body)
                || flow_collection_mapping_key_colon(body, line.content_start + indent)?.is_some()
                || find_mapping_colon(body).is_some())
        {
            self.close_sequence_at_indent(indent);
        }
        reject_unexpected_line_start(body, line.content_start + indent)?;

        let document = self.ensure_current_document(false, line);
        self.parse_content_body(
            document,
            lines,
            index,
            indent,
            body,
            line.content_start + indent,
        )
    }

    fn parse_content_body(
        &mut self,
        document: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        indent: usize,
        body: &str,
        absolute_start: usize,
    ) -> Result<usize, YamlError> {
        if self.nodes[document.as_usize()].kind == NodeKind::Document
            && self.document_has_content
            && indent == 0
            && self.document_has_root_flow_collection(document)
        {
            return Err(invalid_orphaned_block_content(absolute_start));
        }
        self.document_has_content = true;
        if let Some(next_indent) = property_only_node_indent(body, lines, index, absolute_start)? {
            self.push_pending_node_properties(body, absolute_start, next_indent)?;
            return Ok(1);
        }

        if is_sequence_entry(body) {
            self.parse_sequence_entry(document, lines, index, indent, body)
        } else if is_explicit_mapping_key(body) {
            self.parse_explicit_mapping_entry(document, lines, index, indent, body)
        } else if body.starts_with('|') || body.starts_with('>') {
            let (node, consumed) =
                self.parse_block_scalar(lines, index, absolute_start, indent, body, true)?;
            self.nodes[document.0 as usize].children.push(node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        } else if let Some(colon_byte) = flow_collection_mapping_key_colon(body, absolute_start)? {
            self.parse_mapping_entry(
                document,
                lines,
                index,
                indent,
                body,
                colon_byte,
                absolute_start,
            )
        } else if body_starts_flow_value(body, absolute_start)? {
            let (flow_text, consumed) = self.flow_value_text(lines, index, absolute_start, body)?;
            let (node, end) = self.parse_flow_value(flow_text, absolute_start)?;
            reject_trailing_flow_content(flow_text, end, absolute_start)?;
            self.nodes[document.0 as usize].children.push(node);
            self.emit_node_event(node)?;
            Ok(consumed)
        } else if body.starts_with('"') {
            if let Some(colon_byte) = find_mapping_colon(body) {
                self.parse_mapping_entry(
                    document,
                    lines,
                    index,
                    indent,
                    body,
                    colon_byte,
                    absolute_start,
                )
            } else {
                let (node, consumed) =
                    self.parse_quoted_scalar_lines(lines, index, absolute_start, '"')?;
                self.nodes[document.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                Ok(consumed)
            }
        } else if let Some(colon_byte) = find_mapping_colon(body) {
            self.parse_mapping_entry(
                document,
                lines,
                index,
                indent,
                body,
                colon_byte,
                absolute_start,
            )
        } else {
            let allow_same_indent_continuation = !self.has_parent_collection_below(indent);
            let (scalar, consumed) = self.parse_block_plain_scalar(
                lines,
                index,
                indent,
                absolute_start,
                allow_same_indent_continuation,
            )?;
            self.nodes[document.0 as usize].children.push(scalar);
            self.emit_scalar_event(scalar)?;
            Ok(consumed)
        }
    }

    fn parse_quoted_scalar_lines(
        &mut self,
        lines: &[SourceLine<'_>],
        index: usize,
        absolute_start: usize,
        quote: char,
    ) -> Result<(NodeId, usize), YamlError> {
        let source_tail = &self.source.as_str()[absolute_start..];
        let end = match quote {
            '"' => double_quoted_scalar_end(source_tail),
            '\'' => single_quoted_scalar_end(source_tail),
            _ => None,
        }
        .ok_or_else(|| {
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    "could not decode quoted scalar",
                    Span::empty_from_usize(absolute_start),
                )
                .with_expected("a closed quoted scalar"),
            )
        })?;
        let absolute_end = absolute_start + end;
        let mut consumed = 1;
        for line in &lines[index + 1..] {
            if line.content_start < absolute_end {
                if quote == '"' {
                    validate_double_quoted_continuation_line(line)?;
                }
                consumed += 1;
            } else {
                break;
            }
        }

        Ok((
            self.push_node(
                NodeKind::Scalar,
                Span::from_usize(absolute_start, absolute_end),
            ),
            consumed,
        ))
    }

    fn flow_collection_text<'lines>(
        &self,
        lines: &'lines [SourceLine<'_>],
        index: usize,
        absolute_start: usize,
    ) -> Result<(&'source str, usize), YamlError> {
        let source_tail = &self.source.as_str()[absolute_start..];
        let end = flow_collection_source_end(source_tail, absolute_start)?;
        let validate_sequence_indent = source_tail
            .chars()
            .find(|character| !character.is_whitespace())
            == Some('[')
            && absolute_start > lines[index].content_start + self.source_indent_at(absolute_start);
        let absolute_end = absolute_start + end;
        let mut consumed = 1;
        let mut validation_end = lines[index].content_end;
        let flow_indent = self.source_indent_at(absolute_start);
        let marker_prefix = &lines[index].content_without_break
            [..absolute_start.saturating_sub(lines[index].content_start)];
        let allow_tab_continuation = marker_prefix.as_bytes().contains(&b'\t');
        for line in &lines[index + 1..] {
            if line.content_start < absolute_end {
                if validate_sequence_indent {
                    reject_invalid_flow_continuation_indent(
                        line,
                        flow_indent,
                        allow_tab_continuation,
                    )?;
                }
                consumed += 1;
                validation_end = line.content_end;
            } else {
                break;
            }
        }
        let collection_text = &self.source.as_str()[absolute_start..validation_end];
        reject_trailing_flow_content(collection_text, end, absolute_start)?;
        Ok((collection_text, consumed))
    }

    fn flow_value_text<'lines>(
        &self,
        lines: &'lines [SourceLine<'_>],
        index: usize,
        value_start: usize,
        value_text: &str,
    ) -> Result<(&'source str, usize), YamlError> {
        let properties = parse_node_properties(
            value_text,
            Span::from_usize(value_start, value_start + value_text.len()),
        )?;
        reject_invalid_node_property_placement(value_text, value_start, &properties)?;
        let marker_offset =
            properties.value_start + leading_flow_whitespace(&value_text[properties.value_start..]);
        let marker_start = value_start + marker_offset;
        let (marker_text, consumed) = self.flow_collection_text(lines, index, marker_start)?;
        Ok((
            &self.source.as_str()[value_start..marker_start + marker_text.len()],
            consumed,
        ))
    }

    fn parse_mapping_entry(
        &mut self,
        document: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        indent: usize,
        body: &str,
        colon_byte: usize,
        absolute_start: usize,
    ) -> Result<usize, YamlError> {
        let line = lines[index];
        let mapping = self.ensure_mapping(
            document,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(line.content_start, line.content_end),
        );
        self.nodes[mapping.0 as usize].children.push(entry);
        self.extend_node_span(mapping, line.content_end);

        let key_start = absolute_start;
        let key_text = body[..colon_byte].trim_end();
        let key_end = key_start + key_text.len();
        let key_properties = parse_node_properties(key_text, Span::from_usize(key_start, key_end))?;
        reject_invalid_node_property_placement(key_text, key_start, &key_properties)?;
        if key_start < key_end && body_starts_flow_value(key_text, key_start)? {
            let (key, end) = self.parse_flow_value(key_text, key_start)?;
            reject_trailing_flow_content(key_text, end, key_start)?;
            self.nodes[entry.0 as usize].children.push(key);
            self.emit_node_event(key)?;
        } else {
            let key = if key_start < key_end {
                self.push_node(NodeKind::Scalar, Span::from_usize(key_start, key_end))
            } else {
                self.push_empty_scalar(key_start)
            };
            self.nodes[entry.0 as usize].children.push(key);
            self.emit_scalar_event(key)?;
        }

        let raw_value = &body[colon_byte + 1..];
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
                self.nodes[entry.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
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
                self.nodes[entry.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else if let Some(next_indent) = property_only_mapping_value_collection_indent(
                value_trimmed,
                lines,
                index,
                value_start,
                indent,
            )? {
                self.push_pending_node_properties(value_trimmed, value_start, next_indent)?;
                let consumed =
                    self.parse_nested_mapping_entry_value(entry, lines, index, indent)?;
                let end = lines[index + consumed - 1].content_end;
                self.extend_node_span(entry, end);
                self.extend_node_span(mapping, end);
                return Ok(consumed);
            } else if body_starts_flow_value(raw_value_trimmed, value_start)? {
                let (flow_text, consumed) =
                    self.flow_value_text(lines, index, value_start, raw_value_trimmed)?;
                let (node, end) = self.parse_flow_value(flow_text, value_start)?;
                reject_trailing_flow_content(flow_text, end, value_start)?;
                self.nodes[entry.0 as usize].children.push(node);
                self.emit_node_event(node)?;
                return Ok(consumed);
            }
            validate_quoted_scalar_trailing_content(value_trimmed, value_start)?;
            reject_nested_plain_mapping_colon(value_trimmed, value_start)?;
            let (node, consumed) =
                self.parse_block_plain_scalar(lines, index, indent, value_start, false)?;
            self.nodes[entry.0 as usize].children.push(node);
            self.emit_scalar_event(node)?;
            return Ok(consumed);
        }
        let next_significant = next_significant_body_with_index(lines, index);
        if next_significant.is_some_and(|(_, next_indent, next_body)| {
            next_indent == indent && is_sequence_entry(next_body)
        }) {
            let consumed = self.parse_nested_mapping_entry_value(entry, lines, index, indent)?;
            let end = lines[index + consumed - 1].content_end;
            self.extend_node_span(entry, end);
            self.extend_node_span(mapping, end);
            return Ok(consumed);
        }
        let next_significant_indent = next_significant.map(|(_, next_indent, _)| next_indent);
        if next_significant_indent.is_none_or(|next| next <= indent) {
            let empty = self.push_empty_scalar(line.content_end);
            self.nodes[entry.0 as usize].children.push(empty);
            self.emit_scalar_event(empty)?;
            return Ok(1);
        }
        if next_significant_indent.is_some_and(|next| next > indent) {
            let consumed = self.parse_nested_mapping_entry_value(entry, lines, index, indent)?;
            let end = lines[index + consumed - 1].content_end;
            self.extend_node_span(entry, end);
            self.extend_node_span(mapping, end);
            return Ok(consumed);
        }

        Ok(1)
    }

    fn parse_explicit_mapping_entry(
        &mut self,
        document: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        indent: usize,
        body: &str,
    ) -> Result<usize, YamlError> {
        let line = lines[index];
        let mapping = self.ensure_mapping(
            document,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(line.content_start, line.content_end),
        );
        self.nodes[mapping.0 as usize].children.push(entry);
        self.extend_node_span(mapping, line.content_end);

        let after_question = if body == "?" { "" } else { &body[1..] };
        reject_invalid_indicator_tab(body, line.content_start + indent)?;
        let key_text = strip_inline_comment(after_question).trim_start();
        let key_consumed = if key_text.is_empty() {
            if next_significant_body_with_index(lines, index).is_some_and(
                |(_, next_indent, next_body)| {
                    next_indent >= indent && !is_explicit_mapping_value(next_body)
                },
            ) {
                self.parse_following_explicit_key_block(entry, lines, index, indent)?
            } else {
                let key = self.push_empty_scalar(line.content_start + indent + 1);
                self.nodes[entry.0 as usize].children.push(key);
                self.emit_scalar_event(key)?;
                1
            }
        } else {
            let leading = after_question.len() - after_question.trim_start().len();
            let key_start = line.content_start + indent + 1 + leading;
            let key_properties = parse_node_properties(
                key_text,
                Span::from_usize(key_start, key_start + key_text.len()),
            )?;
            reject_invalid_node_property_placement(key_text, key_start, &key_properties)?;
            self.parse_explicit_mapping_key_node(entry, lines, index, indent, key_text, key_start)?
        };

        let Some((value_index, value_indent, value_body)) =
            next_significant_body_with_index(lines, index + key_consumed - 1)
        else {
            self.close_sequence_at_indent(indent);
            self.close_collections_deeper_than(indent);
            let empty = self.push_empty_scalar(line.content_end);
            self.nodes[entry.0 as usize].children.push(empty);
            self.emit_scalar_event(empty)?;
            return Ok(key_consumed);
        };

        if value_indent != indent || !is_explicit_mapping_value(value_body) {
            self.close_sequence_at_indent(indent);
            self.close_collections_deeper_than(indent);
            let empty = self.push_empty_scalar(line.content_end);
            self.nodes[entry.0 as usize].children.push(empty);
            self.emit_scalar_event(empty)?;
            return Ok(key_consumed);
        }

        self.close_sequence_at_indent(indent);
        self.close_collections_deeper_than(indent);
        let value_consumed =
            self.parse_explicit_mapping_value(entry, lines, value_index, indent, value_body)?;
        let end = lines[value_index + value_consumed - 1].content_end;
        self.extend_node_span(entry, end);
        self.extend_node_span(mapping, end);
        Ok(value_index - index + value_consumed)
    }

    fn parse_following_explicit_key_block(
        &mut self,
        entry: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        parent_indent: usize,
    ) -> Result<usize, YamlError> {
        let mut consumed = 1;
        let mut nested_index = index + 1;

        while nested_index < lines.len() {
            let line = lines[nested_index];
            let trimmed = line.content_without_break.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                consumed += 1;
                nested_index += 1;
                continue;
            }

            let indent = content_line_indent(line.content_without_break);
            let body = &line.content_without_break[indent..];
            if indent == parent_indent && is_explicit_mapping_value(body) {
                break;
            }
            if indent < parent_indent {
                break;
            }

            self.close_collections_deeper_than(indent);
            reject_unexpected_line_start(body, line.content_start + indent)?;
            let nested_consumed = self.parse_content_body(
                entry,
                lines,
                nested_index,
                indent,
                body,
                line.content_start + indent,
            )?;
            consumed += nested_consumed;
            nested_index += nested_consumed;
        }

        Ok(consumed)
    }

    fn parse_explicit_mapping_key_node(
        &mut self,
        entry: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        parent_indent: usize,
        key_text: &str,
        key_start: usize,
    ) -> Result<usize, YamlError> {
        if key_text.starts_with('|') || key_text.starts_with('>') {
            let (node, consumed) =
                self.parse_block_scalar(lines, index, key_start, parent_indent, key_text, false)?;
            self.nodes[entry.0 as usize].children.push(node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        } else if is_sequence_entry(key_text) {
            let key_indent = key_start - lines[index].content_start;
            let mut consumed =
                self.parse_sequence_entry(entry, lines, index, key_indent, key_text)?;
            let mut nested_index = index + consumed;
            while nested_index < lines.len() {
                let line = lines[nested_index];
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
                let nested_consumed = self.parse_content_body(
                    entry,
                    lines,
                    nested_index,
                    indent,
                    body,
                    line.content_start + indent,
                )?;
                consumed += nested_consumed;
                nested_index += nested_consumed;
            }
            Ok(consumed)
        } else if let Some(colon_byte) = flow_collection_mapping_key_colon(key_text, key_start)? {
            self.parse_compact_block_mapping_node(entry, key_text, key_start, colon_byte)
        } else if body_starts_flow_value(key_text, key_start)? {
            let (flow_text, consumed) = self.flow_value_text(lines, index, key_start, key_text)?;
            let (key, end) = self.parse_flow_value(flow_text, key_start)?;
            reject_trailing_flow_content(flow_text, end, key_start)?;
            self.nodes[entry.0 as usize].children.push(key);
            self.emit_node_event(key)?;
            Ok(consumed)
        } else if let Some(colon_byte) = find_mapping_colon(key_text) {
            self.parse_compact_block_mapping_node(entry, key_text, key_start, colon_byte)
        } else {
            validate_quoted_scalar_trailing_content(key_text, key_start)?;
            reject_nested_plain_mapping_colon(key_text, key_start)?;
            let (key, consumed) =
                self.parse_block_plain_scalar(lines, index, parent_indent, key_start, false)?;
            self.nodes[entry.0 as usize].children.push(key);
            self.emit_scalar_event(key)?;
            Ok(consumed)
        }
    }

    fn parse_explicit_mapping_value(
        &mut self,
        entry: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        indent: usize,
        body: &str,
    ) -> Result<usize, YamlError> {
        let line = lines[index];
        let raw_value = &body[1..];
        reject_invalid_indicator_tab(body, line.content_start + indent)?;
        let raw_value_trimmed = raw_value.trim_start();
        let value = strip_inline_comment(raw_value);
        let value_trimmed = value.trim_start();

        if value_trimmed.is_empty() {
            if next_significant_body_with_index(lines, index).is_some_and(
                |(_, next_indent, next_body)| {
                    next_indent >= indent
                        && !is_explicit_mapping_key(next_body)
                        && !is_explicit_mapping_value(next_body)
                },
            ) {
                return self.parse_following_explicit_value_block(entry, lines, index, indent);
            }
            let empty = self.push_empty_scalar(line.content_end);
            self.nodes[entry.0 as usize].children.push(empty);
            self.emit_scalar_event(empty)?;
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
            self.nodes[entry.0 as usize].children.push(node);
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
            self.nodes[entry.0 as usize].children.push(node);
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
            self.parse_nested_mapping_entry_value(entry, lines, index, indent)
        } else if is_sequence_entry(value_trimmed) {
            self.parse_sequence_entry(
                entry,
                lines,
                index,
                value_start - line.content_start,
                value_trimmed,
            )
        } else if let Some(colon_byte) = find_mapping_colon(value_trimmed) {
            self.parse_compact_block_mapping_node(entry, value_trimmed, value_start, colon_byte)?;
            Ok(1)
        } else if body_starts_flow_value(raw_value_trimmed, value_start)? {
            let (flow_text, consumed) =
                self.flow_value_text(lines, index, value_start, raw_value_trimmed)?;
            let (value_node, end) = self.parse_flow_value(flow_text, value_start)?;
            reject_trailing_flow_content(flow_text, end, value_start)?;
            self.nodes[entry.0 as usize].children.push(value_node);
            self.emit_node_event(value_node)?;
            Ok(consumed)
        } else {
            validate_quoted_scalar_trailing_content(value_trimmed, value_start)?;
            reject_nested_plain_mapping_colon(value_trimmed, value_start)?;
            let (node, consumed) =
                self.parse_block_plain_scalar(lines, index, indent, value_start, false)?;
            self.nodes[entry.0 as usize].children.push(node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        }
    }

    fn parse_following_explicit_value_block(
        &mut self,
        entry: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        parent_indent: usize,
    ) -> Result<usize, YamlError> {
        let mut consumed = 1;
        let mut nested_index = index + 1;

        while nested_index < lines.len() {
            let line = lines[nested_index];
            let trimmed = line.content_without_break.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                consumed += 1;
                nested_index += 1;
                continue;
            }

            let indent = content_line_indent(line.content_without_break);
            let body = &line.content_without_break[indent..];
            if indent < parent_indent
                || (indent == parent_indent
                    && (is_explicit_mapping_key(body) || is_explicit_mapping_value(body)))
            {
                break;
            }

            self.close_collections_deeper_than(indent);
            reject_unexpected_line_start(body, line.content_start + indent)?;
            let nested_consumed = self.parse_content_body(
                entry,
                lines,
                nested_index,
                indent,
                body,
                line.content_start + indent,
            )?;
            consumed += nested_consumed;
            nested_index += nested_consumed;
        }

        self.close_sequence_at_indent(parent_indent);
        Ok(consumed)
    }

    fn parse_nested_mapping_entry_value(
        &mut self,
        entry: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        parent_indent: usize,
    ) -> Result<usize, YamlError> {
        let mut nested_index = index + 1;
        while nested_index < lines.len() {
            let line = lines[nested_index];
            let trimmed = line.content_without_break.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                nested_index += 1;
                continue;
            }

            let nested_indent = content_line_indent(line.content_without_break);
            if nested_indent < parent_indent {
                break;
            }

            let body = &line.content_without_break[nested_indent..];
            if nested_indent == parent_indent
                && body.as_bytes().first() == Some(&b'\t')
                && flow_collection_mapping_key_colon(body, line.content_start + nested_indent)?
                    .is_some()
            {
                return Err(tab_indentation_error(line.content_start + nested_indent));
            }
            if nested_indent == parent_indent && !is_sequence_entry(body) {
                break;
            }
            if nested_indent == parent_indent {
                self.close_sequence_at_indent(parent_indent);
            }
            if property_only_block_collection_indent(
                body,
                lines,
                nested_index,
                line.content_start + nested_indent,
            )?
            .is_some()
            {
                return self.parse_nested_block_value(entry, lines, index, parent_indent);
            }
            if property_only_node_indent(
                body,
                lines,
                nested_index,
                line.content_start + nested_indent,
            )?
            .is_some()
            {
                return self.parse_nested_block_value(entry, lines, index, parent_indent);
            }
            if is_sequence_entry(body)
                || is_explicit_mapping_key(body)
                || find_mapping_colon(body).is_some()
            {
                return self.parse_nested_block_value(entry, lines, index, parent_indent);
            }

            let (scalar, consumed) = self.parse_block_plain_scalar(
                lines,
                nested_index,
                parent_indent,
                line.content_start + nested_indent,
                false,
            )?;
            self.nodes[entry.0 as usize].children.push(scalar);
            self.emit_scalar_event(scalar)?;
            return Ok(nested_index - index + consumed);
        }

        Ok(1)
    }

    fn push_pending_node_properties(
        &mut self,
        text: &str,
        absolute_start: usize,
        indent: usize,
    ) -> Result<(), YamlError> {
        let mut properties = parse_node_properties(
            text,
            Span::from_usize(absolute_start, absolute_start + text.len()),
        )?;
        reject_invalid_node_property_placement(text, absolute_start, &properties)?;
        self.resolve_node_properties(
            &mut properties,
            Span::from_usize(absolute_start, absolute_start + text.len()),
        )?;
        if let Some(pending) = self
            .pending_node_properties
            .iter_mut()
            .find(|pending| pending.indent == indent)
        {
            pending.span_start = pending.span_start.min(absolute_start);
            if pending.properties.anchor.is_none() {
                pending.properties.anchor = properties.anchor;
            }
            if pending.properties.tag.is_none() {
                pending.properties.tag = properties.tag;
            }
        } else {
            self.pending_node_properties.push(PendingNodeProperties {
                indent,
                span_start: absolute_start,
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
        self.nodes[stream.0 as usize].children.push(document);
        self.document = Some(document);
        self.document_has_content = false;
        self.document_was_explicitly_opened = explicit;
        self.mappings.clear();
        self.sequences.clear();
        self.event_collections.clear();
        self.pending_node_properties.clear();
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
            self.nodes[document.0 as usize].children.push(empty);
            self.emit_scalar_event(empty)?;
        }
        self.push_event(YamlEventKind::DocumentEnd { explicit }, span);
        self.document = None;
        self.document_has_content = false;
        self.document_was_explicitly_opened = false;
        self.document_yaml_directive_seen = false;
        self.tag_handles = default_tag_handles();
        self.mappings.clear();
        self.sequences.clear();
        self.event_collections.clear();
        self.pending_node_properties.clear();
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
        self.nodes[stream.0 as usize].children.push(directive);

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
                self.tag_handles
                    .insert(handle.to_owned(), prefix.to_owned());
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn parse_sequence_entry(
        &mut self,
        document: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        indent: usize,
        body: &str,
    ) -> Result<usize, YamlError> {
        let line = lines[index];
        let sequence = self.ensure_sequence(
            document,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        let entry = self.push_node(
            NodeKind::SequenceEntry,
            Span::from_usize(line.content_start, line.content_end),
        );
        self.nodes[sequence.0 as usize].children.push(entry);
        self.extend_node_span(sequence, line.content_end);

        let after_dash = if body == "-" { "" } else { &body[1..] };
        reject_invalid_indicator_tab(body, line.content_start + indent)?;
        reject_invalid_sequence_tab_separated_nested_indicator(body, line.content_start + indent)?;
        let value = after_dash.trim_start();
        let value_without_comment = strip_inline_comment(after_dash).trim_start();
        if value.is_empty() || value_without_comment.is_empty() {
            if next_significant_indent(lines, index)?.is_some_and(|next| next > indent) {
                let consumed =
                    self.parse_nested_sequence_entry_value(entry, lines, index, indent)?;
                let end = lines[index + consumed - 1].content_end;
                self.extend_node_span(entry, end);
                self.extend_node_span(sequence, end);
                return Ok(consumed);
            }
            let empty = self.push_empty_scalar(line.content_start + indent + 1);
            self.nodes[entry.0 as usize].children.push(empty);
            self.emit_scalar_event(empty)?;
        } else {
            let leading = after_dash.len() - value.len();
            let value_start = line.content_start + indent + 1 + leading;
            let value_properties = parse_node_properties(
                value,
                Span::from_usize(value_start, value_start + value.len()),
            )?;
            reject_invalid_node_property_placement(value, value_start, &value_properties)?;
            reject_invalid_block_node_property_punctuation(value, value_start, &value_properties)?;
            if value.starts_with('|') || value.starts_with('>') {
                let (node, consumed) =
                    self.parse_block_scalar(lines, index, value_start, indent, value, false)?;
                self.nodes[entry.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else if let Some(header_offset) =
                block_scalar_after_node_properties(value, value_start)?
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
                self.nodes[entry.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else if let Some(next_indent) =
                property_only_block_collection_indent(value, lines, index, value_start)?
                    .filter(|next_indent| *next_indent > indent)
            {
                self.push_pending_node_properties(value, value_start, next_indent)?;
                let consumed =
                    self.parse_nested_sequence_entry_value(entry, lines, index, indent)?;
                let end = lines[index + consumed - 1].content_end;
                self.extend_node_span(entry, end);
                self.extend_node_span(sequence, end);
                return Ok(consumed);
            } else if is_sequence_entry(value) {
                return self.parse_sequence_entry(
                    entry,
                    lines,
                    index,
                    value_start - line.content_start,
                    value,
                );
            } else if body_starts_flow_value(value, value_start)? {
                let (flow_text, consumed) =
                    self.flow_value_text(lines, index, value_start, value)?;
                let (value_node, end) = self.parse_flow_value(flow_text, value_start)?;
                reject_trailing_flow_content(flow_text, end, value_start)?;
                self.nodes[entry.0 as usize].children.push(value_node);
                self.emit_node_event(value_node)?;
                return Ok(consumed);
            } else if is_explicit_mapping_key(value) {
                return self.parse_explicit_mapping_entry(
                    entry,
                    lines,
                    index,
                    value_start - line.content_start,
                    value,
                );
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
            self.nodes[entry.0 as usize].children.push(value_node);
            self.emit_scalar_event(value_node)?;
            return Ok(consumed);
        }

        Ok(1)
    }

    fn parse_compact_explicit_empty_key_mapping(
        &mut self,
        parent: NodeId,
        line: SourceLine<'_>,
        indent: usize,
        body: &str,
        absolute_start: usize,
    ) -> Result<(), YamlError> {
        let mapping = self.ensure_mapping(
            parent,
            indent,
            Span::from_usize(line.content_start + indent, line.content_end),
        );
        let outer_entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(absolute_start, line.content_end),
        );
        self.nodes[mapping.0 as usize].children.push(outer_entry);

        let inner_mapping = self.push_node(
            NodeKind::BlockMapping,
            Span::from_usize(absolute_start, line.content_end),
        );
        self.nodes[outer_entry.0 as usize]
            .children
            .push(inner_mapping);
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
        self.nodes[inner_mapping.0 as usize]
            .children
            .push(inner_entry);

        let colon_offset = body
            .find(':')
            .expect("compact explicit empty key mapping contains colon");
        let colon_start = absolute_start + colon_offset;
        let key = self.push_empty_scalar(colon_start);
        self.nodes[inner_entry.0 as usize].children.push(key);
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
        self.nodes[inner_entry.0 as usize].children.push(value);
        self.emit_scalar_event(value)?;
        self.push_event(
            YamlEventKind::MappingEnd,
            Span::empty_from_usize(line.content_end),
        );

        let outer_value = self.push_empty_scalar(line.content_end);
        self.nodes[outer_entry.0 as usize]
            .children
            .push(outer_value);
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
        self.nodes[parent.0 as usize].children.push(mapping);
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
        self.nodes[mapping.0 as usize].children.push(entry);

        let key_text = body[..colon_byte].trim_end();
        let key = if key_text.is_empty() {
            self.push_empty_scalar(absolute_start)
        } else if body_starts_flow_value(key_text, absolute_start)? {
            let (key, end) = self.parse_flow_value(key_text, absolute_start)?;
            reject_trailing_flow_content(key_text, end, absolute_start)?;
            key
        } else {
            let key_end = absolute_start + key_text.len();
            self.push_node(NodeKind::Scalar, Span::from_usize(absolute_start, key_end))
        };
        self.nodes[entry.0 as usize].children.push(key);
        self.emit_node_event(key)?;

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
        self.nodes[entry.0 as usize].children.push(value);
        self.emit_scalar_event(value)?;

        self.push_event(
            YamlEventKind::MappingEnd,
            Span::empty_from_usize(absolute_start + body.len()),
        );
        Ok(1)
    }

    fn parse_nested_sequence_entry_value(
        &mut self,
        entry: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        parent_indent: usize,
    ) -> Result<usize, YamlError> {
        self.parse_nested_block_value(entry, lines, index, parent_indent)
    }

    fn parse_nested_block_value(
        &mut self,
        parent: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        parent_indent: usize,
    ) -> Result<usize, YamlError> {
        let mut consumed = 1;
        let mut nested_index = index + 1;

        while nested_index < lines.len() {
            let line = lines[nested_index];
            let trimmed = line.content_without_break.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                consumed += 1;
                nested_index += 1;
                continue;
            }

            let nested_indent = content_line_indent(line.content_without_break);
            if nested_indent <= parent_indent {
                break;
            }

            let body = &line.content_without_break[nested_indent..];
            if property_only_node_indent(
                body,
                lines,
                nested_index,
                line.content_start + nested_indent,
            )?
            .is_some()
                && let Some((header_index, header_indent, header_body)) =
                    first_non_property_node_after_with_index(
                        lines,
                        nested_index,
                        line.content_start + nested_indent,
                    )?
                && (header_body.starts_with('|') || header_body.starts_with('>'))
            {
                for property_line in &lines[nested_index..header_index] {
                    let property_trimmed = property_line.content_without_break.trim();
                    if property_trimmed.is_empty() || property_trimmed.starts_with('#') {
                        continue;
                    }
                    let property_indent = content_line_indent(property_line.content_without_break);
                    let property_body = &property_line.content_without_break[property_indent..];
                    self.push_pending_node_properties(
                        property_body,
                        property_line.content_start + property_indent,
                        header_indent,
                    )?;
                }
                let header_start = lines[header_index].content_start + header_indent;
                let (node, scalar_consumed) = self.parse_block_scalar(
                    lines,
                    header_index,
                    header_start,
                    parent_indent,
                    header_body,
                    true,
                )?;
                self.nodes[parent.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                consumed += header_index - nested_index + scalar_consumed;
                nested_index = header_index + scalar_consumed;
                continue;
            }

            self.close_collections_deeper_than(nested_indent);
            reject_unexpected_line_start(body, line.content_start + nested_indent)?;
            if self
                .sequences
                .iter()
                .any(|(level, _)| *level == nested_indent)
                && !is_sequence_entry(body)
            {
                return Err(invalid_nested_block_sequence_sibling(
                    line.content_start + nested_indent,
                ));
            }
            let nested_consumed = self.parse_content_body(
                parent,
                lines,
                nested_index,
                nested_indent,
                body,
                line.content_start + nested_indent,
            )?;
            consumed += nested_consumed;
            nested_index += nested_consumed;
        }

        Ok(consumed)
    }

    fn parse_block_plain_scalar(
        &mut self,
        lines: &[SourceLine<'_>],
        index: usize,
        parent_indent: usize,
        value_start: usize,
        allow_same_indent_continuation: bool,
    ) -> Result<(NodeId, usize), YamlError> {
        let mut consumed = 1;
        let mut end = lines[index].content_end;
        let mut pending_blank_lines = 0usize;
        let mut scalar_has_inline_comment = plain_scalar_line_has_inline_comment(
            &self.source.as_str()[value_start..lines[index].content_end],
        );
        let initial_properties = parse_node_properties(
            &self.source.as_str()[value_start..lines[index].content_end],
            Span::from_usize(value_start, lines[index].content_end),
        )?;
        let scalar_has_node_properties =
            initial_properties.anchor.is_some() || initial_properties.tag.is_some();

        for line in &lines[index + 1..] {
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
                return Err(directive_after_document_content(*line).with_position_from(self.source));
            }
            if self.source.as_str()[value_start..lines[index].content_end].starts_with('"')
                && line.content_without_break.starts_with('\t')
            {
                return Err(tab_indentation_error(line.content_start));
            }
            let indent = content_line_indent(line.content_without_break);
            let body = &line.content_without_break[indent..];
            if scalar_has_inline_comment || find_mapping_colon(body).is_some() {
                return Err(invalid_plain_scalar_continuation(
                    line.content_start + indent,
                ));
            }

            consumed += pending_blank_lines + 1;
            pending_blank_lines = 0;
            end = line.content_end;
            scalar_has_inline_comment |= plain_scalar_line_has_inline_comment(body);
        }

        Ok((
            self.push_node(NodeKind::Scalar, Span::from_usize(value_start, end)),
            consumed,
        ))
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
        let value_start =
            properties.value_start + leading_flow_whitespace(&text[properties.value_start..]);
        let value_text = &text[value_start..];
        if value_text.starts_with('[') {
            let (node, consumed) =
                self.parse_flow_sequence(value_text, absolute_start + value_start)?;
            self.nodes[node.as_usize()].span.start = Span::usize_to_u32(absolute_start);
            Ok((node, value_start + consumed))
        } else if value_text.starts_with('{') {
            let (node, consumed) =
                self.parse_flow_mapping(value_text, absolute_start + value_start)?;
            self.nodes[node.as_usize()].span.start = Span::usize_to_u32(absolute_start);
            Ok((node, value_start + consumed))
        } else {
            let end = flow_scalar_end(text, value_start, absolute_start, &[',', ']', '}'])?;
            let scalar_start = value_start + leading_flow_whitespace(&text[value_start..end]);
            let scalar_end = end - trailing_flow_whitespace(&text[value_start..end]);
            if scalar_start >= scalar_end {
                return Err(empty_flow_value(absolute_start));
            }
            Ok((
                self.push_node(
                    NodeKind::Scalar,
                    Span::from_usize(absolute_start, absolute_start + scalar_end),
                ),
                end,
            ))
        }
    }

    fn parse_block_scalar(
        &mut self,
        lines: &[SourceLine<'_>],
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
        let mut end = lines[index].line_end;
        let inline_header =
            header_start > lines[index].content_start + parent_indent && !allow_same_indent_content;
        let mut pending_blank_lines = 0usize;
        let mut pending_blank_end = end;
        let mut pending_blank_indent = None::<usize>;
        let mut reached_end = true;

        for line in &lines[index + 1..] {
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
        self.block_scalar_content_indents
            .insert(scalar, content_indent);
        Ok((scalar, consumed))
    }

    fn parse_flow_sequence(
        &mut self,
        text: &str,
        absolute_start: usize,
    ) -> Result<(NodeId, usize), YamlError> {
        debug_assert!(text.starts_with('['));

        let sequence = self.push_node(
            NodeKind::FlowSequence,
            Span::from_usize(absolute_start, absolute_start + 1),
        );
        let mut position = 1;
        let mut expecting_value = true;
        let mut saw_item = false;

        loop {
            position = skip_flow_whitespace(text, position);
            let Some(character) = text[position..].chars().next() else {
                return Err(missing_flow_sequence_end(absolute_start, text.len()));
            };

            match character {
                ']' => {
                    if expecting_value || saw_item {
                        position += 1;
                        self.nodes[sequence.as_usize()].span.end =
                            Span::usize_to_u32(absolute_start + position);
                        return Ok((sequence, position));
                    }
                    return Err(empty_flow_sequence_item(absolute_start + position));
                }
                ',' => {
                    return Err(unexpected_flow_comma(absolute_start + position));
                }
                '?' if is_flow_explicit_key_indicator(text, position) => {
                    let (mapping, consumed) =
                        self.parse_explicit_flow_mapping_item(text, position, absolute_start, ']')?;
                    self.nodes[sequence.0 as usize].children.push(mapping);
                    position = consumed;
                }
                '[' | '{' => {
                    if let Some(colon) =
                        flow_mapping_separator(text, position, absolute_start, &[',', ']'])?
                    {
                        let (mapping, consumed) = self.parse_implicit_flow_mapping(
                            text,
                            position,
                            colon,
                            absolute_start,
                        )?;
                        self.nodes[sequence.0 as usize].children.push(mapping);
                        position = consumed;
                    } else {
                        let child_start = absolute_start + position;
                        let (child, consumed) =
                            self.parse_flow_value(&text[position..], child_start)?;
                        self.nodes[sequence.0 as usize].children.push(child);
                        position += consumed;
                    }
                }
                _ => {
                    let value_start = position;
                    if let Some(colon) =
                        flow_mapping_separator(text, position, absolute_start, &[',', ']'])?
                    {
                        let (mapping, consumed) = self.parse_implicit_flow_mapping(
                            text,
                            value_start,
                            colon,
                            absolute_start,
                        )?;
                        self.nodes[sequence.0 as usize].children.push(mapping);
                        position = consumed;
                    } else if body_starts_flow_value(&text[position..], absolute_start + position)?
                    {
                        let (value, consumed) =
                            self.parse_flow_value(&text[position..], absolute_start + position)?;
                        self.nodes[sequence.0 as usize].children.push(value);
                        position += consumed;
                    } else {
                        let value_end =
                            flow_scalar_end(text, position, absolute_start, &[',', ']'])?;
                        let scalar_start =
                            value_start + leading_flow_whitespace(&text[value_start..value_end]);
                        let scalar_end =
                            value_end - trailing_flow_whitespace(&text[value_start..value_end]);
                        if scalar_start >= scalar_end {
                            return Err(empty_flow_sequence_item(absolute_start + position));
                        }
                        let scalar = self.push_node(
                            NodeKind::Scalar,
                            Span::from_usize(
                                absolute_start + scalar_start,
                                absolute_start + scalar_end,
                            ),
                        );
                        self.nodes[sequence.0 as usize].children.push(scalar);
                        position = value_end;
                    }
                }
            }

            saw_item = true;
            position = skip_flow_whitespace(text, position);
            let Some(separator) = text[position..].chars().next() else {
                return Err(missing_flow_sequence_end(absolute_start, text.len()));
            };

            match separator {
                ',' => {
                    position += 1;
                    expecting_value = true;
                }
                ']' => {
                    expecting_value = false;
                }
                _ => {
                    return Err(expected_flow_separator(
                        absolute_start + position,
                        separator,
                    ));
                }
            }
        }
    }

    fn parse_implicit_flow_mapping(
        &mut self,
        text: &str,
        entry_start: usize,
        colon: usize,
        absolute_start: usize,
    ) -> Result<(NodeId, usize), YamlError> {
        let mapping = self.push_node(
            NodeKind::FlowMapping,
            Span::from_usize(absolute_start + entry_start, absolute_start + entry_start),
        );
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(absolute_start + entry_start, absolute_start + entry_start),
        );
        let key = self.parse_flow_node_segment(text, entry_start, colon, absolute_start)?;
        reject_split_implicit_flow_mapping_key(text, entry_start, colon, absolute_start)?;
        self.nodes[entry.0 as usize].children.push(key);

        let mut value_position = skip_flow_whitespace(text, colon + 1);
        match text[value_position..].chars().next() {
            Some(',' | ']') => {
                let value = self.push_empty_scalar(absolute_start + value_position);
                self.nodes[entry.0 as usize].children.push(value);
            }
            Some(_) => {
                value_position = self.parse_flow_mapping_value_with_close(
                    text,
                    value_position,
                    absolute_start,
                    entry,
                    ']',
                )?;
            }
            None => return Err(missing_flow_sequence_end(absolute_start, text.len())),
        }

        self.nodes[entry.as_usize()].span.end = Span::usize_to_u32(absolute_start + value_position);
        self.nodes[mapping.as_usize()].span.end =
            Span::usize_to_u32(absolute_start + value_position);
        self.nodes[mapping.0 as usize].children.push(entry);
        Ok((mapping, value_position))
    }

    fn parse_flow_mapping(
        &mut self,
        text: &str,
        absolute_start: usize,
    ) -> Result<(NodeId, usize), YamlError> {
        debug_assert!(text.starts_with('{'));

        let mapping = self.push_node(
            NodeKind::FlowMapping,
            Span::from_usize(absolute_start, absolute_start + 1),
        );
        let mut position = 1;
        let mut expecting_pair = true;
        let mut saw_pair = false;

        loop {
            position = skip_flow_whitespace(text, position);
            let Some(character) = text[position..].chars().next() else {
                return Err(missing_flow_mapping_end(absolute_start, text.len()));
            };

            match character {
                '}' => {
                    if expecting_pair || saw_pair {
                        position += 1;
                        self.nodes[mapping.as_usize()].span.end =
                            Span::usize_to_u32(absolute_start + position);
                        return Ok((mapping, position));
                    }
                    return Err(empty_flow_mapping_pair(absolute_start + position));
                }
                ',' => return Err(unexpected_flow_mapping_comma(absolute_start + position)),
                _ => {}
            }

            if character == '?' && is_flow_explicit_key_indicator(text, position) {
                let (entry, consumed) =
                    self.parse_explicit_flow_mapping_entry(text, position, absolute_start, '}')?;
                self.nodes[mapping.0 as usize].children.push(entry);
                self.nodes[entry.as_usize()].span.end =
                    Span::usize_to_u32(absolute_start + consumed);
                self.nodes[mapping.as_usize()].span.end =
                    Span::usize_to_u32(absolute_start + consumed);
                saw_pair = true;
                position = skip_flow_whitespace(text, consumed);
                let Some(separator) = text[position..].chars().next() else {
                    return Err(missing_flow_mapping_end(absolute_start, text.len()));
                };
                match separator {
                    ',' => {
                        position += 1;
                        expecting_pair = true;
                    }
                    '}' => {
                        expecting_pair = false;
                    }
                    _ => {
                        return Err(expected_flow_mapping_separator(
                            absolute_start + position,
                            separator,
                        ));
                    }
                }
                continue;
            }

            let entry_start = position;
            let entry = self.push_node(
                NodeKind::MappingEntry,
                Span::from_usize(absolute_start + entry_start, absolute_start + entry_start),
            );
            let (key, key_end) =
                self.parse_flow_mapping_key(text, position, absolute_start, character)?;
            position = key_end;
            self.nodes[entry.0 as usize].children.push(key);

            let separator_position = skip_flow_whitespace(text, position);
            position = separator_position;
            let Some(separator) = text[position..].chars().next() else {
                return Err(missing_flow_mapping_end(absolute_start, text.len()));
            };
            if separator == ',' {
                let value = self.push_empty_scalar(absolute_start + position);
                self.nodes[entry.0 as usize].children.push(value);
                self.nodes[entry.as_usize()].span.end =
                    Span::usize_to_u32(absolute_start + position);
                self.nodes[mapping.0 as usize].children.push(entry);
                saw_pair = true;
                position += 1;
                expecting_pair = true;
                continue;
            }
            if separator != ':' {
                return Err(missing_flow_mapping_colon(
                    absolute_start + position,
                    separator,
                ));
            }
            position += 1;
            let value_start = position;
            position = skip_flow_whitespace(text, position);
            reject_unindented_split_flow_mapping_value(
                text,
                value_start,
                position,
                absolute_start,
                self.source_indent_at(absolute_start),
            )?;
            position = self.parse_flow_mapping_value(text, position, absolute_start, entry)?;

            self.nodes[entry.as_usize()].span.end = Span::usize_to_u32(absolute_start + position);
            self.nodes[mapping.0 as usize].children.push(entry);
            saw_pair = true;
            position = skip_flow_whitespace(text, position);
            let Some(separator) = text[position..].chars().next() else {
                return Err(missing_flow_mapping_end(absolute_start, text.len()));
            };

            match separator {
                ',' => {
                    position += 1;
                    expecting_pair = true;
                }
                '}' => {
                    expecting_pair = false;
                }
                _ => {
                    return Err(expected_flow_mapping_separator(
                        absolute_start + position,
                        separator,
                    ));
                }
            }
        }
    }

    fn parse_flow_mapping_key(
        &mut self,
        text: &str,
        position: usize,
        absolute_start: usize,
        character: char,
    ) -> Result<(NodeId, usize), YamlError> {
        if character == '[' || character == '{' {
            let (key, consumed) =
                self.parse_flow_value(&text[position..], absolute_start + position)?;
            return Ok((key, position + consumed));
        }

        let key_separator = flow_mapping_separator(text, position, absolute_start, &[',', '}'])?;
        let key_end = match key_separator {
            Some(separator) => separator,
            None => flow_scalar_end(text, position, absolute_start, &[',', '}'])?,
        };
        if body_starts_flow_value(&text[position..key_end], absolute_start + position)? {
            let (key, consumed) =
                self.parse_flow_value(&text[position..key_end], absolute_start + position)?;
            reject_trailing_flow_content(
                &text[position..key_end],
                consumed,
                absolute_start + position,
            )?;
            return Ok((key, position + consumed));
        }
        let key_start = position + leading_flow_whitespace(&text[position..key_end]);
        let key_trimmed_end = key_end - trailing_flow_whitespace(&text[position..key_end]);

        if key_start >= key_trimmed_end && text[key_end..].starts_with(':') {
            Ok((self.push_empty_scalar(absolute_start + key_start), key_end))
        } else if key_start >= key_trimmed_end {
            Err(empty_flow_mapping_key(absolute_start + position))
        } else {
            Ok((
                self.push_node(
                    NodeKind::Scalar,
                    Span::from_usize(absolute_start + key_start, absolute_start + key_trimmed_end),
                ),
                key_end,
            ))
        }
    }

    fn parse_explicit_flow_mapping_item(
        &mut self,
        text: &str,
        position: usize,
        absolute_start: usize,
        close: char,
    ) -> Result<(NodeId, usize), YamlError> {
        let mapping = self.push_node(
            NodeKind::FlowMapping,
            Span::from_usize(absolute_start + position, absolute_start + position),
        );
        let (entry, consumed) =
            self.parse_explicit_flow_mapping_entry(text, position, absolute_start, close)?;
        self.nodes[mapping.0 as usize].children.push(entry);
        self.nodes[mapping.as_usize()].span.end = Span::usize_to_u32(absolute_start + consumed);
        Ok((mapping, consumed))
    }

    fn parse_explicit_flow_mapping_entry(
        &mut self,
        text: &str,
        position: usize,
        absolute_start: usize,
        close: char,
    ) -> Result<(NodeId, usize), YamlError> {
        debug_assert!(text[position..].starts_with('?'));
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::from_usize(absolute_start + position, absolute_start + position),
        );
        let key_position = skip_flow_whitespace(text, position + '?'.len_utf8());
        if text[key_position..]
            .chars()
            .next()
            .is_some_and(|character| character == ',' || character == close)
        {
            let key = self.push_empty_scalar(absolute_start + key_position);
            let value = self.push_empty_scalar(absolute_start + key_position);
            self.nodes[entry.0 as usize].children.push(key);
            self.nodes[entry.0 as usize].children.push(value);
            return Ok((entry, key_position));
        }

        let terminators = [',', close];
        let colon = flow_mapping_separator(text, key_position, absolute_start, &terminators)?;
        let (key_end, has_value) = if let Some(colon) = colon {
            (colon, true)
        } else {
            (
                flow_scalar_end(text, key_position, absolute_start, &terminators)?,
                false,
            )
        };
        let key = self.parse_flow_node_segment(text, key_position, key_end, absolute_start)?;
        self.nodes[entry.0 as usize].children.push(key);

        let consumed = if has_value {
            let value_position = skip_flow_whitespace(text, key_end + ':'.len_utf8());
            self.parse_flow_mapping_value_with_close(
                text,
                value_position,
                absolute_start,
                entry,
                close,
            )?
        } else {
            let value = self.push_empty_scalar(absolute_start + key_end);
            self.nodes[entry.0 as usize].children.push(value);
            key_end
        };
        Ok((entry, consumed))
    }

    fn parse_flow_node_segment(
        &mut self,
        text: &str,
        start: usize,
        end: usize,
        absolute_start: usize,
    ) -> Result<NodeId, YamlError> {
        let node_start = start + leading_flow_whitespace(&text[start..end]);
        let node_end = end - trailing_flow_whitespace(&text[start..end]);
        if node_start >= node_end {
            return Ok(self.push_empty_scalar(absolute_start + node_start));
        }
        let segment = &text[node_start..node_end];
        if body_starts_flow_value(segment, absolute_start + node_start)? {
            let (node, consumed) = self.parse_flow_value(segment, absolute_start + node_start)?;
            reject_trailing_flow_content(segment, consumed, absolute_start + node_start)?;
            Ok(node)
        } else {
            Ok(self.push_node(
                NodeKind::Scalar,
                Span::from_usize(absolute_start + node_start, absolute_start + node_end),
            ))
        }
    }

    fn parse_flow_mapping_value(
        &mut self,
        text: &str,
        position: usize,
        absolute_start: usize,
        entry: NodeId,
    ) -> Result<usize, YamlError> {
        self.parse_flow_mapping_value_with_close(text, position, absolute_start, entry, '}')
    }

    fn parse_flow_mapping_value_with_close(
        &mut self,
        text: &str,
        mut position: usize,
        absolute_start: usize,
        entry: NodeId,
        close: char,
    ) -> Result<usize, YamlError> {
        if body_starts_flow_value(&text[position..], absolute_start + position)? {
            let (value, consumed) =
                self.parse_flow_value(&text[position..], absolute_start + position)?;
            self.nodes[entry.0 as usize].children.push(value);
            return Ok(position + consumed);
        }

        match text[position..].chars().next() {
            None => return Err(missing_flow_mapping_end(absolute_start, text.len())),
            Some(',') => {
                let value = self.push_empty_scalar(absolute_start + position);
                self.nodes[entry.0 as usize].children.push(value);
            }
            Some(character) if character == close => {
                let value = self.push_empty_scalar(absolute_start + position);
                self.nodes[entry.0 as usize].children.push(value);
            }
            Some(_) => {
                let terminators = [',', close];
                let value_end = flow_scalar_end(text, position, absolute_start, &terminators)?;
                let value_start = position + leading_flow_whitespace(&text[position..value_end]);
                let value_trimmed_end =
                    value_end - trailing_flow_whitespace(&text[position..value_end]);
                if value_start < value_trimmed_end {
                    reject_flow_plain_scalar_mapping_separator(
                        text,
                        value_start,
                        value_trimmed_end,
                        absolute_start,
                    )?;
                    let value = self.push_node(
                        NodeKind::Scalar,
                        Span::from_usize(
                            absolute_start + value_start,
                            absolute_start + value_trimmed_end,
                        ),
                    );
                    self.nodes[entry.0 as usize].children.push(value);
                }
                position = value_end;
            }
        }
        Ok(position)
    }

    fn ensure_mapping(&mut self, parent: NodeId, indent: usize, span: Span) -> NodeId {
        if let Some((_, node)) = self.mappings.iter().find(|(level, _)| *level == indent) {
            *node
        } else {
            let (span, tag, anchor) = self.collection_properties(indent, span);
            let mapping = self.push_node(NodeKind::BlockMapping, span);
            self.nodes[parent.0 as usize].children.push(mapping);
            self.mappings.push((indent, mapping));
            self.open_event_collection(
                indent,
                OpenEventCollection::Mapping,
                YamlEventKind::MappingStart {
                    style: CollectionStyle::Block,
                    tag,
                    anchor,
                },
                span,
                mapping,
            );
            mapping
        }
    }

    fn document_has_root_flow_collection(&self, document: NodeId) -> bool {
        self.nodes[document.as_usize()]
            .children
            .iter()
            .any(|child| {
                matches!(
                    self.nodes[child.as_usize()].kind,
                    NodeKind::FlowSequence | NodeKind::FlowMapping
                )
            })
    }

    fn ensure_sequence(&mut self, parent: NodeId, indent: usize, span: Span) -> NodeId {
        if let Some((_, node)) = self.sequences.iter().find(|(level, _)| *level == indent) {
            *node
        } else {
            let (span, tag, anchor) = self.collection_properties(indent, span);
            let sequence = self.push_node(NodeKind::BlockSequence, span);
            self.nodes[parent.0 as usize].children.push(sequence);
            self.sequences.push((indent, sequence));
            self.open_event_collection(
                indent,
                OpenEventCollection::Sequence,
                YamlEventKind::SequenceStart {
                    style: CollectionStyle::Block,
                    tag,
                    anchor,
                },
                span,
                sequence,
            );
            sequence
        }
    }

    fn collection_properties(
        &mut self,
        indent: usize,
        span: Span,
    ) -> (Span, Option<String>, Option<String>) {
        let Some(pending) = self.take_pending_node_properties(indent) else {
            return (span, None, None);
        };
        (
            Span::new(Span::usize_to_u32(pending.span_start), span.end),
            pending.properties.tag,
            pending.properties.anchor,
        )
    }

    fn take_pending_node_properties(&mut self, indent: usize) -> Option<PendingNodeProperties> {
        let index = self
            .pending_node_properties
            .iter()
            .rposition(|pending| pending.indent == indent)?;
        Some(self.pending_node_properties.remove(index))
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

        let has_parent_collection = self.has_parent_collection_below(indent);

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

    fn has_parent_collection_below(&self, indent: usize) -> bool {
        self.mappings
            .iter()
            .chain(self.sequences.iter())
            .any(|(level, _)| *level < indent)
    }

    fn reject_invalid_block_sibling(
        &self,
        indent: usize,
        line: SourceLine<'_>,
        body: &str,
    ) -> Result<(), YamlError> {
        if self.sequences.iter().any(|(level, _)| *level == indent) && !is_sequence_entry(body) {
            let has_mapping_at_indent = self.mappings.iter().any(|(level, _)| *level == indent);
            let is_mapping_sibling = is_explicit_mapping_key(body)
                || is_explicit_mapping_value(body)
                || flow_collection_mapping_key_colon(body, line.content_start + indent)?.is_some()
                || find_mapping_colon(body).is_some();
            if !has_mapping_at_indent || !is_mapping_sibling {
                return Err(invalid_nested_block_sequence_sibling(
                    line.content_start + indent,
                ));
            }
        }
        if self.mappings.iter().any(|(level, _)| *level == indent)
            && comment_text_contains_mapping_colon(body)
        {
            return Err(invalid_orphaned_block_content(
                line.content_start + indent + separated_comment_offset(body).unwrap_or(0),
            ));
        }

        let has_collection_at_indent = self
            .mappings
            .iter()
            .chain(self.sequences.iter())
            .any(|(level, _)| *level == indent);
        let is_indented_root_collection = indent > 0
            && !self.has_parent_collection_below(indent)
            && (is_sequence_entry(body) || body_starts_flow_value_start(body));
        if indent > 0
            && !has_collection_at_indent
            && !is_indented_root_collection
            && (is_sequence_entry(body)
                || is_explicit_mapping_key(body)
                || is_explicit_mapping_value(body)
                || find_mapping_colon(body).is_some())
        {
            return Err(invalid_orphaned_block_content(line.content_start + indent));
        }

        Ok(())
    }

    fn close_collections_deeper_than(&mut self, indent: usize) {
        self.mappings.retain(|(level, _)| *level <= indent);
        self.sequences.retain(|(level, _)| *level <= indent);
        self.close_event_collections_deeper_than(indent);
    }

    fn close_sequence_at_indent(&mut self, indent: usize) {
        let Some(index) = self
            .sequences
            .iter()
            .rposition(|(level, _)| *level == indent)
        else {
            return;
        };
        if self
            .event_collections
            .last()
            .is_some_and(|(level, collection)| {
                *level == indent && *collection == OpenEventCollection::Sequence
            })
        {
            self.sequences.remove(index);
            self.close_last_event_collection();
        }
    }

    fn push_node(&mut self, kind: NodeKind, span: Span) -> NodeId {
        let id = NodeId::from_usize(self.nodes.len());
        self.nodes.push(Node {
            kind,
            span,
            children: Vec::with_capacity(2),
        });
        id
    }

    fn push_empty_scalar(&mut self, offset: usize) -> NodeId {
        self.push_node(NodeKind::Scalar, Span::empty_from_usize(offset))
    }

    fn extend_node_span(&mut self, node: NodeId, end: usize) {
        let node = &mut self.nodes[node.as_usize()];
        node.span.end = node.span.end.max(Span::usize_to_u32(end));
    }

    fn push_event(&mut self, kind: YamlEventKind, span: Span) {
        self.events.push(YamlEvent {
            kind,
            span,
            cst: None,
        });
    }

    fn push_node_event(&mut self, kind: YamlEventKind, span: Span, cst: NodeId) {
        self.events.push(YamlEvent {
            kind,
            span,
            cst: Some(cst),
        });
    }

    fn open_event_collection(
        &mut self,
        indent: usize,
        collection: OpenEventCollection,
        kind: YamlEventKind,
        span: Span,
        cst: NodeId,
    ) {
        self.event_collections.push((indent, collection));
        self.push_node_event(kind, span, cst);
    }

    fn close_event_collections_deeper_than(&mut self, indent: usize) {
        while self
            .event_collections
            .last()
            .is_some_and(|(level, _)| *level > indent)
        {
            self.close_last_event_collection();
        }
    }

    fn close_all_event_collections(&mut self) {
        while !self.event_collections.is_empty() {
            self.close_last_event_collection();
        }
    }

    fn close_last_event_collection(&mut self) {
        let Some((_, collection)) = self.event_collections.pop() else {
            return;
        };
        let offset = Span::usize_to_u32(self.source.len());
        let kind = match collection {
            OpenEventCollection::Mapping => YamlEventKind::MappingEnd,
            OpenEventCollection::Sequence => YamlEventKind::SequenceEnd,
        };
        self.push_event(kind, Span::empty(offset));
    }

    fn emit_node_event(&mut self, node: NodeId) -> Result<(), YamlError> {
        match self.nodes[node.0 as usize].kind {
            NodeKind::FlowSequence => self.emit_flow_sequence_events(node),
            NodeKind::FlowMapping => self.emit_flow_mapping_events(node),
            NodeKind::Scalar | NodeKind::LiteralScalar | NodeKind::FoldedScalar => {
                self.emit_scalar_event(node)
            }
            _ => Ok(()),
        }
    }

    fn emit_flow_sequence_events(&mut self, node: NodeId) -> Result<(), YamlError> {
        let sequence = self.nodes[node.0 as usize].clone();
        let mut properties = self.flow_event_properties(sequence.span)?;
        self.resolve_node_properties(&mut properties, sequence.span)?;
        let span = self.apply_pending_event_properties(&mut properties, sequence.span);
        self.push_node_event(
            YamlEventKind::SequenceStart {
                style: CollectionStyle::Flow,
                tag: properties.tag,
                anchor: properties.anchor,
            },
            span,
            node,
        );
        for child in sequence.children {
            self.emit_node_event(child)?;
        }
        self.push_event(YamlEventKind::SequenceEnd, span);
        Ok(())
    }

    fn emit_flow_mapping_events(&mut self, node: NodeId) -> Result<(), YamlError> {
        let mapping = self.nodes[node.0 as usize].clone();
        let mut properties = self.flow_event_properties(mapping.span)?;
        self.resolve_node_properties(&mut properties, mapping.span)?;
        let span = self.apply_pending_event_properties(&mut properties, mapping.span);
        self.push_node_event(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Flow,
                tag: properties.tag,
                anchor: properties.anchor,
            },
            span,
            node,
        );
        for entry in mapping.children {
            let entry = self.nodes[entry.0 as usize].clone();
            for child in entry.children {
                self.emit_node_event(child)?;
            }
        }
        self.push_event(YamlEventKind::MappingEnd, span);
        Ok(())
    }

    fn apply_pending_event_properties(
        &mut self,
        properties: &mut NodeProperties,
        span: Span,
    ) -> Span {
        if let Some(pending) =
            self.take_pending_node_properties(self.source_indent_at(span.start as usize))
        {
            if properties.anchor.is_none() {
                properties.anchor = pending.properties.anchor;
            }
            if properties.tag.is_none() {
                properties.tag = pending.properties.tag;
            }
            Span::new(Span::usize_to_u32(pending.span_start), span.end)
        } else {
            span
        }
    }

    fn flow_event_properties(&self, span: Span) -> Result<NodeProperties, YamlError> {
        let text = self.source.slice(span);
        if body_starts_flow_value(text, span.start as usize)? {
            parse_node_properties(text, span)
        } else {
            Ok(NodeProperties::default())
        }
    }

    fn emit_scalar_event(&mut self, node: NodeId) -> Result<(), YamlError> {
        let node_id = node;
        let node_kind = self.nodes[node.0 as usize].kind;
        let node_span = self.nodes[node.0 as usize].span;
        let text = self.source.slice(node_span);
        let mut properties = parse_node_properties(text, node_span)?;
        self.resolve_node_properties(&mut properties, node_span)?;
        let span = if let Some(pending) =
            self.take_pending_node_properties(self.source_indent_at(node_span.start as usize))
        {
            if properties.anchor.is_none() {
                properties.anchor = pending.properties.anchor;
            }
            if properties.tag.is_none() {
                properties.tag = pending.properties.tag;
            }
            Span::new(Span::usize_to_u32(pending.span_start), node_span.end)
        } else {
            node_span
        };
        let value_text = &text[properties.value_start..];
        let style = match node_kind {
            NodeKind::LiteralScalar => YamlScalarStyle::Literal,
            NodeKind::FoldedScalar => YamlScalarStyle::Folded,
            NodeKind::Scalar if value_text.starts_with('"') => YamlScalarStyle::DoubleQuoted,
            NodeKind::Scalar if value_text.starts_with('\'') => YamlScalarStyle::SingleQuoted,
            NodeKind::Scalar => YamlScalarStyle::Plain,
            _ => unreachable!("emit_scalar_event only receives scalar nodes"),
        };
        if style == YamlScalarStyle::Plain {
            let trimmed = strip_inline_comment(value_text).trim();
            if let Some(alias) = trimmed.strip_prefix('*')
                && !alias.is_empty()
                && !alias.chars().any(char::is_whitespace)
            {
                self.push_node_event(
                    YamlEventKind::Alias {
                        name: alias.to_owned(),
                    },
                    span,
                    node,
                );
                return Ok(());
            }
        }
        let value = if matches!(node_kind, NodeKind::LiteralScalar | NodeKind::FoldedScalar) {
            decode_scalar_value_with_content_indent(
                value_text,
                self.block_scalar_content_indents.get(&node_id).copied(),
            )?
        } else {
            decode_scalar_value(value_text)?
        };
        self.push_node_event(
            YamlEventKind::Scalar {
                style,
                value,
                tag: properties.tag,
                anchor: properties.anchor,
            },
            span,
            node,
        );
        Ok(())
    }

    fn resolve_node_properties(
        &self,
        properties: &mut NodeProperties,
        span: Span,
    ) -> Result<(), YamlError> {
        if let Some(tag) = properties.tag.as_deref() {
            properties.tag = Some(resolve_tag(tag, &self.tag_handles, span)?);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct NodeProperties {
    pub(crate) tag: Option<String>,
    pub(crate) anchor: Option<String>,
    pub(crate) value_start: usize,
}

fn reject_invalid_node_property_placement(
    text: &str,
    absolute_start: usize,
    properties: &NodeProperties,
) -> Result<(), YamlError> {
    if properties.anchor.is_none() && properties.tag.is_none() {
        return Ok(());
    }

    let value_start =
        properties.value_start + leading_flow_whitespace(&text[properties.value_start..]);
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
    if properties.anchor.is_none() && properties.tag.is_none() {
        return Ok(());
    }
    let value_start =
        properties.value_start + leading_flow_whitespace(&text[properties.value_start..]);
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
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
    reject_invalid_node_property_placement(body, absolute_start, &properties)?;
    let value_start =
        properties.value_start + leading_flow_whitespace(&body[properties.value_start..]);
    Ok(matches!(
        body[value_start..].chars().next(),
        Some('[' | '{')
    ))
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
        properties.value_start + leading_flow_whitespace(&body[properties.value_start..]);
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
    if properties.anchor.is_none() && properties.tag.is_none() {
        return Ok(None);
    }
    let header_offset =
        properties.value_start + leading_flow_whitespace(&body[properties.value_start..]);
    if matches!(body[header_offset..].chars().next(), Some('|' | '>')) {
        Ok(Some(header_offset))
    } else {
        Ok(None)
    }
}

fn property_only_block_collection_indent(
    body: &str,
    lines: &[SourceLine<'_>],
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
    lines: &[SourceLine<'_>],
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
    lines: &[SourceLine<'_>],
    index: usize,
    absolute_start: usize,
) -> Result<(), YamlError> {
    let body = strip_inline_comment(body).trim_end();
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
    if properties.anchor.is_none()
        || properties.tag.is_some()
        || properties.value_start < body.len()
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
    if (nested_properties.anchor.is_some() || nested_properties.tag.is_some())
        && nested_properties.value_start < nested_body.len()
        && find_mapping_colon(nested_body).is_none()
    {
        return Err(invalid_node_property_placement(
            absolute_start + nested_properties.value_start,
            nested_body[nested_properties.value_start..]
                .chars()
                .next()
                .unwrap_or(':'),
        ));
    }

    Ok(())
}

fn first_non_property_node_after<'line>(
    lines: &'line [SourceLine<'_>],
    index: usize,
    absolute_start: usize,
) -> Result<Option<(usize, &'line str)>, YamlError> {
    Ok(
        first_non_property_node_after_with_index(lines, index, absolute_start)?
            .map(|(_, indent, body)| (indent, body)),
    )
}

fn first_non_property_node_after_with_index<'line>(
    lines: &'line [SourceLine<'_>],
    index: usize,
    absolute_start: usize,
) -> Result<Option<(usize, usize, &'line str)>, YamlError> {
    let mut scan_index = index;
    while let Some((next_index, indent, nested_body)) =
        next_significant_body_with_index(lines, scan_index)
    {
        let nested_body = strip_inline_comment(nested_body).trim_end();
        let nested_properties = parse_node_properties(
            nested_body,
            Span::from_usize(absolute_start, absolute_start + nested_body.len()),
        )?;
        if (nested_properties.anchor.is_some() || nested_properties.tag.is_some())
            && nested_properties.value_start == nested_body.len()
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
    lines: &[SourceLine<'_>],
    index: usize,
    absolute_start: usize,
) -> Result<Option<usize>, YamlError> {
    let body = strip_inline_comment(body).trim_end();
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
    if properties.anchor.is_none() && properties.tag.is_none() {
        return Ok(None);
    }
    if properties.value_start < body.len() {
        return Ok(None);
    }

    Ok(first_non_property_node_after(lines, index, absolute_start)?.map(|(indent, _)| indent))
}

fn next_significant_body_with_index<'line>(
    lines: &'line [SourceLine<'_>],
    current_index: usize,
) -> Option<(usize, usize, &'line str)> {
    for (index, line) in lines.iter().enumerate().skip(current_index + 1) {
        let trimmed = line.content_without_break.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = content_line_indent(line.content_without_break);
        return Some((index, indent, &line.content_without_break[indent..]));
    }

    None
}

pub(crate) fn default_tag_handles() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("!".to_owned(), "!".to_owned()),
        ("!!".to_owned(), "tag:yaml.org,2002:".to_owned()),
    ])
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
    let Some(prefix) = handles.get(handle) else {
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
            properties.value_start = position;
            return Ok(properties);
        };

        match character {
            '&' => {
                if properties.anchor.is_some() {
                    return Err(node_property_error(
                        "duplicate anchor property",
                        span,
                        position,
                    ));
                }
                let (anchor, next) = parse_anchor_property(text, position, span)?;
                properties.anchor = Some(anchor);
                position = next;
            }
            '!' => {
                if properties.tag.is_some() {
                    return Err(node_property_error(
                        "duplicate tag property",
                        span,
                        position,
                    ));
                }
                let (tag, next) = parse_tag_property(text, position, span)?;
                properties.tag = Some(tag);
                position = next;
            }
            _ => {
                properties.value_start = position;
                return Ok(properties);
            }
        }

        if next_property_character_is_not_whitespace(text, position) {
            properties.value_start = position;
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
        Some(b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C) => false,
        Some(byte) if byte.is_ascii() => true,
        Some(_) => text[position..]
            .chars()
            .next()
            .is_some_and(|next| !next.is_whitespace()),
        None => false,
    }
}

fn parse_anchor_property(
    text: &str,
    position: usize,
    span: Span,
) -> Result<(String, usize), YamlError> {
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
    Ok((text[start..end].to_owned(), end))
}

fn parse_tag_property(
    text: &str,
    position: usize,
    span: Span,
) -> Result<(String, usize), YamlError> {
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
        return Ok((format!("!<{}>", &text[start..end]), end + 1));
    }

    let end = property_token_end(text, position);
    if end == position + 1 {
        return Ok(("!".to_owned(), end));
    }
    Ok((text[position..end].to_owned(), end))
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
}

struct SourceLines<'source> {
    source: &'source Source,
    index: usize,
}

impl<'source> SourceLines<'source> {
    pub(crate) fn new(source: &'source Source) -> Self {
        Self { source, index: 0 }
    }
}

impl<'source> Iterator for SourceLines<'source> {
    type Item = Result<SourceLine<'source>, YamlError>;

    fn next(&mut self) -> Option<Self::Item> {
        let starts = self.source.line_starts();
        if self.index >= starts.len() {
            return None;
        }

        let start = starts[self.index];
        let next_start = starts
            .get(self.index + 1)
            .copied()
            .unwrap_or(self.source.len());
        self.index += 1;

        if start == self.source.len() && start == next_start {
            return None;
        }

        let mut content_end = next_start;
        let text = self.source.as_str();
        if content_end > start && text.as_bytes()[content_end - 1] == b'\n' {
            content_end -= 1;
            if content_end > start && text.as_bytes()[content_end - 1] == b'\r' {
                content_end -= 1;
            }
        } else if content_end > start && text.as_bytes()[content_end - 1] == b'\r' {
            content_end -= 1;
        }

        Some(
            self.source
                .try_slice(Span::from_usize(start, content_end))
                .map(|content_without_break| SourceLine {
                    content_start: start,
                    content_end,
                    line_end: next_start,
                    content_without_break,
                }),
        )
    }
}

fn next_significant_indent(
    lines: &[SourceLine<'_>],
    current_index: usize,
) -> Result<Option<usize>, YamlError> {
    for line in &lines[current_index + 1..] {
        let trimmed = line.content_without_break.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        return count_indent(line.content_without_break, line.content_start).map(Some);
    }

    Ok(None)
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
    if properties.anchor.is_none() && properties.tag.is_none() {
        return Ok(());
    }
    let value = rest[properties.value_start..].trim_start();
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
    let trailing = &text[parsed_end..];
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

fn flow_mapping_separator(
    text: &str,
    start: usize,
    absolute_start: usize,
    terminators: &[char],
) -> Result<Option<usize>, YamlError> {
    let mut position = start;
    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        if terminators.contains(&character) {
            return Ok(None);
        }
        match character {
            ':' if is_flow_mapping_separator_colon(text, position) => return Ok(Some(position)),
            '#' if is_flow_comment_start(text, position) => {
                position = flow_comment_end(text, position);
            }
            '"' => position = double_quoted_flow_end(text, position, absolute_start)?,
            '\'' => position = single_quoted_flow_end(text, position, absolute_start)?,
            '[' | '{' => {
                position +=
                    flow_collection_source_end(&text[position..], absolute_start + position)?;
            }
            _ => position += character.len_utf8(),
        }
    }

    Ok(None)
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

fn reject_flow_plain_scalar_mapping_separator(
    text: &str,
    start: usize,
    end: usize,
    absolute_start: usize,
) -> Result<(), YamlError> {
    if let Some(colon) = flow_mapping_separator(&text[start..end], 0, absolute_start + start, &[])?
    {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                "mapping separator is not allowed inside a flow plain scalar",
                Span::from_usize(
                    absolute_start + start + colon,
                    absolute_start + start + colon + 1,
                ),
            )
            .with_expected("a comma before the next flow mapping entry"),
        ));
    }
    Ok(())
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
    let mut position = start + 1;
    let mut escaped = false;

    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        position += character.len_utf8();
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Ok(position);
        }
    }

    Err(YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "unterminated double-quoted scalar in flow sequence",
            Span::from_usize(absolute_start + start, absolute_start + text.len()),
        )
        .with_expected("closing \""),
    ))
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

fn empty_flow_sequence_item(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "empty flow sequence item",
            Span::empty_from_usize(offset),
        )
        .with_expected("a scalar or nested flow sequence"),
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

fn empty_flow_mapping_pair(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "empty flow mapping pair",
            Span::empty_from_usize(offset),
        )
        .with_expected("a mapping key"),
    )
}

fn empty_flow_mapping_key(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "empty flow mapping key",
            Span::empty_from_usize(offset),
        )
        .with_expected("a mapping key"),
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

pub(crate) fn find_mapping_colon(body: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut previous_was_whitespace = false;
    let start = parse_node_properties(body, Span::empty(0))
        .ok()
        .filter(|properties| properties.anchor.is_some() || properties.tag.is_some())
        .map_or(0, |properties| properties.value_start);

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

pub(crate) fn next_line_content_start(text: &str, mut position: usize) -> usize {
    if text[position..].starts_with("\r\n") {
        position += 2;
    } else if text[position..].starts_with(['\r', '\n']) {
        position += 1;
    }

    while text[position..].starts_with([' ', '\t']) {
        position += 1;
    }

    position
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CollectionTarget {
    pub(crate) span: Span,
    pub(crate) indent: usize,
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
                "single-quoted scalar replacement cannot contain line breaks in the MVP writer",
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

pub(crate) fn decode_scalar_value(text: &str) -> Result<String, YamlError> {
    decode_scalar_value_with_content_indent(text, None)
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
pub fn events_to_test_string(events: &[YamlEvent]) -> String {
    let mut output = String::new();
    for event in events {
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
