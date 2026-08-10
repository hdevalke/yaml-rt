use std::collections::HashSet;
use std::fmt;

use crate::{NodeId, ResolvedScalar, SemanticKind, YamlDoc, resolve_scalar};

/// One decoded RFC 6901 reference token.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReferenceToken(String);

impl ReferenceToken {
    /// Returns the decoded token text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A parsed plain RFC 6901 JSON Pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPointer {
    original: String,
    tokens: Vec<ReferenceToken>,
}

impl JsonPointer {
    /// Parses a plain JSON Pointer. URI fragment syntax is not accepted.
    ///
    /// # Errors
    ///
    /// Returns an error when `input` is not valid RFC 6901 pointer syntax.
    pub fn parse(input: &str) -> Result<Self, PointerError> {
        if input.is_empty() {
            return Ok(Self {
                original: String::new(),
                tokens: Vec::new(),
            });
        }
        if !input.starts_with('/') {
            return Err(PointerError::new(
                input,
                None,
                PointerErrorKind::InvalidSyntax,
                "a non-empty JSON Pointer must begin with `/`",
            ));
        }
        let tokens = input[1..]
            .split('/')
            .enumerate()
            .map(|(index, token)| {
                decode_token(token).map(ReferenceToken).map_err(|escape| {
                    PointerError::new(
                        input,
                        Some(index),
                        PointerErrorKind::InvalidEscape,
                        format!("invalid escape `{escape}`"),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            original: input.to_owned(),
            tokens,
        })
    }

    /// Returns the original pointer spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.original
    }

    /// Returns the decoded reference tokens.
    #[must_use]
    pub fn tokens(&self) -> &[ReferenceToken] {
        &self.tokens
    }

    /// Returns whether this pointer identifies the document root.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Returns whether this pointer is a proper token-prefix of `other`.
    #[must_use]
    pub fn is_proper_prefix_of(&self, other: &Self) -> bool {
        self.tokens.len() < other.tokens.len() && other.tokens.starts_with(&self.tokens)
    }

    pub(crate) fn parent(&self) -> Option<(Self, &ReferenceToken)> {
        let (last, parent) = self.tokens.split_last()?;
        let original = encode_tokens(parent);
        Some((
            Self {
                original,
                tokens: parent.to_vec(),
            },
            last,
        ))
    }
}

impl std::str::FromStr for JsonPointer {
    type Err = PointerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

fn decode_token(token: &str) -> Result<String, String> {
    let mut output = String::with_capacity(token.len());
    let mut characters = token.chars();
    while let Some(character) = characters.next() {
        if character != '~' {
            output.push(character);
            continue;
        }
        match characters.next() {
            Some('0') => output.push('~'),
            Some('1') => output.push('/'),
            Some(other) => return Err(format!("~{other}")),
            None => return Err("~".to_owned()),
        }
    }
    Ok(output)
}

fn encode_tokens(tokens: &[ReferenceToken]) -> String {
    let mut pointer = String::new();
    for token in tokens {
        pointer.push('/');
        pointer.push_str(&token.as_str().replace('~', "~0").replace('/', "~1"));
    }
    pointer
}

/// Classification of a JSON Pointer parse or resolution failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerErrorKind {
    /// The whole pointer has invalid syntax.
    InvalidSyntax,
    /// A reference token contains an invalid `~` escape.
    InvalidEscape,
    /// The selected YAML document has no root node.
    EmptyDocument,
    /// A mapping member or sequence item does not exist.
    MissingValue,
    /// A reference token was evaluated against a scalar.
    TypeMismatch,
    /// A sequence token is not a canonical unsigned index.
    InvalidIndex,
    /// A canonical sequence index is larger than the sequence.
    IndexOutOfBounds,
    /// The special `-` token was used outside an add destination.
    DashNotAllowed,
    /// A traversed YAML mapping contains a non-string key.
    NonStringKey,
    /// More than one mapping key matches the token.
    AmbiguousKey,
    /// An alias has no preceding anchor binding.
    UnresolvedAlias,
    /// An alias chain is cyclic.
    AliasCycle,
    /// A YAML semantic operation failed during resolution.
    Semantic,
}

/// Structured JSON Pointer parse or resolution error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerError {
    pointer: String,
    token_index: Option<usize>,
    kind: PointerErrorKind,
    message: String,
}

impl PointerError {
    pub(crate) fn new(
        pointer: impl Into<String>,
        token_index: Option<usize>,
        kind: PointerErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            pointer: pointer.into(),
            token_index,
            kind,
            message: message.into(),
        }
    }

    /// Returns the pointer associated with the failure.
    #[must_use]
    pub fn pointer(&self) -> &str {
        &self.pointer
    }

    /// Returns the zero-based failing token index, when applicable.
    #[must_use]
    pub const fn token_index(&self) -> Option<usize> {
        self.token_index
    }

    /// Returns the failure classification.
    #[must_use]
    pub const fn kind(&self) -> PointerErrorKind {
        self.kind
    }
}

impl fmt::Display for PointerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pointer.is_empty() {
            write!(formatter, "cannot resolve document root: {}", self.message)
        } else {
            write!(
                formatter,
                "cannot resolve JSON Pointer {:?}: {}",
                self.pointer, self.message
            )
        }
    }
}

impl std::error::Error for PointerError {}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MappingMatch {
    pub(crate) value: NodeId,
}

impl YamlDoc {
    /// Resolves a JSON Pointer against one YAML document representation graph.
    ///
    /// Alias nodes are traversed when another reference token remains. A
    /// pointer that ends on an alias returns the alias occurrence itself.
    ///
    /// # Errors
    ///
    /// Returns an error when the document, path, or selected mapping or sequence
    /// element does not exist, or when alias traversal fails.
    pub fn resolve_pointer(
        &self,
        document: usize,
        pointer: &JsonPointer,
    ) -> Result<NodeId, PointerError> {
        let mut current = self
            .document_root(document)
            .map_err(|error| {
                PointerError::new(
                    pointer.as_str(),
                    None,
                    PointerErrorKind::Semantic,
                    error.to_string(),
                )
            })?
            .ok_or_else(|| {
                PointerError::new(
                    pointer.as_str(),
                    None,
                    PointerErrorKind::EmptyDocument,
                    "selected YAML document has no root node",
                )
            })?;

        for (index, token) in pointer.tokens().iter().enumerate() {
            current = self.resolve_aliases_for_pointer(current, pointer, index)?;
            current = match self.semantic_kind(current) {
                Some(SemanticKind::Mapping { .. }) => {
                    self.mapping_match(current, token, pointer, index)?
                        .ok_or_else(|| {
                            PointerError::new(
                                pointer.as_str(),
                                Some(index),
                                PointerErrorKind::MissingValue,
                                format!("mapping has no member {:?}", token.as_str()),
                            )
                        })?
                        .value
                }
                Some(SemanticKind::Sequence { .. }) => {
                    let items = self.sequence_items(current).collect::<Vec<_>>();
                    let item_index = parse_sequence_index(token, pointer, index, false)?;
                    *items.get(item_index).ok_or_else(|| {
                        PointerError::new(
                            pointer.as_str(),
                            Some(index),
                            PointerErrorKind::IndexOutOfBounds,
                            format!(
                                "sequence index {item_index} is out of bounds for length {}",
                                items.len()
                            ),
                        )
                    })?
                }
                Some(SemanticKind::Scalar { .. }) | Some(SemanticKind::Alias) => {
                    return Err(PointerError::new(
                        pointer.as_str(),
                        Some(index),
                        PointerErrorKind::TypeMismatch,
                        format!(
                            "token {:?} cannot be evaluated against a scalar",
                            token.as_str()
                        ),
                    ));
                }
                Some(SemanticKind::Document) | None => {
                    return Err(PointerError::new(
                        pointer.as_str(),
                        Some(index),
                        PointerErrorKind::TypeMismatch,
                        "token cannot be evaluated against this YAML node",
                    ));
                }
            };
        }
        Ok(current)
    }

    pub(crate) fn resolve_aliases_for_pointer(
        &self,
        mut node: NodeId,
        pointer: &JsonPointer,
        token_index: usize,
    ) -> Result<NodeId, PointerError> {
        let mut seen = HashSet::new();
        while matches!(self.semantic_kind(node), Some(SemanticKind::Alias)) {
            if !seen.insert(node) {
                return Err(PointerError::new(
                    pointer.as_str(),
                    Some(token_index),
                    PointerErrorKind::AliasCycle,
                    "cyclic alias chain",
                ));
            }
            node = self.resolve_alias(node).ok_or_else(|| {
                PointerError::new(
                    pointer.as_str(),
                    Some(token_index),
                    PointerErrorKind::UnresolvedAlias,
                    format!(
                        "unresolved alias `*{}`",
                        self.alias_name(node).unwrap_or_default()
                    ),
                )
            })?;
        }
        Ok(node)
    }

    pub(crate) fn mapping_match(
        &self,
        mapping: NodeId,
        token: &ReferenceToken,
        pointer: &JsonPointer,
        token_index: usize,
    ) -> Result<Option<MappingMatch>, PointerError> {
        let mut found = None;
        for (key, value) in self.mapping_entries(mapping) {
            let key = self.resolve_aliases_for_pointer(key, pointer, token_index)?;
            let Some(SemanticKind::Scalar { style }) = self.semantic_kind(key) else {
                return Err(non_string_key_error(pointer, token_index));
            };
            let scalar = self.scalar_value(key).map_err(|error| {
                PointerError::new(
                    pointer.as_str(),
                    Some(token_index),
                    PointerErrorKind::Semantic,
                    error.to_string(),
                )
            })?;
            let tag = self.resolved_tag(key).map_err(|error| {
                PointerError::new(
                    pointer.as_str(),
                    Some(token_index),
                    PointerErrorKind::Semantic,
                    error.to_string(),
                )
            })?;
            let resolved = resolve_scalar(&scalar, style, tag.as_deref())
                .map_err(|_| non_string_key_error(pointer, token_index))?;
            if resolved != ResolvedScalar::String {
                return Err(non_string_key_error(pointer, token_index));
            }
            if scalar == token.as_str() {
                if found.is_some() {
                    return Err(PointerError::new(
                        pointer.as_str(),
                        Some(token_index),
                        PointerErrorKind::AmbiguousKey,
                        format!(
                            "mapping contains multiple matching keys {:?}",
                            token.as_str()
                        ),
                    ));
                }
                found = Some(MappingMatch { value });
            }
        }
        Ok(found)
    }
}

fn non_string_key_error(pointer: &JsonPointer, token_index: usize) -> PointerError {
    PointerError::new(
        pointer.as_str(),
        Some(token_index),
        PointerErrorKind::NonStringKey,
        "cannot traverse a mapping containing non-string keys",
    )
}

pub(crate) fn parse_sequence_index(
    token: &ReferenceToken,
    pointer: &JsonPointer,
    token_index: usize,
    allow_dash: bool,
) -> Result<usize, PointerError> {
    let value = token.as_str();
    if value == "-" {
        return if allow_dash {
            Ok(usize::MAX)
        } else {
            Err(PointerError::new(
                pointer.as_str(),
                Some(token_index),
                PointerErrorKind::DashNotAllowed,
                "`-` is allowed only as the final add destination token",
            ))
        };
    }
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || value.len() > 1 && value.starts_with('0')
    {
        return Err(PointerError::new(
            pointer.as_str(),
            Some(token_index),
            PointerErrorKind::InvalidIndex,
            format!("{value:?} is not a canonical sequence index"),
        ));
    }
    value.parse().map_err(|_| {
        PointerError::new(
            pointer.as_str(),
            Some(token_index),
            PointerErrorKind::InvalidIndex,
            format!("sequence index {value:?} overflows this platform"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_decodes_plain_json_pointers() {
        for (input, expected) in [
            ("", vec![]),
            ("/foo/0", vec!["foo", "0"]),
            ("/a~1b", vec!["a/b"]),
            ("/m~0n", vec!["m~n"]),
            ("/~01", vec!["~1"]),
        ] {
            let pointer = JsonPointer::parse(input).unwrap();
            assert_eq!(
                pointer
                    .tokens()
                    .iter()
                    .map(ReferenceToken::as_str)
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn rejects_invalid_pointer_syntax() {
        for input in ["foo", "/foo/~", "/foo/~2"] {
            assert!(JsonPointer::parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn resolves_mappings_sequences_and_escaped_keys() {
        let doc = YamlDoc::parse("'a/b':\n  - zero\n  - one\n'm~n': value\n").expect("valid YAML");
        let pointer = JsonPointer::parse("/a~1b/1").unwrap();
        let node = doc.resolve_pointer(0, &pointer).unwrap();
        assert_eq!(doc.scalar_value(node).unwrap(), "one");
        let pointer = JsonPointer::parse("/m~0n").unwrap();
        assert_eq!(
            doc.scalar_value(doc.resolve_pointer(0, &pointer).unwrap())
                .unwrap(),
            "value"
        );
    }

    #[test]
    fn traverses_aliases_but_returns_a_terminal_alias() {
        let doc =
            YamlDoc::parse("defaults: &defaults\n  timeout: 30\nservice:\n  config: *defaults\n")
                .unwrap();
        let terminal = doc
            .resolve_pointer(0, &JsonPointer::parse("/service/config").unwrap())
            .unwrap();
        assert_eq!(doc.alias_name(terminal), Some("defaults"));
        let traversed = doc
            .resolve_pointer(0, &JsonPointer::parse("/service/config/timeout").unwrap())
            .unwrap();
        assert_eq!(doc.scalar_value(traversed).unwrap(), "30");
    }

    #[test]
    fn rejects_noncanonical_indices_and_non_string_keys() {
        let sequence = YamlDoc::parse("[a, b]\n").unwrap();
        for path in ["/01", "/-1", "/+1", "/ ", "/-"] {
            assert!(
                sequence
                    .resolve_pointer(0, &JsonPointer::parse(path).unwrap())
                    .is_err(),
                "{path}"
            );
        }
        let mapping = YamlDoc::parse("1: value\n").unwrap();
        let error = mapping
            .resolve_pointer(0, &JsonPointer::parse("/1").unwrap())
            .unwrap_err();
        assert_eq!(error.kind(), PointerErrorKind::NonStringKey);
    }

    #[test]
    fn duplicate_matching_keys_are_ambiguous() {
        let doc = YamlDoc::parse("foo: one\nfoo: two\n").unwrap();
        let error = doc
            .resolve_pointer(0, &JsonPointer::parse("/foo").unwrap())
            .unwrap_err();
        assert_eq!(error.kind(), PointerErrorKind::AmbiguousKey);
    }
}
