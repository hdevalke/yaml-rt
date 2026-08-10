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
const LINE_BLANK: u16 = 1 << 0;
const LINE_SIMPLE_MAPPING: u16 = 1 << 1;
const LINE_OFFSET_OVERFLOW: u16 = 1 << 2;
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
    flags: u16,
}

impl LineFacts {
    const FALLBACK: Self = Self {
        indent: NO_LINE_OFFSET,
        mapping_colon: NO_LINE_OFFSET,
        value_start: NO_LINE_OFFSET,
        flags: LINE_OFFSET_OVERFLOW,
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

    pub(crate) const fn is_blank(self) -> bool {
        self.has(LINE_BLANK)
    }

    const fn has(self, flag: u16) -> bool {
        self.flags & flag != 0
    }
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
        let mut line_starts = Vec::with_capacity(text.len() / 32 + 1);
        line_starts.push(0);
        if text.is_ascii() {
            for (offset, byte) in text.bytes().enumerate() {
                if !matches!(byte, b'\t' | b'\n' | b'\r' | b' '..=b'~') {
                    return Err(invalid_yaml_character(offset, char::from(byte)));
                }
                if byte == b'\n' {
                    line_starts.push(Span::usize_to_u32(offset + 1));
                }
            }
        } else {
            for (offset, character) in text.char_indices() {
                if !is_yaml_printable(character) {
                    return Err(invalid_yaml_character(offset, character));
                }
                if character == '\n' {
                    line_starts.push(Span::usize_to_u32(offset + 1));
                }
            }
        }
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
    facts
}

fn analyze_line(line: &[u8]) -> LineFacts {
    let indent = line.iter().take_while(|byte| **byte == b' ').count();
    let mut flags = 0;
    if line[indent..].is_empty() {
        flags |= LINE_BLANK;
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
            flags: flags | LINE_OFFSET_OVERFLOW,
        };
    };

    let body = &line[indent as usize..];
    if let Some((colon, value_start)) = plain_key_mapping_offsets(body)
        && let Ok(colon) = u16::try_from(colon)
        && colon != NO_LINE_OFFSET
    {
        let value_start = value_start
            .and_then(|offset| u16::try_from(offset).ok())
            .filter(|offset| *offset != NO_LINE_OFFSET);
        if value_start.is_some() {
            flags |= LINE_SIMPLE_MAPPING;
        }
        return LineFacts {
            indent,
            mapping_colon: colon,
            value_start: value_start.unwrap_or(NO_LINE_OFFSET),
            flags,
        };
    }

    LineFacts {
        indent,
        mapping_colon: NO_LINE_OFFSET,
        value_start: NO_LINE_OFFSET,
        flags,
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
    for &byte in &body[value_start..] {
        if !is_simple_plain_byte(byte) {
            return Some((colon, None));
        }
    }
    Some((colon, Some(value_start)))
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
    use crate::YamlDoc;

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
