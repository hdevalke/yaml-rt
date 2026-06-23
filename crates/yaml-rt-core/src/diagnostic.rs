use std::fmt;

use crate::{LineCol, Source, Span};

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
