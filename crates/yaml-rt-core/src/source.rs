use crate::{Diagnostic, DiagnosticKind, YamlError};

/// YAML version targeted by this workspace.
pub const TARGET_YAML_VERSION: &str = "1.2.2";

/// Identifier for a node stored inside a [`crate::YamlDoc`].
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

    pub(crate) fn usize_to_u32(offset: usize) -> u32 {
        u32::try_from(offset).expect("YAML source is too large for u32-based spans")
    }

    pub(crate) fn offset_from_usize(base: u32, offset: usize) -> u32 {
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
    line_starts: Vec<u32>,
    line_facts: Vec<LineFacts>,
}

const NO_LINE_OFFSET: u16 = u16::MAX;
const NO_LINE_INDEX: u32 = u32::MAX;
const LINE_BLANK: u16 = 1 << 0;
const LINE_SIMPLE_MAPPING: u16 = 1 << 1;
const LINE_OFFSET_OVERFLOW: u16 = 1 << 2;
const LINE_COMMENT: u16 = 1 << 3;
const LINE_SCALAR_PLAIN: u16 = 1 << 4;
const LINE_SCALAR_SINGLE_QUOTED: u16 = 1 << 5;
const LINE_SCALAR_DOUBLE_QUOTED: u16 = 1 << 6;
const LINE_FACTS_MIN_SOURCE_BYTES: usize = 1024;

/// Compact facts for the common block-line parser path.
///
/// Offsets are relative to the start of the line. Exceptionally long lines use
/// the existing full scanners instead of retaining wider offsets for every
/// ordinary line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineFacts {
    indent: u16,
    mapping_colon: u16,
    value_start: u16,
    scalar_end: u16,
    flags: u16,
    next_significant: u32,
}

impl LineFacts {
    const FALLBACK: Self = Self {
        indent: NO_LINE_OFFSET,
        mapping_colon: NO_LINE_OFFSET,
        value_start: NO_LINE_OFFSET,
        scalar_end: NO_LINE_OFFSET,
        flags: LINE_OFFSET_OVERFLOW,
        next_significant: NO_LINE_INDEX,
    };

    pub(crate) fn indent(self) -> Option<usize> {
        (!self.has(LINE_OFFSET_OVERFLOW)).then_some(self.indent as usize)
    }

    pub(crate) fn simple_mapping(self) -> Option<(usize, usize)> {
        self.has(LINE_SIMPLE_MAPPING)
            .then_some((self.mapping_colon as usize, self.value_start as usize))
    }

    pub(crate) fn mapping_colon(self) -> Option<usize> {
        (self.mapping_colon != NO_LINE_OFFSET).then_some(self.mapping_colon as usize)
    }

    pub(crate) fn scalar_mapping(self) -> Option<(usize, usize, usize, CachedScalarStyle)> {
        let style = if self.has(LINE_SCALAR_PLAIN) {
            CachedScalarStyle::Plain
        } else if self.has(LINE_SCALAR_SINGLE_QUOTED) {
            CachedScalarStyle::SingleQuoted
        } else if self.has(LINE_SCALAR_DOUBLE_QUOTED) {
            CachedScalarStyle::DoubleQuoted
        } else {
            return None;
        };
        Some((
            self.mapping_colon as usize,
            self.value_start as usize,
            self.scalar_end as usize,
            style,
        ))
    }

    pub(crate) const fn is_blank(self) -> bool {
        self.has(LINE_BLANK)
    }

    fn next_significant(self) -> Option<usize> {
        (self.next_significant != NO_LINE_INDEX).then_some(self.next_significant as usize)
    }

    const fn has(self, flag: u16) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CachedScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
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
        let bytes = text.as_bytes();
        let mut line_starts = Vec::with_capacity(text.len() / 32 + 1);
        line_starts.push(0);
        const SOURCE_SCAN_CHUNK: usize = 32;
        let (chunks, remainder) = bytes.as_chunks::<SOURCE_SCAN_CHUNK>();
        for (chunk_index, chunk) in chunks.iter().enumerate() {
            validate_source_chunk(
                &text,
                bytes,
                chunk_index * SOURCE_SCAN_CHUNK,
                chunk,
                &mut line_starts,
            )?;
        }
        validate_source_chunk(
            &text,
            bytes,
            chunks.len() * SOURCE_SCAN_CHUNK,
            remainder,
            &mut line_starts,
        )?;
        let line_facts = if text.len() >= LINE_FACTS_MIN_SOURCE_BYTES {
            build_line_facts(&text, &line_starts)
        } else {
            Vec::new()
        };

        Ok(Self {
            text,
            line_starts,
            line_facts,
        })
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
    pub fn line_starts(&self) -> &[u32] {
        &self.line_starts
    }

    pub(crate) fn line_facts(&self, index: usize) -> LineFacts {
        self.line_facts
            .get(index)
            .copied()
            .unwrap_or(LineFacts::FALLBACK)
    }

    pub(crate) fn cached_next_significant_line(&self, index: usize) -> Option<Option<usize>> {
        self.line_facts
            .get(index)
            .copied()
            .map(LineFacts::next_significant)
    }

    pub(crate) fn has_line_facts(&self) -> bool {
        !self.line_facts.is_empty()
    }

    pub(crate) fn validated_line_slice(&self, start: usize, end: usize) -> &str {
        debug_assert!(start <= end);
        debug_assert!(end <= self.text.len());
        debug_assert!(self.text.is_char_boundary(start));
        debug_assert!(self.text.is_char_boundary(end));
        // SAFETY: line offsets are produced while scanning this owned, valid
        // UTF-8 string. The assertions retain those invariants in debug builds.
        unsafe { self.text.get_unchecked(start..end) }
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
        let offset = Span::usize_to_u32(offset.min(self.text.len()));
        let line_index = match self.line_starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let line_start = self.line_starts[line_index];

        LineCol {
            line: line_index + 1,
            column: (offset - line_start) as usize + 1,
        }
    }

    /// Returns the line/column pair for a diagnostic's primary span.
    #[must_use]
    pub fn diagnostic_position(&self, diagnostic: &Diagnostic) -> LineCol {
        self.line_col(diagnostic.span.start as usize)
    }
}

fn validate_source_chunk(
    text: &str,
    bytes: &[u8],
    base: usize,
    chunk: &[u8],
    line_starts: &mut Vec<u32>,
) -> Result<(), YamlError> {
    let mut cursor = 0;
    while let Some(relative) = chunk[cursor..]
        .iter()
        .position(|byte| *byte < b' ' || matches!(*byte, 0x7F | 0xC2 | 0xEF))
    {
        let relative = cursor + relative;
        let byte = chunk[relative];
        let offset = base + relative;
        if byte < 0x80 {
            if !matches!(byte, b'\t' | b'\n' | b'\r' | b' '..=b'~') {
                return Err(invalid_yaml_character(offset, char::from(byte)));
            }
            if byte == b'\n' {
                line_starts.push(Span::usize_to_u32(offset + 1));
            }
        } else if unicode_sequence_may_be_non_printable(bytes, offset) {
            let character = text[offset..]
                .chars()
                .next()
                .expect("offset starts a valid UTF-8 character");
            if !is_yaml_printable(character) {
                return Err(invalid_yaml_character(offset, character));
            }
        }
        cursor = relative + 1;
    }
    Ok(())
}

fn unicode_sequence_may_be_non_printable(bytes: &[u8], offset: usize) -> bool {
    match bytes[offset..] {
        [0xC2, continuation, ..] => (0x80..=0x9F).contains(&continuation) && continuation != 0x85,
        [0xEF, 0xBF, 0xBE | 0xBF, ..] => true,
        _ => false,
    }
}

fn build_line_facts(text: &str, line_starts: &[u32]) -> Vec<LineFacts> {
    let mut facts = Vec::with_capacity(line_starts.len());
    for (index, &start) in line_starts.iter().enumerate() {
        let start = start as usize;
        let mut end = line_starts
            .get(index + 1)
            .map_or(text.len(), |next| *next as usize);
        if end > start && text.as_bytes()[end - 1] == b'\n' {
            end -= 1;
            if end > start && text.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
        } else if end > start && text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        facts.push(analyze_line(&text.as_bytes()[start..end]));
    }
    populate_next_significant_lines(&mut facts);
    facts
}

fn populate_next_significant_lines(facts: &mut [LineFacts]) {
    let mut next = NO_LINE_INDEX;
    for (index, fact) in facts.iter_mut().enumerate().rev() {
        fact.next_significant = next;
        if !fact.has(LINE_BLANK | LINE_COMMENT) {
            next = u32::try_from(index).expect("line index exceeds u32 capacity");
        }
    }
}

fn analyze_line(line: &[u8]) -> LineFacts {
    let indent = line.iter().take_while(|byte| **byte == b' ').count();
    let mut flags = 0;
    if line[indent..].is_empty() {
        flags |= LINE_BLANK;
    } else if line[indent] == b'#' {
        flags |= LINE_COMMENT;
    }

    if line.len() >= NO_LINE_OFFSET as usize {
        return LineFacts {
            flags: flags | LINE_OFFSET_OVERFLOW,
            ..LineFacts::FALLBACK
        };
    }

    let Some(indent) = u16::try_from(indent)
        .ok()
        .filter(|value| *value != NO_LINE_OFFSET)
    else {
        return LineFacts {
            indent: NO_LINE_OFFSET,
            mapping_colon: NO_LINE_OFFSET,
            value_start: NO_LINE_OFFSET,
            scalar_end: NO_LINE_OFFSET,
            flags: flags | LINE_OFFSET_OVERFLOW,
            next_significant: NO_LINE_INDEX,
        };
    };

    let body = &line[indent as usize..];
    if let Some((colon, value_start)) = plain_key_mapping_offsets(body)
        && let Ok(colon) = u16::try_from(colon)
        && colon != NO_LINE_OFFSET
    {
        let scalar_value_start = value_start;
        let value_start = scalar_value_start
            .and_then(|offset| u16::try_from(offset).ok())
            .filter(|offset| *offset != NO_LINE_OFFSET);
        let mut scalar_end = NO_LINE_OFFSET;
        if let Some(start) = scalar_value_start
            && let Some((end, scalar_flag)) = cached_scalar_offsets(&body[start..])
            && let Ok(end) = u16::try_from(start + end)
            && end != NO_LINE_OFFSET
        {
            scalar_end = end;
            flags |= scalar_flag;
            if scalar_flag == LINE_SCALAR_PLAIN {
                flags |= LINE_SIMPLE_MAPPING;
            }
        }
        return LineFacts {
            indent,
            mapping_colon: colon,
            value_start: value_start.unwrap_or(NO_LINE_OFFSET),
            scalar_end,
            flags,
            next_significant: NO_LINE_INDEX,
        };
    }

    LineFacts {
        indent,
        mapping_colon: NO_LINE_OFFSET,
        value_start: NO_LINE_OFFSET,
        scalar_end: NO_LINE_OFFSET,
        flags,
        next_significant: NO_LINE_INDEX,
    }
}

fn plain_key_mapping_offsets(body: &[u8]) -> Option<(usize, Option<usize>)> {
    let mut colon = 0;
    while colon < body.len() && body[colon] != b':' {
        if !is_simple_plain_byte(body[colon]) {
            return None;
        }
        colon += 1;
    }
    if colon == 0 || colon == body.len() {
        return None;
    }

    let mut value_start = colon + 1;
    if value_start == body.len() {
        return Some((colon, None));
    }
    if body.get(value_start) != Some(&b' ') {
        return None;
    }
    while body.get(value_start) == Some(&b' ') {
        value_start += 1;
    }
    if value_start == body.len() || &body[value_start..] == b"-" {
        return Some((colon, None));
    }
    Some((colon, Some(value_start)))
}

fn cached_scalar_offsets(text: &[u8]) -> Option<(usize, u16)> {
    match text.first().copied()? {
        b'"' => {
            let end = text[1..]
                .iter()
                .position(|byte| matches!(*byte, b'"' | b'\\'))?
                + 1;
            if text[end] == b'\\' || !valid_cached_quoted_trailing_text(&text[end + 1..]) {
                return None;
            }
            Some((end + 1, LINE_SCALAR_DOUBLE_QUOTED))
        }
        b'\'' => {
            let mut position = 1;
            loop {
                let quote = text[position..].iter().position(|byte| *byte == b'\'')? + position;
                if text.get(quote + 1) == Some(&b'\'') {
                    position = quote + 2;
                    continue;
                }
                if !valid_cached_quoted_trailing_text(&text[quote + 1..]) {
                    return None;
                }
                return Some((quote + 1, LINE_SCALAR_SINGLE_QUOTED));
            }
        }
        _ if text.iter().all(|byte| is_simple_plain_byte(*byte)) => {
            Some((text.len(), LINE_SCALAR_PLAIN))
        }
        _ => None,
    }
}

fn valid_cached_quoted_trailing_text(trailing: &[u8]) -> bool {
    if trailing.iter().all(|byte| *byte == b' ') {
        return true;
    }
    let whitespace = trailing.iter().take_while(|byte| **byte == b' ').count();
    whitespace > 0 && trailing.get(whitespace) == Some(&b'#')
}

const fn is_simple_plain_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/')
}

fn invalid_yaml_character(offset: usize, character: char) -> YamlError {
    let span = Span::from_usize(offset, offset + character.len_utf8());
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Source,
            format!("invalid YAML 1.2.2 character U+{:04X}", character as u32),
            span,
        )
        .with_note(
            "YAML streams may contain tab, line feeds, carriage returns, printable Unicode characters, and non-breaking spaces",
        ),
    )
}

pub(crate) fn validate_yaml_chars(text: &str) -> Result<(), YamlError> {
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

#[cfg(test)]
mod line_facts_tests {
    use std::fmt::Write;

    use super::*;
    use crate::parser::Parser;
    use crate::semantic::SemanticStore;
    use crate::{Node, ParsedYaml, YamlDoc, YamlEvent};

    #[derive(Debug, PartialEq, Eq)]
    struct ParseFingerprint {
        nodes: Vec<Node>,
        semantics: SemanticStore,
        events: Vec<YamlEvent>,
        rendering: String,
    }

    fn parse_fingerprint(source: Source, parsed: ParsedYaml) -> ParseFingerprint {
        let document = YamlDoc {
            source,
            nodes: parsed.nodes.clone(),
            semantics: parsed.semantics.clone(),
            root_override: None,
            edits: Vec::new(),
        };
        ParseFingerprint {
            nodes: parsed.nodes,
            semantics: parsed.semantics,
            events: document.events().collect(),
            rendering: document.to_string(),
        }
    }

    #[test]
    fn caches_common_lines_and_leaves_complex_lines_on_the_fallback_path() {
        let source = Source::new(
            "alpha: beta\r\nunicode: café\n\tbad: tab\n\"quoted\": value\n&anchor key: value\nflow: [one, two]\nkey: value # comment\n# comment\nliteral: |\n  text\n"
                .to_owned(),
        )
        .expect("fixture is printable YAML");

        let facts = build_line_facts(source.as_str(), source.line_starts());
        assert_eq!(facts[0].simple_mapping(), Some((5, 7)));
        assert_eq!(facts[1].mapping_colon(), Some(7));
        assert_eq!(facts[1].simple_mapping(), None);
        assert_eq!(facts[2].mapping_colon(), None);
        assert_eq!(facts[3].mapping_colon(), None);
        assert_eq!(facts[4].mapping_colon(), None);
        assert_eq!(facts[5].mapping_colon(), Some(4));
        assert_eq!(facts[5].simple_mapping(), None);
        assert_eq!(facts[6].mapping_colon(), Some(3));
        assert_eq!(facts[6].simple_mapping(), None);
        assert_eq!(facts[7].mapping_colon(), None);
        assert_eq!(facts[8].mapping_colon(), Some(7));
        assert_eq!(facts[9].indent(), Some(2));
        assert_eq!(facts[5].next_significant(), Some(6));
        assert_eq!(facts[6].next_significant(), Some(8));
        assert_eq!(facts[9].next_significant(), None);
        assert_eq!(std::mem::size_of::<LineFacts>(), 16);
    }

    #[test]
    fn cached_scalar_facts_match_the_general_parser() {
        let mut input = String::from("root:\n");
        for index in 0..40 {
            writeln!(input, "  plain_{index}: value_{index}")
                .expect("writing to a String cannot fail");
            writeln!(input, "  single_{index}: 'quoted # {index}' # trailing")
                .expect("writing to a String cannot fail");
            writeln!(
                input,
                "  double_{index}: \"Unicode café {index}\" # trailing"
            )
            .expect("writing to a String cannot fail");
        }

        let optimized = Source::new(input.clone()).expect("fixture is printable YAML");
        assert!(optimized.line_facts(2).scalar_mapping().is_some());
        assert!(optimized.line_facts(3).scalar_mapping().is_some());
        let optimized_parse = Parser::new(&optimized)
            .parse()
            .expect("optimized fixture should parse");

        let mut general = Source::new(input).expect("fixture is printable YAML");
        general.line_facts.clear();
        let general_parse = Parser::new(&general)
            .parse()
            .expect("general fixture should parse");
        assert_eq!(
            parse_fingerprint(optimized, optimized_parse),
            parse_fingerprint(general, general_parse)
        );
    }

    #[test]
    fn cached_line_path_matches_fallback_diagnostics() {
        let input = format!(
            "{}---\nquoted: \"a\nb\nc\"\n",
            "# oracle padding\n".repeat(80)
        );
        let optimized = Source::new(input.clone()).expect("fixture is printable YAML");
        let optimized_error = Parser::new(&optimized)
            .parse()
            .expect_err("unindented quoted continuation is invalid")
            .with_position_from(&optimized);

        let mut general = Source::new(input).expect("fixture is printable YAML");
        general.line_facts.clear();
        let general_error = Parser::new(&general)
            .parse()
            .expect_err("fallback path must reject the same input")
            .with_position_from(&general);

        assert_eq!(optimized_error.diagnostic, general_error.diagnostic);
    }

    #[test]
    fn cached_common_mapping_path_preserves_the_complete_source() {
        let mut input = String::new();
        for index in 0..100 {
            writeln!(input, "key_{index:04}: value_{index:04}")
                .expect("writing to a String cannot fail");
        }
        let source = Source::new(input.clone()).expect("generated mapping is printable YAML");
        assert_eq!(source.line_facts(0).simple_mapping(), Some((8, 10)));

        let doc = YamlDoc::parse(&input).expect("cached mapping should parse");
        assert_eq!(doc.to_string(), input);
    }

    #[test]
    fn long_line_offsets_fall_back_without_changing_parse_behavior() {
        let key = "k".repeat(u16::MAX as usize);
        let input = format!("{key}: value\n");
        let source = Source::new(input.clone()).expect("long fixture is printable YAML");
        assert_eq!(source.line_facts(0).mapping_colon(), None);

        let doc = YamlDoc::parse(&input).expect("long mapping key should use the full scanner");
        assert_eq!(doc.to_string(), input);
    }
}
