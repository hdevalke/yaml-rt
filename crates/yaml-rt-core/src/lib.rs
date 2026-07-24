//! Core types for a YAML 1.2.2 round-trip parser.
//!
//! This crate is intentionally dependency-free. It owns source storage, the
//! lossless lexer and CST, semantic metadata, diagnostics, JSON Pointer
//! operations, typed-overlay traits, editor APIs, and patch-based emission.
//! The original source and CST remain authoritative so untouched YAML is
//! emitted byte-for-byte.

mod diagnostic;
mod doc;
mod edit;
mod fragment;
mod lexer;
mod parser;
mod pointer;
mod semantic;
mod source;
mod syntax;
mod typed;
mod value;

pub use diagnostic::{Diagnostic, DiagnosticKind, ParseError, YamlError};
pub use doc::{
    Edit, MappingEntryStyle, ReservedDirective, TagDirective, YamlDirective, YamlDoc, YamlEvents,
};
pub use edit::YamlEditError;
pub use fragment::{FragmentError, YamlFragment};
pub use lexer::{Token, TokenKind, lex, tokens_to_string};
pub use parser::events_to_test_string;
pub use pointer::{JsonPointer, PointerError, PointerErrorKind, ReferenceToken};
pub use semantic::SemanticKind;
pub use source::{LineCol, NodeId, Source, Span, TARGET_YAML_VERSION};
pub(crate) use syntax::ParsedYaml;
pub use syntax::{
    Children, CollectionStyle, Node, NodeKind, YamlEvent, YamlEventKind, YamlScalarStyle, parse_cst,
};
pub use typed::{FromYamlDoc, ToYamlDoc, ToYamlFragment, YamlValue};
pub use value::{
    NonFiniteFloat, ResolvedScalar, ScalarResolveError, SemanticValueError, YamlNumber,
    resolve_scalar, semantically_equal,
};

pub(crate) use parser::{
    BlockChomp, CollectionTarget, Parser, ScalarStyle, decode_scalar_value_with_content_indent,
    directive_emit_error, double_quoted_scalar_end, edits_conflict, format_scalar_value,
    next_line_content_start, parse_block_scalar_header, parse_node_properties, plain_scalar_end,
    resolve_tag, single_quoted_scalar_end, strip_inline_comment, validate_plain_mapping_fragment,
    validate_tag_directive_parts_for_emit, validate_yaml_directive_version_for_emit,
};
pub(crate) use semantic::{SemanticBuilder, SemanticProperties, SemanticStore};
pub(crate) use source::validate_yaml_chars;

#[cfg(test)]
mod tests;
