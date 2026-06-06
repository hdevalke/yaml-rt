//! Core types for a YAML 1.2.2 round-trip parser.
//!
//! This crate is intentionally dependency-free. The first implementation keeps
//! the source text intact while the source model, lexer, CST parser, semantic
//! graph, editor, and patch emitter are built out according to the workspace
//! roadmap.

use std::fmt;

/// YAML version targeted by this workspace.
pub const TARGET_YAML_VERSION: &str = "1.2.2";

/// Identifier for a node stored inside a [`YamlDoc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

/// A byte span inside a [`Source`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: u32,
    /// Exclusive end byte offset.
    pub end: u32,
}

impl Span {
    /// Creates a new byte span.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Returns an empty span at `offset`.
    #[must_use]
    pub const fn empty(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    /// Returns the span length in bytes.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Returns true when this span covers no bytes.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Returns true when `offset` is inside this span.
    #[must_use]
    pub const fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }
}

/// One-based line and column location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    /// One-based line number.
    pub line: usize,
    /// One-based column number in bytes for the current bootstrap model.
    pub column: usize,
}

/// Original YAML input plus line-start metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    text: String,
    line_starts: Vec<usize>,
}

impl Source {
    /// Builds a source buffer, validates YAML 1.2.2 printable characters, and
    /// records all line starts.
    pub fn new(text: String) -> Result<Self, YamlError> {
        validate_yaml_chars(&text)?;

        let mut line_starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset + 1);
            }
        }

        Ok(Self { text, line_starts })
    }

    /// Returns the original input text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Returns the source length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Returns true when the source is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Returns the recorded line-start byte offsets.
    #[must_use]
    pub fn line_starts(&self) -> &[usize] {
        &self.line_starts
    }

    /// Returns the source slice for `span`.
    ///
    /// # Panics
    ///
    /// Panics if `span` is outside the source or does not fall on UTF-8
    /// boundaries. Use [`Source::try_slice`] when handling user-provided spans.
    #[must_use]
    pub fn slice(&self, span: Span) -> &str {
        self.try_slice(span)
            .expect("span must be in bounds and on UTF-8 boundaries")
    }

    /// Returns the source slice for `span`, or a span-rich error when invalid.
    pub fn try_slice(&self, span: Span) -> Result<&str, YamlError> {
        let start = span.start as usize;
        let end = span.end as usize;

        if start > end || end > self.text.len() {
            return Err(YamlError::new(Diagnostic::new(
                DiagnosticKind::Source,
                "span is outside the source text",
                span,
            )));
        }

        self.text.get(start..end).ok_or_else(|| {
            YamlError::new(Diagnostic::new(
                DiagnosticKind::Source,
                "span does not align with UTF-8 character boundaries",
                span,
            ))
        })
    }

    /// Converts a byte offset into a one-based line/column pair.
    #[must_use]
    pub fn line_col(&self, offset: usize) -> LineCol {
        let offset = offset.min(self.text.len());
        let line_index = match self.line_starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let line_start = self.line_starts[line_index];

        LineCol {
            line: line_index + 1,
            column: offset - line_start + 1,
        }
    }

    /// Returns the line/column pair for a diagnostic's primary span.
    #[must_use]
    pub fn diagnostic_position(&self, diagnostic: &Diagnostic) -> LineCol {
        self.line_col(diagnostic.span.start as usize)
    }
}

fn validate_yaml_chars(text: &str) -> Result<(), YamlError> {
    for (offset, character) in text.char_indices() {
        if !is_yaml_printable(character) {
            let span = Span::new(offset as u32, (offset + character.len_utf8()) as u32);
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Source,
                    format!(
                        "invalid YAML 1.2.2 character U+{:04X}",
                        character as u32
                    ),
                    span,
                )
                .with_note(
                    "YAML streams may contain tab, line feeds, carriage returns, printable Unicode characters, and non-breaking spaces",
                ),
            ));
        }
    }

    Ok(())
}

const fn is_yaml_printable(character: char) -> bool {
    matches!(
        character as u32,
        0x09 | 0x0A | 0x0D | 0x20..=0x7E | 0x85 | 0xA0..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF
    )
}

/// Lexical token preserving its exact source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Token classification.
    pub kind: TokenKind,
    /// Original source span for this token.
    pub span: Span,
}

/// Token kinds emitted by the lossless lexer MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// UTF-8 byte-order mark at the start of a stream.
    Bom,
    /// Spaces, tabs, or other separation characters that are not line breaks.
    Whitespace,
    /// A YAML line break, preserving the original spelling through the span.
    Newline,
    /// A comment from `#` through the byte before the line break.
    Comment,
    /// `---` document start marker.
    DocumentStart,
    /// `...` document end marker.
    DocumentEnd,
    /// `:` mapping value indicator.
    Colon,
    /// `-` sequence entry indicator or dash token.
    Dash,
    /// `[` flow sequence start.
    FlowSequenceStart,
    /// `]` flow sequence end.
    FlowSequenceEnd,
    /// `{` flow mapping start.
    FlowMappingStart,
    /// `}` flow mapping end.
    FlowMappingEnd,
    /// `,` flow separator.
    Comma,
    /// `?` explicit mapping key indicator.
    Question,
    /// A double-quoted scalar, including its quotes.
    DoubleQuotedScalar,
    /// A single-quoted scalar, including its quotes.
    SingleQuotedScalar,
    /// An unquoted scalar chunk.
    PlainScalar,
}

/// Lexes YAML source into lossless tokens for the MVP subset.
pub fn lex(source: &Source) -> Result<Vec<Token>, YamlError> {
    Lexer::new(source).lex()
}

/// Reconstructs source text from token spans.
#[must_use]
pub fn tokens_to_string(tokens: &[Token], source: &Source) -> String {
    let mut output = String::new();
    for token in tokens {
        output.push_str(source.slice(token.span));
    }
    output
}

struct Lexer<'source> {
    source: &'source Source,
    text: &'source str,
    position: usize,
    tokens: Vec<Token>,
}

impl<'source> Lexer<'source> {
    fn new(source: &'source Source) -> Self {
        Self {
            source,
            text: source.as_str(),
            position: 0,
            tokens: Vec::new(),
        }
    }

    fn lex(mut self) -> Result<Vec<Token>, YamlError> {
        while self.position < self.text.len() {
            let start = self.position;

            if self.consume_bom() {
                self.push(TokenKind::Bom, start);
            } else if self.consume_line_break() {
                self.push(TokenKind::Newline, start);
            } else if self.consume_horizontal_whitespace() {
                self.push(TokenKind::Whitespace, start);
            } else if self.consume_comment() {
                self.push(TokenKind::Comment, start);
            } else if self.consume_document_marker("---") {
                self.push(TokenKind::DocumentStart, start);
            } else if self.consume_document_marker("...") {
                self.push(TokenKind::DocumentEnd, start);
            } else if self.consume_double_quoted_scalar()? {
                self.push(TokenKind::DoubleQuotedScalar, start);
            } else if self.consume_single_quoted_scalar()? {
                self.push(TokenKind::SingleQuotedScalar, start);
            } else if self.consume_single_byte_indicator() {
                let kind = match self.text.as_bytes()[start] {
                    b':' => TokenKind::Colon,
                    b'-' => TokenKind::Dash,
                    b'[' => TokenKind::FlowSequenceStart,
                    b']' => TokenKind::FlowSequenceEnd,
                    b'{' => TokenKind::FlowMappingStart,
                    b'}' => TokenKind::FlowMappingEnd,
                    b',' => TokenKind::Comma,
                    b'?' => TokenKind::Question,
                    _ => unreachable!("consume_single_byte_indicator only consumes indicators"),
                };
                self.push(kind, start);
            } else {
                self.consume_plain_scalar();
                self.push(TokenKind::PlainScalar, start);
            }
        }

        Ok(self.tokens)
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start as u32, self.position as u32),
        });
    }

    fn consume_bom(&mut self) -> bool {
        if self.position == 0 && self.text[self.position..].starts_with('\u{FEFF}') {
            self.position += '\u{FEFF}'.len_utf8();
            true
        } else {
            false
        }
    }

    fn consume_line_break(&mut self) -> bool {
        let remaining = &self.text[self.position..];
        if remaining.starts_with("\r\n") {
            self.position += 2;
            true
        } else if remaining.starts_with('\n') || remaining.starts_with('\r') {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn consume_horizontal_whitespace(&mut self) -> bool {
        let start = self.position;
        while let Some(character) = self.current_char() {
            if character == ' ' || character == '\t' {
                self.position += character.len_utf8();
            } else {
                break;
            }
        }

        self.position != start
    }

    fn consume_comment(&mut self) -> bool {
        if !self.text[self.position..].starts_with('#') {
            return false;
        }

        self.position += 1;
        while let Some(character) = self.current_char() {
            if character == '\n' || character == '\r' {
                break;
            }
            self.position += character.len_utf8();
        }

        true
    }

    fn consume_document_marker(&mut self, marker: &str) -> bool {
        if !self.is_line_start() || !self.text[self.position..].starts_with(marker) {
            return false;
        }

        let end = self.position + marker.len();
        let followed_by_boundary = self.text[end..]
            .chars()
            .next()
            .is_none_or(|character| matches!(character, ' ' | '\t' | '\r' | '\n'));

        if followed_by_boundary {
            self.position = end;
            true
        } else {
            false
        }
    }

    fn consume_double_quoted_scalar(&mut self) -> Result<bool, YamlError> {
        if !self.text[self.position..].starts_with('"') {
            return Ok(false);
        }

        let start = self.position;
        self.position += 1;
        let mut escaped = false;

        while let Some(character) = self.current_char() {
            self.position += character.len_utf8();
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Ok(true);
            }
        }

        Err(self.unterminated_scalar_error(start, "double-quoted scalar", '"'))
    }

    fn consume_single_quoted_scalar(&mut self) -> Result<bool, YamlError> {
        if !self.text[self.position..].starts_with('\'') {
            return Ok(false);
        }

        let start = self.position;
        self.position += 1;

        while let Some(character) = self.current_char() {
            self.position += character.len_utf8();
            if character == '\'' {
                if self.text[self.position..].starts_with('\'') {
                    self.position += 1;
                } else {
                    return Ok(true);
                }
            }
        }

        Err(self.unterminated_scalar_error(start, "single-quoted scalar", '\''))
    }

    fn consume_single_byte_indicator(&mut self) -> bool {
        if matches!(
            self.text.as_bytes()[self.position],
            b':' | b'-' | b'[' | b']' | b'{' | b'}' | b',' | b'?'
        ) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn consume_plain_scalar(&mut self) {
        while let Some(character) = self.current_char() {
            if matches!(
                character,
                ' ' | '\t'
                    | '\r'
                    | '\n'
                    | '#'
                    | ':'
                    | '-'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | ','
                    | '?'
                    | '\''
                    | '"'
            ) {
                break;
            }
            self.position += character.len_utf8();
        }

        if self.position == 0
            || self.position
                == self
                    .tokens
                    .last()
                    .map_or(0, |token| token.span.end as usize)
        {
            self.position += self.current_char().map_or(0, char::len_utf8);
        }
    }

    fn current_char(&self) -> Option<char> {
        self.text[self.position..].chars().next()
    }

    fn is_line_start(&self) -> bool {
        self.position == 0
            || matches!(
                self.text.as_bytes().get(self.position.wrapping_sub(1)),
                Some(b'\n' | b'\r')
            )
    }

    fn unterminated_scalar_error(
        &self,
        start: usize,
        scalar_name: &'static str,
        terminator: char,
    ) -> YamlError {
        YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Lexer,
                format!("unterminated {scalar_name}"),
                Span::new(start as u32, self.source.len() as u32),
            )
            .with_expected(format!("closing {terminator}")),
        )
    }
}

/// Lossless syntax node produced by the CST parser MVP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Node classification.
    pub kind: NodeKind,
    /// Original source span for this node.
    pub span: Span,
    /// Child node identifiers in source order.
    pub children: Vec<NodeId>,
}

/// Node kinds emitted by the CST parser MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Complete YAML stream.
    Stream,
    /// Content document inside a stream.
    Document,
    /// Explicit document start or end marker.
    DocumentMarker,
    /// Block mapping collection.
    BlockMapping,
    /// One mapping entry line.
    MappingEntry,
    /// Block sequence collection.
    BlockSequence,
    /// One sequence entry line.
    SequenceEntry,
    /// Single-line flow sequence collection.
    FlowSequence,
    /// Single-line flow mapping collection.
    FlowMapping,
    /// Literal block scalar collection.
    LiteralScalar,
    /// Folded block scalar collection.
    FoldedScalar,
    /// Scalar syntax span.
    Scalar,
}

/// Semantic YAML event produced by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlEvent {
    /// Event classification.
    pub kind: YamlEventKind,
    /// Source span associated with this event.
    pub span: Span,
}

/// YAML collection spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionStyle {
    /// Block collection syntax.
    Block,
    /// Flow collection syntax.
    Flow,
}

/// YAML scalar spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamlScalarStyle {
    /// Plain scalar syntax.
    Plain,
    /// Single-quoted scalar syntax.
    SingleQuoted,
    /// Double-quoted scalar syntax.
    DoubleQuoted,
    /// Literal block scalar syntax.
    Literal,
    /// Folded block scalar syntax.
    Folded,
}

/// Semantic YAML event kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YamlEventKind {
    /// Start of a YAML stream.
    StreamStart,
    /// End of a YAML stream.
    StreamEnd,
    /// Start of a YAML document.
    DocumentStart {
        /// Whether the source used an explicit `---` marker.
        explicit: bool,
    },
    /// End of a YAML document.
    DocumentEnd,
    /// Start of a sequence node.
    SequenceStart {
        /// Sequence spelling style.
        style: CollectionStyle,
    },
    /// End of a sequence node.
    SequenceEnd,
    /// Start of a mapping node.
    MappingStart {
        /// Mapping spelling style.
        style: CollectionStyle,
    },
    /// End of a mapping node.
    MappingEnd,
    /// Scalar node with decoded content.
    Scalar {
        /// Scalar spelling style.
        style: YamlScalarStyle,
        /// Decoded scalar value.
        value: String,
    },
    /// Alias node.
    Alias {
        /// Alias name without the leading `*`.
        name: String,
    },
}

/// Parses the MVP token/source pair into a lossless CST node arena.
pub fn parse_cst(source: &Source, tokens: &[Token]) -> Result<Vec<Node>, YamlError> {
    Parser::new(source, tokens)
        .parse()
        .map(|parsed| parsed.nodes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedYaml {
    nodes: Vec<Node>,
    events: Vec<YamlEvent>,
}

/// Identifier for a semantic graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GraphNodeId(pub u32);

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
        /// Key/value node pairs in source order.
        entries: Vec<(GraphNodeId, GraphNodeId)>,
    },
    /// YAML sequence node.
    Sequence {
        /// Sequence spelling style.
        style: CollectionStyle,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenEventCollection {
    Mapping,
    Sequence,
}

struct Parser<'source> {
    source: &'source Source,
    tokens: &'source [Token],
    nodes: Vec<Node>,
    events: Vec<YamlEvent>,
    document: Option<NodeId>,
    mappings: Vec<(usize, NodeId)>,
    sequences: Vec<(usize, NodeId)>,
    event_collections: Vec<(usize, OpenEventCollection)>,
}

impl<'source> Parser<'source> {
    fn new(source: &'source Source, tokens: &'source [Token]) -> Self {
        Self {
            source,
            tokens,
            nodes: Vec::new(),
            events: Vec::new(),
            document: None,
            mappings: Vec::new(),
            sequences: Vec::new(),
            event_collections: Vec::new(),
        }
    }

    fn parse(mut self) -> Result<ParsedYaml, YamlError> {
        let stream = self.push_node(NodeKind::Stream, Span::new(0, self.source.len() as u32));
        let document = self.ensure_document(stream, Span::new(0, self.source.len() as u32));
        let document_explicit = self
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::DocumentStart);
        self.push_event(
            YamlEventKind::StreamStart,
            Span::new(0, self.source.len() as u32),
        );
        self.push_event(
            YamlEventKind::DocumentStart {
                explicit: document_explicit,
            },
            Span::new(0, self.source.len() as u32),
        );

        for token in self.tokens {
            if token.kind == TokenKind::DocumentStart || token.kind == TokenKind::DocumentEnd {
                let marker = self.push_node(token.kind.into_node_kind(), token.span);
                self.nodes[document.0 as usize].children.push(marker);
            }
        }

        let lines = SourceLines::new(self.source).collect::<Result<Vec<_>, _>>()?;
        let mut index = 0;
        while index < lines.len() {
            index += self.parse_line(document, &lines, index)?;
        }
        self.close_event_collections_deeper_than(0);
        self.close_all_event_collections();
        self.push_event(
            YamlEventKind::DocumentEnd,
            Span::new(self.source.len() as u32, self.source.len() as u32),
        );
        self.push_event(
            YamlEventKind::StreamEnd,
            Span::new(self.source.len() as u32, self.source.len() as u32),
        );

        Ok(ParsedYaml {
            nodes: self.nodes,
            events: self.events,
        })
    }

    fn parse_line(
        &mut self,
        document: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
    ) -> Result<usize, YamlError> {
        let line = lines[index];
        let content = line.content_without_break;
        let trimmed = content.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return Ok(1);
        }

        if trimmed == "---" || trimmed == "..." {
            return Ok(1);
        }
        if trimmed.starts_with("---") || trimmed.starts_with("...") {
            return Err(invalid_document_marker(line));
        }

        let indent = count_indent(content, line.content_start)?;
        let body = &content[indent..];
        self.validate_indent(indent, line)?;
        self.close_collections_deeper_than(indent);
        reject_unexpected_line_start(body, line.content_start + indent)?;

        if is_sequence_entry(body) {
            self.parse_sequence_entry(document, lines, index, indent, body)
        } else if body.starts_with('|') || body.starts_with('>') {
            let (node, consumed) =
                self.parse_block_scalar(lines, index, line.content_start + indent, indent, body)?;
            self.nodes[document.0 as usize].children.push(node);
            self.emit_scalar_event(node)?;
            Ok(consumed)
        } else if body.starts_with('[') || body.starts_with('{') {
            let (node, end) = self.parse_flow_value(body, line.content_start + indent)?;
            reject_trailing_flow_content(body, end, line.content_start + indent)?;
            self.nodes[document.0 as usize].children.push(node);
            self.emit_node_event(node)?;
            Ok(1)
        } else if let Some(colon_byte) = find_mapping_colon(body) {
            self.parse_mapping_entry(document, lines, index, indent, body, colon_byte)
        } else {
            let scalar_span = Span::new(
                (line.content_start + indent) as u32,
                line.content_end as u32,
            );
            let scalar = self.push_node(NodeKind::Scalar, scalar_span);
            self.nodes[document.0 as usize].children.push(scalar);
            self.emit_scalar_event(scalar)?;
            Ok(1)
        }
    }

    fn parse_mapping_entry(
        &mut self,
        document: NodeId,
        lines: &[SourceLine<'_>],
        index: usize,
        indent: usize,
        body: &str,
        colon_byte: usize,
    ) -> Result<usize, YamlError> {
        let line = lines[index];
        let mapping = self.ensure_mapping(
            document,
            indent,
            Span::new(
                (line.content_start + indent) as u32,
                line.content_end as u32,
            ),
        );
        let entry = self.push_node(
            NodeKind::MappingEntry,
            Span::new(line.content_start as u32, line.content_end as u32),
        );
        self.nodes[mapping.0 as usize].children.push(entry);
        self.extend_node_span(mapping, line.content_end);

        let key_start = line.content_start + indent;
        let key_end = key_start + body[..colon_byte].trim_end().len();
        if key_start < key_end {
            let key = self.push_node(
                NodeKind::Scalar,
                Span::new(key_start as u32, key_end as u32),
            );
            self.nodes[entry.0 as usize].children.push(key);
            self.emit_scalar_event(key)?;
        }

        let value = &body[colon_byte + 1..];
        let value_trimmed = value.trim_start();
        let next_significant_indent = next_significant_indent(lines, index)?;
        if value_trimmed.is_empty() && next_significant_indent.is_none_or(|next| next <= indent) {
            return Err(missing_mapping_value(line, indent, colon_byte));
        }

        if !value_trimmed.is_empty() {
            let leading = value.len() - value_trimmed.len();
            let value_start = line.content_start + indent + colon_byte + 1 + leading;
            let value_node = if value_trimmed.starts_with('|') || value_trimmed.starts_with('>') {
                let (node, consumed) =
                    self.parse_block_scalar(lines, index, value_start, indent, value_trimmed)?;
                self.nodes[entry.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else {
                self.parse_inline_value(value_trimmed, value_start)?
            };
            self.nodes[entry.0 as usize].children.push(value_node);
            self.emit_node_event(value_node)?;
        }

        Ok(1)
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
            Span::new(
                (line.content_start + indent) as u32,
                line.content_end as u32,
            ),
        );
        let entry = self.push_node(
            NodeKind::SequenceEntry,
            Span::new(line.content_start as u32, line.content_end as u32),
        );
        self.nodes[sequence.0 as usize].children.push(entry);
        self.extend_node_span(sequence, line.content_end);

        let after_dash = if body == "-" { "" } else { &body[1..] };
        let value = after_dash.trim_start();
        if !value.is_empty() {
            let leading = after_dash.len() - value.len();
            let value_start = line.content_start + indent + 1 + leading;
            let value_node = if value.starts_with('|') || value.starts_with('>') {
                let (node, consumed) =
                    self.parse_block_scalar(lines, index, value_start, indent, value)?;
                self.nodes[entry.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                return Ok(consumed);
            } else {
                self.parse_inline_value(value, value_start)?
            };
            self.nodes[entry.0 as usize].children.push(value_node);
            self.emit_node_event(value_node)?;
        }

        Ok(1)
    }

    fn parse_inline_value(
        &mut self,
        text: &str,
        absolute_start: usize,
    ) -> Result<NodeId, YamlError> {
        if text.starts_with('[') || text.starts_with('{') {
            let (node, end) = self.parse_flow_value(text, absolute_start)?;
            reject_trailing_flow_content(text, end, absolute_start)?;
            Ok(node)
        } else {
            Ok(self.push_node(
                NodeKind::Scalar,
                Span::new(absolute_start as u32, (absolute_start + text.len()) as u32),
            ))
        }
    }

    fn parse_flow_value(
        &mut self,
        text: &str,
        absolute_start: usize,
    ) -> Result<(NodeId, usize), YamlError> {
        if text.starts_with('[') {
            self.parse_flow_sequence(text, absolute_start)
        } else if text.starts_with('{') {
            self.parse_flow_mapping(text, absolute_start)
        } else {
            let end = flow_scalar_end(text, 0, absolute_start, &[',', ']', '}'])?;
            let scalar_start = leading_flow_whitespace(&text[..end]);
            let scalar_end = end - trailing_flow_whitespace(&text[..end]);
            if scalar_start >= scalar_end {
                return Err(empty_flow_value(absolute_start));
            }
            Ok((
                self.push_node(
                    NodeKind::Scalar,
                    Span::new(
                        (absolute_start + scalar_start) as u32,
                        (absolute_start + scalar_end) as u32,
                    ),
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
    ) -> Result<(NodeId, usize), YamlError> {
        let header = parse_block_scalar_header(header, header_start)?;
        let mut consumed = 1;
        let mut content_indent = header
            .indent
            .map(|indent| parent_indent + indent)
            .unwrap_or(usize::MAX);
        let mut end = lines[index].line_end;

        for line in &lines[index + 1..] {
            let trimmed = line.content_without_break.trim();
            if trimmed == "---" || trimmed == "..." {
                break;
            }

            if trimmed.is_empty() {
                consumed += 1;
                end = line.line_end;
                continue;
            }

            let indent = count_literal_content_indent(line.content_without_break);
            if content_indent == usize::MAX {
                if indent <= parent_indent && parent_indent > 0 {
                    break;
                }
                content_indent = indent;
            }

            if indent < content_indent {
                break;
            }

            consumed += 1;
            end = line.line_end;
        }

        let scalar = self.push_node(
            header.kind.node_kind(),
            Span::new(header_start as u32, end as u32),
        );
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
            Span::new(absolute_start as u32, (absolute_start + 1) as u32),
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
                        self.nodes[sequence.0 as usize].span.end =
                            (absolute_start + position) as u32;
                        return Ok((sequence, position));
                    }
                    return Err(empty_flow_sequence_item(absolute_start + position));
                }
                ',' => {
                    return Err(unexpected_flow_comma(absolute_start + position));
                }
                '#' => {
                    return Err(flow_sequence_comment(absolute_start + position));
                }
                '[' => {
                    let child_start = absolute_start + position;
                    let (child, consumed) =
                        self.parse_flow_sequence(&text[position..], child_start)?;
                    self.nodes[sequence.0 as usize].children.push(child);
                    position += consumed;
                }
                '{' => {
                    let child_start = absolute_start + position;
                    let (child, consumed) =
                        self.parse_flow_mapping(&text[position..], child_start)?;
                    self.nodes[sequence.0 as usize].children.push(child);
                    position += consumed;
                }
                _ => {
                    let value_start = position;
                    let value_end = flow_scalar_end(text, position, absolute_start, &[',', ']'])?;
                    let scalar_start =
                        value_start + leading_flow_whitespace(&text[value_start..value_end]);
                    let scalar_end =
                        value_end - trailing_flow_whitespace(&text[value_start..value_end]);
                    if scalar_start >= scalar_end {
                        return Err(empty_flow_sequence_item(absolute_start + position));
                    }
                    let scalar = self.push_node(
                        NodeKind::Scalar,
                        Span::new(
                            (absolute_start + scalar_start) as u32,
                            (absolute_start + scalar_end) as u32,
                        ),
                    );
                    self.nodes[sequence.0 as usize].children.push(scalar);
                    position = value_end;
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
                '#' => return Err(flow_sequence_comment(absolute_start + position)),
                _ => {
                    return Err(expected_flow_separator(
                        absolute_start + position,
                        separator,
                    ));
                }
            }
        }
    }

    fn parse_flow_mapping(
        &mut self,
        text: &str,
        absolute_start: usize,
    ) -> Result<(NodeId, usize), YamlError> {
        debug_assert!(text.starts_with('{'));

        let mapping = self.push_node(
            NodeKind::FlowMapping,
            Span::new(absolute_start as u32, (absolute_start + 1) as u32),
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
                        self.nodes[mapping.0 as usize].span.end =
                            (absolute_start + position) as u32;
                        return Ok((mapping, position));
                    }
                    return Err(empty_flow_mapping_pair(absolute_start + position));
                }
                ',' => return Err(unexpected_flow_mapping_comma(absolute_start + position)),
                '#' => return Err(flow_mapping_comment(absolute_start + position)),
                _ => {}
            }

            let entry_start = position;
            let entry = self.push_node(
                NodeKind::MappingEntry,
                Span::new(
                    (absolute_start + entry_start) as u32,
                    (absolute_start + entry_start) as u32,
                ),
            );
            let key = if character == '[' || character == '{' {
                let (key, consumed) =
                    self.parse_flow_value(&text[position..], absolute_start + position)?;
                position += consumed;
                key
            } else {
                let key_end = flow_scalar_end(text, position, absolute_start, &[':', ',', '}'])?;
                let key_start = position + leading_flow_whitespace(&text[position..key_end]);
                let key_trimmed_end = key_end - trailing_flow_whitespace(&text[position..key_end]);
                if key_start >= key_trimmed_end {
                    return Err(empty_flow_mapping_key(absolute_start + position));
                }
                position = key_end;
                self.push_node(
                    NodeKind::Scalar,
                    Span::new(
                        (absolute_start + key_start) as u32,
                        (absolute_start + key_trimmed_end) as u32,
                    ),
                )
            };
            self.nodes[entry.0 as usize].children.push(key);

            position = skip_flow_whitespace(text, position);
            let Some(separator) = text[position..].chars().next() else {
                return Err(missing_flow_mapping_end(absolute_start, text.len()));
            };
            if separator != ':' {
                return Err(missing_flow_mapping_colon(
                    absolute_start + position,
                    separator,
                ));
            }
            position += 1;
            position = skip_flow_whitespace(text, position);

            match text[position..].chars().next() {
                None => return Err(missing_flow_mapping_end(absolute_start, text.len())),
                Some(',') | Some('}') => {}
                Some('#') => return Err(flow_mapping_comment(absolute_start + position)),
                Some('[') | Some('{') => {
                    let (value, consumed) =
                        self.parse_flow_value(&text[position..], absolute_start + position)?;
                    self.nodes[entry.0 as usize].children.push(value);
                    position += consumed;
                }
                Some(_) => {
                    let value_end = flow_scalar_end(text, position, absolute_start, &[',', '}'])?;
                    let value_start =
                        position + leading_flow_whitespace(&text[position..value_end]);
                    let value_trimmed_end =
                        value_end - trailing_flow_whitespace(&text[position..value_end]);
                    if value_start < value_trimmed_end {
                        let value = self.push_node(
                            NodeKind::Scalar,
                            Span::new(
                                (absolute_start + value_start) as u32,
                                (absolute_start + value_trimmed_end) as u32,
                            ),
                        );
                        self.nodes[entry.0 as usize].children.push(value);
                    }
                    position = value_end;
                }
            }

            self.nodes[entry.0 as usize].span.end = (absolute_start + position) as u32;
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
                '#' => return Err(flow_mapping_comment(absolute_start + position)),
                _ => {
                    return Err(expected_flow_mapping_separator(
                        absolute_start + position,
                        separator,
                    ));
                }
            }
        }
    }

    fn ensure_document(&mut self, stream: NodeId, span: Span) -> NodeId {
        if let Some(document) = self.document {
            document
        } else {
            let document = self.push_node(NodeKind::Document, span);
            self.nodes[stream.0 as usize].children.push(document);
            self.document = Some(document);
            document
        }
    }

    fn ensure_mapping(&mut self, parent: NodeId, indent: usize, span: Span) -> NodeId {
        if let Some((_, node)) = self.mappings.iter().find(|(level, _)| *level == indent) {
            *node
        } else {
            let mapping = self.push_node(NodeKind::BlockMapping, span);
            self.nodes[parent.0 as usize].children.push(mapping);
            self.mappings.push((indent, mapping));
            self.open_event_collection(
                indent,
                OpenEventCollection::Mapping,
                YamlEventKind::MappingStart {
                    style: CollectionStyle::Block,
                },
                span,
            );
            mapping
        }
    }

    fn ensure_sequence(&mut self, parent: NodeId, indent: usize, span: Span) -> NodeId {
        if let Some((_, node)) = self.sequences.iter().find(|(level, _)| *level == indent) {
            *node
        } else {
            let sequence = self.push_node(NodeKind::BlockSequence, span);
            self.nodes[parent.0 as usize].children.push(sequence);
            self.sequences.push((indent, sequence));
            self.open_event_collection(
                indent,
                OpenEventCollection::Sequence,
                YamlEventKind::SequenceStart {
                    style: CollectionStyle::Block,
                },
                span,
            );
            sequence
        }
    }

    fn validate_indent(&self, indent: usize, line: SourceLine<'_>) -> Result<(), YamlError> {
        if indent == 0 {
            return Ok(());
        }

        let has_parent_collection = self
            .mappings
            .iter()
            .chain(self.sequences.iter())
            .any(|(level, _)| *level < indent);

        if has_parent_collection {
            Ok(())
        } else {
            Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Parser,
                    "invalid indentation without a parent collection",
                    Span::new(
                        line.content_start as u32,
                        (line.content_start + indent) as u32,
                    ),
                )
                .with_expected("a parent mapping or sequence at a lower indentation level"),
            ))
        }
    }

    fn close_collections_deeper_than(&mut self, indent: usize) {
        self.mappings.retain(|(level, _)| *level <= indent);
        self.sequences.retain(|(level, _)| *level <= indent);
        self.close_event_collections_deeper_than(indent);
    }

    fn push_node(&mut self, kind: NodeKind, span: Span) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node {
            kind,
            span,
            children: Vec::new(),
        });
        id
    }

    fn extend_node_span(&mut self, node: NodeId, end: usize) {
        let node = &mut self.nodes[node.0 as usize];
        node.span.end = node.span.end.max(end as u32);
    }

    fn push_event(&mut self, kind: YamlEventKind, span: Span) {
        self.events.push(YamlEvent { kind, span });
    }

    fn open_event_collection(
        &mut self,
        indent: usize,
        collection: OpenEventCollection,
        kind: YamlEventKind,
        span: Span,
    ) {
        self.event_collections.push((indent, collection));
        self.push_event(kind, span);
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
        let offset = self.source.len() as u32;
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
        self.push_event(
            YamlEventKind::SequenceStart {
                style: CollectionStyle::Flow,
            },
            sequence.span,
        );
        for child in sequence.children {
            self.emit_node_event(child)?;
        }
        self.push_event(YamlEventKind::SequenceEnd, sequence.span);
        Ok(())
    }

    fn emit_flow_mapping_events(&mut self, node: NodeId) -> Result<(), YamlError> {
        let mapping = self.nodes[node.0 as usize].clone();
        self.push_event(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Flow,
            },
            mapping.span,
        );
        for entry in mapping.children {
            let entry = self.nodes[entry.0 as usize].clone();
            for child in entry.children {
                self.emit_node_event(child)?;
            }
        }
        self.push_event(YamlEventKind::MappingEnd, mapping.span);
        Ok(())
    }

    fn emit_scalar_event(&mut self, node: NodeId) -> Result<(), YamlError> {
        let node = self.nodes[node.0 as usize].clone();
        let text = self.source.slice(node.span);
        let trimmed = text.trim();
        if let Some(alias) = trimmed.strip_prefix('*')
            && !alias.is_empty()
            && !alias.chars().any(char::is_whitespace)
        {
            self.push_event(
                YamlEventKind::Alias {
                    name: alias.to_owned(),
                },
                node.span,
            );
            return Ok(());
        }
        if trimmed.starts_with('&') || trimmed.starts_with('!') {
            return Err(YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                "anchors and tags are not supported in the event stream yet",
                node.span,
            )));
        }

        let style = match node.kind {
            NodeKind::LiteralScalar => YamlScalarStyle::Literal,
            NodeKind::FoldedScalar => YamlScalarStyle::Folded,
            NodeKind::Scalar if text.starts_with('"') => YamlScalarStyle::DoubleQuoted,
            NodeKind::Scalar if text.starts_with('\'') => YamlScalarStyle::SingleQuoted,
            NodeKind::Scalar => YamlScalarStyle::Plain,
            _ => unreachable!("emit_scalar_event only receives scalar nodes"),
        };
        let value = decode_scalar_value(text)?;
        self.push_event(YamlEventKind::Scalar { style, value }, node.span);
        Ok(())
    }
}

impl TokenKind {
    const fn into_node_kind(self) -> NodeKind {
        match self {
            TokenKind::DocumentStart | TokenKind::DocumentEnd => NodeKind::DocumentMarker,
            _ => NodeKind::Scalar,
        }
    }
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
    fn new(source: &'source Source) -> Self {
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
                .try_slice(Span::new(start as u32, content_end as u32))
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
            Span::new(line.content_start as u32, line.content_end as u32),
        )
        .with_expected("--- or ... followed by separation or line break"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockChomp {
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
struct BlockScalarHeader {
    kind: BlockScalarKind,
    chomp: BlockChomp,
    indent: Option<usize>,
}

fn parse_block_scalar_header(
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
            Span::new(offset as u32, (offset + found.len_utf8()) as u32),
        )
        .with_expected("|, >, chomping indicator, or a one-digit indentation indicator"),
    )
}

fn reject_unexpected_line_start(body: &str, body_start: usize) -> Result<(), YamlError> {
    let Some(first) = body.chars().next() else {
        return Ok(());
    };

    if matches!(first, ':' | ',' | ']' | '}') {
        Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Parser,
                format!("unexpected token `{first}`"),
                Span::new(body_start as u32, (body_start + first.len_utf8()) as u32),
            )
            .with_expected("mapping entry, sequence entry, or scalar"),
        ))
    } else {
        Ok(())
    }
}

fn missing_mapping_value(line: SourceLine<'_>, indent: usize, colon_byte: usize) -> YamlError {
    let colon_offset = line.content_start + indent + colon_byte;
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "missing mapping value",
            Span::new(colon_offset as u32, (colon_offset + 1) as u32),
        )
        .with_expected("a scalar value or an indented collection on the following line"),
    )
}

fn count_indent(content: &str, content_start: usize) -> Result<usize, YamlError> {
    let mut indent = 0;
    for (offset, byte) in content.bytes().enumerate() {
        match byte {
            b' ' => indent += 1,
            b'\t' => {
                return Err(YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Parser,
                        "tab character is not allowed in indentation",
                        Span::new(
                            (content_start + offset) as u32,
                            (content_start + offset + 1) as u32,
                        ),
                    )
                    .with_expected("spaces for indentation"),
                ));
            }
            _ => break,
        }
    }
    Ok(indent)
}

fn count_literal_content_indent(content: &str) -> usize {
    content.bytes().take_while(|byte| *byte == b' ').count()
}

fn is_sequence_entry(body: &str) -> bool {
    body == "-" || body.starts_with("- ") || body.starts_with("-\t")
}

fn reject_trailing_flow_content(
    text: &str,
    parsed_end: usize,
    absolute_start: usize,
) -> Result<(), YamlError> {
    let trailing = &text[parsed_end..];
    let trailing_whitespace = leading_flow_whitespace(trailing);
    let offset = parsed_end + trailing_whitespace;
    let Some(character) = text[offset..].chars().next() else {
        return Ok(());
    };

    if character == '#' {
        return Ok(());
    }

    Err(YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("unexpected token `{character}` after flow collection"),
            Span::new(
                (absolute_start + offset) as u32,
                (absolute_start + offset + character.len_utf8()) as u32,
            ),
        )
        .with_expected("line break or comment"),
    ))
}

fn skip_flow_whitespace(text: &str, mut position: usize) -> usize {
    while let Some(character) = text[position..].chars().next() {
        if matches!(character, ' ' | '\t') {
            position += character.len_utf8();
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
        if matches!(character, ' ' | '\t') {
            length += character.len_utf8();
        } else {
            break;
        }
    }
    length
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
            '#' => return Err(flow_collection_comment(absolute_start + position)),
            '"' => position = double_quoted_flow_end(text, position, absolute_start)?,
            '\'' => position = single_quoted_flow_end(text, position, absolute_start)?,
            _ => position += character.len_utf8(),
        }
    }

    Ok(position)
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
            Span::new(
                (absolute_start + start) as u32,
                (absolute_start + text.len()) as u32,
            ),
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
            Span::new(
                (absolute_start + start) as u32,
                (absolute_start + text.len()) as u32,
            ),
        )
        .with_expected("closing '"),
    ))
}

fn missing_flow_sequence_end(absolute_start: usize, text_len: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "missing flow sequence closing bracket",
            Span::empty((absolute_start + text_len) as u32),
        )
        .with_expected("]"),
    )
}

fn empty_flow_sequence_item(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "empty flow sequence item",
            Span::empty(offset as u32),
        )
        .with_expected("a scalar or nested flow sequence"),
    )
}

fn empty_flow_value(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "empty flow value",
            Span::empty(offset as u32),
        )
        .with_expected("a scalar or nested flow collection"),
    )
}

fn unexpected_flow_comma(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "unexpected comma in flow sequence",
            Span::new(offset as u32, (offset + 1) as u32),
        )
        .with_expected("a scalar, nested flow sequence, or ]"),
    )
}

fn expected_flow_separator(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("unexpected token `{found}` in flow sequence"),
            Span::new(offset as u32, (offset + found.len_utf8()) as u32),
        )
        .with_expected(", or ]"),
    )
}

fn flow_sequence_comment(offset: usize) -> YamlError {
    flow_collection_comment(offset)
}

fn missing_flow_mapping_end(absolute_start: usize, text_len: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "missing flow mapping closing brace",
            Span::empty((absolute_start + text_len) as u32),
        )
        .with_expected("}"),
    )
}

fn empty_flow_mapping_pair(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "empty flow mapping pair",
            Span::empty(offset as u32),
        )
        .with_expected("a mapping key"),
    )
}

fn empty_flow_mapping_key(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "empty flow mapping key",
            Span::empty(offset as u32),
        )
        .with_expected("a mapping key"),
    )
}

fn unexpected_flow_mapping_comma(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "unexpected comma in flow mapping",
            Span::new(offset as u32, (offset + 1) as u32),
        )
        .with_expected("a mapping key or }"),
    )
}

fn missing_flow_mapping_colon(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("missing colon after flow mapping key before `{found}`"),
            Span::new(offset as u32, (offset + found.len_utf8()) as u32),
        )
        .with_expected(":"),
    )
}

fn expected_flow_mapping_separator(offset: usize, found: char) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            format!("unexpected token `{found}` in flow mapping"),
            Span::new(offset as u32, (offset + found.len_utf8()) as u32),
        )
        .with_expected(", or }"),
    )
}

fn flow_mapping_comment(offset: usize) -> YamlError {
    flow_collection_comment(offset)
}

fn flow_collection_comment(offset: usize) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Parser,
            "comments inside flow collections are not supported yet",
            Span::new(offset as u32, (offset + 1) as u32),
        )
        .with_expected("a scalar, separator, or closing delimiter before any comment"),
    )
}

fn find_mapping_colon(body: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for (offset, character) in body.char_indices() {
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
        } else if character == '"' {
            in_double = true;
        } else if character == '\'' {
            in_single = true;
        } else if character == ':' {
            return Some(offset);
        }
    }

    None
}

fn validate_plain_mapping_fragment(text: &str, role: &str) -> Result<(), YamlError> {
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

fn edits_conflict(left: Span, right: Span) -> bool {
    if left.is_empty() && right.is_empty() {
        left.start == right.start
    } else {
        left.start < right.end && right.start < left.end
    }
}

fn double_quoted_scalar_end(text: &str) -> Option<usize> {
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

fn single_quoted_scalar_end(text: &str) -> Option<usize> {
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

fn plain_scalar_end(text: &str) -> usize {
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
enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
}

fn format_scalar_value(value: &str, style: ScalarStyle) -> Result<String, YamlError> {
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

fn decode_scalar_value(text: &str) -> Result<String, YamlError> {
    if text.starts_with('|') {
        return decode_literal_scalar_value(text);
    }
    if text.starts_with('>') {
        return decode_folded_scalar_value(text);
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
        return decode_double_quoted_scalar(&text[1..end - 1]);
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
        return Ok(text[1..end - 1].replace("''", "'"));
    }

    Ok(text[..plain_scalar_end(text)].to_owned())
}

fn decode_literal_scalar_value(text: &str) -> Result<String, YamlError> {
    let (header, content_start) = split_first_line(text);
    let header = parse_block_scalar_header(header, 0)?;
    let content = &text[content_start..];
    let content_indent = header
        .indent
        .unwrap_or_else(|| detect_literal_content_indent(content));
    let mut decoded = String::new();
    let mut position = 0;

    while position < content.len() {
        let (line, next_position) = next_literal_content_line(content, position);
        let (body, break_text) = split_line_break(line);
        let stripped = if body.trim().is_empty() {
            ""
        } else {
            strip_literal_indent(body, content_indent)
        };
        decoded.push_str(stripped);
        decoded.push_str(break_text);
        position = next_position;
    }

    Ok(apply_block_chomp(decoded, header.chomp))
}

fn decode_folded_scalar_value(text: &str) -> Result<String, YamlError> {
    let (header, content_start) = split_first_line(text);
    let header = parse_block_scalar_header(header, 0)?;
    let content = &text[content_start..];
    let content_indent = header
        .indent
        .unwrap_or_else(|| detect_literal_content_indent(content));
    let literal = decode_block_scalar_content(content, content_indent);

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
        let stripped = if body.trim().is_empty() {
            ""
        } else {
            strip_literal_indent(body, content_indent)
        };
        decoded.push_str(stripped);
        decoded.push_str(break_text);
        position = next_position;
    }

    decoded
}

fn fold_block_scalar_lines(literal: &str) -> String {
    let lines = literal_lines(literal);
    let mut output = String::new();

    for index in 0..lines.len() {
        let (body, break_text) = lines[index];
        output.push_str(body);
        if break_text.is_empty() {
            continue;
        }

        let next = lines.get(index + 1).copied();
        if next.is_none() {
            output.push_str(break_text);
        } else if body.is_empty() {
            output.push_str(break_text);
        } else if next
            .is_some_and(|(next_body, _)| next_body.is_empty() || line_is_more_indented(next_body))
            || line_is_more_indented(body)
        {
            output.push_str(break_text);
        } else {
            output.push(' ');
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
            return (&text[start..index + 1], index + 1);
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
    let mut stripped = 0;
    for (offset, byte) in line.bytes().enumerate() {
        if stripped == indent || byte != b' ' {
            return &line[offset..];
        }
        stripped += 1;
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
            if had_line_break {
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
            YamlEventKind::DocumentEnd => output.push_str("-DOC\n"),
            YamlEventKind::SequenceStart { style } => match style {
                CollectionStyle::Block => output.push_str("+SEQ\n"),
                CollectionStyle::Flow => output.push_str("+SEQ []\n"),
            },
            YamlEventKind::SequenceEnd => output.push_str("-SEQ\n"),
            YamlEventKind::MappingStart { style } => match style {
                CollectionStyle::Block => output.push_str("+MAP\n"),
                CollectionStyle::Flow => output.push_str("+MAP {}\n"),
            },
            YamlEventKind::MappingEnd => output.push_str("-MAP\n"),
            YamlEventKind::Scalar { style, value } => {
                output.push_str("=VAL ");
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

fn escape_event_value(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
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
    let mut chars = text.chars();

    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }

        let Some(escaped) = chars.next() else {
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
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            '/' => output.push('/'),
            '0' => output.push('\0'),
            'a' => output.push('\u{0007}'),
            'b' => output.push('\u{0008}'),
            't' | '\t' => output.push('\t'),
            'n' => output.push('\n'),
            'v' => output.push('\u{000B}'),
            'f' => output.push('\u{000C}'),
            'r' => output.push('\r'),
            'e' => output.push('\u{001B}'),
            other => output.push(other),
        }
    }

    Ok(output)
}

/// Pending source edit used by the patch-based emitter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Span to replace. Empty spans represent insertions.
    pub span: Span,
    /// Replacement text.
    pub replacement: String,
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
    pub source: Source,
    /// Lossless token stream in source order.
    pub tokens: Vec<Token>,
    /// CST and semantic nodes. The CST remains the source of truth.
    pub nodes: Vec<Node>,
    /// Semantic YAML event stream produced by the parser.
    pub events: Vec<YamlEvent>,
    /// CST-linked semantic graph composed from parser events.
    pub graph: SemanticGraph,
    /// Pending patch edits applied from highest offset to lowest offset.
    pub edits: Vec<Edit>,
}

impl YamlDoc {
    /// Parses a YAML stream into a round-trip document.
    ///
    /// This bootstrap parser preserves the input text exactly and records a root
    /// stream node. Real lexing and parsing will replace this placeholder.
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

    /// Returns the original source text.
    #[must_use]
    pub fn as_source(&self) -> &str {
        self.source.as_str()
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
    pub fn root_graph_mapping(&self) -> Result<GraphNodeId, YamlError> {
        self.root_mapping_graph()
    }

    /// Returns the first root-level block mapping in the document.
    pub fn root_mapping(&self) -> Result<NodeId, YamlError> {
        let mapping = self.root_mapping_graph()?;
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

    /// Looks up a mapping entry by key inside `mapping`.
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
    pub fn get_graph_path(&self, path: &[&str]) -> Result<Option<GraphNodeId>, YamlError> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(None);
        };

        let mut current = match self.get_graph_mapping_entry(self.root_mapping_graph()?, first)? {
            Some((_, value)) => value,
            None => return Ok(None),
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
    pub fn get_path(&self, path: &[&str]) -> Result<Option<NodeId>, YamlError> {
        Ok(self
            .get_graph_path(path)?
            .and_then(|node| self.graph_node_cst(node)))
    }

    fn root_mapping_graph(&self) -> Result<GraphNodeId, YamlError> {
        let root = self.graph.root.ok_or_else(|| {
            YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                "document does not contain a semantic root",
                Span::empty(0),
            ))
        })?;
        let root = self.expect_graph_node(root)?;
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
                        Span::empty(0),
                    )
                    .with_expected("a mapping graph node"),
                )
            })
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
                Span::empty(self.source.len() as u32),
            ))
        })
    }

    fn graph_for_cst(&self, cst: NodeId) -> Option<GraphNodeId> {
        self.graph
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.cst == Some(cst))
            .map(|(index, _)| GraphNodeId(index as u32))
    }

    fn graph_scalar_value(&self, node: NodeId) -> Option<&str> {
        let graph = self.graph_for_cst(node)?;
        let graph = self.graph_node(graph)?;
        match &graph.kind {
            GraphKind::Scalar { value, .. } => Some(value),
            _ => None,
        }
    }

    fn graph_sequence_items(&self, node: NodeId) -> Option<Vec<NodeId>> {
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

    fn graph_mapping_entries(&self, node: NodeId) -> Option<Vec<(NodeId, NodeId)>> {
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

    /// Returns the source text for a scalar node.
    pub fn scalar_text(&self, node: NodeId) -> Result<&str, YamlError> {
        let node = self.expect_node_kind(node, NodeKind::Scalar)?;
        Ok(self.source.slice(node.span))
    }

    /// Returns the decoded value text for a scalar node in the MVP scalar subset.
    ///
    /// Plain scalars have trailing inline comments stripped, single-quoted
    /// scalars unescape doubled apostrophes, and double-quoted scalars unescape
    /// the common JSON/YAML escapes currently used by the typed overlay MVP.
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
        decode_scalar_value(self.source.slice(node.span))
    }

    /// Queues a scalar value replacement at `path` while preserving the existing
    /// scalar style where the MVP writer can do so safely.
    ///
    /// Plain scalars remain plain, single-quoted scalars remain single-quoted,
    /// and double-quoted scalars remain double-quoted. Inline comments and
    /// trailing whitespace outside the scalar spelling are left untouched.
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

        self.queue_edit(Span::empty(insertion_offset as u32), replacement)
    }

    /// Queues insertion of a plain `key: value` entry before `before_entry`.
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

        self.queue_edit(Span::empty(insertion_offset as u32), replacement)
    }

    /// Queues insertion according to a declaration-order key list.
    ///
    /// If a later key from `ordered_keys` already exists in `mapping`, the new
    /// entry is inserted before that entry. Otherwise this falls back to append
    /// insertion. This is the MVP primitive behind `insert_order = "struct"`.
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
    pub fn remove_node(&mut self, node: NodeId) -> Result<(), YamlError> {
        let node = self.expect_node(node)?;
        let span = if matches!(node.kind, NodeKind::MappingEntry | NodeKind::SequenceEntry) {
            self.line_span_including_break(node.span)
        } else {
            node.span
        };
        self.queue_edit(span, String::new())
    }

    fn scalar_replacement_target(&self, node: NodeId) -> Result<(Span, ScalarStyle), YamlError> {
        let node = self.expect_node_kind(node, NodeKind::Scalar)?;
        let text = self.source.slice(node.span);

        if text.starts_with('"') {
            let end = double_quoted_scalar_end(text).ok_or_else(|| {
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
                Span::new(node.span.start, node.span.start + end as u32),
                ScalarStyle::DoubleQuoted,
            ));
        }

        if text.starts_with('\'') {
            let end = single_quoted_scalar_end(text).ok_or_else(|| {
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
                Span::new(node.span.start, node.span.start + end as u32),
                ScalarStyle::SingleQuoted,
            ));
        }

        let end = plain_scalar_end(text);
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
            Span::new(node.span.start, node.span.start + end as u32),
            ScalarStyle::Plain,
        ))
    }

    fn expect_node(&self, node: NodeId) -> Result<&Node, YamlError> {
        self.node(node).ok_or_else(|| {
            YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                format!("unknown node id {}", node.0),
                Span::empty(self.source.len() as u32),
            ))
        })
    }

    fn expect_node_kind(&self, node: NodeId, expected: NodeKind) -> Result<&Node, YamlError> {
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

    fn containing_entry(&self, value: NodeId) -> Option<NodeId> {
        self.nodes.iter().enumerate().find_map(|(index, node)| {
            matches!(node.kind, NodeKind::MappingEntry | NodeKind::SequenceEntry)
                .then_some(())
                .filter(|_| node.children.contains(&value))
                .map(|_| NodeId(index as u32))
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

    fn node_indent(&self, node: &Node) -> usize {
        let line_start = self.line_start_for_offset(node.span.start as usize);
        self.source.as_str()[line_start..node.span.start as usize]
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

    fn find_nested_collection_after(&self, entry: &Node, parent_indent: usize) -> Option<NodeId> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                matches!(node.kind, NodeKind::BlockMapping | NodeKind::BlockSequence)
                    && node.span.start >= entry.span.end
                    && self.node_indent(node) > parent_indent
            })
            .min_by_key(|(_, node)| node.span.start)
            .map(|(index, _)| NodeId(index as u32))
    }

    fn queue_edit(&mut self, span: Span, replacement: String) -> Result<(), YamlError> {
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

        Span::new(start as u32, end as u32)
    }

    fn preferred_line_ending(&self) -> &str {
        let bytes = self.source.as_str().as_bytes();
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                return if index > 0 && bytes[index - 1] == b'\r' {
                    "\r\n"
                } else {
                    "\n"
                };
            }
            if *byte == b'\r' {
                return "\r";
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
}

fn compose_graph(events: &[YamlEvent], nodes: &[Node]) -> Result<SemanticGraph, YamlError> {
    GraphComposer::new(events, nodes).compose()
}

struct GraphComposer<'events> {
    events: &'events [YamlEvent],
    cst_nodes: &'events [Node],
    graph_nodes: Vec<GraphNode>,
    stack: Vec<OpenGraphNode>,
    root: Option<GraphNodeId>,
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
    fn new(events: &'events [YamlEvent], cst_nodes: &'events [Node]) -> Self {
        Self {
            events,
            cst_nodes,
            graph_nodes: Vec::new(),
            stack: Vec::new(),
            root: None,
        }
    }

    fn compose(mut self) -> Result<SemanticGraph, YamlError> {
        for event in self.events {
            match &event.kind {
                YamlEventKind::StreamStart | YamlEventKind::StreamEnd => {}
                YamlEventKind::DocumentStart { .. } => {
                    let id = self.push_node(GraphNode {
                        kind: GraphKind::Document {
                            children: Vec::new(),
                        },
                        span: event.span,
                        cst: self.find_cst_node(NodeKind::Document, event.span),
                    });
                    if self.root.replace(id).is_some() {
                        return Err(graph_error(
                            "multiple documents are not supported yet",
                            event.span,
                        ));
                    }
                    self.stack.push(OpenGraphNode {
                        id,
                        kind: OpenGraphKind::Document,
                    });
                }
                YamlEventKind::DocumentEnd => {
                    self.close_expected(OpenGraphKind::Document, event.span)?;
                }
                YamlEventKind::MappingStart { style } => {
                    let id = self.push_node(GraphNode {
                        kind: GraphKind::Mapping {
                            style: *style,
                            entries: Vec::new(),
                        },
                        span: event.span,
                        cst: self.find_cst_node(mapping_node_kind(*style), event.span),
                    });
                    self.stack.push(OpenGraphNode {
                        id,
                        kind: OpenGraphKind::Mapping,
                    });
                }
                YamlEventKind::MappingEnd => {
                    let id = self.close_expected(OpenGraphKind::Mapping, event.span)?;
                    self.attach_node(id, event.span)?;
                }
                YamlEventKind::SequenceStart { style } => {
                    let id = self.push_node(GraphNode {
                        kind: GraphKind::Sequence {
                            style: *style,
                            items: Vec::new(),
                        },
                        span: event.span,
                        cst: self.find_cst_node(sequence_node_kind(*style), event.span),
                    });
                    self.stack.push(OpenGraphNode {
                        id,
                        kind: OpenGraphKind::Sequence,
                    });
                }
                YamlEventKind::SequenceEnd => {
                    let id = self.close_expected(OpenGraphKind::Sequence, event.span)?;
                    self.attach_node(id, event.span)?;
                }
                YamlEventKind::Scalar { style, value } => {
                    let id = self.push_node(GraphNode {
                        kind: GraphKind::Scalar {
                            style: *style,
                            value: value.clone(),
                            tag: None,
                            anchor: None,
                        },
                        span: event.span,
                        cst: self.find_cst_node(scalar_node_kind(*style), event.span),
                    });
                    self.attach_node(id, event.span)?;
                }
                YamlEventKind::Alias { name } => {
                    let id = self.push_node(GraphNode {
                        kind: GraphKind::Alias { name: name.clone() },
                        span: event.span,
                        cst: self.find_cst_node(NodeKind::Scalar, event.span),
                    });
                    self.attach_node(id, event.span)?;
                }
            }
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
        })
    }

    fn push_node(&mut self, node: GraphNode) -> GraphNodeId {
        let id = GraphNodeId(self.graph_nodes.len() as u32);
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
        match &mut self.graph_nodes[parent.id.0 as usize].kind {
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

    fn find_cst_node(&self, kind: NodeKind, span: Span) -> Option<NodeId> {
        self.cst_nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == kind && node.span == span)
            .or_else(|| {
                self.cst_nodes
                    .iter()
                    .enumerate()
                    .find(|(_, node)| node.kind == kind && node.span.start == span.start)
            })
            .map(|(index, _)| NodeId(index as u32))
    }
}

fn graph_error(message: impl Into<String>, span: Span) -> YamlError {
    YamlError::new(Diagnostic::new(DiagnosticKind::Semantic, message, span))
}

const fn mapping_node_kind(style: CollectionStyle) -> NodeKind {
    match style {
        CollectionStyle::Block => NodeKind::BlockMapping,
        CollectionStyle::Flow => NodeKind::FlowMapping,
    }
}

const fn sequence_node_kind(style: CollectionStyle) -> NodeKind {
    match style {
        CollectionStyle::Block => NodeKind::BlockSequence,
        CollectionStyle::Flow => NodeKind::FlowSequence,
    }
}

const fn scalar_node_kind(style: YamlScalarStyle) -> NodeKind {
    match style {
        YamlScalarStyle::Literal => NodeKind::LiteralScalar,
        YamlScalarStyle::Folded => NodeKind::FoldedScalar,
        YamlScalarStyle::Plain | YamlScalarStyle::SingleQuoted | YamlScalarStyle::DoubleQuoted => {
            NodeKind::Scalar
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

/// Error type for YAML parsing, semantic lookup, typed overlays, and emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlError {
    /// Primary diagnostic.
    pub diagnostic: Diagnostic,
}

impl YamlError {
    /// Creates a new error from a diagnostic.
    #[must_use]
    pub const fn new(diagnostic: Diagnostic) -> Self {
        Self { diagnostic }
    }

    /// Adds line/column information from `source` when the diagnostic does not
    /// already have a position.
    #[must_use]
    pub fn with_position_from(mut self, source: &Source) -> Self {
        if self.diagnostic.position.is_none() {
            self.diagnostic.position = Some(source.diagnostic_position(&self.diagnostic));
        }
        self
    }
}

impl fmt::Display for YamlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.fmt(formatter)
    }
}

impl std::error::Error for YamlError {}

/// Structured user-facing diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Error phase.
    pub kind: DiagnosticKind,
    /// Primary message.
    pub message: String,
    /// Primary source span.
    pub span: Span,
    /// One-based line/column for the primary span when a source is available.
    pub position: Option<LineCol>,
    /// Expected syntax or semantic items.
    pub expected: Vec<String>,
    /// Additional context notes.
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Creates a diagnostic with no expected items or notes.
    #[must_use]
    pub fn new(kind: DiagnosticKind, message: impl Into<String>, span: Span) -> Self {
        Self {
            kind,
            message: message.into(),
            span,
            position: None,
            expected: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Sets a one-based line/column position for the primary span.
    #[must_use]
    pub const fn with_position(mut self, position: LineCol) -> Self {
        self.position = Some(position);
        self
    }

    /// Adds one expected syntax or semantic item.
    #[must_use]
    pub fn with_expected(mut self, expected: impl Into<String>) -> Self {
        self.expected.push(expected.into());
        self
    }

    /// Adds one explanatory note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)?;

        if let Some(position) = self.position {
            write!(formatter, " at {}:{}", position.line, position.column)?;
        }

        if !self.expected.is_empty() {
            write!(formatter, " (expected: {})", self.expected.join(", "))?;
        }

        for note in &self.notes {
            write!(formatter, "\nnote: {note}")?;
        }

        Ok(())
    }
}

/// Diagnostic phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// Source validation failure.
    Source,
    /// Lexer failure.
    Lexer,
    /// Parser failure.
    Parser,
    /// Semantic graph or schema failure.
    Semantic,
    /// Typed overlay failure.
    Typed,
    /// Emitter failure.
    Emitter,
}

/// Alias for parse errors until richer phase-specific errors are introduced.
pub type ParseError = YamlError;

/// Converts a YAML document into a typed overlay.
pub trait FromYamlDoc: Sized {
    /// Reads `Self` from `doc` while preserving the document as the source of
    /// truth for future edits.
    fn from_yaml_doc(doc: &YamlDoc) -> Result<Self, YamlError>;
}

/// Applies a typed overlay back to a YAML document as minimal patches.
pub trait ToYamlDoc {
    /// Writes `self` into `doc` without discarding unknown fields or comments.
    fn apply_to_yaml_doc(&self, doc: &mut YamlDoc) -> Result<(), YamlError>;
}

/// Reads and writes individual YAML node values.
pub trait YamlValue: Sized {
    /// Reads a typed value from `node`.
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError>;

    /// Writes a typed value into an existing node or inserts a new node.
    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError>;
}

fn missing_write_node_error() -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Typed,
            "cannot insert a standalone YAML value without collection context",
            Span::empty(0),
        )
        .with_expected("an existing YAML node"),
    )
}

fn write_existing_scalar(
    doc: &mut YamlDoc,
    node: Option<NodeId>,
    value: &str,
) -> Result<NodeId, YamlError> {
    let node = node.ok_or_else(missing_write_node_error)?;
    if doc
        .node(node)
        .is_some_and(|node| matches!(node.kind, NodeKind::LiteralScalar | NodeKind::FoldedScalar))
    {
        let block_scalar = doc.expect_node(node)?;
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Emitter,
                "block scalar rewriting is not implemented yet",
                block_scalar.span,
            )
            .with_expected("an existing plain, single-quoted, or double-quoted scalar"),
        )
        .with_position_from(&doc.source));
    }
    let (span, style) = doc.scalar_replacement_target(node)?;
    let replacement = format_scalar_value(value, style)?;
    doc.queue_edit(span, replacement)?;
    Ok(node)
}

fn missing_collection_item_error(doc: &YamlDoc, node: &Node, collection: &str) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Typed,
            format!("{collection} entry does not contain a value"),
            node.span,
        )
        .with_expected("a YAML value node"),
    )
    .with_position_from(&doc.source)
}

fn format_block_sequence_replacement<T>(
    doc: &YamlDoc,
    sequence: &Node,
    values: &[T],
) -> Result<String, YamlError>
where
    T: ToString,
{
    let indent = doc.node_indent(sequence);
    let indent_text = " ".repeat(indent);
    let line_ending = doc.preferred_line_ending();
    let mut output = String::new();

    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push_str(line_ending);
        }
        let value = value.to_string();
        validate_plain_mapping_fragment(&value, "sequence value")?;
        if index > 0 {
            output.push_str(&indent_text);
        }
        output.push_str("- ");
        output.push_str(&value);
    }

    Ok(output)
}

fn format_block_mapping_replacement<T>(
    doc: &YamlDoc,
    mapping: &Node,
    values: &std::collections::BTreeMap<String, T>,
) -> Result<String, YamlError>
where
    T: ToString,
{
    let indent = doc.node_indent(mapping);
    let indent_text = " ".repeat(indent);
    let line_ending = doc.preferred_line_ending();
    let mut output = String::new();

    for (index, (key, value)) in values.iter().enumerate() {
        if index > 0 {
            output.push_str(line_ending);
        }
        let value = value.to_string();
        validate_plain_mapping_fragment(key, "mapping key")?;
        validate_plain_mapping_fragment(&value, "mapping value")?;
        if index > 0 {
            output.push_str(&indent_text);
        }
        output.push_str(key);
        output.push_str(": ");
        output.push_str(&value);
    }

    Ok(output)
}

fn typed_parse_error(doc: &YamlDoc, node: NodeId, type_name: &str, value: &str) -> YamlError {
    let span = doc.node(node).map_or(Span::empty(0), |node| node.span);
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Typed,
            format!("could not parse `{value}` as {type_name}"),
            span,
        )
        .with_expected(type_name),
    )
    .with_position_from(&doc.source)
}

impl YamlValue for String {
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        doc.scalar_value(node)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        write_existing_scalar(doc, node, self)
    }
}

impl YamlValue for bool {
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        let value = doc.scalar_value(node)?;
        match value.as_str() {
            "true" | "True" | "TRUE" => Ok(true),
            "false" | "False" | "FALSE" => Ok(false),
            _ => Err(typed_parse_error(doc, node, "bool", &value)),
        }
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        write_existing_scalar(doc, node, if *self { "true" } else { "false" })
    }
}

macro_rules! impl_yaml_number {
    ($($type:ty),* $(,)?) => {
        $(
            impl YamlValue for $type {
                fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
                    let value = doc.scalar_value(node)?;
                    value.parse::<$type>().map_err(|_| {
                        typed_parse_error(doc, node, stringify!($type), &value)
                    })
                }

                fn write_yaml(
                    &self,
                    doc: &mut YamlDoc,
                    node: Option<NodeId>,
                ) -> Result<NodeId, YamlError> {
                    write_existing_scalar(doc, node, &self.to_string())
                }
            }
        )*
    };
}

impl_yaml_number!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);

impl<T> YamlValue for Option<T>
where
    T: YamlValue,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        T::read_yaml(doc, node).map(Some)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        match self {
            Some(value) => value.write_yaml(doc, node),
            None => {
                let node = node.ok_or_else(missing_write_node_error)?;
                let remove_node = doc.containing_entry(node).unwrap_or(node);
                doc.remove_node(remove_node)?;
                Ok(node)
            }
        }
    }
}

impl<T> YamlValue for Vec<T>
where
    T: YamlValue + ToString,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        if let Some(items) = doc.graph_sequence_items(node) {
            let mut values = Vec::new();
            for item in items {
                values.push(T::read_yaml(doc, item)?);
            }
            return Ok(values);
        }

        let sequence = doc.expect_node(node)?;
        let mut values = Vec::new();

        match sequence.kind {
            NodeKind::BlockSequence => {
                for entry in &sequence.children {
                    let entry_node = doc.expect_node(*entry)?;
                    let Some(value_node) = entry_node.children.first().copied() else {
                        return Err(missing_collection_item_error(doc, entry_node, "sequence"));
                    };
                    values.push(T::read_yaml(doc, value_node)?);
                }
            }
            NodeKind::FlowSequence => {
                for value_node in &sequence.children {
                    values.push(T::read_yaml(doc, *value_node)?);
                }
            }
            _ => {
                return Err(YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Typed,
                        format!("expected sequence, found {:?}", sequence.kind),
                        sequence.span,
                    )
                    .with_expected("BlockSequence or FlowSequence"),
                )
                .with_position_from(&doc.source));
            }
        }

        Ok(values)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        let node = node.ok_or_else(missing_write_node_error)?;
        let sequence = doc.expect_node(node)?;
        if sequence.kind == NodeKind::FlowSequence {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Emitter,
                    "flow sequence rewriting is not implemented yet",
                    sequence.span,
                )
                .with_expected("an existing block sequence"),
            )
            .with_position_from(&doc.source));
        }
        let sequence = doc.expect_node_kind(node, NodeKind::BlockSequence)?;
        let replacement = format_block_sequence_replacement(doc, sequence, self)?;
        doc.queue_edit(sequence.span, replacement)?;
        Ok(node)
    }
}

impl<T> YamlValue for std::collections::BTreeMap<String, T>
where
    T: YamlValue + ToString,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        if let Some(entries) = doc.graph_mapping_entries(node) {
            let mut values = std::collections::BTreeMap::new();
            for (key_node, value_node) in entries {
                let key = doc.scalar_value(key_node)?;
                values.insert(key, T::read_yaml(doc, value_node)?);
            }
            return Ok(values);
        }

        let mapping = doc.expect_node(node)?;
        let mut values = std::collections::BTreeMap::new();

        match mapping.kind {
            NodeKind::BlockMapping => {
                let mapping_indent = doc.node_indent(mapping);
                for entry in &mapping.children {
                    let entry_node = doc.expect_node(*entry)?;
                    let Some(key_node) = entry_node.children.first().copied() else {
                        continue;
                    };
                    let key = doc.scalar_text(key_node)?.to_owned();
                    let value_node = if let Some(value_node) = entry_node.children.get(1).copied() {
                        value_node
                    } else {
                        doc.find_nested_collection_after(entry_node, mapping_indent)
                            .ok_or_else(|| {
                                missing_collection_item_error(doc, entry_node, "mapping")
                            })?
                    };
                    values.insert(key, T::read_yaml(doc, value_node)?);
                }
            }
            NodeKind::FlowMapping => {
                for entry in &mapping.children {
                    let entry_node = doc.expect_node(*entry)?;
                    let Some(key_node) = entry_node.children.first().copied() else {
                        continue;
                    };
                    let key = doc.scalar_text(key_node)?.to_owned();
                    let value_node =
                        entry_node.children.get(1).copied().ok_or_else(|| {
                            missing_collection_item_error(doc, entry_node, "mapping")
                        })?;
                    values.insert(key, T::read_yaml(doc, value_node)?);
                }
            }
            _ => {
                return Err(YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Typed,
                        format!("expected mapping, found {:?}", mapping.kind),
                        mapping.span,
                    )
                    .with_expected("BlockMapping or FlowMapping"),
                )
                .with_position_from(&doc.source));
            }
        }

        Ok(values)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        let node = node.ok_or_else(missing_write_node_error)?;
        let mapping = doc.expect_node(node)?;
        if mapping.kind == NodeKind::FlowMapping {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Emitter,
                    "flow mapping rewriting is not implemented yet",
                    mapping.span,
                )
                .with_expected("an existing block mapping"),
            )
            .with_position_from(&doc.source));
        }
        let mapping = doc.expect_node_kind(node, NodeKind::BlockMapping)?;
        let replacement = format_block_mapping_replacement(doc, mapping, self)?;
        doc.queue_edit(mapping.span, replacement)?;
        Ok(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_parser_preserves_source() {
        let source = "---\nkey: value\n# comment\n";
        let doc = YamlDoc::parse(source).expect("placeholder parser should accept text");

        assert_eq!(doc.as_source(), source);
        assert_eq!(doc.to_string(), source);
    }

    #[test]
    fn target_version_is_yaml_1_2_2() {
        assert_eq!(TARGET_YAML_VERSION, "1.2.2");
    }

    #[test]
    fn source_tracks_line_columns() {
        let source = Source::new("a\nbc\n".to_owned()).expect("valid YAML characters");

        assert_eq!(source.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(source.line_col(2), LineCol { line: 2, column: 1 });
        assert_eq!(source.slice(Span::new(2, 4)), "bc");
        assert_eq!(source.line_starts(), &[0, 2, 5]);
    }

    #[test]
    fn source_rejects_invalid_yaml_characters() {
        let error = Source::new("valid\0invalid".to_owned()).expect_err("NUL is not YAML text");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Source);
        assert_eq!(error.diagnostic.span, Span::new(5, 6));
        assert!(error.to_string().contains("U+0000"));
    }

    #[test]
    fn try_slice_reports_invalid_spans() {
        let source = Source::new("é".to_owned()).expect("valid YAML characters");
        let error = source
            .try_slice(Span::new(0, 1))
            .expect_err("span splits UTF-8 code point");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Source);
        assert_eq!(
            source.diagnostic_position(&error.diagnostic),
            LineCol { line: 1, column: 1 }
        );
    }

    #[test]
    fn lexer_preserves_mvp_yaml_source() {
        let input = "# comment
---
key: value
list:
  - item
quoted: \"hello\"
single: 'hello'
...
";
        let doc = YamlDoc::parse(input).expect("lexer MVP should accept fixture");

        assert_eq!(tokens_to_string(&doc.tokens, &doc.source), input);
        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.tokens.first().map(|token| token.kind),
            Some(TokenKind::Comment)
        );
        assert!(
            doc.tokens
                .iter()
                .any(|token| token.kind == TokenKind::DocumentStart)
        );
        assert!(
            doc.tokens
                .iter()
                .any(|token| token.kind == TokenKind::DocumentEnd)
        );
        assert!(
            doc.tokens
                .iter()
                .any(|token| token.kind == TokenKind::DoubleQuotedScalar)
        );
        assert!(
            doc.tokens
                .iter()
                .any(|token| token.kind == TokenKind::SingleQuotedScalar)
        );
    }

    #[test]
    fn lexer_emits_flow_marker_tokens() {
        let source = Source::new(
            "flow: [a, {b: c}]
"
            .to_owned(),
        )
        .expect("valid YAML characters");
        let tokens = lex(&source).expect("lexer MVP should accept flow markers");
        let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();

        assert_eq!(tokens_to_string(&tokens, &source), source.as_str());
        assert!(kinds.contains(&TokenKind::FlowSequenceStart));
        assert!(kinds.contains(&TokenKind::FlowSequenceEnd));
        assert!(kinds.contains(&TokenKind::FlowMappingStart));
        assert!(kinds.contains(&TokenKind::FlowMappingEnd));
        assert!(kinds.contains(&TokenKind::Comma));
    }

    #[test]
    fn lexer_reports_unterminated_quoted_scalars() {
        let error = YamlDoc::parse("quoted: \"oops").expect_err("quote is unterminated");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Lexer);
        assert!(
            error
                .to_string()
                .contains("unterminated double-quoted scalar")
        );
        assert_eq!(error.diagnostic.expected, ["closing \"".to_owned()]);
    }

    #[test]
    fn events_render_root_scalar() {
        let doc = YamlDoc::parse("value\n").expect("valid scalar");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n=VAL :value\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_explicit_document_block_mapping() {
        let doc = YamlDoc::parse("---\nhost: localhost\n").expect("valid mapping");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n+MAP\n=VAL :host\n=VAL :localhost\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_nested_block_sequence() {
        let doc = YamlDoc::parse("ports:\n  - 8080\n  - 9090\n").expect("valid sequence");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :ports\n+SEQ\n=VAL :8080\n=VAL :9090\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_flow_collections() {
        let doc = YamlDoc::parse("settings: {a: [b, c]}\n").expect("valid flow collections");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :settings\n+MAP {}\n=VAL :a\n+SEQ []\n=VAL :b\n=VAL :c\n-SEQ\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_scalar_styles_and_decoded_values() {
        let doc = YamlDoc::parse(
            "plain: value\nsingle: 'Bob''s'\ndouble: \"line\\nnext\"\nliteral: |\n  one\n  two\nfolded: >\n  one\n  two\n",
        )
        .expect("valid scalars");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :plain\n=VAL :value\n=VAL :single\n=VAL 'Bob's\n=VAL :double\n=VAL \"line\\nnext\n=VAL :literal\n=VAL |one\\ntwo\\n\n=VAL :folded\n=VAL >one two\\n\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_events_carry_source_spans() {
        let doc = YamlDoc::parse("host: localhost\n").expect("valid mapping");
        let scalar_events: Vec<_> = doc
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                YamlEventKind::Scalar { value, .. } => Some((value.as_str(), event.span)),
                _ => None,
            })
            .collect();

        assert_eq!(
            scalar_events,
            [("host", Span::new(0, 4)), ("localhost", Span::new(6, 15))]
        );
    }

    #[test]
    fn graph_builds_root_scalar_with_cst_link() {
        let doc = YamlDoc::parse("value\n").expect("valid scalar");
        let root = doc.graph().root.expect("graph root exists");
        let root = doc.graph_node(root).expect("root graph node exists");
        let GraphKind::Document { children } = &root.kind else {
            panic!("root should be document");
        };
        let scalar = doc
            .graph_node(children[0])
            .expect("scalar graph node exists");

        assert_eq!(scalar.span, Span::new(0, 5));
        assert_eq!(
            scalar
                .cst
                .and_then(|node| doc.node(node))
                .map(|node| node.kind),
            Some(NodeKind::Scalar)
        );
        assert_eq!(
            scalar.kind,
            GraphKind::Scalar {
                style: YamlScalarStyle::Plain,
                value: "value".to_owned(),
                tag: None,
                anchor: None,
            }
        );
    }

    #[test]
    fn graph_builds_mapping_sequence_and_preserves_path_lookup() {
        let doc = YamlDoc::parse("ports:\n  - 8080\n  - 9090\n").expect("valid sequence");
        let ports = doc
            .get_path(&["ports"])
            .expect("path lookup succeeds")
            .expect("ports exists");
        let items = doc
            .graph_sequence_items(ports)
            .expect("ports is graph-backed sequence");

        assert_eq!(
            doc.node(ports).map(|node| node.kind),
            Some(NodeKind::BlockSequence)
        );
        assert_eq!(
            items
                .iter()
                .map(|item| doc.scalar_value(*item).expect("scalar value"))
                .collect::<Vec<_>>(),
            ["8080".to_owned(), "9090".to_owned()]
        );
    }

    #[test]
    fn graph_path_lookup_returns_semantic_node_with_cst_bridge() {
        let doc = YamlDoc::parse("server:\n  host: localhost\n").expect("valid nested mapping");
        let host_graph = doc
            .get_graph_path(&["server", "host"])
            .expect("graph path lookup succeeds")
            .expect("host graph node exists");
        let host_cst = doc.graph_node_cst(host_graph).expect("host has CST link");

        assert_eq!(
            doc.graph_node(host_graph).map(|node| &node.kind),
            Some(&GraphKind::Scalar {
                style: YamlScalarStyle::Plain,
                value: "localhost".to_owned(),
                tag: None,
                anchor: None,
            })
        );
        assert_eq!(
            doc.get_path(&["server", "host"])
                .expect("CST path lookup succeeds"),
            Some(host_cst)
        );
    }

    #[test]
    fn graph_builds_flow_mapping_and_sequence() {
        let doc = YamlDoc::parse("settings: {a: [b, c]}\n").expect("valid flow collections");
        let settings = doc
            .get_path(&["settings"])
            .expect("path lookup succeeds")
            .expect("settings exists");
        let entries = doc
            .graph_mapping_entries(settings)
            .expect("settings is graph-backed mapping");

        assert_eq!(
            doc.node(settings).map(|node| node.kind),
            Some(NodeKind::FlowMapping)
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(doc.scalar_value(entries[0].0).expect("key reads"), "a");
        assert_eq!(
            doc.node(entries[0].1).map(|node| node.kind),
            Some(NodeKind::FlowSequence)
        );
    }

    #[test]
    fn graph_builds_literal_and_folded_scalars() {
        let doc = YamlDoc::parse("literal: |\n  one\nfolded: >\n  one\n  two\n")
            .expect("valid block scalars");
        let literal = doc
            .get_path(&["literal"])
            .expect("path lookup succeeds")
            .expect("literal exists");
        let folded = doc
            .get_path(&["folded"])
            .expect("path lookup succeeds")
            .expect("folded exists");

        assert_eq!(doc.scalar_value(literal).expect("literal reads"), "one\n");
        assert_eq!(doc.scalar_value(folded).expect("folded reads"), "one two\n");
    }

    #[test]
    fn parser_builds_block_mapping_and_sequence_cst() {
        let input = "host: localhost
ports:
  - 8080
  - 9090
";
        let doc = YamlDoc::parse(input).expect("parser MVP should accept block collections");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.nodes.first().map(|node| node.kind),
            Some(NodeKind::Stream)
        );
        assert_eq!(count_nodes(&doc, NodeKind::BlockMapping), 1);
        assert_eq!(count_nodes(&doc, NodeKind::MappingEntry), 2);
        assert_eq!(count_nodes(&doc, NodeKind::BlockSequence), 1);
        assert_eq!(count_nodes(&doc, NodeKind::SequenceEntry), 2);
        assert!(scalar_texts(&doc).contains(&"host"));
        assert!(scalar_texts(&doc).contains(&"localhost"));
        assert!(scalar_texts(&doc).contains(&"8080"));
        assert!(scalar_texts(&doc).contains(&"9090"));
    }

    #[test]
    fn parser_builds_root_literal_scalar_cst() {
        let input = "|\n  hello\n  world\n";
        let doc = YamlDoc::parse(input).expect("parser should accept root literal scalar");

        assert_eq!(doc.to_string(), input);
        assert_eq!(count_nodes(&doc, NodeKind::LiteralScalar), 1);
        let literal = literal_scalar(&doc).expect("literal scalar exists");
        assert_eq!(
            doc.source
                .slice(doc.node(literal).expect("node exists").span),
            input
        );
    }

    #[test]
    fn parser_builds_literal_scalar_mapping_value_cst() {
        let input = "message: |\n  hello\n  world\nnext: value\n";
        let doc = YamlDoc::parse(input).expect("parser should accept literal mapping value");
        let message = doc
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.node(message).map(|node| node.kind),
            Some(NodeKind::LiteralScalar)
        );
        assert_eq!(
            String::read_yaml(&doc, message).expect("literal reads"),
            "hello\nworld\n"
        );
        assert_eq!(
            doc.get_path(&["next"])
                .expect("lookup succeeds")
                .map(|node| doc.scalar_text(node).expect("scalar")),
            Some("value")
        );
    }

    #[test]
    fn yaml_value_reads_literal_scalar_chomping() {
        let strip = YamlDoc::parse("message: |-\n  hello\n\n").expect("valid strip literal");
        let keep = YamlDoc::parse("message: |+\n  hello\n\n").expect("valid keep literal");
        let strip_node = strip
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");
        let keep_node = keep
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");

        assert_eq!(
            String::read_yaml(&strip, strip_node).expect("strip reads"),
            "hello"
        );
        assert_eq!(
            String::read_yaml(&keep, keep_node).expect("keep reads"),
            "hello\n\n"
        );
    }

    #[test]
    fn parser_builds_root_folded_scalar_cst() {
        let input = ">\n  folded\n  line\n";
        let doc = YamlDoc::parse(input).expect("parser should accept root folded scalar");

        assert_eq!(doc.to_string(), input);
        assert_eq!(count_nodes(&doc, NodeKind::FoldedScalar), 1);
        let folded = folded_scalar(&doc).expect("folded scalar exists");
        assert_eq!(
            doc.source
                .slice(doc.node(folded).expect("node exists").span),
            input
        );
    }

    #[test]
    fn parser_builds_folded_scalar_mapping_value_cst() {
        let input = "message: >\n  folded\n  line\nnext: value\n";
        let doc = YamlDoc::parse(input).expect("parser should accept folded mapping value");
        let message = doc
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.node(message).map(|node| node.kind),
            Some(NodeKind::FoldedScalar)
        );
        assert_eq!(
            String::read_yaml(&doc, message).expect("folded reads"),
            "folded line\n"
        );
        assert_eq!(
            doc.get_path(&["next"])
                .expect("lookup succeeds")
                .map(|node| doc.scalar_text(node).expect("scalar")),
            Some("value")
        );
    }

    #[test]
    fn yaml_value_reads_folded_scalar_paragraphs_and_more_indented_lines() {
        let doc = YamlDoc::parse("message: >\n  folded\n  line\n\n    literal\n  tail\n")
            .expect("valid folded scalar");
        let message = doc
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");

        assert_eq!(
            String::read_yaml(&doc, message).expect("folded reads"),
            "folded line\n\n  literal\ntail\n"
        );
    }

    #[test]
    fn yaml_value_reads_folded_scalar_chomping() {
        let strip =
            YamlDoc::parse("message: >-\n  folded\n  line\n\n").expect("valid strip folded");
        let keep = YamlDoc::parse("message: >+\n  folded\n  line\n\n").expect("valid keep folded");
        let strip_node = strip
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");
        let keep_node = keep
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");

        assert_eq!(
            String::read_yaml(&strip, strip_node).expect("strip reads"),
            "folded line"
        );
        assert_eq!(
            String::read_yaml(&keep, keep_node).expect("keep reads"),
            "folded line\n\n"
        );
    }

    #[test]
    fn parser_builds_literal_scalar_inside_block_sequence_entry() {
        let input = "- |\n  hello\n  # content comment\n- next\n";
        let doc = YamlDoc::parse(input).expect("parser should accept literal sequence value");
        let literal = literal_scalar(&doc).expect("literal scalar exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            String::read_yaml(&doc, literal).expect("literal reads"),
            "hello\n# content comment\n"
        );
        assert_eq!(count_nodes(&doc, NodeKind::SequenceEntry), 2);
    }

    #[test]
    fn parser_builds_folded_scalar_inside_block_sequence_entry() {
        let input = "- >\n  folded\n  line\n- next\n";
        let doc = YamlDoc::parse(input).expect("parser should accept folded sequence value");
        let folded = folded_scalar(&doc).expect("folded scalar exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            String::read_yaml(&doc, folded).expect("folded reads"),
            "folded line\n"
        );
        assert_eq!(count_nodes(&doc, NodeKind::SequenceEntry), 2);
    }

    #[test]
    fn parser_reports_invalid_literal_scalar_headers() {
        for input in ["message: |bad\n", "message: |0\n", "message: |--\n"] {
            let error = YamlDoc::parse(input).expect_err("literal header should be rejected");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
            assert!(
                error
                    .diagnostic
                    .message
                    .starts_with("invalid block scalar header before")
            );
            assert!(!error.diagnostic.expected.is_empty());
            assert!(error.diagnostic.position.is_some());
        }
    }

    #[test]
    fn parser_reports_invalid_folded_scalar_headers() {
        for input in ["message: >bad\n", "message: >0\n", "message: >--\n"] {
            let error = YamlDoc::parse(input).expect_err("folded header should be rejected");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
            assert!(
                error
                    .diagnostic
                    .message
                    .starts_with("invalid block scalar header before")
            );
            assert!(!error.diagnostic.expected.is_empty());
            assert!(error.diagnostic.position.is_some());
        }
    }

    #[test]
    fn yaml_value_rejects_literal_scalar_writes_for_now() {
        let mut doc = YamlDoc::parse("message: |\n  hello\n").expect("valid literal scalar");
        let message = doc
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");

        let error = "updated"
            .to_owned()
            .write_yaml(&mut doc, Some(message))
            .expect_err("literal scalar writes are intentionally not implemented yet");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
        assert_eq!(
            error.diagnostic.message,
            "block scalar rewriting is not implemented yet"
        );
    }

    #[test]
    fn yaml_value_rejects_folded_scalar_writes_for_now() {
        let mut doc = YamlDoc::parse("message: >\n  hello\n").expect("valid folded scalar");
        let message = doc
            .get_path(&["message"])
            .expect("lookup succeeds")
            .expect("message exists");

        let error = "updated"
            .to_owned()
            .write_yaml(&mut doc, Some(message))
            .expect_err("folded scalar writes are intentionally not implemented yet");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
        assert_eq!(
            error.diagnostic.message,
            "block scalar rewriting is not implemented yet"
        );
    }

    #[test]
    fn parser_builds_flow_sequence_mapping_value_cst() {
        let input = "items: [a, b, c]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow sequence mapping value");
        let items = doc
            .get_path(&["items"])
            .expect("lookup succeeds")
            .expect("items exists");
        let sequence = doc.node(items).expect("items node exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(sequence.kind, NodeKind::FlowSequence);
        assert_eq!(sequence.span, Span::new(7, 16));
        assert_eq!(flow_sequence_scalar_texts(&doc, items), ["a", "b", "c"]);
    }

    #[test]
    fn parser_builds_flow_sequence_inside_block_sequence_entry() {
        let input = "- [one, two,]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow sequence entry value");
        let flow = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowSequence)
            .map(|(index, _)| NodeId(index as u32))
            .expect("flow sequence exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(flow_sequence_scalar_texts(&doc, flow), ["one", "two"]);
    }

    #[test]
    fn parser_builds_nested_root_flow_sequences() {
        let input = "[a, [b, c]]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept nested flow sequences");
        let flow_sequences: Vec<NodeId> = doc
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.kind == NodeKind::FlowSequence)
            .map(|(index, _)| NodeId(index as u32))
            .collect();

        assert_eq!(doc.to_string(), input);
        assert_eq!(flow_sequences.len(), 2);
        assert_eq!(flow_sequence_scalar_texts(&doc, flow_sequences[0]), ["a"]);
        assert_eq!(
            flow_sequence_scalar_texts(&doc, flow_sequences[1]),
            ["b", "c"]
        );
    }

    #[test]
    fn yaml_value_reads_flow_sequence_values() {
        let doc = YamlDoc::parse("items: [one, two]\n").expect("valid flow sequence mapping");
        let items = doc
            .get_path(&["items"])
            .expect("lookup succeeds")
            .expect("items exists");

        assert_eq!(
            Vec::<String>::read_yaml(&doc, items).expect("flow sequence reads"),
            ["one".to_owned(), "two".to_owned()]
        );
    }

    #[test]
    fn yaml_value_rejects_flow_sequence_writes_for_now() {
        let mut doc = YamlDoc::parse("items: [one, two]\n").expect("valid flow sequence mapping");
        let items = doc
            .get_path(&["items"])
            .expect("lookup succeeds")
            .expect("items exists");

        let error = vec!["three".to_owned()]
            .write_yaml(&mut doc, Some(items))
            .expect_err("flow sequence writes are intentionally not implemented yet");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
        assert_eq!(
            error.diagnostic.message,
            "flow sequence rewriting is not implemented yet"
        );
    }

    #[test]
    fn parser_builds_flow_mapping_mapping_value_cst() {
        let input = "settings: {a: b, c: d}\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow mapping value");
        let settings = doc
            .get_path(&["settings"])
            .expect("lookup succeeds")
            .expect("settings exists");
        let mapping = doc.node(settings).expect("settings node exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(mapping.kind, NodeKind::FlowMapping);
        assert_eq!(mapping.children.len(), 2);
        assert_eq!(
            flow_mapping_scalar_pairs(&doc, settings),
            [("a", "b"), ("c", "d")]
        );
    }

    #[test]
    fn parser_builds_flow_mapping_inside_block_sequence_entry() {
        let input = "- {a: b}\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow mapping entry value");
        let flow = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowMapping)
            .map(|(index, _)| NodeId(index as u32))
            .expect("flow mapping exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(flow_mapping_scalar_pairs(&doc, flow), [("a", "b")]);
    }

    #[test]
    fn parser_builds_nested_flow_mapping_collections() {
        let input = "{a: [b, c], nested: {d: e}}\n";
        let doc = YamlDoc::parse(input).expect("parser should accept nested flow collections");
        let flow_mappings = count_nodes(&doc, NodeKind::FlowMapping);
        let flow_sequences = count_nodes(&doc, NodeKind::FlowSequence);

        assert_eq!(doc.to_string(), input);
        assert_eq!(flow_mappings, 2);
        assert_eq!(flow_sequences, 1);
        assert!(scalar_texts(&doc).contains(&"a"));
        assert!(scalar_texts(&doc).contains(&"nested"));
        assert!(scalar_texts(&doc).contains(&"d"));
        assert!(scalar_texts(&doc).contains(&"e"));
    }

    #[test]
    fn yaml_value_reads_flow_mapping_values() {
        let doc = YamlDoc::parse("settings: {a: b, c: d}\n").expect("valid flow mapping");
        let settings = doc
            .get_path(&["settings"])
            .expect("lookup succeeds")
            .expect("settings exists");
        let values = std::collections::BTreeMap::<String, String>::read_yaml(&doc, settings)
            .expect("flow mapping reads");

        assert_eq!(values.get("a").map(String::as_str), Some("b"));
        assert_eq!(values.get("c").map(String::as_str), Some("d"));
    }

    #[test]
    fn yaml_value_rejects_flow_mapping_writes_for_now() {
        let mut doc = YamlDoc::parse("settings: {a: b}\n").expect("valid flow mapping");
        let settings = doc
            .get_path(&["settings"])
            .expect("lookup succeeds")
            .expect("settings exists");
        let values = std::collections::BTreeMap::from([("a".to_owned(), "updated".to_owned())]);

        let error = values
            .write_yaml(&mut doc, Some(settings))
            .expect_err("flow mapping writes are intentionally not implemented yet");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
        assert_eq!(
            error.diagnostic.message,
            "flow mapping rewriting is not implemented yet"
        );
    }

    #[test]
    fn parser_reports_malformed_flow_mappings() {
        for (input, message) in [
            ("settings: {a: b\n", "missing flow mapping closing brace"),
            ("settings: {, a: b}\n", "unexpected comma in flow mapping"),
            (
                "settings: {a: b, , c: d}\n",
                "unexpected comma in flow mapping",
            ),
            (
                "settings: {a b}\n",
                "missing colon after flow mapping key before `}`",
            ),
            ("{a: b} }\n", "unexpected token `}` after flow collection"),
            (
                "settings: {a: # nope}\n",
                "comments inside flow collections are not supported yet",
            ),
        ] {
            let error = YamlDoc::parse(input).expect_err("input should be rejected");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
            assert_eq!(error.diagnostic.message, message);
            assert!(
                !error.diagnostic.expected.is_empty(),
                "{input:?} should report expected items"
            );
            assert!(
                error.diagnostic.position.is_some(),
                "{input:?} should include source position"
            );
        }
    }

    #[test]
    fn parser_reports_malformed_flow_sequences() {
        for (input, message) in [
            ("items: [a, b\n", "missing flow sequence closing bracket"),
            ("items: [a, , b]\n", "unexpected comma in flow sequence"),
            ("items: [, a]\n", "unexpected comma in flow sequence"),
            ("[a] ]\n", "unexpected token `]` after flow collection"),
            (
                "items: [a, # nope]\n",
                "comments inside flow collections are not supported yet",
            ),
        ] {
            let error = YamlDoc::parse(input).expect_err("input should be rejected");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
            assert_eq!(error.diagnostic.message, message);
            assert!(
                !error.diagnostic.expected.is_empty(),
                "{input:?} should report expected items"
            );
            assert!(
                error.diagnostic.position.is_some(),
                "{input:?} should include source position"
            );
        }
    }

    #[test]
    fn parser_reports_tabs_in_indentation() {
        let error = YamlDoc::parse(
            "	key: value
",
        )
        .expect_err("tabs are invalid indentation");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.span, Span::new(0, 1));
        assert_eq!(
            error.diagnostic.expected,
            ["spaces for indentation".to_owned()]
        );
    }

    #[test]
    fn parser_reports_invalid_indentation_without_parent() {
        let error = YamlDoc::parse(
            "  key: value
",
        )
        .expect_err("indented root line has no parent");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.span, Span::new(0, 2));
        assert_eq!(
            error.diagnostic.position,
            Some(LineCol { line: 1, column: 1 })
        );
        assert!(error.to_string().contains("invalid indentation"));
    }

    #[test]
    fn parser_reports_unexpected_line_start_tokens() {
        let error = YamlDoc::parse(
            ": value
",
        )
        .expect_err("colon cannot start an MVP line");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.span, Span::new(0, 1));
        assert_eq!(
            error.diagnostic.position,
            Some(LineCol { line: 1, column: 1 })
        );
        assert_eq!(
            error.diagnostic.expected,
            ["mapping entry, sequence entry, or scalar".to_owned()]
        );
    }

    #[test]
    fn parser_reports_missing_mapping_values() {
        let error = YamlDoc::parse(
            "key:
other: value
",
        )
        .expect_err("key has no value");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.span, Span::new(3, 4));
        assert_eq!(
            error.diagnostic.position,
            Some(LineCol { line: 1, column: 4 })
        );
        assert_eq!(error.diagnostic.message, "missing mapping value");
    }

    #[test]
    fn parser_reports_invalid_document_markers() {
        let error = YamlDoc::parse(
            "---bad
",
        )
        .expect_err("document marker needs separation");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.span, Span::new(0, 6));
        assert_eq!(
            error.diagnostic.position,
            Some(LineCol { line: 1, column: 1 })
        );
        assert_eq!(error.diagnostic.message, "invalid document marker");
    }

    #[test]
    fn semantic_lookup_reads_root_mapping_values() {
        let input = "host: localhost
port: 8080
";
        let doc = YamlDoc::parse(input).expect("valid MVP mapping");
        let root = doc.root_mapping().expect("root mapping exists");
        let host = doc
            .get_mapping_value(root, "host")
            .expect("lookup succeeds")
            .expect("host exists");
        let port = doc
            .get_path(&["port"])
            .expect("lookup succeeds")
            .expect("port exists");

        assert_eq!(doc.scalar_text(host).expect("host is scalar"), "localhost");
        assert_eq!(doc.scalar_text(port).expect("port is scalar"), "8080");
        assert_eq!(doc.get_mapping_value(root, "missing"), Ok(None));
    }

    #[test]
    fn semantic_lookup_follows_nested_block_mappings() {
        let input = "server:
  host: localhost
  port: 8080
";
        let doc = YamlDoc::parse(input).expect("valid nested MVP mapping");
        let host = doc
            .get_path(&["server", "host"])
            .expect("lookup succeeds")
            .expect("nested host exists");
        let port = doc
            .get_path(&["server", "port"])
            .expect("lookup succeeds")
            .expect("nested port exists");

        assert_eq!(doc.scalar_text(host).expect("host is scalar"), "localhost");
        assert_eq!(doc.scalar_text(port).expect("port is scalar"), "8080");
        assert_eq!(doc.get_path(&["server", "missing"]), Ok(None));
    }

    #[test]
    fn semantic_lookup_can_return_nested_sequences() {
        let input = "ports:
  - 8080
  - 9090
";
        let doc = YamlDoc::parse(input).expect("valid nested MVP sequence");
        let ports = doc
            .get_path(&["ports"])
            .expect("lookup succeeds")
            .expect("ports exists");

        assert_eq!(
            doc.node(ports).map(|node| node.kind),
            Some(NodeKind::BlockSequence)
        );
    }

    #[test]
    fn patch_writer_replaces_scalar_node_text() {
        let mut doc = YamlDoc::parse(
            "host: localhost
port: 8080
",
        )
        .expect("valid MVP mapping");
        let port = doc
            .get_path(&["port"])
            .expect("lookup succeeds")
            .expect("port exists");

        doc.replace_node_text(port, "9090")
            .expect("replacement edit queues");

        assert_eq!(
            doc.to_string(),
            "host: localhost
port: 9090
"
        );
        assert_eq!(doc.scalar_text(port).expect("CST is unchanged"), "8080");
    }

    #[test]
    fn patch_writer_inserts_mapping_entry_with_inherited_style() {
        let mut doc = YamlDoc::parse(
            "server:
  host: localhost
other: keep
",
        )
        .expect("valid nested MVP mapping");
        let server = doc
            .get_path(&["server"])
            .expect("lookup succeeds")
            .expect("server mapping exists");

        doc.insert_mapping_entry(server, "port", "8080", MappingEntryStyle::Inherit)
            .expect("insert edit queues");

        assert_eq!(
            doc.to_string(),
            "server:
  host: localhost
  port: 8080
other: keep
"
        );
    }

    #[test]
    fn patch_writer_inserts_after_final_line_without_line_break() {
        let mut doc = YamlDoc::parse("host: localhost").expect("valid MVP mapping");
        let root = doc.root_mapping().expect("root mapping exists");

        doc.insert_mapping_entry(root, "port", "8080", MappingEntryStyle::default())
            .expect("insert edit queues");

        assert_eq!(
            doc.to_string(),
            "host: localhost
port: 8080
"
        );
    }

    #[test]
    fn patch_writer_removes_mapping_entry_line() {
        let mut doc = YamlDoc::parse(
            "host: localhost
port: 8080
extra: keep
",
        )
        .expect("valid MVP mapping");
        let port_entry = mapping_entry_by_key(&doc, "port").expect("port entry exists");

        doc.remove_node(port_entry).expect("remove edit queues");

        assert_eq!(
            doc.to_string(),
            "host: localhost
extra: keep
"
        );
    }

    #[test]
    fn patch_writer_retains_only_allowed_mapping_entries() {
        let mut doc = YamlDoc::parse(
            "host: localhost
port: 8080
extra: remove
debug: false
",
        )
        .expect("valid MVP mapping");
        let root = doc.root_mapping().expect("root mapping exists");

        doc.retain_mapping_entries(root, &["host", "debug"])
            .expect("retain edits queue");

        assert_eq!(
            doc.to_string(),
            "host: localhost
debug: false
"
        );
    }

    #[test]
    fn set_scalar_preserves_double_quoted_style_and_inline_comment() {
        let mut doc = YamlDoc::parse("# leading comment\nname: \"old\" # keep me\n")
            .expect("valid MVP mapping");

        doc.set_scalar(&["name"], "new \"value\"")
            .expect("scalar replacement queues");

        assert_eq!(
            doc.to_string(),
            "# leading comment\nname: \"new \\\"value\\\"\" # keep me\n"
        );
    }

    #[test]
    fn set_scalar_preserves_single_quoted_style() {
        let mut doc = YamlDoc::parse("name: 'old'\n").expect("valid MVP mapping");

        doc.set_scalar(&["name"], "Bob's")
            .expect("scalar replacement queues");

        assert_eq!(doc.to_string(), "name: 'Bob''s'\n");
    }

    #[test]
    fn set_scalar_preserves_plain_style() {
        let mut doc = YamlDoc::parse("port: 8080\n").expect("valid MVP mapping");

        doc.set_scalar(&["port"], "9090")
            .expect("scalar replacement queues");

        assert_eq!(doc.to_string(), "port: 9090\n");
    }

    #[test]
    fn set_scalar_preserves_plain_inline_comment() {
        let mut doc = YamlDoc::parse("port: 8080 # chosen port\n").expect("valid MVP mapping");

        doc.set_scalar(&["port"], "9090")
            .expect("scalar replacement queues");

        assert_eq!(doc.to_string(), "port: 9090 # chosen port\n");
    }

    #[test]
    fn set_scalar_rejects_plain_replacement_that_would_change_style() {
        let mut doc = YamlDoc::parse("name: old\n").expect("valid MVP mapping");

        let error = doc
            .set_scalar(&["name"], "new value # comment-like")
            .expect_err("plain style cannot safely preserve this value");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
        assert_eq!(
            error.diagnostic.message,
            "plain scalar replacement cannot preserve plain style"
        );
    }

    #[test]
    fn patch_writer_rejects_overlapping_edits() {
        let mut doc = YamlDoc::parse(
            "host: localhost
",
        )
        .expect("valid MVP mapping");
        let host = doc
            .get_path(&["host"])
            .expect("lookup succeeds")
            .expect("host exists");

        doc.replace_node_text(host, "example.com")
            .expect("first replacement queues");
        let error = doc
            .replace_node_text(host, "localhost.local")
            .expect_err("same span overlaps pending edit");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
        assert_eq!(
            error.diagnostic.message,
            "edit overlaps an existing pending edit"
        );
    }

    fn mapping_entry_by_key(doc: &YamlDoc, key: &str) -> Option<NodeId> {
        let root = doc.root_mapping().ok()?;
        let mapping = doc.node(root)?;
        mapping.children.iter().copied().find(|entry| {
            let Some(entry_node) = doc.node(*entry) else {
                return false;
            };
            let Some(key_node) = entry_node.children.first().copied() else {
                return false;
            };
            doc.scalar_text(key_node) == Ok(key)
        })
    }

    fn count_nodes(doc: &YamlDoc, kind: NodeKind) -> usize {
        doc.nodes.iter().filter(|node| node.kind == kind).count()
    }

    fn scalar_texts(doc: &YamlDoc) -> Vec<&str> {
        doc.nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Scalar)
            .map(|node| doc.source.slice(node.span))
            .collect()
    }

    fn literal_scalar(doc: &YamlDoc) -> Option<NodeId> {
        doc.nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::LiteralScalar)
            .map(|(index, _)| NodeId(index as u32))
    }

    fn folded_scalar(doc: &YamlDoc) -> Option<NodeId> {
        doc.nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FoldedScalar)
            .map(|(index, _)| NodeId(index as u32))
    }

    fn flow_sequence_scalar_texts(doc: &YamlDoc, sequence: NodeId) -> Vec<&str> {
        doc.node(sequence)
            .expect("sequence exists")
            .children
            .iter()
            .filter_map(|child| {
                let child = doc.node(*child)?;
                (child.kind == NodeKind::Scalar).then(|| doc.source.slice(child.span))
            })
            .collect()
    }

    fn flow_mapping_scalar_pairs(doc: &YamlDoc, mapping: NodeId) -> Vec<(&str, &str)> {
        doc.node(mapping)
            .expect("mapping exists")
            .children
            .iter()
            .filter_map(|entry| {
                let entry = doc.node(*entry)?;
                let key = entry.children.first().copied()?;
                let value = entry.children.get(1).copied()?;
                Some((doc.scalar_text(key).ok()?, doc.scalar_text(value).ok()?))
            })
            .collect()
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Config {
        host: String,
        port: u16,
        debug: bool,
    }

    impl FromYamlDoc for Config {
        fn from_yaml_doc(doc: &YamlDoc) -> Result<Self, YamlError> {
            let host = doc.get_path(&["host"])?.ok_or_else(|| {
                YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Typed,
                        "missing required field `host`",
                        Span::empty(0),
                    )
                    .with_expected("host"),
                )
            })?;
            let port = doc.get_path(&["port"])?;
            let debug = doc.get_path(&["debug"])?;

            Ok(Self {
                host: String::read_yaml(doc, host)?,
                port: match port {
                    Some(node) => u16::read_yaml(doc, node)?,
                    None => 8080,
                },
                debug: match debug {
                    Some(node) => bool::read_yaml(doc, node)?,
                    None => false,
                },
            })
        }
    }

    impl ToYamlDoc for Config {
        fn apply_to_yaml_doc(&self, doc: &mut YamlDoc) -> Result<(), YamlError> {
            let root = doc.root_mapping()?;

            if let Some(host) = doc.get_path(&["host"])? {
                self.host.write_yaml(doc, Some(host))?;
            } else {
                doc.insert_mapping_entry(root, "host", &self.host, MappingEntryStyle::default())?;
            }

            if let Some(port) = doc.get_path(&["port"])? {
                self.port.write_yaml(doc, Some(port))?;
            } else {
                doc.insert_mapping_entry(
                    root,
                    "port",
                    &self.port.to_string(),
                    MappingEntryStyle::default(),
                )?;
            }

            if let Some(debug) = doc.get_path(&["debug"])? {
                self.debug.write_yaml(doc, Some(debug))?;
            } else {
                doc.insert_mapping_entry(
                    root,
                    "debug",
                    if self.debug { "true" } else { "false" },
                    MappingEntryStyle::default(),
                )?;
            }

            Ok(())
        }
    }

    #[test]
    fn yaml_value_reads_and_writes_scalar_values() {
        let mut doc = YamlDoc::parse("name: \"old\"\nenabled: false\nport: 3000\n")
            .expect("valid MVP mapping");
        let name = doc
            .get_path(&["name"])
            .expect("lookup succeeds")
            .expect("name exists");
        let enabled = doc
            .get_path(&["enabled"])
            .expect("lookup succeeds")
            .expect("enabled exists");
        let port = doc
            .get_path(&["port"])
            .expect("lookup succeeds")
            .expect("port exists");

        assert_eq!(String::read_yaml(&doc, name).expect("string reads"), "old");
        assert!(!bool::read_yaml(&doc, enabled).expect("bool reads"));
        assert_eq!(u16::read_yaml(&doc, port).expect("u16 reads"), 3000);

        true.write_yaml(&mut doc, Some(enabled))
            .expect("bool writes");
        9090_u16
            .write_yaml(&mut doc, Some(port))
            .expect("u16 writes");

        assert_eq!(
            doc.to_string(),
            "name: \"old\"\nenabled: true\nport: 9090\n"
        );
    }

    #[test]
    fn yaml_value_reads_and_writes_option_values() {
        let mut doc = YamlDoc::parse(
            "name: old
maybe: value
keep: yes
",
        )
        .expect("valid MVP mapping");
        let name = doc
            .get_path(&["name"])
            .expect("lookup succeeds")
            .expect("name exists");
        let maybe = doc
            .get_path(&["maybe"])
            .expect("lookup succeeds")
            .expect("maybe exists");

        assert_eq!(
            Option::<String>::read_yaml(&doc, name).expect("option reads"),
            Some("old".to_owned())
        );

        Option::<String>::None
            .write_yaml(&mut doc, Some(maybe))
            .expect("none removes containing entry");

        assert_eq!(
            doc.to_string(),
            "name: old
keep: yes
"
        );
    }

    #[test]
    fn yaml_value_reads_and_writes_vec_values() {
        let mut doc = YamlDoc::parse(
            "ports:
  - 8080
  - 9090
",
        )
        .expect("valid MVP sequence");
        let ports = doc
            .get_path(&["ports"])
            .expect("lookup succeeds")
            .expect("ports exists");

        assert_eq!(
            Vec::<u16>::read_yaml(&doc, ports).expect("vec reads"),
            vec![8080, 9090]
        );

        vec![3000_u16, 3001]
            .write_yaml(&mut doc, Some(ports))
            .expect("vec writes existing sequence");

        assert_eq!(
            doc.to_string(),
            "ports:
  - 3000
  - 3001
"
        );
    }

    #[test]
    fn yaml_value_reads_and_writes_btree_map_values() {
        let mut doc = YamlDoc::parse(
            "limits:
  low: 1
  high: 5
",
        )
        .expect("valid MVP mapping");
        let limits = doc
            .get_path(&["limits"])
            .expect("lookup succeeds")
            .expect("limits exists");

        let values =
            std::collections::BTreeMap::<String, u16>::read_yaml(&doc, limits).expect("map reads");
        assert_eq!(values.get("low"), Some(&1));
        assert_eq!(values.get("high"), Some(&5));

        let mut replacement = std::collections::BTreeMap::new();
        replacement.insert("high".to_owned(), 7_u16);
        replacement.insert("low".to_owned(), 2_u16);
        replacement
            .write_yaml(&mut doc, Some(limits))
            .expect("map writes existing mapping");

        assert_eq!(
            doc.to_string(),
            "limits:
  high: 7
  low: 2
"
        );
    }

    #[test]
    fn manual_typed_config_overlay_preserves_unknown_fields_and_style() {
        let mut doc = YamlDoc::parse(
            "# main server\nhost: \"localhost\"\n\n# chosen port\nport: 3000\n\nextra: keep-me\n",
        )
        .expect("valid MVP mapping");
        let mut config = Config::from_yaml_doc(&doc).expect("manual overlay reads");

        assert_eq!(
            config,
            Config {
                host: "localhost".to_owned(),
                port: 3000,
                debug: false,
            }
        );

        config.port = 9090;
        config.debug = true;
        config
            .apply_to_yaml_doc(&mut doc)
            .expect("manual overlay writes");

        assert_eq!(
            doc.to_string(),
            "# main server\nhost: \"localhost\"\n\n# chosen port\nport: 9090\n\nextra: keep-me\ndebug: true\n"
        );
    }

    #[test]
    fn yaml_value_reports_typed_parse_errors() {
        let doc = YamlDoc::parse("port: nope\n").expect("valid MVP mapping");
        let port = doc
            .get_path(&["port"])
            .expect("lookup succeeds")
            .expect("port exists");

        let error = u16::read_yaml(&doc, port).expect_err("not a u16");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Typed);
        assert_eq!(
            error.diagnostic.position,
            Some(LineCol { line: 1, column: 7 })
        );
    }

    #[test]
    fn diagnostics_render_expected_items_and_notes() {
        let diagnostic =
            Diagnostic::new(DiagnosticKind::Parser, "unexpected token", Span::empty(0))
                .with_expected("mapping value")
                .with_expected("sequence entry")
                .with_note("while parsing a block collection");

        assert_eq!(
            diagnostic.to_string(),
            "Parser: unexpected token (expected: mapping value, sequence entry)\nnote: while parsing a block collection"
        );
    }
}
