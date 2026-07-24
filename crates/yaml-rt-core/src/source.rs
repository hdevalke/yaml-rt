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
    pub fn line_starts(&self) -> &[u32] {
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
