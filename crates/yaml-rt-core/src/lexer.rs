use crate::{Diagnostic, DiagnosticKind, Source, Span, YamlError};

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
            tokens: Vec::with_capacity(source.len() / 4 + 1),
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
            } else if self.can_start_quoted_scalar() && self.consume_double_quoted_scalar()? {
                self.push(TokenKind::DoubleQuotedScalar, start);
            } else if self.can_start_quoted_scalar() && self.consume_single_quoted_scalar()? {
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
                ' ' | '\t' | '\r' | '\n' | '#' | ':' | '-' | '[' | ']' | '{' | '}' | ',' | '?'
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

    fn can_start_quoted_scalar(&self) -> bool {
        self.tokens.last().is_none_or(|token| {
            matches!(
                token.kind,
                TokenKind::Bom
                    | TokenKind::Whitespace
                    | TokenKind::Newline
                    | TokenKind::Colon
                    | TokenKind::Dash
                    | TokenKind::Question
                    | TokenKind::FlowSequenceStart
                    | TokenKind::FlowMappingStart
                    | TokenKind::Comma
            )
        })
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
