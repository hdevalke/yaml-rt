//! Core types for a YAML 1.2.2 round-trip parser.
//!
//! This crate is intentionally dependency-free. The first implementation keeps
//! the source text intact while the source model, lexer, CST parser, semantic
//! graph, editor, and patch emitter are built out according to the workspace
//! roadmap.

use std::{collections::BTreeMap, fmt};

/// YAML version targeted by this workspace.
pub const TARGET_YAML_VERSION: &str = "1.2.2";

/// Identifier for a node stored inside a [`YamlDoc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

impl NodeId {
    /// Creates a node ID from a vector index.
    ///
    /// # Panics
    ///
    /// Panics when `index` cannot fit in the u32-backed node ID.
    #[must_use]
    pub fn from_usize(index: usize) -> Self {
        Self(u32::try_from(index).expect("node arena is too large for u32-based node IDs"))
    }

    /// Returns this node ID as a vector index.
    #[must_use]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

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

    /// Creates a span from usize byte offsets.
    ///
    /// # Panics
    ///
    /// Panics when either offset cannot fit in the u32-backed span.
    #[must_use]
    pub fn from_usize(start: usize, end: usize) -> Self {
        Self::try_from((start, end)).expect("YAML source is too large for u32-based spans")
    }

    /// Returns an empty span at `offset`.
    #[must_use]
    pub const fn empty(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    fn usize_to_u32(offset: usize) -> u32 {
        u32::try_from(offset).expect("YAML source is too large for u32-based spans")
    }

    fn offset_from_usize(base: u32, offset: usize) -> u32 {
        base.checked_add(Self::usize_to_u32(offset))
            .expect("YAML source is too large for u32-based spans")
    }

    /// Returns an empty span at `offset`.
    #[must_use]
    pub fn empty_from_usize(offset: usize) -> Self {
        Self::empty(Self::usize_to_u32(offset))
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

impl TryFrom<(usize, usize)> for Span {
    type Error = std::num::TryFromIntError;

    fn try_from((start, end): (usize, usize)) -> Result<Self, Self::Error> {
        Ok(Self {
            start: Self::usize_to_u32(start),
            end: Self::usize_to_u32(end),
        })
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
    ///
    /// # Errors
    ///
    /// Returns an error when `text` contains characters that are not valid in a
    /// YAML 1.2.2 stream.
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
    ///
    /// # Errors
    ///
    /// Returns an error when `span` is outside the source text or does not fall
    /// on UTF-8 boundaries.
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
            let span = Span::from_usize(offset, offset + character.len_utf8());
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
        0x09 | 0x0A | 0x0D | 0x20..=0x7E | 0x85 | 0xA0..=0xD7FF | 0xE000..=0xFFFD | 0x001_0000..=0x0010_FFFF
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
///
/// # Errors
///
/// Returns an error when the source contains malformed quoted scalars or other
/// token-level syntax that the lexer can diagnose.
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
            span: Span::from_usize(start, self.position),
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
                Span::from_usize(start, self.source.len()),
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
    /// YAML directive line.
    Directive,
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
    DocumentEnd {
        /// Whether the source used an explicit `...` marker.
        explicit: bool,
    },
    /// Start of a sequence node.
    SequenceStart {
        /// Sequence spelling style.
        style: CollectionStyle,
        /// Explicit tag, when present.
        tag: Option<String>,
        /// Explicit anchor, when present.
        anchor: Option<String>,
    },
    /// End of a sequence node.
    SequenceEnd,
    /// Start of a mapping node.
    MappingStart {
        /// Mapping spelling style.
        style: CollectionStyle,
        /// Explicit tag, when present.
        tag: Option<String>,
        /// Explicit anchor, when present.
        anchor: Option<String>,
    },
    /// End of a mapping node.
    MappingEnd,
    /// Scalar node with decoded content.
    Scalar {
        /// Scalar spelling style.
        style: YamlScalarStyle,
        /// Decoded scalar value.
        value: String,
        /// Explicit tag, when present.
        tag: Option<String>,
        /// Explicit anchor, when present.
        anchor: Option<String>,
    },
    /// Alias node.
    Alias {
        /// Alias name without the leading `*`.
        name: String,
    },
}

/// Parses the MVP token/source pair into a lossless CST node arena.
///
/// # Errors
///
/// Returns an error when the token stream contains YAML syntax the parser
/// cannot accept or when parser events cannot be produced from the CST.
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

struct Parser<'source> {
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
    fn new(source: &'source Source, _tokens: &'source [Token]) -> Self {
        Self {
            source,
            nodes: Vec::new(),
            events: Vec::new(),
            stream: None,
            document: None,
            document_has_content: false,
            document_was_explicitly_opened: false,
            document_yaml_directive_seen: false,
            tag_handles: default_tag_handles(),
            mappings: Vec::new(),
            sequences: Vec::new(),
            event_collections: Vec::new(),
            pending_node_properties: Vec::new(),
            block_scalar_content_indents: BTreeMap::new(),
        }
    }

    fn parse(mut self) -> Result<ParsedYaml, YamlError> {
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
            self.parse_mapping_entry(document, lines, index, indent, body, colon_byte)
        } else if body_starts_flow_value(body, absolute_start)? {
            let (flow_text, consumed) = self.flow_value_text(lines, index, absolute_start, body)?;
            let (node, end) = self.parse_flow_value(flow_text, absolute_start)?;
            reject_trailing_flow_content(flow_text, end, absolute_start)?;
            self.nodes[document.0 as usize].children.push(node);
            self.emit_node_event(node)?;
            Ok(consumed)
        } else if body.starts_with('"') {
            if let Some(colon_byte) = find_mapping_colon(body) {
                self.parse_mapping_entry(document, lines, index, indent, body, colon_byte)
            } else {
                let (node, consumed) =
                    self.parse_quoted_scalar_lines(lines, index, absolute_start, '"')?;
                self.nodes[document.0 as usize].children.push(node);
                self.emit_scalar_event(node)?;
                Ok(consumed)
            }
        } else if let Some(colon_byte) = find_mapping_colon(body) {
            self.parse_mapping_entry(document, lines, index, indent, body, colon_byte)
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
        for line in &lines[index + 1..] {
            if line.content_start < absolute_end {
                if validate_sequence_indent {
                    reject_invalid_flow_continuation_indent(line, flow_indent)?;
                }
                consumed += 1;
                validation_end = line.content_end;
            } else {
                break;
            }
        }
        Ok((
            &self.source.as_str()[absolute_start..validation_end],
            consumed,
        ))
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

        let key_start = line.content_start + indent;
        let key_text = body[..colon_byte].trim_end();
        let key_end = key_start + key_text.len();
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
            let value_start = line.content_start + indent + colon_byte + 1 + leading;
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
            let key = self.parse_inline_value(key_text, key_start)?;
            self.nodes[entry.0 as usize].children.push(key);
            self.emit_node_event(key)?;
            Ok(1)
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
        self.push_event(YamlEventKind::DocumentStart { explicit }, span);
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
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Parser,
                    "directives must appear before document content",
                    Span::from_usize(line.content_start, line.content_end),
                )
                .with_expected("a directive before the document start marker or content"),
            )
            .with_position_from(self.source));
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
        let value = after_dash.trim_start();
        if value.is_empty() {
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
        self.push_event(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Block,
                tag: None,
                anchor: None,
            },
            Span::from_usize(absolute_start, line.content_end),
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
        self.push_event(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Block,
                tag: None,
                anchor: None,
            },
            Span::from_usize(absolute_start, absolute_start + body.len()),
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

    fn parse_inline_value(
        &mut self,
        text: &str,
        absolute_start: usize,
    ) -> Result<NodeId, YamlError> {
        let properties = parse_node_properties(
            text,
            Span::from_usize(absolute_start, absolute_start + text.len()),
        )?;
        let value_text = &text[properties.value_start..];
        if value_text.starts_with('[') || value_text.starts_with('{') {
            let (node, end) = self.parse_flow_value(text, absolute_start)?;
            reject_trailing_flow_content(text, end, absolute_start)?;
            Ok(node)
        } else {
            Ok(self.push_node(
                NodeKind::Scalar,
                Span::from_usize(absolute_start, absolute_start + text.len()),
            ))
        }
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

        for line in &lines[index + 1..] {
            let trimmed = line.content_without_break.trim();
            if trimmed == "---" || trimmed == "..." {
                break;
            }

            if trimmed.is_empty() {
                if content_indent == usize::MAX && inline_header {
                    pending_blank_lines += 1;
                    continue;
                }
                consumed += 1;
                end = line.line_end;
                continue;
            }

            let indent = count_literal_content_indent(line.content_without_break);
            if content_indent == usize::MAX {
                if indent <= parent_indent && (parent_indent > 0 || inline_header) {
                    break;
                }
                content_indent = indent;
            }

            if indent < content_indent {
                break;
            }

            if pending_blank_lines > 0 {
                consumed += pending_blank_lines;
                pending_blank_lines = 0;
            }
            consumed += 1;
            end = line.line_end;
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

            position = skip_flow_whitespace(text, position);
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
            position = skip_flow_whitespace(text, position);
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
        let key_end = key_separator.unwrap_or_else(|| {
            flow_scalar_end(text, position, absolute_start, &[',', '}'])
                .expect("separator scan already validates flow scalar content")
        });
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
            );
            mapping
        }
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
            children: Vec::new(),
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
        self.push_event(
            YamlEventKind::SequenceStart {
                style: CollectionStyle::Flow,
                tag: properties.tag,
                anchor: properties.anchor,
            },
            span,
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
        self.push_event(
            YamlEventKind::MappingStart {
                style: CollectionStyle::Flow,
                tag: properties.tag,
                anchor: properties.anchor,
            },
            span,
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
        let node = self.nodes[node.0 as usize].clone();
        let text = self.source.slice(node.span);
        let mut properties = parse_node_properties(text, node.span)?;
        self.resolve_node_properties(&mut properties, node.span)?;
        let span = if let Some(pending) =
            self.take_pending_node_properties(self.source_indent_at(node.span.start as usize))
        {
            if properties.anchor.is_none() {
                properties.anchor = pending.properties.anchor;
            }
            if properties.tag.is_none() {
                properties.tag = pending.properties.tag;
            }
            Span::new(Span::usize_to_u32(pending.span_start), node.span.end)
        } else {
            node.span
        };
        let value_text = &text[properties.value_start..];
        let trimmed = strip_inline_comment(value_text).trim();
        if let Some(alias) = trimmed.strip_prefix('*')
            && !alias.is_empty()
            && !alias.chars().any(char::is_whitespace)
        {
            self.push_event(
                YamlEventKind::Alias {
                    name: alias.to_owned(),
                },
                span,
            );
            return Ok(());
        }

        let style = match node.kind {
            NodeKind::LiteralScalar => YamlScalarStyle::Literal,
            NodeKind::FoldedScalar => YamlScalarStyle::Folded,
            NodeKind::Scalar if value_text.starts_with('"') => YamlScalarStyle::DoubleQuoted,
            NodeKind::Scalar if value_text.starts_with('\'') => YamlScalarStyle::SingleQuoted,
            NodeKind::Scalar => YamlScalarStyle::Plain,
            _ => unreachable!("emit_scalar_event only receives scalar nodes"),
        };
        let value = if matches!(node.kind, NodeKind::LiteralScalar | NodeKind::FoldedScalar) {
            decode_scalar_value_with_content_indent(
                value_text,
                self.block_scalar_content_indents.get(&node_id).copied(),
            )?
        } else {
            decode_scalar_value(value_text)?
        };
        self.push_event(
            YamlEventKind::Scalar {
                style,
                value,
                tag: properties.tag,
                anchor: properties.anchor,
            },
            span,
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
struct NodeProperties {
    tag: Option<String>,
    anchor: Option<String>,
    value_start: usize,
}

fn body_starts_flow_value(body: &str, absolute_start: usize) -> Result<bool, YamlError> {
    let properties = parse_node_properties(
        body,
        Span::from_usize(absolute_start, absolute_start + body.len()),
    )?;
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

fn default_tag_handles() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("!".to_owned(), "!".to_owned()),
        ("!!".to_owned(), "tag:yaml.org,2002:".to_owned()),
    ])
}

fn resolve_tag(
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

fn parse_node_properties(text: &str, span: Span) -> Result<NodeProperties, YamlError> {
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

        if text[position..]
            .chars()
            .next()
            .is_some_and(|next| !next.is_whitespace())
        {
            properties.value_start = position;
            return Ok(properties);
        }
    }
}

fn skip_property_whitespace(text: &str, mut position: usize) -> usize {
    while let Some(character) = text[position..].chars().next() {
        if character == ' ' || character == '\t' {
            position += character.len_utf8();
        } else {
            break;
        }
    }
    position
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
        if hex_end > suffix.len()
            || !suffix[hex_start..hex_end]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Parser,
                    "malformed tag URI escape",
                    Span::empty(Span::offset_from_usize(span.start, position)),
                )
                .with_expected("two hexadecimal digits after `%`"),
            ));
        }
        let byte =
            u8::from_str_radix(&suffix[hex_start..hex_end], 16).expect("hex digits were validated");
        output.push(char::from(byte));
        position = hex_end;
    }
    Ok(output)
}

fn property_token_end(text: &str, mut position: usize) -> usize {
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

fn document_marker_rest<'text>(body: &'text str, marker: &str) -> Option<&'text str> {
    let rest = body.strip_prefix(marker)?;
    if rest.chars().next().is_none_or(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

fn strip_inline_comment(text: &str) -> &str {
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

fn valid_yaml_directive_version_syntax(version: &str) -> bool {
    let Some((major, minor)) = version.split_once('.') else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.chars().all(|character| character.is_ascii_digit())
        && minor.chars().all(|character| character.is_ascii_digit())
}

fn validate_tag_handle(handle: &str, line: SourceLine<'_>) -> Result<(), YamlError> {
    let valid = handle == "!"
        || handle == "!!"
        || (handle.starts_with('!')
            && handle.ends_with('!')
            && handle.len() > 2
            && handle[1..handle.len() - 1].chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '-' || character == '_'
            }));
    if valid {
        Ok(())
    } else {
        Err(invalid_directive(line, "invalid TAG directive handle"))
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
            Span::from_usize(offset, offset + found.len_utf8()),
        )
        .with_expected("|, >, chomping indicator, or a one-digit indentation indicator"),
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
    if indent > flow_indent || line.content_without_break.as_bytes().get(indent) == Some(&b'\t') {
        return Ok(());
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
            '#' if is_flow_comment_start(text, position) => return Ok(position),
            '"' => position = double_quoted_flow_end(text, position, absolute_start)?,
            '\'' => position = single_quoted_flow_end(text, position, absolute_start)?,
            _ => position += character.len_utf8(),
        }
    }

    Ok(position)
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

fn find_mapping_colon(body: &str) -> Option<usize> {
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
        } else if character == '"' {
            in_double = true;
        } else if character == '\'' {
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
    decode_scalar_value_with_content_indent(text, None)
}

fn decode_scalar_value_with_content_indent(
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
        let continued = strip_double_quoted_line_continuations(&text[1..end - 1]);
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

fn strip_double_quoted_line_continuations(text: &str) -> String {
    let mut output = String::new();
    let mut position = 0;

    while position < text.len() {
        let character = text[position..]
            .chars()
            .next()
            .expect("position is inside text");
        if character == '\\'
            && text[position + character.len_utf8()..]
                .chars()
                .next()
                .is_some_and(|next| matches!(next, '\n' | '\r'))
        {
            position = skip_escaped_line_break(text, position + character.len_utf8());
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
            other => {
                output.push(other);
                position += other.len_utf8();
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
    ///
    /// # Errors
    ///
    /// Returns an error when the document has no semantic root, the root is not
    /// a document, or the document has no root block mapping.
    pub fn root_graph_mapping(&self) -> Result<GraphNodeId, YamlError> {
        self.root_mapping_graph()
    }

    /// Returns the first root-level block mapping in the document.
    ///
    /// # Errors
    ///
    /// Returns an error when no root block mapping exists or when the semantic
    /// root mapping is not linked back to a CST node.
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
        let Some((first, rest)) = path.split_first() else {
            return Ok(None);
        };

        let Some((_, mut current)) =
            self.get_graph_mapping_entry(self.root_mapping_graph()?, first)?
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
                Span::empty_from_usize(self.source.len()),
            ))
        })
    }

    fn graph_for_cst(&self, cst: NodeId) -> Option<GraphNodeId> {
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

    fn scalar_replacement_target(&self, node: NodeId) -> Result<(Span, ScalarStyle), YamlError> {
        let node = self.expect_node_kind(node, NodeKind::Scalar)?;
        let text = self.source.slice(node.span);
        let properties = parse_node_properties(text, node.span)?;
        if properties.anchor.is_some() || properties.tag.is_some() {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Emitter,
                    "anchored or tagged scalar rewriting is not implemented yet",
                    node.span,
                )
                .with_expected("an untagged and unanchored scalar"),
            )
            .with_position_from(&self.source));
        }

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
                Span::new(
                    node.span.start,
                    Span::offset_from_usize(node.span.start, end),
                ),
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
                Span::new(
                    node.span.start,
                    Span::offset_from_usize(node.span.start, end),
                ),
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
            Span::new(
                node.span.start,
                Span::offset_from_usize(node.span.start, end),
            ),
            ScalarStyle::Plain,
        ))
    }

    fn expect_node(&self, node: NodeId) -> Result<&Node, YamlError> {
        self.node(node).ok_or_else(|| {
            YamlError::new(Diagnostic::new(
                DiagnosticKind::Semantic,
                format!("unknown node id {}", node.0),
                Span::empty_from_usize(self.source.len()),
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
            .map(|(index, _)| NodeId::from_usize(index))
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

        Span::from_usize(start, end)
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
    fn new(events: &'events [YamlEvent], cst_nodes: &'events [Node]) -> Self {
        Self {
            events,
            cst_nodes,
            graph_nodes: Vec::new(),
            stack: Vec::new(),
            root: None,
            documents: Vec::new(),
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
                self.open_document(event.span);
                Ok(())
            }
            YamlEventKind::DocumentEnd { .. } => {
                self.close_expected(OpenGraphKind::Document, event.span)?;
                Ok(())
            }
            YamlEventKind::MappingStart { style, tag, anchor } => {
                self.open_mapping(*style, tag.clone(), anchor.clone(), event.span);
                Ok(())
            }
            YamlEventKind::MappingEnd => self.close_and_attach(OpenGraphKind::Mapping, event.span),
            YamlEventKind::SequenceStart { style, tag, anchor } => {
                self.open_sequence(*style, tag.clone(), anchor.clone(), event.span);
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
                event.span,
            ),
            YamlEventKind::Alias { name } => self.attach_alias(name.clone(), event.span),
        }
    }

    fn open_document(&mut self, span: Span) {
        let id = self.push_node(GraphNode {
            kind: GraphKind::Document {
                children: Vec::new(),
            },
            span,
            cst: self.find_cst_node(NodeKind::Document, span),
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
                entries: Vec::new(),
            },
            span,
            cst: self.find_cst_node(mapping_node_kind(style), span),
        });
        self.stack.push(OpenGraphNode {
            id,
            kind: OpenGraphKind::Mapping,
        });
    }

    fn open_sequence(
        &mut self,
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
                items: Vec::new(),
            },
            span,
            cst: self.find_cst_node(sequence_node_kind(style), span),
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
            cst: self.find_cst_node(scalar_node_kind(style), span),
        });
        self.attach_node(id, span)
    }

    fn attach_alias(&mut self, name: String, span: Span) -> Result<(), YamlError> {
        let id = self.push_node(GraphNode {
            kind: GraphKind::Alias { name },
            span,
            cst: self.find_cst_node(NodeKind::Scalar, span),
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
            .map(|(index, _)| NodeId::from_usize(index))
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
    ///
    /// # Errors
    ///
    /// Returns an error when the implementing overlay cannot be read from the
    /// document or when required YAML paths, node kinds, or scalar values are
    /// invalid for that overlay.
    fn from_yaml_doc(doc: &YamlDoc) -> Result<Self, YamlError>;
}

/// Applies a typed overlay back to a YAML document as minimal patches.
pub trait ToYamlDoc {
    /// Writes `self` into `doc` without discarding unknown fields or comments.
    ///
    /// # Errors
    ///
    /// Returns an error when the implementing overlay cannot be written to the
    /// document, when generated YAML is invalid, or when a queued edit conflicts
    /// with another pending edit.
    fn apply_to_yaml_doc(&self, doc: &mut YamlDoc) -> Result<(), YamlError>;
}

/// Reads and writes individual YAML node values.
pub trait YamlValue: Sized {
    /// Reads a typed value from `node`.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` is unknown, has an unsupported YAML kind, or
    /// cannot be decoded as `Self`.
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError>;

    /// Writes a typed value into an existing node or inserts a new node.
    ///
    /// # Errors
    ///
    /// Returns an error when the value cannot be represented in the target YAML
    /// shape, when insertion requires a missing parent context, or when the edit
    /// cannot be queued.
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
        if let Some(value) = self {
            value.write_yaml(doc, node)
        } else {
            let node = node.ok_or_else(missing_write_node_error)?;
            let remove_node = doc.containing_entry(node).unwrap_or(node);
            doc.remove_node(remove_node)?;
            Ok(node)
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
    fn events_decode_double_quoted_hex_escapes() {
        let doc = YamlDoc::parse(
            "unicode: \"Sosa did fine.\\u263A\"\nhex esc: \"\\x0d\\x0a is \\r\\n\"\nwide: \"\\U0001F600\"\n",
        )
        .expect("valid double-quoted hex escapes");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :unicode\n=VAL \"Sosa did fine.☺\n=VAL :hex esc\n=VAL \"\\r\\n is \\r\\n\n=VAL :wide\n=VAL \"😀\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(
            doc.to_string(),
            "unicode: \"Sosa did fine.\\u263A\"\nhex esc: \"\\x0d\\x0a is \\r\\n\"\nwide: \"\\U0001F600\"\n"
        );
    }

    #[test]
    fn events_decode_double_quoted_escaped_line_continuation() {
        let input = concat!("quoted: \"folded \\", "\n  non-content\"\n");
        let doc = YamlDoc::parse(input).expect("valid escaped line continuation");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :quoted\n=VAL \"folded non-content\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(doc.to_string(), input);
    }

    #[test]
    fn double_quoted_hex_escape_errors_are_typed() {
        for input in ["\"\\u12\"", "\"\\xZZ\"", "\"\\U00110000\""] {
            let error = YamlDoc::parse(input).expect_err("invalid escape should fail");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Typed);
            assert!(
                error.diagnostic.message.contains("double-quoted"),
                "{input:?} should report a double-quoted escape error"
            );
        }
    }

    #[test]
    fn events_fold_multiline_double_quoted_scalar() {
        let doc = YamlDoc::parse("quoted: \"So does this\n  quoted scalar.\\n\"\n")
            .expect("valid multiline double-quoted scalar");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :quoted\n=VAL \"So does this quoted scalar.\\n\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(
            doc.to_string(),
            "quoted: \"So does this\n  quoted scalar.\\n\"\n"
        );
    }

    #[test]
    fn events_fold_multiline_single_quoted_blank_values() {
        let doc = YamlDoc::parse("a: '\n  '\ne: '\n\n  '\ng: '\n\n\n  '\n")
            .expect("valid multiline single-quoted blanks");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :a\n=VAL ' \n=VAL :e\n=VAL '\\n\n=VAL :g\n=VAL '\\n\\n\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(doc.to_string(), "a: '\n  '\ne: '\n\n  '\ng: '\n\n\n  '\n");
    }

    #[test]
    fn events_fold_multiline_quoted_flow_sequence_values() {
        let doc = YamlDoc::parse("[\"double\n quoted\", 'single\n quoted']\n")
            .expect("valid multiline quoted flow scalars");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ []\n=VAL \"double quoted\n=VAL 'single quoted\n-SEQ\n-DOC\n-STR\n"
        );
        assert_eq!(
            doc.to_string(),
            "[\"double\n quoted\", 'single\n quoted']\n"
        );
    }

    #[test]
    fn events_render_implicit_flow_mapping_sequence_entry() {
        let doc = YamlDoc::parse("[foo: bar]\n").expect("valid implicit flow mapping item");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ []\n+MAP {}\n=VAL :foo\n=VAL :bar\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
        assert_eq!(doc.to_string(), "[foo: bar]\n");
    }

    #[test]
    fn events_render_yaml_test_8udb_flow_sequence_shape() {
        let doc = YamlDoc::parse(
            "[\n\"double\n quoted\", 'single\n           quoted',\nplain\n text, [ nested ],\nsingle: pair,\n]\n",
        )
        .expect("valid flow sequence with implicit mapping");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ []\n=VAL \"double quoted\n=VAL 'single quoted\n=VAL :plain text\n+SEQ []\n=VAL :nested\n-SEQ\n+MAP {}\n=VAL :single\n=VAL :pair\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_explicit_block_mapping_key_value_pair() {
        let doc = YamlDoc::parse("? key\n: value\n").expect("valid explicit mapping key");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :value\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(doc.to_string(), "? key\n: value\n");
    }

    #[test]
    fn events_render_explicit_key_with_comment_before_value() {
        let doc =
            YamlDoc::parse("? key\n# comment\n: value\n").expect("valid explicit key comment");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :value\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(doc.to_string(), "? key\n# comment\n: value\n");
    }

    #[test]
    fn events_render_explicit_set_keys_as_empty_values() {
        let doc = YamlDoc::parse("--- !!set\n? Mark McGwire\n? Sammy Sosa\n")
            .expect("valid explicit set keys");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n+MAP <tag:yaml.org,2002:set>\n=VAL :Mark McGwire\n=VAL :\n=VAL :Sammy Sosa\n=VAL :\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_explicit_sequence_key() {
        let doc =
            YamlDoc::parse("complex:\n  ? - a\n  : b\n").expect("valid explicit sequence key");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :complex\n+MAP\n+SEQ\n=VAL :a\n-SEQ\n=VAL :b\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(doc.to_string(), "complex:\n  ? - a\n  : b\n");
    }

    #[test]
    fn events_render_explicit_folded_scalar_key_with_empty_value() {
        let doc =
            YamlDoc::parse("complex:\n  ? >\n    a\n  :\n").expect("valid explicit scalar key");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :complex\n+MAP\n=VAL >a\\n\n=VAL :\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(doc.to_string(), "complex:\n  ? >\n    a\n  :\n");
    }

    #[test]
    fn parser_events_render_explicit_following_sequence_key() {
        let source =
            Source::new("---\n?\n- a\n- b\n:\n- c\n- d\n".to_owned()).expect("valid source");
        let tokens = lex(&source).expect("valid tokens");
        let parsed = Parser::new(&source, &tokens).parse().expect("valid parser");

        assert_eq!(
            events_to_test_string(&parsed.events),
            "+STR\n+DOC ---\n+MAP\n+SEQ\n=VAL :a\n=VAL :b\n-SEQ\n+SEQ\n=VAL :c\n=VAL :d\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_scalar_anchors_and_tags() {
        let doc = YamlDoc::parse(
            "plain: &anchor !<tag:example.com,2026:x> value\nquoted: !!str \"123\"\n",
        )
        .expect("valid scalar node properties");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :plain\n=VAL &anchor <tag:example.com,2026:x> :value\n=VAL :quoted\n=VAL <tag:yaml.org,2002:str> \"123\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_bare_non_specific_tags() {
        for (input, expected) in [
            ("! a\n", "+STR\n+DOC\n=VAL <!> :a\n-DOC\n-STR\n"),
            (
                "- ! 12\n",
                "+STR\n+DOC\n+SEQ\n=VAL <!> :12\n-SEQ\n-DOC\n-STR\n",
            ),
            ("!\n", "+STR\n+DOC\n=VAL <!> :\n-DOC\n-STR\n"),
        ] {
            let doc = YamlDoc::parse(input).expect("valid bare non-specific tag");

            assert_eq!(doc.events_to_test_string(), expected);
            assert_eq!(doc.to_string(), input);
        }
    }

    #[test]
    fn events_render_plain_alias_before_inline_comment() {
        let doc = YamlDoc::parse("rbi:\n  - *SS # Subsequent occurrence\n")
            .expect("valid alias sequence entry");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :rbi\n+SEQ\n=ALI *SS\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn graph_preserves_scalar_anchors_and_tags() {
        let doc =
            YamlDoc::parse("plain: &anchor !local value\n").expect("valid scalar node properties");
        let value = doc
            .get_graph_path(&["plain"])
            .expect("graph path succeeds")
            .expect("value exists");

        assert_eq!(
            doc.graph_node(value).map(|node| &node.kind),
            Some(&GraphKind::Scalar {
                style: YamlScalarStyle::Plain,
                value: "value".to_owned(),
                tag: Some("!local".to_owned()),
                anchor: Some("anchor".to_owned()),
            })
        );
    }

    #[test]
    fn parser_builds_anchored_and_tagged_flow_collection_values() {
        let doc = YamlDoc::parse(
            "items: &seq !!seq [one, two]\nsettings: !<tag:yaml.org,2002:map> {a: b}\n",
        )
        .expect("valid flow collection node properties");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :items\n+SEQ [] &seq <tag:yaml.org,2002:seq>\n=VAL :one\n=VAL :two\n-SEQ\n=VAL :settings\n+MAP {} <tag:yaml.org,2002:map>\n=VAL :a\n=VAL :b\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
        let items = doc
            .get_graph_path(&["items"])
            .expect("graph path succeeds")
            .expect("items exists");
        assert_eq!(
            doc.graph_node(items).map(|node| &node.kind),
            Some(&GraphKind::Sequence {
                style: CollectionStyle::Flow,
                tag: Some("tag:yaml.org,2002:seq".to_owned()),
                anchor: Some("seq".to_owned()),
                items: vec![GraphNodeId(items.0 + 1), GraphNodeId(items.0 + 2),],
            })
        );
    }

    #[test]
    fn directives_accept_yaml_version_before_explicit_document() {
        let doc = YamlDoc::parse("%YAML 1.2 # comment\n--- value\n").expect("valid directive");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n=VAL :value\n-DOC\n-STR\n"
        );
        assert_eq!(count_nodes(&doc, NodeKind::Directive), 1);
    }

    #[test]
    fn directives_tolerate_reserved_and_unsupported_versions_before_document() {
        for (input, expected) in [
            (
                "%FOO  bar baz # ignored\n---\n\"foo\"\n",
                "+STR\n+DOC ---\n=VAL \"foo\n-DOC\n-STR\n",
            ),
            (
                "%YAML 1.3 # Attempt parsing\n---\n\"foo\"\n",
                "+STR\n+DOC ---\n=VAL \"foo\n-DOC\n-STR\n",
            ),
            ("%YAM 1.1\n---\n", "+STR\n+DOC ---\n=VAL :\n-DOC\n-STR\n"),
            ("%YAMLL 1.1\n---\n", "+STR\n+DOC ---\n=VAL :\n-DOC\n-STR\n"),
        ] {
            let doc = YamlDoc::parse(input).expect("reserved directive should be tolerated");

            assert_eq!(doc.events_to_test_string(), expected);
            assert_eq!(doc.to_string(), input);
            assert_eq!(count_nodes(&doc, NodeKind::Directive), 1);
        }
    }

    #[test]
    fn tag_directive_resolves_secondary_handle() {
        let doc = YamlDoc::parse("%TAG !! tag:example.com,2000:app/\n---\n!!int 1 - 3\n")
            .expect("valid tag directive");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n=VAL <tag:example.com,2000:app/int> :1 - 3\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn tag_directive_resolves_named_handle() {
        let doc = YamlDoc::parse("%TAG !e! tag:example.com,2000:app/\n---\n!e!foo \"bar\"\n")
            .expect("valid named tag directive");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n=VAL <tag:example.com,2000:app/foo> \"bar\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn tag_directive_percent_decodes_suffix() {
        let doc = YamlDoc::parse("%TAG !e! tag:example.com,2000:app/\n---\n- !e!tag%21 baz\n")
            .expect("valid tag directive with URI escape");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n+SEQ\n=VAL <tag:example.com,2000:app/tag!> :baz\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn events_render_multi_document_stream_with_explicit_end() {
        let doc =
            YamlDoc::parse("%YAML 1.2\n--- |\n%!PS-Adobe-2.0\n...\n%YAML 1.2\n---\n# Empty\n...\n")
                .expect("valid multi-document stream");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n=VAL |%!PS-Adobe-2.0\\n\n-DOC ...\n+DOC ---\n=VAL :\n-DOC ...\n-STR\n"
        );
        assert_eq!(doc.graph().documents.len(), 2);
        assert_eq!(doc.graph().root, doc.graph().documents.first().copied());
    }

    #[test]
    fn parser_builds_empty_documents_in_stream() {
        let doc = YamlDoc::parse("---\n...\n---\n...\n").expect("valid empty documents");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n=VAL :\n-DOC ...\n+DOC ---\n=VAL :\n-DOC ...\n-STR\n"
        );
        assert_eq!(doc.graph().documents.len(), 2);
    }

    #[test]
    fn parser_keeps_contentless_streams_empty() {
        for input in [
            "",
            "# Comment only.\n",
            "  # Comment\n   \n\n",
            "...\n",
            "# comment\n...\n",
        ] {
            let doc = YamlDoc::parse(input).expect("contentless stream is valid");

            assert_eq!(doc.events_to_test_string(), "+STR\n-STR\n");
            assert_eq!(doc.to_string(), input);
            assert!(doc.graph().documents.is_empty());
            assert!(doc.graph().root.is_none());
        }
    }

    #[test]
    fn parser_preserves_explicit_empty_documents() {
        for (input, expected) in [
            ("---\n", "+STR\n+DOC ---\n=VAL :\n-DOC\n-STR\n"),
            ("---\n...\n", "+STR\n+DOC ---\n=VAL :\n-DOC ...\n-STR\n"),
        ] {
            let doc = YamlDoc::parse(input).expect("explicit empty document is valid");

            assert_eq!(doc.events_to_test_string(), expected);
            assert_eq!(doc.to_string(), input);
            assert_eq!(doc.graph().documents.len(), 1);
        }
    }

    #[test]
    fn parser_reports_malformed_and_duplicate_directives() {
        for (input, message) in [
            ("%YAML\n---\n", "missing YAML directive version"),
            ("%YAML 1.2\n%YAML 1.2\n---\n", "duplicate YAML directive"),
            (
                "key: value\n%YAML 1.2\n",
                "directives must appear before document content",
            ),
            (
                "%YAML 1.2\n",
                "directives must be followed by document content",
            ),
            ("%YAML 1.1#...\n---\n", "invalid YAML directive version"),
            (
                "%TAG !bad tag:example.com,2000:app/\n---\n",
                "invalid TAG directive handle",
            ),
        ] {
            let error = YamlDoc::parse(input).expect_err("directive should be rejected");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
            assert_eq!(error.diagnostic.message, message);
        }
    }

    #[test]
    fn yaml_value_rejects_tagged_or_anchored_scalar_writes_for_now() {
        let mut doc = YamlDoc::parse("plain: &anchor value\n").expect("valid anchor");
        let plain = doc
            .get_path(&["plain"])
            .expect("path lookup succeeds")
            .expect("plain exists");

        let error = String::from("updated")
            .write_yaml(&mut doc, Some(plain))
            .expect_err("anchored scalar writes are intentionally not implemented yet");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Emitter);
        assert_eq!(
            error.diagnostic.message,
            "anchored or tagged scalar rewriting is not implemented yet"
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
    fn parser_attaches_same_indent_sequence_after_empty_mapping_value() {
        let input = "one:\n- 2\n- 3\nfour: 5\n";
        let doc = YamlDoc::parse(input).expect("valid same-indent sequence value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :one\n+SEQ\n=VAL :2\n=VAL :3\n-SEQ\n=VAL :four\n=VAL :5\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_closes_same_indent_sequence_before_next_mapping_entry() {
        let input = "foo:\n- 42\nbar:\n  - 44\n";
        let doc = YamlDoc::parse(input).expect("valid sibling sequence values");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :foo\n+SEQ\n=VAL :42\n-SEQ\n=VAL :bar\n+SEQ\n=VAL :44\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_attaches_sequence_before_nested_mapping_value() {
        let input = "sequence:\n- one\n- two\nmapping:\n  ? sky\n  : blue\n  sea : green\n";
        let doc = YamlDoc::parse(input).expect("valid sequence then mapping values");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :sequence\n+SEQ\n=VAL :one\n=VAL :two\n-SEQ\n=VAL :mapping\n+MAP\n=VAL :sky\n=VAL :blue\n=VAL :sea\n=VAL :green\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_preserves_empty_mapping_value_before_sibling_mapping() {
        let input = "key:\nnext: value\n";
        let doc = YamlDoc::parse(input).expect("valid empty value before sibling mapping");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :\n=VAL :next\n=VAL :value\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_composes_empty_anchored_mapping_value_before_alias() {
        let input = "---\na: &anchor\nb: *anchor\n";
        let doc = YamlDoc::parse(input).expect("valid empty anchored scalar value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n+MAP\n=VAL :a\n=VAL &anchor :\n=VAL :b\n=ALI *anchor\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_composes_empty_tagged_scalars_in_sequence_mappings() {
        let input = "- !!str\n-\n  !!null : a\n  b: !!str\n- !!str : !!null\n";
        let doc = YamlDoc::parse(input).expect("valid empty tagged scalar positions");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n=VAL <tag:yaml.org,2002:str> :\n+MAP\n=VAL <tag:yaml.org,2002:null> :\n=VAL :a\n=VAL :b\n=VAL <tag:yaml.org,2002:str> :\n-MAP\n+MAP\n=VAL <tag:yaml.org,2002:str> :\n=VAL <tag:yaml.org,2002:null> :\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_composes_empty_anchored_scalars_in_explicit_entries() {
        let input = "- &a\n- a\n-\n  &a : a\n  b: &b\n-\n  &c : &a\n-\n  ? &d\n-\n  ? &e\n  : &a\n";
        let doc = YamlDoc::parse(input).expect("valid empty anchored scalar positions");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n=VAL &a :\n=VAL :a\n+MAP\n=VAL &a :\n=VAL :a\n=VAL :b\n=VAL &b :\n-MAP\n+MAP\n=VAL &c :\n=VAL &a :\n-MAP\n+MAP\n=VAL &d :\n=VAL :\n-MAP\n+MAP\n=VAL &e :\n=VAL &a :\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_tag_to_same_indent_mapping_sequence_value() {
        let input = "sequence: !!seq\n- entry\n- !!seq\n - nested\nmapping: !!map\n foo: bar\n";
        let doc = YamlDoc::parse(input).expect("valid tagged same-indent collection values");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :sequence\n+SEQ <tag:yaml.org,2002:seq>\n=VAL :entry\n+SEQ <tag:yaml.org,2002:seq>\n=VAL :nested\n-SEQ\n-SEQ\n=VAL :mapping\n+MAP <tag:yaml.org,2002:map>\n=VAL :foo\n=VAL :bar\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_alias_and_anchored_block_mapping_keys() {
        let input = "\"top1\" : \n  \"key1\" : &alias1 scalar1\n'top2' : \n  'key2' : &alias2 scalar2\ntop3: &node3 \n  *alias1 : scalar3\ntop4: \n  *alias2 : scalar4\ntop5   :    \n  scalar5\ntop6: \n  &anchor6 'key6' : scalar6\n";
        let doc = YamlDoc::parse(input).expect("valid anchored and alias block keys");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL \"top1\n+MAP\n=VAL \"key1\n=VAL &alias1 :scalar1\n-MAP\n=VAL 'top2\n+MAP\n=VAL 'key2\n=VAL &alias2 :scalar2\n-MAP\n=VAL :top3\n+MAP &node3\n=ALI *alias1\n=VAL :scalar3\n-MAP\n=VAL :top4\n+MAP\n=ALI *alias2\n=VAL :scalar4\n-MAP\n=VAL :top5\n=VAL :scalar5\n=VAL :top6\n+MAP\n=VAL &anchor6 'key6\n=VAL :scalar6\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_flow_sequence_value_with_implicit_mapping_item() {
        let input = "\"implicit block key\" : [\n  \"implicit flow key\" : value,\n ]\n";
        let doc = YamlDoc::parse(input).expect("valid flow sequence mapping value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL \"implicit block key\n+SEQ []\n+MAP {}\n=VAL \"implicit flow key\n=VAL :value\n-MAP\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_explicit_block_sequence_and_flow_keys() {
        let input = "? - Detroit Tigers\n  - Chicago cubs\n:\n  - 2001-07-23\n\n? [ New York Yankees,\n    Atlanta Braves ]\n: [ 2001-07-02, 2001-08-12,\n    2001-08-14 ]\n";
        let doc = YamlDoc::parse(input).expect("valid explicit sequence and flow keys");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n+SEQ\n=VAL :Detroit Tigers\n=VAL :Chicago cubs\n-SEQ\n+SEQ\n=VAL :2001-07-23\n-SEQ\n+SEQ []\n=VAL :New York Yankees\n=VAL :Atlanta Braves\n-SEQ\n+SEQ []\n=VAL :2001-07-02\n=VAL :2001-08-12\n=VAL :2001-08-14\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_flow_collection_key_after_explicit_indicator() {
        let input = "? []: x\n";
        let doc = YamlDoc::parse(input).expect("valid explicit flow collection key");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n+MAP\n+SEQ []\n-SEQ\n=VAL :x\n-MAP\n=VAL :\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_anchor_colon_and_alias_colon_keys() {
        let input = "&a: key: &a value\nfoo:\n  *a:\n";
        let doc = YamlDoc::parse(input).expect("valid colon-suffixed anchor and alias keys");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL &a: :key\n=VAL &a :value\n=VAL :foo\n=ALI *a:\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_explicit_compact_mapping_key_and_value() {
        let input = "- sun: yellow\n- ? earth: blue\n  : moon: white\n";
        let doc = YamlDoc::parse(input).expect("valid explicit compact mapping key and value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :sun\n=VAL :yellow\n-MAP\n+MAP\n+MAP\n=VAL :earth\n=VAL :blue\n-MAP\n+MAP\n=VAL :moon\n=VAL :white\n-MAP\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_builds_mapping_value_for_bare_sequence_entry() {
        let input = "-\n  name: Mark\n";
        let doc = YamlDoc::parse(input).expect("parser should accept nested mapping entry value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :name\n=VAL :Mark\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_builds_nested_compact_block_sequence_entry() {
        let input = "- - s1_i1\n  - s1_i2\n- s2\n";
        let doc = YamlDoc::parse(input).expect("parser should accept nested sequence entry value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n+SEQ\n=VAL :s1_i1\n=VAL :s1_i2\n-SEQ\n=VAL :s2\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_builds_compact_mapping_sequence_entry_value() {
        let input = "block sequence:\n  - one\n  - two : three\n";
        let doc = YamlDoc::parse(input).expect("parser should accept compact mapping entry value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :block sequence\n+SEQ\n=VAL :one\n+MAP\n=VAL :two\n=VAL :three\n-MAP\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_builds_nested_mapping_and_sequence_for_bare_sequence_entries() {
        let input = "-\n foo: bar\n-\n - 42\n";
        let doc =
            YamlDoc::parse(input).expect("parser should accept nested bare sequence entry values");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :foo\n=VAL :bar\n-MAP\n+SEQ\n=VAL :42\n-SEQ\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_preserves_true_empty_block_sequence_entry() {
        let input = "-\n- value\n";
        let doc = YamlDoc::parse(input).expect("parser should accept empty sequence entry");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n=VAL :\n=VAL :value\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_folds_same_line_plain_scalar_continuations() {
        let input = "plain: a\n b\n\n c\n";
        let doc = YamlDoc::parse(input).expect("parser should accept plain scalar continuations");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :plain\n=VAL :a b\\nc\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_folds_next_line_plain_scalar_mapping_values() {
        let input = "key:\n  value\n  with\n  \t\n  tabs\n";
        let doc = YamlDoc::parse(input).expect("parser should accept next-line plain scalar");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :value with\\ntabs\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_folds_log_message_plain_scalar_mapping_value() {
        let input = "Warning:\n  This is an error message\n  for the log file\n";
        let doc = YamlDoc::parse(input).expect("parser should accept log message scalar");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :Warning\n=VAL :This is an error message for the log file\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_preserves_tab_prefixed_plain_scalar_continuation() {
        let input = "plain: text\n \tlines\n";
        let doc = YamlDoc::parse(input).expect("parser should accept tab-prefixed content");
        let plain = doc
            .get_path(&["plain"])
            .expect("lookup succeeds")
            .expect("plain exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(doc.scalar_value(plain).expect("plain reads"), "text lines");
    }

    #[test]
    fn parser_accepts_tab_prefixed_quoted_scalar_continuation() {
        let input = "quoted: \"text\n  \tlines\"\n";
        let doc = YamlDoc::parse(input).expect("parser should accept quoted tab content");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :quoted\n=VAL \"text lines\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_tab_prefixed_sequence_entry_continuation() {
        let input = "x:\n - x\n  \tx\n";
        let doc = YamlDoc::parse(input).expect("parser should accept sequence continuation");
        let items = doc
            .get_path(&["x"])
            .expect("lookup succeeds")
            .expect("x exists");
        let sequence = doc.graph_sequence_items(items).expect("sequence items");

        assert_eq!(doc.to_string(), input);
        assert_eq!(sequence.len(), 1);
        assert_eq!(
            doc.scalar_value(sequence[0]).expect("sequence item reads"),
            "x x"
        );
    }

    #[test]
    fn parser_accepts_root_flow_collection_with_leading_tab() {
        let input = "\t[\n\t]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept tab-prefixed root flow");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ []\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_treats_inline_comment_mapping_value_as_empty_for_nested_block_value() {
        let input = "hr: # 1998 hr ranking\n  - Mark McGwire\n  - Sammy Sosa\n";
        let doc = YamlDoc::parse(input).expect("parser should accept commented nested value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :hr\n+SEQ\n=VAL :Mark McGwire\n=VAL :Sammy Sosa\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_anchor_to_root_block_sequence() {
        let input = "&sequence\n- a\n";
        let doc = YamlDoc::parse(input).expect("parser should accept anchored root sequence");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ &sequence\n=VAL :a\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_anchor_to_nested_block_mapping() {
        let input = "top1: &node1\n  key1: one\n";
        let doc = YamlDoc::parse(input).expect("parser should accept anchored nested mapping");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :top1\n+MAP &node1\n=VAL :key1\n=VAL :one\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_tag_to_nested_block_sequence() {
        let input = "foo: !!seq\n  - !!str a\n";
        let doc = YamlDoc::parse(input).expect("parser should accept tagged nested sequence");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :foo\n+SEQ <tag:yaml.org,2002:seq>\n=VAL <tag:yaml.org,2002:str> :a\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_anchor_to_compact_nested_mapping_key() {
        let input = "top3:\n  &k3 key3: three\n";
        let doc = YamlDoc::parse(input).expect("parser should accept anchored nested key");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :top3\n+MAP\n=VAL &k3 :key3\n=VAL :three\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_keeps_property_only_value_as_scalar_when_nested_value_is_plain() {
        let input = "top6: &val6\n  six\n";
        let doc = YamlDoc::parse(input).expect("parser should accept anchored scalar value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :top6\n=VAL &val6 :six\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_split_root_scalar_properties() {
        let input = "---\n&a1\n!!str\nscalar1\n";
        let doc = YamlDoc::parse(input).expect("parser should accept split scalar properties");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n=VAL &a1 <tag:yaml.org,2002:str> :scalar1\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_split_root_scalar_properties_in_reversed_order() {
        let input = "---\n!!str\n&a2\nscalar2\n";
        let doc =
            YamlDoc::parse(input).expect("parser should accept reversed split scalar properties");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC ---\n=VAL &a2 <tag:yaml.org,2002:str> :scalar2\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_split_properties_to_nested_block_mapping() {
        let input = "key: &anchor\n !!map\n  a: b\n";
        let doc =
            YamlDoc::parse(input).expect("parser should accept split nested mapping properties");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :key\n+MAP &anchor <tag:yaml.org,2002:map>\n=VAL :a\n=VAL :b\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_applies_split_property_to_block_scalar() {
        let input = "folded:\n   !foo\n  >1\n value\n";
        let doc = YamlDoc::parse(input).expect("parser should accept tagged block scalar");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :folded\n=VAL <!foo> >value\\n\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_keeps_same_line_split_property_cases() {
        let input = "&a4 !!map\n&a5 !!str key5: value4\n";
        let doc = YamlDoc::parse(input).expect("parser should accept same-line properties");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP &a4 <tag:yaml.org,2002:map>\n=VAL &a5 <tag:yaml.org,2002:str> :key5\n=VAL :value4\n-MAP\n-DOC\n-STR\n"
        );
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
    fn yaml_value_preserves_literal_whitespace_only_lines() {
        let input = "text: |\n  a\n    \n  b\n";
        let doc = YamlDoc::parse(input).expect("valid literal scalar with whitespace line");
        let text = doc
            .get_path(&["text"])
            .expect("lookup succeeds")
            .expect("text exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            String::read_yaml(&doc, text).expect("literal reads"),
            "a\n  \nb\n"
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
    fn yaml_value_reads_folded_scalar_blank_line_paragraphs() {
        let doc = YamlDoc::parse(">\n  ab\n  cd\n\n  ef\n\n\n  gh\n").expect("valid folded scalar");
        let folded = folded_scalar(&doc).expect("folded scalar exists");

        assert_eq!(
            String::read_yaml(&doc, folded).expect("folded reads"),
            "ab cd\nef\n\ngh\n"
        );
    }

    #[test]
    fn yaml_value_reads_folded_scalar_more_indented_lines() {
        let doc = YamlDoc::parse(">\n  folded\n    * bullet\n\n    * list\n  tail\n")
            .expect("valid folded scalar");
        let folded = folded_scalar(&doc).expect("folded scalar exists");

        assert_eq!(
            String::read_yaml(&doc, folded).expect("folded reads"),
            "folded\n  * bullet\n\n  * list\ntail\n"
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
    fn parser_respects_explicit_block_scalar_indentation() {
        let input = "- aaa: |2\n    xxx\n  bbb: |\n    xxx\n";
        let doc = YamlDoc::parse(input).expect("valid explicit indentation scalar");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :aaa\n=VAL |xxx\\n\n=VAL :bbb\n=VAL |xxx\\n\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_tab_prefixed_block_scalar_content() {
        let input = "block: |\n  text\n   \tlines\n";
        let doc = YamlDoc::parse(input).expect("valid tab content in block scalar");
        let block = doc
            .get_path(&["block"])
            .expect("lookup succeeds")
            .expect("block exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            String::read_yaml(&doc, block).expect("literal reads"),
            "text\n \tlines\n"
        );
    }

    #[test]
    fn parser_keeps_empty_block_scalars_from_consuming_siblings() {
        let input = "strip: >-\n\nclip: >\n\nkeep: |+\n";
        let doc = YamlDoc::parse(input).expect("valid empty block scalars");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :strip\n=VAL >\n=VAL :clip\n=VAL >\n=VAL :keep\n=VAL |\\n\n-MAP\n-DOC\n-STR\n"
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
            .map(|(index, _)| NodeId::from_usize(index))
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
            .map(|(index, _)| NodeId::from_usize(index))
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
    fn parser_keeps_non_separating_colons_in_flow_plain_scalars() {
        let input = "{url: http://foo.com, empty:, key: value:with:colons}\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow plain colons");
        let mapping = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowMapping)
            .map(|(index, _)| NodeId::from_usize(index))
            .expect("flow mapping exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            flow_mapping_scalar_pairs(&doc, mapping),
            [
                ("url", "http://foo.com"),
                ("empty", ""),
                ("key", "value:with:colons")
            ]
        );
    }

    #[test]
    fn parser_keeps_non_separating_colons_in_block_plain_scalars() {
        let input = "items:\n  - ::vector\n  - http://example.com/foo#bar\nkey ends with two colons::: value\n";
        let doc = YamlDoc::parse(input).expect("parser should accept block plain colons");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :items\n+SEQ\n=VAL :::vector\n=VAL :http://example.com/foo#bar\n-SEQ\n=VAL :key ends with two colons::\n=VAL :value\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_builds_key_only_flow_mapping_entry_with_colon_text() {
        let input = "{http://foo.com, omitted value:}\n";
        let doc = YamlDoc::parse(input).expect("parser should accept key-only flow entry");
        let mapping = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowMapping)
            .map(|(index, _)| NodeId::from_usize(index))
            .expect("flow mapping exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            flow_mapping_scalar_pairs(&doc, mapping),
            [("http://foo.com", ""), ("omitted value", "")]
        );
    }

    #[test]
    fn parser_accepts_quoted_flow_keys_without_separator_space() {
        let input = "{ \"key\":value, \"key\"::value }\n";
        let doc = YamlDoc::parse(input).expect("parser should accept quoted flow keys");
        let mapping = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowMapping)
            .map(|(index, _)| NodeId::from_usize(index))
            .expect("flow mapping exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            flow_mapping_scalar_pairs(&doc, mapping),
            [("\"key\"", "value"), ("\"key\"", ":value")]
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
            .map(|(index, _)| NodeId::from_usize(index))
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
    fn parser_accepts_flow_sequence_comments_between_items() {
        let input = "---\n[ word1\n# comment\n, word2]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow sequence comments");
        let sequence = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowSequence)
            .map(|(index, _)| NodeId::from_usize(index))
            .expect("flow sequence exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            flow_sequence_scalar_texts(&doc, sequence),
            ["word1", "word2"]
        );
    }

    #[test]
    fn parser_accepts_flow_mapping_comment_before_colon() {
        let input = "---\n{ \"foo\" # comment\n  :bar }\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow mapping comment");
        let mapping = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowMapping)
            .map(|(index, _)| NodeId::from_usize(index))
            .expect("flow mapping exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            flow_mapping_scalar_pairs(&doc, mapping),
            [("\"foo\"", "bar")]
        );
    }

    #[test]
    fn parser_accepts_flow_sequence_end_of_line_comments() {
        let input = "flow: [    # Leading spaces\n   By two,        # in flow style\n  Also by two,    # are neither\n  \tStill by two   # content nor\n    ]             # indentation.\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow item comments");
        let flow = doc
            .get_path(&["flow"])
            .expect("lookup succeeds")
            .expect("flow exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            flow_sequence_scalar_texts(&doc, flow),
            ["By two", "Also by two", "Still by two"]
        );
    }

    #[test]
    fn parser_keeps_non_comment_hash_in_flow_plain_scalar() {
        let input = "[http://example.com/foo#bar]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept hash in flow scalar");
        let sequence = doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowSequence)
            .map(|(index, _)| NodeId::from_usize(index))
            .expect("flow sequence exists");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            flow_sequence_scalar_texts(&doc, sequence),
            ["http://example.com/foo#bar"]
        );
    }

    #[test]
    fn parser_accepts_multiline_flow_mapping_entries() {
        let sequence_input = "- { multi\n  line, a: b}\n";
        let sequence_doc =
            YamlDoc::parse(sequence_input).expect("parser should accept multiline flow entry");
        let sequence_mapping = sequence_doc
            .nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FlowMapping)
            .map(|(index, _)| NodeId::from_usize(index))
            .expect("flow mapping exists");

        assert_eq!(sequence_doc.to_string(), sequence_input);
        assert_eq!(
            flow_mapping_scalar_pairs(&sequence_doc, sequence_mapping),
            [("multi\n  line", ""), ("a", "b")]
        );

        let mapping_input = "Sammy Sosa: {\n    hr: 63,\n    avg: 0.288\n  }\n";
        let mapping_doc =
            YamlDoc::parse(mapping_input).expect("parser should accept multiline flow mapping");
        let nested_mapping = mapping_doc
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.kind == NodeKind::FlowMapping)
            .map(|(index, _)| NodeId::from_usize(index))
            .next()
            .expect("flow mapping exists");

        assert_eq!(mapping_doc.to_string(), mapping_input);
        assert_eq!(
            flow_mapping_scalar_pairs(&mapping_doc, nested_mapping),
            [("hr", "63"), ("avg", "0.288")]
        );
    }

    #[test]
    fn parser_accepts_flow_collection_key_in_block_mapping() {
        let input = "[flow]: block\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow sequence key");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n+SEQ []\n=VAL :flow\n-SEQ\n=VAL :block\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_flow_mapping_key_with_nested_block_value() {
        let input = "{ first: Sammy, last: Sosa }:\n  hr: 65\n  avg: 0.278\n";
        let doc = YamlDoc::parse(input).expect("parser should accept flow mapping key");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n+MAP {}\n=VAL :first\n=VAL :Sammy\n=VAL :last\n=VAL :Sosa\n-MAP\n+MAP\n=VAL :hr\n=VAL :65\n=VAL :avg\n=VAL :0.278\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_anchored_flow_sequence_key() {
        let input = "{ &a [a, &b b]: *b, *a : [c, *b, d]}\n";
        let doc = YamlDoc::parse(input).expect("parser should accept anchored flow key");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP {}\n+SEQ [] &a\n=VAL :a\n=VAL &b :b\n-SEQ\n=ALI *b\n=ALI *a\n+SEQ []\n=VAL :c\n=ALI *b\n=VAL :d\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_explicit_flow_mapping_entries() {
        let input = "{\n? explicit: entry,\nimplicit: entry,\n?\n}\n";
        let doc = YamlDoc::parse(input).expect("parser should accept explicit flow entries");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP {}\n=VAL :explicit\n=VAL :entry\n=VAL :implicit\n=VAL :entry\n=VAL :\n=VAL :\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_explicit_flow_sequence_mapping_entries() {
        let flow_input = "[\n? foo\n bar : baz\n]\n";
        let flow_doc =
            YamlDoc::parse(flow_input).expect("parser should accept explicit flow seq entry");
        assert_eq!(flow_doc.to_string(), flow_input);
        assert_eq!(
            flow_doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ []\n+MAP {}\n=VAL :foo bar\n=VAL :baz\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );

        let block_input = "- ? : x\n";
        let block_doc =
            YamlDoc::parse(block_input).expect("parser should accept compact explicit entry");
        assert_eq!(block_doc.to_string(), block_input);
        assert_eq!(
            block_doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n+MAP\n+MAP\n=VAL :\n=VAL :x\n-MAP\n=VAL :\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_property_prefixed_root_flow_sequence() {
        let input = "&flowseq [\n a: b,\n &c c: d\n]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept anchored flow sequence");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ [] &flowseq\n+MAP {}\n=VAL :a\n=VAL :b\n-MAP\n+MAP {}\n=VAL &c :c\n=VAL :d\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_split_tag_before_flow_value() {
        let input = "!!map {\n  k: !!seq\n  [ a, !!str b]\n}\n";
        let doc = YamlDoc::parse(input).expect("parser should accept tagged flow value");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP {} <tag:yaml.org,2002:map>\n=VAL :k\n+SEQ [] <tag:yaml.org,2002:seq>\n=VAL :a\n=VAL <tag:yaml.org,2002:str> :b\n-SEQ\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_implicit_flow_mapping_collection_key() {
        let input = "[ {JSON: like}:adjacent ]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept collection key");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ []\n+MAP {}\n+MAP {}\n=VAL :JSON\n=VAL :like\n-MAP\n=VAL :adjacent\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_nested_flow_collection_key_in_implicit_mapping() {
        let input = "[[[b,c]]: d, e]\n";
        let doc = YamlDoc::parse(input).expect("parser should accept nested collection key");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ []\n+MAP {}\n+SEQ []\n+SEQ []\n=VAL :b\n=VAL :c\n-SEQ\n-SEQ\n=VAL :d\n-MAP\n=VAL :e\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_empty_implicit_flow_mapping_keys() {
        for (input, value) in [
            ("[ : empty key ]\n", "empty key"),
            ("[: another empty key]\n", "another empty key"),
        ] {
            let doc = YamlDoc::parse(input).expect("parser should accept empty key");

            assert_eq!(doc.to_string(), input);
            assert_eq!(
                doc.events_to_test_string(),
                format!(
                    "+STR\n+DOC\n+SEQ []\n+MAP {{}}\n=VAL :\n=VAL :{value}\n-MAP\n-SEQ\n-DOC\n-STR\n"
                )
            );
        }
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
    fn parser_rejects_tabs_used_as_block_indicator_separation() {
        for input in ["-\t-\n", "?\t-\n", "?\tkey:\n", "? key:\n:\tkey:\n"] {
            let error = YamlDoc::parse(input).expect_err("tab must not enable block structure");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        }
    }

    #[test]
    fn parser_preserves_tab_after_sequence_indicator_as_scalar_content() {
        let input = "-\t-1\n";
        let doc = YamlDoc::parse(input).expect("tab before plain scalar should be accepted");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n=VAL :-1\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_indented_root_block_sequence() {
        let input = " - !!str a\n - b\n - !!int 42\n - d\n";
        let doc = YamlDoc::parse(input).expect("valid indented root sequence");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n=VAL <tag:yaml.org,2002:str> :a\n=VAL :b\n=VAL <tag:yaml.org,2002:int> :42\n=VAL :d\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_indented_root_flow_sequence() {
        let input = "  [1, 2, 3]\n";
        let doc = YamlDoc::parse(input).expect("valid indented root flow sequence");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ []\n=VAL :1\n=VAL :2\n=VAL :3\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_accepts_nested_mappings_in_indented_root_sequence() {
        let input = " - key: value\n   key2: value2\n -\n   key3: value3\n";
        let doc = YamlDoc::parse(input).expect("valid indented root sequence mappings");

        assert_eq!(doc.to_string(), input);
        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+SEQ\n+MAP\n=VAL :key\n=VAL :value\n=VAL :key2\n=VAL :value2\n-MAP\n+MAP\n=VAL :key3\n=VAL :value3\n-MAP\n-SEQ\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_folds_root_plain_scalar_continuations() {
        for (input, expected) in [
            (
                "---\nk:#foo\n &a !t s\n",
                "+STR\n+DOC ---\n=VAL :k:#foo &a !t s\n-DOC\n-STR\n",
            ),
            (
                "---\nscalar\n%YAML 1.2\n",
                "+STR\n+DOC ---\n=VAL :scalar %YAML 1.2\n-DOC\n-STR\n",
            ),
            (
                "Bare\ndocument\n...\n|\n%!PS-Adobe-2.0 # Not the first line\n",
                "+STR\n+DOC\n=VAL :Bare document\n-DOC ...\n+DOC\n=VAL |%!PS-Adobe-2.0 # Not the first line\\n\n-DOC\n-STR\n",
            ),
        ] {
            let doc = YamlDoc::parse(input).expect("valid root plain scalar continuation");

            assert_eq!(doc.to_string(), input);
            assert_eq!(doc.events_to_test_string(), expected);
        }
    }

    #[test]
    fn parser_rejects_invalid_compact_block_collection_values() {
        for input in [
            "key: - a\n     - b\n",
            "--- &anchor a: b\n",
            "key:\n - bar\n - baz\n invalid\n",
            "---\nflow: [a,\nb,\nc]\n",
            "---\n[ key\n  : value ]\n",
        ] {
            let error = YamlDoc::parse(input).expect_err("compact block syntax is invalid");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        }
    }

    #[test]
    fn parser_rejects_invalid_scalar_termination_and_orphaned_block_content() {
        for input in [
            "this\n is\n  invalid: x\n",
            "- item1\n- item2\ninvalid\n",
            "k1: v1\n k2: v2\n",
            "word1  # comment\nword2\n",
            "key:\n  word1 word2\n  no: key\n",
            "key2: \"quoted2\" trailing content\n",
            "key: \"value\"# invalid comment\n",
            "a: b: c: d\n",
            "a: 'b': c\n",
        ] {
            let error = YamlDoc::parse(input).expect_err("invalid scalar syntax is rejected");

            assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser, "{input:?}");
        }
    }

    #[test]
    fn parser_preserves_valid_scalar_termination_neighbors() {
        for input in [
            "---\nscalar\n%YAML 1.2\n",
            "key: \"value\" # separated comment\n",
            "url: http://foo.com\n",
            "{key: value:with:colons}\n",
        ] {
            let doc = YamlDoc::parse(input).unwrap_or_else(|error| {
                panic!("nearby valid scalar syntax remains valid for {input:?}: {error:?}")
            });

            assert_eq!(doc.to_string(), input);
        }
    }

    #[test]
    fn parser_rejects_directive_followed_by_document_end_without_document() {
        let error =
            YamlDoc::parse("%YAML 1.2\n...\n").expect_err("document end cannot start a document");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
    }

    #[test]
    fn parser_reports_tabs_in_indentation() {
        let error =
            YamlDoc::parse("root:\n\tchild: value\n").expect_err("tabs are invalid indentation");

        assert_eq!(error.diagnostic.kind, DiagnosticKind::Parser);
        assert_eq!(error.diagnostic.span, Span::new(6, 7));
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
    fn parser_accepts_empty_block_mapping_key() {
        let doc = YamlDoc::parse(
            ": value
",
        )
        .expect("empty mapping keys are valid YAML");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :\n=VAL :value\n-MAP\n-DOC\n-STR\n"
        );
    }

    #[test]
    fn parser_builds_empty_scalar_values() {
        let doc = YamlDoc::parse(
            "key:
items:
  -
flow: {empty:}
",
        )
        .expect("empty nodes are valid YAML scalars in the accepted subset");

        assert_eq!(
            doc.events_to_test_string(),
            "+STR\n+DOC\n+MAP\n=VAL :key\n=VAL :\n=VAL :items\n+SEQ\n=VAL :\n-SEQ\n=VAL :flow\n+MAP {}\n=VAL :empty\n=VAL :\n-MAP\n-MAP\n-DOC\n-STR\n"
        );
        assert_eq!(
            doc.scalar_value(doc.get_path(&["key"]).expect("lookup").expect("key"))
                .expect("empty mapping scalar reads"),
            ""
        );
        let items = doc
            .get_path(&["items"])
            .expect("lookup")
            .expect("items exists");
        assert_eq!(
            Vec::<String>::read_yaml(&doc, items).expect("empty sequence scalar reads"),
            [String::new()]
        );
        let flow_empty = doc
            .get_path(&["flow", "empty"])
            .expect("lookup")
            .expect("flow empty exists");
        assert_eq!(
            doc.scalar_value(flow_empty)
                .expect("empty flow scalar reads"),
            ""
        );
    }

    #[test]
    fn parser_treats_marker_like_text_as_plain_scalar() {
        for (input, expected) in [
            (
                "---word1\nword2\n",
                "+STR\n+DOC\n=VAL :---word1 word2\n-DOC\n-STR\n",
            ),
            (
                "---\n---word1\nword2\n",
                "+STR\n+DOC ---\n=VAL :---word1 word2\n-DOC\n-STR\n",
            ),
        ] {
            let doc = YamlDoc::parse(input).expect("marker-like text is plain scalar content");

            assert_eq!(doc.events_to_test_string(), expected);
            assert_eq!(doc.to_string(), input);
        }
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
            .map(|(index, _)| NodeId::from_usize(index))
    }

    fn folded_scalar(doc: &YamlDoc) -> Option<NodeId> {
        doc.nodes
            .iter()
            .enumerate()
            .find(|(_, node)| node.kind == NodeKind::FoldedScalar)
            .map(|(index, _)| NodeId::from_usize(index))
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
