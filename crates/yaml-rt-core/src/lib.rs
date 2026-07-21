//! Core types for a YAML 1.2.2 round-trip parser.
//!
//! This crate is intentionally dependency-free. The first implementation keeps
//! the source text intact while the source model, lexer, CST parser, semantic
//! graph, editor, and patch emitter are built out according to the workspace
//! roadmap.

mod diagnostic;
mod doc;
mod lexer;
mod parser;
mod semantic;
mod source;
mod syntax;
mod typed;

pub use diagnostic::{Diagnostic, DiagnosticKind, ParseError, YamlError};
pub use doc::{
    Edit, MappingEntryStyle, ReservedDirective, TagDirective, YamlDirective, YamlDoc, YamlEvents,
};
pub use lexer::{Token, TokenKind, lex, tokens_to_string};
pub use parser::events_to_test_string;
pub use semantic::SemanticKind;
pub use source::{LineCol, NodeId, Source, Span, TARGET_YAML_VERSION};
pub(crate) use syntax::ParsedYaml;
pub use syntax::{
    Children, CollectionStyle, Node, NodeKind, YamlEvent, YamlEventKind, YamlScalarStyle, parse_cst,
};
pub use typed::{FromYamlDoc, ToYamlDoc, ToYamlFragment, YamlValue};

pub(crate) use parser::{
    BlockChomp, CollectionTarget, Parser, ScalarStyle, decode_scalar_value_with_content_indent,
    directive_emit_error, double_quoted_scalar_end, edits_conflict, format_scalar_value,
    next_line_content_start, parse_block_scalar_header, parse_node_properties, plain_scalar_end,
    single_quoted_scalar_end, strip_inline_comment, validate_plain_mapping_fragment,
    validate_tag_directive_parts_for_emit, validate_yaml_directive_version_for_emit,
};
pub(crate) use semantic::{SemanticBuilder, SemanticStore};
pub(crate) use source::validate_yaml_chars;

#[cfg(test)]
mod tests;
