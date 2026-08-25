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

    /// Creates a source-aware renderer for this error.
    #[must_use]
    pub fn render<'a>(&'a self, source: &'a str) -> DiagnosticRenderer<'a> {
        self.diagnostic.render(source)
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

    /// Creates a source-aware renderer for this diagnostic.
    #[must_use]
    pub const fn render<'a>(&'a self, source: &'a str) -> DiagnosticRenderer<'a> {
        DiagnosticRenderer {
            diagnostic: self,
            source,
            source_name: None,
            color: DiagnosticColor::Never,
        }
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

impl DiagnosticKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Lexer => "lexer",
            Self::Parser => "parser",
            Self::Semantic => "semantic",
            Self::Typed => "typed",
            Self::Emitter => "emitter",
        }
    }
}

/// ANSI color policy for source-aware diagnostic rendering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiagnosticColor {
    /// Never emit ANSI escape sequences.
    #[default]
    Never,
    /// Use standard terminal foreground colors and bold emphasis.
    Always,
}

/// A source-aware, rustc-style diagnostic display adapter.
#[derive(Debug, Clone, Copy)]
pub struct DiagnosticRenderer<'a> {
    diagnostic: &'a Diagnostic,
    source: &'a str,
    source_name: Option<&'a str>,
    color: DiagnosticColor,
}

impl<'a> DiagnosticRenderer<'a> {
    /// Sets the filename or logical input name shown in the location header.
    #[must_use]
    pub const fn with_source_name(mut self, source_name: &'a str) -> Self {
        self.source_name = Some(source_name);
        self
    }

    /// Sets the ANSI color policy.
    #[must_use]
    pub const fn with_color(mut self, color: DiagnosticColor) -> Self {
        self.color = color;
        self
    }

    fn styled(
        &self,
        formatter: &mut fmt::Formatter<'_>,
        code: &str,
        value: impl fmt::Display,
    ) -> fmt::Result {
        if self.color == DiagnosticColor::Always {
            write!(formatter, "\x1b[{code}m{value}\x1b[0m")
        } else {
            write!(formatter, "{value}")
        }
    }
}

impl fmt::Display for DiagnosticRenderer<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.styled(formatter, "1;31", "error")?;
        write!(
            formatter,
            "[{}]: {}",
            self.diagnostic.kind.label(),
            self.diagnostic.message
        )?;

        let source_len = self.source.len();
        let start = (self.diagnostic.span.start as usize).min(source_len);
        let requested_end = self.diagnostic.span.end as usize;
        let end = requested_end.max(start).min(source_len);
        if !self.source.is_char_boundary(start) || !self.source.is_char_boundary(end) {
            return write!(formatter, "\n{}", self.diagnostic);
        }

        let starts = line_starts(self.source);
        let line_index = starts
            .partition_point(|offset| *offset <= start)
            .saturating_sub(1);
        let line_start = starts[line_index];
        let line_end = source_line_end(self.source, &starts, line_index);
        let position = LineCol {
            line: line_index + 1,
            column: display_width(&self.source[line_start..start]) + 1,
        };

        write!(formatter, "\n ")?;
        self.styled(formatter, "1;34", "-->")?;
        write!(
            formatter,
            " {}:{}:{}",
            self.source_name.unwrap_or("<input>"),
            position.line,
            position.column
        )?;

        let first_shown = line_index.saturating_sub(1);
        let width = (line_index + 1).to_string().len();
        write!(formatter, "\n{:width$} ", "", width = width)?;
        self.styled(formatter, "1;34", "|")?;
        for shown in first_shown..=line_index {
            let shown_start = starts[shown];
            let shown_end = source_line_end(self.source, &starts, shown);
            let text = expand_tabs(&self.source[shown_start..shown_end]);
            write!(formatter, "\n{:>width$} ", shown + 1, width = width)?;
            self.styled(formatter, "1;34", "|")?;
            write!(formatter, " {text}")?;
        }

        let prefix_width = display_width(&self.source[line_start..start]);
        let underline_end = end.min(line_end);
        let underline_width = if underline_end > start {
            display_width(&self.source[start..underline_end]).max(1)
        } else {
            1
        };
        write!(formatter, "\n{:width$} ", "", width = width)?;
        self.styled(formatter, "1;34", "|")?;
        write!(formatter, " {}", " ".repeat(prefix_width))?;
        self.styled(formatter, "1;31", "^".repeat(underline_width))?;
        if !self.diagnostic.expected.is_empty() {
            write!(
                formatter,
                " expected {}",
                self.diagnostic.expected.join(", ")
            )?;
        }

        let end_line = starts
            .partition_point(|offset| *offset <= end)
            .saturating_sub(1);
        if end_line > line_index || requested_end > source_len {
            write!(formatter, "\n{:width$} ", "", width = width)?;
            self.styled(formatter, "1;34", "|")?;
            write!(formatter, " ... span continues")?;
        }
        write!(formatter, "\n{:width$} ", "", width = width)?;
        self.styled(formatter, "1;34", "|")?;

        for note in &self.diagnostic.notes {
            writeln!(formatter)?;
            self.styled(formatter, "1;32", "note")?;
            write!(formatter, ": {note}")?;
        }
        Ok(())
    }
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        source
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    starts
}

fn source_line_end(source: &str, starts: &[usize], line_index: usize) -> usize {
    let mut end = starts.get(line_index + 1).copied().unwrap_or(source.len());
    if end > starts[line_index] && source.as_bytes()[end - 1] == b'\n' {
        end -= 1;
    }
    if end > starts[line_index] && source.as_bytes()[end - 1] == b'\r' {
        end -= 1;
    }
    end
}

fn display_width(text: &str) -> usize {
    text.chars().fold(0, |width, character| match character {
        '\t' => width + (4 - width % 4),
        _ => width + 1,
    })
}

fn expand_tabs(text: &str) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut width = 0;
    for character in text.chars() {
        if character == '\t' {
            let spaces = 4 - width % 4;
            expanded.push_str(&" ".repeat(spaces));
            width += spaces;
        } else {
            expanded.push(character);
            width += 1;
        }
    }
    expanded
}

/// Alias for parse errors until richer phase-specific errors are introduced.
pub type ParseError = YamlError;
