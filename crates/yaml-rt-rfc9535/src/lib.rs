//! Native RFC 9535 `JSONPath` queries over yaml-rt semantic documents.
//!
//! Queries are parsed once and can be evaluated repeatedly without converting
//! YAML into an intermediate JSON value tree.
//!
//! ```
//! use yaml_rt_core::YamlDoc;
//! use yaml_rt_rfc9535::JsonPath;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let doc = YamlDoc::parse("users: [{name: Ada}, {name: Linus}]\n")?;
//! let path = JsonPath::parse("$.users[*].name")?;
//! let matches = path.query(&doc, 0)?;
//! let pointers = matches
//!     .iter()
//!     .map(|matched| matched.pointer().as_str())
//!     .collect::<Vec<_>>();
//! assert_eq!(pointers, ["/users/0/name", "/users/1/name"]);
//! # Ok(())
//! # }
//! ```

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;

use regex::Regex;
use yaml_rt_core::{
    JsonPointer, NodeId, ResolvedScalar, SemanticKind, Span, YamlDoc, YamlNumber, YamlScalarStyle,
    resolve_scalar, semantically_equal,
};

const MAX_QUERY_DEPTH: usize = 128;
const MAX_VALUE_DEPTH: usize = 1024;
const MAP_TAG: &str = "tag:yaml.org,2002:map";
const SEQ_TAG: &str = "tag:yaml.org,2002:seq";

/// Classification of a `JSONPath` parse or evaluation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Invalid RFC 9535 syntax.
    Syntax,
    /// Invalid regular expression used by `match()` or `search()`.
    Regex,
    /// An ill-typed function or comparison expression.
    Type,
    /// The selected YAML value is outside the JSON data model.
    DataModel,
    /// An alias is unresolved, cyclic, or recursively expands a collection.
    Alias,
    /// A parser, traversal, nesting, or expansion limit was exceeded.
    Limit,
    /// The requested YAML document does not exist.
    Document,
    /// Another YAML semantic operation failed.
    Semantic,
}

/// A structured `JSONPath` parse or evaluation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    byte_offset: Option<usize>,
    message: String,
    source_span: Option<Span>,
}

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self::with_kind(ErrorKind::Semantic, None, message)
    }

    fn display(error: impl fmt::Display) -> Self {
        Self::new(error.to_string())
    }

    fn with_kind(kind: ErrorKind, byte_offset: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            kind,
            byte_offset,
            message: message.into(),
            source_span: None,
        }
    }

    fn with_source_span(mut self, source_span: Span) -> Self {
        self.source_span = Some(source_span);
        self
    }

    /// Returns the broad failure classification.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns the failing byte offset in the query, when applicable.
    #[must_use]
    pub const fn byte_offset(&self) -> Option<usize> {
        self.byte_offset
    }

    /// Returns the relevant YAML source span for an evaluation failure.
    #[must_use]
    pub const fn source_span(&self) -> Option<Span> {
        self.source_span
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

type QueryError = Error;

/// A parsed and reusable RFC 9535 `JSONPath` query.
#[derive(Debug, Clone)]
pub struct JsonPath {
    segments: Vec<Segment>,
}

impl JsonPath {
    /// Parses and type-checks one RFC 9535 query.
    ///
    /// # Errors
    ///
    /// Returns an error when the query has invalid syntax, exceeds a parser
    /// limit, or contains an ill-typed expression.
    pub fn parse(source: &str) -> Result<Self, Error> {
        Parser::new(source).parse()
    }

    /// Evaluates this query against one selected YAML document.
    ///
    /// The complete document is first validated against the JSON-compatible
    /// data model. Result order and duplicate matches follow RFC 9535 nodelist
    /// semantics.
    ///
    /// # Errors
    ///
    /// Returns an error when the document does not exist, is outside the JSON
    /// data model, contains invalid alias structure, or evaluation fails.
    pub fn query(&self, doc: &YamlDoc, document: usize) -> Result<QueryMatches, Error> {
        let root = Candidate {
            node: doc
                .document_root(document)
                .map_err(|error| Error::with_kind(ErrorKind::Document, None, error.to_string()))?,
            path: Vec::new(),
        };
        let budget = doc.as_source().len().saturating_mul(100).max(10_000);
        let mut validator = Validator::new(doc, budget);
        validator.validate_candidate(&root)?;

        let mut evaluator = Evaluator::new(doc, root.clone(), budget);
        let matches = evaluator.apply_segments(vec![root], &self.segments)?;
        let matches = matches
            .into_iter()
            .map(|matched| {
                let pointer =
                    JsonPointer::parse(&json_pointer(&matched.path)).map_err(Error::display)?;
                Ok(QueryMatch {
                    pointer,
                    node: matched.node,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(QueryMatches { matches })
    }
}

impl std::str::FromStr for JsonPath {
    type Err = Error;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        Self::parse(source)
    }
}

/// One located `JSONPath` match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryMatch {
    pointer: JsonPointer,
    node: Option<NodeId>,
}

impl QueryMatch {
    /// Returns the RFC 6901 pointer for this logical match location.
    #[must_use]
    pub const fn pointer(&self) -> &JsonPointer {
        &self.pointer
    }

    /// Returns the matching semantic node, or `None` for an empty document.
    #[must_use]
    pub const fn node(&self) -> Option<NodeId> {
        self.node
    }
}

/// Ordered `JSONPath` query results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryMatches {
    matches: Vec<QueryMatch>,
}

impl QueryMatches {
    /// Returns the number of matches, including duplicates.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.matches.len()
    }

    /// Returns whether the nodelist is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Iterates over matches in nodelist order.
    pub fn iter(&self) -> std::slice::Iter<'_, QueryMatch> {
        self.matches.iter()
    }
}

impl IntoIterator for QueryMatches {
    type Item = QueryMatch;
    type IntoIter = std::vec::IntoIter<QueryMatch>;

    fn into_iter(self) -> Self::IntoIter {
        self.matches.into_iter()
    }
}

impl<'a> IntoIterator for &'a QueryMatches {
    type Item = &'a QueryMatch;
    type IntoIter = std::slice::Iter<'a, QueryMatch>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Debug, Clone)]
struct Segment {
    descendant: bool,
    selectors: Vec<Selector>,
}

#[derive(Debug, Clone)]
enum Selector {
    Name(String),
    Wildcard,
    Index(i64),
    Slice {
        start: Option<i64>,
        end: Option<i64>,
        step: Option<i64>,
    },
    Filter(Expr),
}

#[derive(Debug, Clone)]
enum Expr {
    Or(Box<Self>, Box<Self>),
    And(Box<Self>, Box<Self>),
    Not(Box<Self>),
    Compare(ValueExpr, CompareOp, ValueExpr),
    Exists(PathExpr),
    Regex {
        value: ValueExpr,
        pattern: RegexPattern,
    },
}

#[derive(Debug, Clone)]
enum RegexPattern {
    Compiled(Regex),
    Dynamic { value: ValueExpr, full: bool },
}

#[derive(Debug, Clone, Copy)]
enum CompareOp {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

#[derive(Debug, Clone)]
enum ValueExpr {
    Literal(Literal),
    Singular(PathExpr),
    Length(Box<Self>),
    Count(PathExpr),
    Value(PathExpr),
}

#[derive(Debug, Clone)]
enum Literal {
    Null,
    Bool(bool),
    Number(YamlNumber),
    String(String),
}

#[derive(Debug, Clone)]
struct PathExpr {
    root: PathRoot,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy)]
enum PathRoot {
    Root,
    Current,
}

struct Parser<'a> {
    input: &'a str,
    offset: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            offset: 0,
            depth: 0,
        }
    }

    fn parse(mut self) -> Result<JsonPath, QueryError> {
        if !self.consume_char('$') {
            return self.error("a JSONPath query must begin with `$`");
        }
        let segments = self.parse_segments()?;
        self.skip_ws();
        if !self.is_end() {
            return self.error("unexpected trailing JSONPath input");
        }
        Ok(JsonPath { segments })
    }

    fn parse_segments(&mut self) -> Result<Vec<Segment>, QueryError> {
        let mut segments = Vec::new();
        loop {
            if self.consume("..") {
                segments.push(self.parse_segment_tail(true)?);
            } else if self.consume_char('.') {
                segments.push(self.parse_segment_tail(false)?);
            } else if self.peek_char() == Some('[') {
                segments.push(self.parse_bracket_segment(false)?);
            } else {
                break;
            }
        }
        Ok(segments)
    }

    fn parse_segment_tail(&mut self, descendant: bool) -> Result<Segment, QueryError> {
        if self.peek_char() == Some('[') {
            return self.parse_bracket_segment(descendant);
        }
        if self.consume_char('*') {
            return Ok(Segment {
                descendant,
                selectors: vec![Selector::Wildcard],
            });
        }
        let name = self.parse_shorthand_name()?;
        Ok(Segment {
            descendant,
            selectors: vec![Selector::Name(name)],
        })
    }

    fn parse_bracket_segment(&mut self, descendant: bool) -> Result<Segment, QueryError> {
        self.expect_char('[')?;
        self.enter()?;
        self.skip_ws();
        let mut selectors = Vec::new();
        loop {
            selectors.push(self.parse_selector()?);
            self.skip_ws();
            if self.consume_char(',') {
                self.skip_ws();
                continue;
            }
            self.expect_char(']')?;
            break;
        }
        self.leave();
        Ok(Segment {
            descendant,
            selectors,
        })
    }

    fn parse_selector(&mut self) -> Result<Selector, QueryError> {
        if self.consume_char('*') {
            return Ok(Selector::Wildcard);
        }
        if self.consume_char('?') {
            self.skip_ws();
            return self.parse_expr().map(Selector::Filter);
        }
        if matches!(self.peek_char(), Some('\'' | '"')) {
            return self.parse_string().map(Selector::Name);
        }

        let start = self.parse_optional_integer()?;
        self.skip_ws();
        if self.consume_char(':') {
            self.skip_ws();
            let end = self.parse_optional_integer()?;
            self.skip_ws();
            let step = if self.consume_char(':') {
                self.skip_ws();
                self.parse_optional_integer()?
            } else {
                None
            };
            if step == Some(0) {
                return self.error("a JSONPath slice step must not be zero");
            }
            return Ok(Selector::Slice { start, end, step });
        }
        start
            .map(Selector::Index)
            .ok_or_else(|| self.error_value("expected a JSONPath selector"))
    }

    fn parse_expr(&mut self) -> Result<Expr, QueryError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, QueryError> {
        let mut expression = self.parse_and()?;
        loop {
            self.skip_ws();
            if !self.consume("||") {
                break;
            }
            self.skip_ws();
            expression = Expr::Or(Box::new(expression), Box::new(self.parse_and()?));
        }
        Ok(expression)
    }

    fn parse_and(&mut self) -> Result<Expr, QueryError> {
        let mut expression = self.parse_unary()?;
        loop {
            self.skip_ws();
            if !self.consume("&&") {
                break;
            }
            self.skip_ws();
            expression = Expr::And(Box::new(expression), Box::new(self.parse_unary()?));
        }
        Ok(expression)
    }

    fn parse_unary(&mut self) -> Result<Expr, QueryError> {
        self.skip_ws();
        if self.consume_char('!') && self.peek_char() != Some('=') {
            self.enter()?;
            let expression = self.parse_unary()?;
            self.leave();
            return Ok(Expr::Not(Box::new(expression)));
        }
        if self.consume_char('(') {
            self.enter()?;
            let expression = self.parse_expr()?;
            self.skip_ws();
            self.expect_char(')')?;
            self.leave();
            return Ok(expression);
        }
        if self.starts_identifier("match") || self.starts_identifier("search") {
            return self.parse_regex_function();
        }

        if matches!(self.peek_char(), Some('$' | '@')) {
            let path = self.parse_path_expr()?;
            self.skip_ws();
            if let Some(operator) = self.parse_compare_operator() {
                if !is_singular(&path) {
                    return self.type_error("a comparison query must be singular");
                }
                self.skip_ws();
                return Ok(Expr::Compare(
                    ValueExpr::Singular(path),
                    operator,
                    self.parse_value_expr()?,
                ));
            }
            return Ok(Expr::Exists(path));
        }

        let left = self.parse_value_expr()?;
        self.skip_ws();
        if let Some(operator) = self.parse_compare_operator() {
            self.skip_ws();
            return Ok(Expr::Compare(left, operator, self.parse_value_expr()?));
        }
        self.type_error("a filter value must be compared or used as a logical expression")
    }

    fn parse_regex_function(&mut self) -> Result<Expr, QueryError> {
        let name = self.parse_identifier()?;
        self.skip_ws();
        self.expect_char('(')?;
        self.enter()?;
        self.skip_ws();
        let value = self.parse_value_expr()?;
        self.skip_ws();
        self.expect_char(',')?;
        self.skip_ws();
        let pattern = self.parse_value_expr()?;
        self.skip_ws();
        self.expect_char(')')?;
        self.leave();
        let full = name == "match";
        let pattern = match pattern {
            ValueExpr::Literal(Literal::String(pattern)) => {
                RegexPattern::Compiled(compile_regex(&name, &pattern, full)?)
            }
            value => RegexPattern::Dynamic { value, full },
        };
        Ok(Expr::Regex { value, pattern })
    }

    fn parse_value_expr(&mut self) -> Result<ValueExpr, QueryError> {
        self.skip_ws();
        match self.peek_char() {
            Some('\'' | '"') => {
                return self
                    .parse_string()
                    .map(Literal::String)
                    .map(ValueExpr::Literal);
            }
            Some('$' | '@') => {
                let path = self.parse_path_expr()?;
                if !is_singular(&path) {
                    return self.type_error("a comparison query must be singular");
                }
                return Ok(ValueExpr::Singular(path));
            }
            Some('-' | '0'..='9') => return self.parse_number_literal(),
            _ => {}
        }
        if self.consume_keyword("true") {
            return Ok(ValueExpr::Literal(Literal::Bool(true)));
        }
        if self.consume_keyword("false") {
            return Ok(ValueExpr::Literal(Literal::Bool(false)));
        }
        if self.consume_keyword("null") {
            return Ok(ValueExpr::Literal(Literal::Null));
        }
        if self.starts_identifier("length")
            || self.starts_identifier("count")
            || self.starts_identifier("value")
        {
            return self.parse_value_function();
        }
        self.error("expected a JSONPath filter value")
    }

    fn parse_value_function(&mut self) -> Result<ValueExpr, QueryError> {
        let name = self.parse_identifier()?;
        self.skip_ws();
        self.expect_char('(')?;
        self.enter()?;
        self.skip_ws();
        let result = match name.as_str() {
            "length" => ValueExpr::Length(Box::new(self.parse_value_expr()?)),
            "count" | "value" => {
                let path = self.parse_path_expr()?;
                if name == "count" {
                    ValueExpr::Count(path)
                } else {
                    ValueExpr::Value(path)
                }
            }
            _ => return self.type_error(format!("unknown JSONPath function `{name}`")),
        };
        self.skip_ws();
        if self.consume_char(',') {
            return self.type_error(format!("{name}() accepts exactly one argument"));
        }
        self.expect_char(')')?;
        self.leave();
        Ok(result)
    }

    fn parse_path_expr(&mut self) -> Result<PathExpr, QueryError> {
        let root = match self.next_char() {
            Some('$') => PathRoot::Root,
            Some('@') => PathRoot::Current,
            _ => return self.error("expected a root or current-node query"),
        };
        Ok(PathExpr {
            root,
            segments: self.parse_segments()?,
        })
    }

    fn parse_compare_operator(&mut self) -> Option<CompareOp> {
        for (text, operator) in [
            ("==", CompareOp::Equal),
            ("!=", CompareOp::NotEqual),
            ("<=", CompareOp::LessEqual),
            (">=", CompareOp::GreaterEqual),
            ("<", CompareOp::Less),
            (">", CompareOp::Greater),
        ] {
            if self.consume(text) {
                return Some(operator);
            }
        }
        None
    }

    fn parse_number_literal(&mut self) -> Result<ValueExpr, QueryError> {
        let start = self.offset;
        self.consume_char('-');
        match self.peek_char() {
            Some('0') => {
                self.next_char();
                if self
                    .peek_char()
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    return self.error("a JSONPath number must not contain a leading zero");
                }
            }
            Some('1'..='9') => {
                self.take_while(|character| character.is_ascii_digit());
            }
            _ => return self.error("invalid JSONPath number"),
        }
        if self.consume_char('.') {
            if !self
                .peek_char()
                .is_some_and(|character| character.is_ascii_digit())
            {
                return self.error("a JSONPath fraction requires at least one digit");
            }
            self.take_while(|character| character.is_ascii_digit());
        }
        if self
            .peek_char()
            .is_some_and(|character| matches!(character, 'e' | 'E'))
        {
            self.next_char();
            if self
                .peek_char()
                .is_some_and(|character| matches!(character, '+' | '-'))
            {
                self.next_char();
            }
            if !self
                .peek_char()
                .is_some_and(|character| character.is_ascii_digit())
            {
                return self.error("a JSONPath exponent requires at least one digit");
            }
            self.take_while(|character| character.is_ascii_digit());
        }
        let text = &self.input[start..self.offset];
        let ResolvedScalar::Number(number) =
            resolve_scalar(text, YamlScalarStyle::Plain, None).map_err(QueryError::display)?
        else {
            return self.error("invalid JSONPath number");
        };
        Ok(ValueExpr::Literal(Literal::Number(number)))
    }

    fn parse_optional_integer(&mut self) -> Result<Option<i64>, QueryError> {
        let start = self.offset;
        self.consume_char('-');
        let digits = self.take_while(|character| character.is_ascii_digit());
        if digits == 0 {
            self.offset = start;
            return Ok(None);
        }
        self.input[start..self.offset]
            .parse::<i64>()
            .map(Some)
            .map_err(|_| self.error_value("JSONPath integer is out of range"))
    }

    fn parse_string(&mut self) -> Result<String, QueryError> {
        let quote = self
            .next_char()
            .filter(|character| matches!(character, '\'' | '"'))
            .ok_or_else(|| self.error_value("expected a quoted JSONPath string"))?;
        let mut output = String::new();
        loop {
            let Some(character) = self.next_char() else {
                return self.error("unterminated JSONPath string");
            };
            if character == quote {
                return Ok(output);
            }
            if character == '\\' {
                let escaped = self
                    .next_char()
                    .ok_or_else(|| self.error_value("unterminated JSONPath escape"))?;
                match escaped {
                    'b' => output.push('\u{0008}'),
                    'f' => output.push('\u{000c}'),
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    '/' => output.push('/'),
                    '\\' => output.push('\\'),
                    '\'' if quote == '\'' => output.push('\''),
                    '"' if quote == '"' => output.push('"'),
                    'u' => output.push(self.parse_unicode_escape()?),
                    _ => return self.error(format!("invalid JSONPath escape `\\{escaped}`")),
                }
            } else if character.is_control() {
                return self.error("unescaped control character in JSONPath string");
            } else {
                output.push(character);
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, QueryError> {
        let first = self.parse_hex_quad()?;
        if (0xd800..=0xdbff).contains(&first) {
            if !self.consume("\\u") {
                return self.error("a high surrogate must be followed by a low surrogate");
            }
            let second = self.parse_hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return self.error("invalid low surrogate in JSONPath string");
            }
            let scalar = 0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00;
            char::from_u32(scalar).ok_or_else(|| self.error_value("invalid Unicode escape"))
        } else if (0xdc00..=0xdfff).contains(&first) {
            self.error("unexpected low surrogate in JSONPath string")
        } else {
            char::from_u32(u32::from(first))
                .ok_or_else(|| self.error_value("invalid Unicode escape"))
        }
    }

    fn parse_hex_quad(&mut self) -> Result<u16, QueryError> {
        let start = self.offset;
        for _ in 0..4 {
            if !self
                .peek_char()
                .is_some_and(|character| character.is_ascii_hexdigit())
            {
                return self.error("a Unicode escape requires four hexadecimal digits");
            }
            self.next_char();
        }
        u16::from_str_radix(&self.input[start..self.offset], 16).map_err(QueryError::display)
    }

    fn parse_shorthand_name(&mut self) -> Result<String, QueryError> {
        let start = self.offset;
        let Some(first) = self.peek_char() else {
            return self.error("expected a name after `.`");
        };
        if !(first == '_' || first.is_alphabetic() || !first.is_ascii()) {
            return self.error("invalid shorthand JSONPath name");
        }
        self.next_char();
        while self.peek_char().is_some_and(|character| {
            character == '_' || character.is_alphanumeric() || !character.is_ascii()
        }) {
            self.next_char();
        }
        Ok(self.input[start..self.offset].to_owned())
    }

    fn parse_identifier(&mut self) -> Result<String, QueryError> {
        let start = self.offset;
        if !self
            .peek_char()
            .is_some_and(|character| character.is_ascii_alphabetic())
        {
            return self.error("expected a function name");
        }
        self.take_while(|character| character.is_ascii_alphanumeric() || character == '_');
        Ok(self.input[start..self.offset].to_owned())
    }

    fn starts_identifier(&self, identifier: &str) -> bool {
        self.input[self.offset..].starts_with(identifier)
            && self.input[self.offset + identifier.len()..]
                .chars()
                .next()
                .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        if self.starts_identifier(keyword) {
            self.offset += keyword.len();
            true
        } else {
            false
        }
    }

    fn enter(&mut self) -> Result<(), QueryError> {
        self.depth += 1;
        if self.depth > MAX_QUERY_DEPTH {
            return Err(Error::with_kind(
                ErrorKind::Limit,
                Some(self.offset),
                format!(
                    "invalid JSONPath at byte {}: JSONPath nesting limit exceeded",
                    self.offset
                ),
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn skip_ws(&mut self) {
        self.take_while(char::is_whitespace);
    }

    fn take_while(&mut self, predicate: impl Fn(char) -> bool) -> usize {
        let start = self.offset;
        while self.peek_char().is_some_and(&predicate) {
            self.next_char();
        }
        self.offset - start
    }

    fn expect_char(&mut self, expected: char) -> Result<(), QueryError> {
        if self.consume_char(expected) {
            Ok(())
        } else {
            self.error(format!("expected `{expected}`"))
        }
    }

    fn consume(&mut self, expected: &str) -> bool {
        if self.input[self.offset..].starts_with(expected) {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }

    fn consume_char(&mut self, expected: char) -> bool {
        if self.peek_char() == Some(expected) {
            self.next_char();
            true
        } else {
            false
        }
    }

    fn next_char(&mut self) -> Option<char> {
        let character = self.peek_char()?;
        self.offset += character.len_utf8();
        Some(character)
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.offset..].chars().next()
    }

    fn is_end(&self) -> bool {
        self.offset == self.input.len()
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, QueryError> {
        Err(self.error_value(message))
    }

    fn error_value(&self, message: impl Into<String>) -> QueryError {
        Error::with_kind(
            ErrorKind::Syntax,
            Some(self.offset),
            format!(
                "invalid JSONPath at byte {}: {}",
                self.offset,
                message.into()
            ),
        )
    }

    fn type_error<T>(&self, message: impl Into<String>) -> Result<T, QueryError> {
        Err(Error::with_kind(
            ErrorKind::Type,
            Some(self.offset),
            format!(
                "invalid JSONPath at byte {}: {}",
                self.offset,
                message.into()
            ),
        ))
    }
}

fn is_singular(path: &PathExpr) -> bool {
    path.segments.iter().all(|segment| {
        !segment.descendant
            && segment.selectors.len() == 1
            && matches!(segment.selectors[0], Selector::Name(_) | Selector::Index(_))
    })
}

fn compile_regex(name: &str, pattern: &str, full: bool) -> Result<Regex, QueryError> {
    let expression = if full {
        format!(r"\A(?:{pattern})\z")
    } else {
        pattern.to_owned()
    };
    Regex::new(&expression).map_err(|error| {
        Error::with_kind(
            ErrorKind::Regex,
            None,
            format!("invalid {name}() regex: {error}"),
        )
    })
}

#[derive(Debug, Clone)]
struct Candidate {
    node: Option<NodeId>,
    path: Vec<String>,
}

struct Evaluator<'a> {
    doc: &'a YamlDoc,
    root: Candidate,
    remaining: usize,
}

impl<'a> Evaluator<'a> {
    fn new(doc: &'a YamlDoc, root: Candidate, budget: usize) -> Self {
        Self {
            doc,
            root,
            remaining: budget,
        }
    }

    fn apply_segments(
        &mut self,
        mut input: Vec<Candidate>,
        segments: &[Segment],
    ) -> Result<Vec<Candidate>, QueryError> {
        for segment in segments {
            let mut output = Vec::new();
            for candidate in input {
                if segment.descendant {
                    for descendant in self.descendants(candidate)? {
                        self.apply_selectors(&descendant, &segment.selectors, &mut output)?;
                    }
                } else {
                    self.apply_selectors(&candidate, &segment.selectors, &mut output)?;
                }
            }
            input = output;
        }
        Ok(input)
    }

    fn descendants(&mut self, root: Candidate) -> Result<Vec<Candidate>, QueryError> {
        let mut result = Vec::new();
        let mut pending = vec![root];
        while let Some(candidate) = pending.pop() {
            self.spend()?;
            let children = self.children(&candidate)?;
            pending.extend(children.into_iter().rev());
            result.push(candidate);
        }
        Ok(result)
    }

    fn apply_selectors(
        &mut self,
        input: &Candidate,
        selectors: &[Selector],
        output: &mut Vec<Candidate>,
    ) -> Result<(), QueryError> {
        for selector in selectors {
            match selector {
                Selector::Name(name) => {
                    if let Some(candidate) = self.member(input, name)? {
                        output.push(candidate);
                    }
                }
                Selector::Wildcard => output.extend(self.children(input)?),
                Selector::Index(index) => {
                    if let Some(candidate) = self.index(input, *index)? {
                        output.push(candidate);
                    }
                }
                Selector::Slice { start, end, step } => {
                    output.extend(self.slice(input, *start, *end, step.unwrap_or(1))?);
                }
                Selector::Filter(expression) => {
                    for child in self.children(input)? {
                        if self.eval_expr(expression, &child)? {
                            output.push(child);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn children(&mut self, candidate: &Candidate) -> Result<Vec<Candidate>, QueryError> {
        self.spend()?;
        let Some(node) = self.resolve(candidate.node)? else {
            return Ok(Vec::new());
        };
        match self.doc.semantic_kind(node) {
            Some(SemanticKind::Sequence { .. }) => Ok(self
                .doc
                .sequence_items(node)
                .enumerate()
                .map(|(index, node)| Candidate {
                    node: Some(node),
                    path: appended(&candidate.path, index.to_string()),
                })
                .collect()),
            Some(SemanticKind::Mapping { .. }) => {
                let mut output = Vec::new();
                for (key, value) in self.doc.mapping_entries(node) {
                    output.push(Candidate {
                        node: Some(value),
                        path: appended(&candidate.path, self.string_key(key)?),
                    });
                }
                Ok(output)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn member(
        &mut self,
        candidate: &Candidate,
        name: &str,
    ) -> Result<Option<Candidate>, QueryError> {
        let Some(node) = self.resolve(candidate.node)? else {
            return Ok(None);
        };
        if !matches!(
            self.doc.semantic_kind(node),
            Some(SemanticKind::Mapping { .. })
        ) {
            return Ok(None);
        }
        for (key, value) in self.doc.mapping_entries(node) {
            if self.string_key(key)? == name {
                return Ok(Some(Candidate {
                    node: Some(value),
                    path: appended(&candidate.path, name.to_owned()),
                }));
            }
        }
        Ok(None)
    }

    fn index(
        &mut self,
        candidate: &Candidate,
        index: i64,
    ) -> Result<Option<Candidate>, QueryError> {
        let Some(node) = self.resolve(candidate.node)? else {
            return Ok(None);
        };
        if !matches!(
            self.doc.semantic_kind(node),
            Some(SemanticKind::Sequence { .. })
        ) {
            return Ok(None);
        }
        let items = self.doc.sequence_items(node).collect::<Vec<_>>();
        let Some(index) = normalized_index(index, items.len()) else {
            return Ok(None);
        };
        Ok(Some(Candidate {
            node: Some(items[index]),
            path: appended(&candidate.path, index.to_string()),
        }))
    }

    fn slice(
        &mut self,
        candidate: &Candidate,
        start: Option<i64>,
        end: Option<i64>,
        step: i64,
    ) -> Result<Vec<Candidate>, QueryError> {
        let Some(node) = self.resolve(candidate.node)? else {
            return Ok(Vec::new());
        };
        if !matches!(
            self.doc.semantic_kind(node),
            Some(SemanticKind::Sequence { .. })
        ) {
            return Ok(Vec::new());
        }
        let items = self.doc.sequence_items(node).collect::<Vec<_>>();
        let indices = slice_indices(items.len(), start, end, step);
        Ok(indices
            .into_iter()
            .map(|index| Candidate {
                node: Some(items[index]),
                path: appended(&candidate.path, index.to_string()),
            })
            .collect())
    }

    fn eval_expr(&mut self, expression: &Expr, current: &Candidate) -> Result<bool, QueryError> {
        match expression {
            Expr::Or(left, right) => {
                Ok(self.eval_expr(left, current)? || self.eval_expr(right, current)?)
            }
            Expr::And(left, right) => {
                Ok(self.eval_expr(left, current)? && self.eval_expr(right, current)?)
            }
            Expr::Not(expression) => Ok(!self.eval_expr(expression, current)?),
            Expr::Exists(path) => Ok(!self.eval_path(path, current)?.is_empty()),
            Expr::Regex { value, pattern } => {
                let Atom::String(value) = self.eval_value(value, current)? else {
                    return Ok(false);
                };
                match pattern {
                    RegexPattern::Compiled(regex) => Ok(regex.is_match(&value)),
                    RegexPattern::Dynamic {
                        value: pattern,
                        full,
                    } => {
                        let Atom::String(pattern) = self.eval_value(pattern, current)? else {
                            return Ok(false);
                        };
                        Ok(
                            compile_regex(if *full { "match" } else { "search" }, &pattern, *full)?
                                .is_match(&value),
                        )
                    }
                }
            }
            Expr::Compare(left, operator, right) => {
                let left = self.eval_value(left, current)?;
                let right = self.eval_value(right, current)?;
                self.compare(left, *operator, right)
            }
        }
    }

    fn eval_value(
        &mut self,
        expression: &ValueExpr,
        current: &Candidate,
    ) -> Result<Atom, QueryError> {
        match expression {
            ValueExpr::Literal(literal) => Ok(Atom::from_literal(literal)),
            ValueExpr::Singular(path) | ValueExpr::Value(path) => {
                let values = self.eval_path(path, current)?;
                if values.len() == 1 {
                    self.atom(&values[0])
                } else {
                    Ok(Atom::Nothing)
                }
            }
            ValueExpr::Count(path) => Ok(Atom::number(self.eval_path(path, current)?.len())),
            ValueExpr::Length(value) => match self.eval_value(value, current)? {
                Atom::String(value) => Ok(Atom::number(value.chars().count())),
                Atom::Node(node) => match self.doc.semantic_kind(node) {
                    Some(SemanticKind::Sequence { .. }) => {
                        Ok(Atom::number(self.doc.sequence_items(node).count()))
                    }
                    Some(SemanticKind::Mapping { .. }) => {
                        Ok(Atom::number(self.doc.mapping_entries(node).count()))
                    }
                    _ => Ok(Atom::Nothing),
                },
                _ => Ok(Atom::Nothing),
            },
        }
    }

    fn eval_path(
        &mut self,
        path: &PathExpr,
        current: &Candidate,
    ) -> Result<Vec<Candidate>, QueryError> {
        let start = match path.root {
            PathRoot::Root => self.root.clone(),
            PathRoot::Current => current.clone(),
        };
        self.apply_segments(vec![start], &path.segments)
    }

    fn atom(&self, candidate: &Candidate) -> Result<Atom, QueryError> {
        let Some(node) = self.resolve(candidate.node)? else {
            return Ok(Atom::Null);
        };
        match self.doc.semantic_kind(node) {
            Some(SemanticKind::Scalar { style }) => {
                let value = self.doc.scalar_value(node).map_err(QueryError::display)?;
                let tag = self.doc.resolved_tag(node).map_err(QueryError::display)?;
                match resolve_scalar(&value, style, tag.as_deref()).map_err(|error| {
                    Error::with_kind(ErrorKind::DataModel, None, error.to_string())
                })? {
                    ResolvedScalar::Null => Ok(Atom::Null),
                    ResolvedScalar::Bool(value) => Ok(Atom::Bool(value)),
                    ResolvedScalar::Number(value) => Ok(Atom::Number(value)),
                    ResolvedScalar::String => Ok(Atom::String(value.into_owned())),
                    ResolvedScalar::NonFinite(_) => Err(Error::with_kind(
                        ErrorKind::DataModel,
                        None,
                        "non-finite YAML numbers are not JSON-compatible",
                    )),
                }
            }
            Some(SemanticKind::Mapping { .. } | SemanticKind::Sequence { .. }) => {
                Ok(Atom::Node(node))
            }
            _ => Err(QueryError::new("unknown semantic YAML node")),
        }
    }

    fn compare(&self, left: Atom, operator: CompareOp, right: Atom) -> Result<bool, QueryError> {
        let equal = match (&left, &right) {
            (Atom::Nothing, Atom::Nothing) | (Atom::Null, Atom::Null) => true,
            (Atom::Bool(left), Atom::Bool(right)) => left == right,
            (Atom::Number(left), Atom::Number(right)) => left == right,
            (Atom::String(left), Atom::String(right)) => left == right,
            (Atom::Node(left), Atom::Node(right)) => {
                semantically_equal(self.doc, *left, self.doc, *right).map_err(|error| {
                    Error::with_kind(ErrorKind::DataModel, None, error.to_string())
                })?
            }
            _ => false,
        };
        match operator {
            CompareOp::Equal => Ok(equal),
            CompareOp::NotEqual => Ok(!equal),
            _ => {
                let ordering = match (left, right) {
                    (Atom::Number(left), Atom::Number(right)) => Some(left.cmp(&right)),
                    (Atom::String(left), Atom::String(right)) => Some(left.cmp(&right)),
                    _ => None,
                };
                Ok(matches!(
                    (operator, ordering),
                    (CompareOp::Less, Some(Ordering::Less))
                        | (CompareOp::LessEqual, Some(Ordering::Less | Ordering::Equal))
                        | (CompareOp::Greater, Some(Ordering::Greater))
                        | (
                            CompareOp::GreaterEqual,
                            Some(Ordering::Greater | Ordering::Equal)
                        )
                ))
            }
        }
    }

    fn string_key(&self, node: NodeId) -> Result<String, QueryError> {
        let node = self.resolve(Some(node))?.ok_or_else(|| {
            Error::with_kind(ErrorKind::DataModel, None, "mapping contains an empty key")
        })?;
        let Some(SemanticKind::Scalar { style }) = self.doc.semantic_kind(node) else {
            return Err(Error::with_kind(
                ErrorKind::DataModel,
                None,
                "mapping contains a non-string key",
            ));
        };
        let value = self.doc.scalar_value(node).map_err(QueryError::display)?;
        let tag = self.doc.resolved_tag(node).map_err(QueryError::display)?;
        if resolve_scalar(&value, style, tag.as_deref())
            .map_err(|error| Error::with_kind(ErrorKind::DataModel, None, error.to_string()))?
            != ResolvedScalar::String
        {
            return Err(Error::with_kind(
                ErrorKind::DataModel,
                None,
                "mapping contains a non-string key",
            ));
        }
        Ok(value.into_owned())
    }

    fn resolve(&self, mut node: Option<NodeId>) -> Result<Option<NodeId>, QueryError> {
        let mut seen = HashSet::new();
        while let Some(current) = node {
            if !matches!(self.doc.semantic_kind(current), Some(SemanticKind::Alias)) {
                break;
            }
            if !seen.insert(current) {
                let mut error = Error::with_kind(ErrorKind::Alias, None, "cyclic YAML alias chain");
                if let Some(span) = self.doc.node(current).map(|node| node.span()) {
                    error = error.with_source_span(span);
                }
                return Err(error);
            }
            node = self.doc.resolve_alias(current);
            if node.is_none() {
                let mut error = Error::with_kind(
                    ErrorKind::Alias,
                    None,
                    format!(
                        "unresolved YAML alias `*{}`",
                        self.doc.alias_name(current).unwrap_or_default()
                    ),
                );
                if let Some(span) = self.doc.node(current).map(|node| node.span()) {
                    error = error.with_source_span(span);
                }
                return Err(error);
            }
        }
        Ok(node)
    }

    fn spend(&mut self) -> Result<(), QueryError> {
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            Error::with_kind(ErrorKind::Limit, None, "JSONPath traversal limit exceeded")
        })?;
        Ok(())
    }
}

#[derive(Debug)]
enum Atom {
    Nothing,
    Null,
    Bool(bool),
    Number(YamlNumber),
    String(String),
    Node(NodeId),
}

impl Atom {
    fn from_literal(literal: &Literal) -> Self {
        match literal {
            Literal::Null => Self::Null,
            Literal::Bool(value) => Self::Bool(*value),
            Literal::Number(value) => Self::Number(value.clone()),
            Literal::String(value) => Self::String(value.clone()),
        }
    }

    fn number(value: usize) -> Self {
        let ResolvedScalar::Number(number) =
            resolve_scalar(&value.to_string(), YamlScalarStyle::Plain, None)
                .expect("usize is a YAML number")
        else {
            unreachable!();
        };
        Self::Number(number)
    }
}

struct Validator<'a> {
    doc: &'a YamlDoc,
    remaining: usize,
    active: HashSet<NodeId>,
}

impl<'a> Validator<'a> {
    fn new(doc: &'a YamlDoc, budget: usize) -> Self {
        Self {
            doc,
            remaining: budget,
            active: HashSet::new(),
        }
    }

    fn validate_candidate(&mut self, candidate: &Candidate) -> Result<(), QueryError> {
        self.validate_node(candidate.node, 0)
    }

    fn validate_node(&mut self, node: Option<NodeId>, depth: usize) -> Result<(), QueryError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(Error::with_kind(
                ErrorKind::Limit,
                None,
                "JSON-compatible value nesting limit exceeded",
            ));
        }
        self.remaining = self.remaining.checked_sub(1).ok_or_else(|| {
            Error::with_kind(
                ErrorKind::Limit,
                None,
                "YAML alias expansion limit exceeded",
            )
        })?;
        let Some(mut node) = node else {
            return Ok(());
        };
        let mut aliases = HashSet::new();
        while matches!(self.doc.semantic_kind(node), Some(SemanticKind::Alias)) {
            if !aliases.insert(node) {
                let mut error = Error::with_kind(ErrorKind::Alias, None, "cyclic YAML alias chain");
                if let Some(span) = self.doc.node(node).map(|node| node.span()) {
                    error = error.with_source_span(span);
                }
                return Err(error);
            }
            let alias = node;
            node = self.doc.resolve_alias(alias).ok_or_else(|| {
                let mut error = Error::with_kind(
                    ErrorKind::Alias,
                    None,
                    format!(
                        "unresolved YAML alias `*{}`",
                        self.doc.alias_name(alias).unwrap_or_default()
                    ),
                );
                if let Some(span) = self.doc.node(alias).map(|node| node.span()) {
                    error = error.with_source_span(span);
                }
                error
            })?;
        }
        match self.doc.semantic_kind(node) {
            Some(SemanticKind::Scalar { style }) => self.validate_scalar(node, style),
            Some(SemanticKind::Sequence { .. }) => {
                self.validate_collection_tag(node, SEQ_TAG)?;
                if !self.active.insert(node) {
                    return Err(Error::with_kind(
                        ErrorKind::Alias,
                        None,
                        "recursive YAML alias graph is not JSON-compatible",
                    ));
                }
                for item in self.doc.sequence_items(node) {
                    self.validate_node(Some(item), depth + 1)?;
                }
                self.active.remove(&node);
                Ok(())
            }
            Some(SemanticKind::Mapping { .. }) => {
                self.validate_collection_tag(node, MAP_TAG)?;
                if !self.active.insert(node) {
                    return Err(Error::with_kind(
                        ErrorKind::Alias,
                        None,
                        "recursive YAML alias graph is not JSON-compatible",
                    ));
                }
                let mut keys = HashSet::new();
                for (key, value) in self.doc.mapping_entries(node) {
                    let key = self.string_key(key)?;
                    if !keys.insert(key.clone()) {
                        return Err(Error::with_kind(
                            ErrorKind::DataModel,
                            None,
                            format!("mapping contains duplicate key `{key}`"),
                        ));
                    }
                    self.validate_node(Some(value), depth + 1)?;
                }
                self.active.remove(&node);
                Ok(())
            }
            _ => Err(QueryError::new("unknown semantic YAML node")),
        }
    }

    fn validate_scalar(&self, node: NodeId, style: YamlScalarStyle) -> Result<(), QueryError> {
        let value = self.doc.scalar_value(node).map_err(QueryError::display)?;
        let tag = self.doc.resolved_tag(node).map_err(QueryError::display)?;
        match resolve_scalar(&value, style, tag.as_deref())
            .map_err(|error| Error::with_kind(ErrorKind::DataModel, None, error.to_string()))?
        {
            ResolvedScalar::NonFinite(_) => Err(Error::with_kind(
                ErrorKind::DataModel,
                None,
                "non-finite YAML numbers are not JSON-compatible",
            )),
            _ => Ok(()),
        }
    }

    fn string_key(&self, node: NodeId) -> Result<String, QueryError> {
        let mut node = node;
        let mut seen = HashSet::new();
        while matches!(self.doc.semantic_kind(node), Some(SemanticKind::Alias)) {
            if !seen.insert(node) {
                let mut error = Error::with_kind(ErrorKind::Alias, None, "cyclic YAML alias key");
                if let Some(span) = self.doc.node(node).map(|node| node.span()) {
                    error = error.with_source_span(span);
                }
                return Err(error);
            }
            let alias = node;
            node = self.doc.resolve_alias(alias).ok_or_else(|| {
                let mut error =
                    Error::with_kind(ErrorKind::Alias, None, "unresolved YAML alias key");
                if let Some(span) = self.doc.node(alias).map(|node| node.span()) {
                    error = error.with_source_span(span);
                }
                error
            })?;
        }
        let Some(SemanticKind::Scalar { style }) = self.doc.semantic_kind(node) else {
            return Err(Error::with_kind(
                ErrorKind::DataModel,
                None,
                "mapping contains a non-string key",
            ));
        };
        let value = self.doc.scalar_value(node).map_err(QueryError::display)?;
        let tag = self.doc.resolved_tag(node).map_err(QueryError::display)?;
        if resolve_scalar(&value, style, tag.as_deref())
            .map_err(|error| Error::with_kind(ErrorKind::DataModel, None, error.to_string()))?
            != ResolvedScalar::String
        {
            return Err(Error::with_kind(
                ErrorKind::DataModel,
                None,
                "mapping contains a non-string key",
            ));
        }
        Ok(value.into_owned())
    }

    fn validate_collection_tag(&self, node: NodeId, expected: &str) -> Result<(), QueryError> {
        let tag = self.doc.resolved_tag(node).map_err(QueryError::display)?;
        if tag.as_deref().is_some_and(|tag| tag != expected) {
            return Err(Error::with_kind(
                ErrorKind::DataModel,
                None,
                format!(
                    "custom-tagged collection `{}` is not JSON-compatible",
                    tag.as_deref().unwrap_or_default()
                ),
            ));
        }
        Ok(())
    }
}

fn json_pointer(tokens: &[String]) -> String {
    let mut pointer = String::new();
    for token in tokens {
        pointer.push('/');
        pointer.push_str(&token.replace('~', "~0").replace('/', "~1"));
    }
    pointer
}

fn appended(path: &[String], token: String) -> Vec<String> {
    let mut result = Vec::with_capacity(path.len() + 1);
    result.extend_from_slice(path);
    result.push(token);
    result
}

fn normalized_index(index: i64, length: usize) -> Option<usize> {
    let length = i128::try_from(length).ok()?;
    let index = i128::from(index);
    let index = if index < 0 { length + index } else { index };
    (0..length)
        .contains(&index)
        .then(|| usize::try_from(index).ok())
        .flatten()
}

fn slice_indices(length: usize, start: Option<i64>, end: Option<i64>, step: i64) -> Vec<usize> {
    let length = i128::try_from(length).unwrap_or(i128::MAX);
    let step = i128::from(step);
    let normalize = |value: i64| {
        let value = i128::from(value);
        if value < 0 { length + value } else { value }
    };
    let (mut index, limit) = if step > 0 {
        (
            start.map_or(0, normalize).clamp(0, length),
            end.map_or(length, normalize).clamp(0, length),
        )
    } else {
        (
            start.map_or(length - 1, normalize).clamp(-1, length - 1),
            end.map_or(-1, normalize).clamp(-1, length - 1),
        )
    };
    let mut result = Vec::new();
    while if step > 0 {
        index < limit
    } else {
        index > limit
    } {
        if let Ok(index) = usize::try_from(index) {
            result.push(index);
        }
        index += step;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointers(path: &str, yaml: &str) -> Result<Vec<String>, Error> {
        let doc = YamlDoc::parse(yaml).unwrap();
        Ok(JsonPath::parse(path)?
            .query(&doc, 0)?
            .iter()
            .map(|matched| matched.pointer().as_str().to_owned())
            .collect())
    }

    #[test]
    fn selects_names_wildcards_unions_descendants_and_slices() {
        let yaml = "users:\n  - {name: Ada, age: 37}\n  - {name: Linus, age: 55}\n";
        assert_eq!(
            pointers("$.users[*].name", yaml).unwrap(),
            ["/users/0/name", "/users/1/name"]
        );
        assert_eq!(
            pointers("$..age", yaml).unwrap(),
            ["/users/0/age", "/users/1/age"]
        );
        assert_eq!(
            pointers("$.users[1,0].name", yaml).unwrap(),
            ["/users/1/name", "/users/0/name"]
        );
        assert_eq!(
            pointers("$.users[::-1].name", yaml).unwrap(),
            ["/users/1/name", "/users/0/name"]
        );
    }

    #[test]
    fn filters_and_standard_functions_work() {
        let yaml =
            "users:\n  - {name: Ada, tags: [rust, yaml]}\n  - {name: linus, tags: [kernel]}\n";
        assert_eq!(
            pointers("$.users[?length(@.tags) > 1].name", yaml).unwrap(),
            ["/users/0/name"]
        );
        assert_eq!(
            pointers(r#"$.users[?match(@.name, "[A-Z].*")].name"#, yaml).unwrap(),
            ["/users/0/name"]
        );
        assert_eq!(
            pointers(r#"$.users[?search(@.name, "inu")].name"#, yaml).unwrap(),
            ["/users/1/name"]
        );
        assert_eq!(
            pointers(
                "$.users[?match(@.name, @.pattern)].name",
                "users: [{name: Ada, pattern: '[A-Z].*'}]\n",
            )
            .unwrap(),
            ["/users/0/name"]
        );
        assert_eq!(
            pointers("$.users[?count(@.tags[*]) == 1].name", yaml).unwrap(),
            ["/users/1/name"]
        );
        assert_eq!(
            pointers("$.users[?@.tags[*] && !@.missing].name", yaml).unwrap(),
            ["/users/0/name", "/users/1/name"]
        );
        assert_eq!(
            pointers(r#"$.users[?value(@.name) == "Ada"].name"#, yaml).unwrap(),
            ["/users/0/name"]
        );
        let error = JsonPath::parse(r#"$[?match(@, "[")]"#).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Regex);
    }

    #[test]
    fn exposes_escaped_pointers_empty_roots_and_duplicate_matches() {
        assert_eq!(
            pointers("$['a/b~c']", "'a/b~c': value\n").unwrap(),
            ["/a~1b~0c"]
        );

        let doc = YamlDoc::parse("---\n").unwrap();
        let matches = JsonPath::parse("$").unwrap().query(&doc, 0).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches.iter().next().unwrap().pointer().as_str(), "");
        assert_eq!(matches.iter().next().unwrap().node(), None);

        assert_eq!(pointers("$['a','a']", "a: 1\n").unwrap(), ["/a", "/a"]);
    }

    #[test]
    fn rejects_values_outside_the_json_data_model() {
        for yaml in [
            "1: value\n",
            "{a: 1, a: 2}\n",
            "value: .inf\n",
            "!custom value\n",
        ] {
            let doc = YamlDoc::parse(yaml).unwrap();
            let error = JsonPath::parse("$").unwrap().query(&doc, 0).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::DataModel, "{error}");
        }

        let doc = YamlDoc::parse("&root [*root]\n").unwrap();
        let error = JsonPath::parse("$").unwrap().query(&doc, 0).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Alias);

        let doc = YamlDoc::parse("copy: *missing\n").unwrap();
        let error = JsonPath::parse("$").unwrap().query(&doc, 0).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Alias);
        assert_eq!(error.source_span(), Some(Span::new(6, 14)));
    }

    #[test]
    fn preserves_alias_occurrence_paths_and_compares_large_numbers_exactly() {
        let yaml = "base: &base {name: Ada}\ncopy: *base\n";
        assert_eq!(
            pointers("$.*.name", yaml).unwrap(),
            ["/base/name", "/copy/name"]
        );

        assert_eq!(
            pointers("$[?@ > 9e999]", "[9e999, 1e1000]\n").unwrap(),
            ["/1"]
        );
    }

    #[test]
    fn from_str_and_structured_syntax_errors_are_public() {
        let parsed = "$.value".parse::<JsonPath>().unwrap();
        let doc = YamlDoc::parse("value: 1\n").unwrap();
        assert_eq!(parsed.query(&doc, 0).unwrap().len(), 1);

        let error = JsonPath::parse("value").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Syntax);
        assert_eq!(error.byte_offset(), Some(0));
        assert_eq!(error.source_span(), None);
    }
}
