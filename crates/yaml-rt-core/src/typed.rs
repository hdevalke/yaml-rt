use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::BuildHasher;

use crate::edit::closing_delimiter_offset;
use crate::{
    BlockChomp, Diagnostic, DiagnosticKind, MappingEntryStyle, Node, NodeId, NodeKind,
    NonFiniteFloat, ResolvedScalar, SemanticKind, Span, YamlDoc, YamlEditError, YamlError,
    YamlFragment, format_scalar_value, parse_block_scalar_header, parse_node_properties,
    resolve_scalar, validate_plain_mapping_fragment, validate_yaml_chars,
};

/// Converts a YAML document into a typed overlay.
pub trait FromYamlDoc: Sized {
    /// Reads `Self` from `doc` while preserving the document as the source of
    /// truth for future edits.
    ///
    /// # Errors
    ///
    /// Returns an error when the implementing overlay cannot be read from the
    /// document or when required YAML paths, node kinds, or scalar values are
    /// invalid for that overlay.
    fn from_yaml_doc(doc: &YamlDoc) -> Result<Self, YamlError>;
}

/// Applies a typed overlay back to a YAML document as minimal patches.
pub trait ToYamlDoc {
    /// Writes `self` into `doc` without discarding unknown fields or comments.
    ///
    /// # Errors
    ///
    /// Returns an error when the implementing overlay cannot be written to the
    /// document, when generated YAML is invalid, or when a queued edit conflicts
    /// with another pending edit.
    fn apply_to_yaml_doc(&self, doc: &mut YamlDoc) -> Result<(), YamlError>;
}

/// Reads and writes individual YAML node values.
pub trait YamlValue: Sized {
    /// Reads a mapping field that may be absent.
    ///
    /// The default implementation reports a missing required field. Container
    /// types such as [`Option`] may override this to define missing-field
    /// semantics without requiring derive-time type inspection.
    fn read_yaml_field(doc: &YamlDoc, node: Option<NodeId>, key: &str) -> Result<Self, YamlError> {
        match node {
            Some(node) => Self::read_yaml(doc, node),
            None => Err(missing_required_field_error(key)),
        }
    }

    /// Reads a typed value from `node`.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` is unknown, has an unsupported YAML kind, or
    /// cannot be decoded as `Self`.
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError>;

    /// Writes a typed value into an existing node or inserts a new node.
    ///
    /// # Errors
    ///
    /// Returns an error when the value cannot be represented in the target YAML
    /// shape, when insertion requires a missing parent context, or when the edit
    /// cannot be queued.
    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError>;
}

/// Formats a typed value as a YAML fragment for parent-aware insertions.
pub trait ToYamlFragment {
    /// Formats `self` using `indent` spaces for nested block lines.
    ///
    /// # Errors
    ///
    /// Returns an error when the value cannot be represented by the current
    /// conservative block-style formatter.
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError>;
}

fn plain_yaml_fragment(value: &str, role: &str) -> Result<String, YamlError> {
    validate_plain_mapping_fragment(value, role)?;
    Ok(value.to_owned())
}

macro_rules! impl_plain_yaml_fragment {
    ($($type:ty),* $(,)?) => {
        $(
            impl ToYamlFragment for $type {
                fn to_yaml_fragment(
                    &self,
                    _indent: usize,
                    _line_ending: &str,
                ) -> Result<String, YamlError> {
                    plain_yaml_fragment(&self.to_string(), "YAML value")
                }
            }
        )*
    };
}

impl ToYamlFragment for String {
    fn to_yaml_fragment(&self, _indent: usize, _line_ending: &str) -> Result<String, YamlError> {
        validate_yaml_chars(self)?;
        Ok(crate::edit::emit_string_key(self))
    }
}

impl ToYamlFragment for &str {
    fn to_yaml_fragment(&self, _indent: usize, _line_ending: &str) -> Result<String, YamlError> {
        validate_yaml_chars(self)?;
        Ok(crate::edit::emit_string_key(self))
    }
}

impl ToYamlFragment for bool {
    fn to_yaml_fragment(&self, _indent: usize, _line_ending: &str) -> Result<String, YamlError> {
        Ok(if *self { "true" } else { "false" }.to_owned())
    }
}

impl_plain_yaml_fragment!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

impl ToYamlFragment for char {
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError> {
        self.to_string().to_yaml_fragment(indent, line_ending)
    }
}

impl<T> ToYamlFragment for Option<T>
where
    T: ToYamlFragment,
{
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError> {
        match self {
            Some(value) => value.to_yaml_fragment(indent, line_ending),
            None => Ok("null".to_owned()),
        }
    }
}

impl<T> ToYamlFragment for Vec<T>
where
    T: ToYamlFragment,
{
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError> {
        sequence_yaml_fragment(self, indent, line_ending)
    }
}

impl<T, const N: usize> ToYamlFragment for [T; N]
where
    T: ToYamlFragment,
{
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError> {
        sequence_yaml_fragment(self, indent, line_ending)
    }
}

impl<T> ToYamlFragment for Box<T>
where
    T: ToYamlFragment,
{
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError> {
        (**self).to_yaml_fragment(indent, line_ending)
    }
}

impl<T> ToYamlFragment for std::collections::BTreeMap<String, T>
where
    T: ToYamlFragment,
{
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError> {
        if self.is_empty() {
            return Ok("{}".to_owned());
        }
        let indent_text = " ".repeat(indent);
        let mut output = String::new();
        for (index, (key, value)) in self.iter().enumerate() {
            validate_yaml_chars(key)?;
            if index > 0 {
                output.push_str(line_ending);
            }
            output.push_str(&indent_text);
            output.push_str(&crate::edit::emit_string_key(key));
            let value = value.to_yaml_fragment(indent + 2, line_ending)?;
            if value.contains('\n') || value.starts_with(' ') {
                output.push(':');
                output.push_str(line_ending);
                output.push_str(&value);
            } else {
                output.push_str(": ");
                output.push_str(&value);
            }
        }
        Ok(output)
    }
}

impl<T, S> ToYamlFragment for HashMap<String, T, S>
where
    T: ToYamlFragment,
    S: BuildHasher,
{
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError> {
        if self.is_empty() {
            return Ok("{}".to_owned());
        }
        let indent_text = " ".repeat(indent);
        let mut entries = self.iter().collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(key, _)| *key);
        let mut output = String::new();
        for (index, (key, value)) in entries.into_iter().enumerate() {
            validate_yaml_chars(key)?;
            if index > 0 {
                output.push_str(line_ending);
            }
            output.push_str(&indent_text);
            output.push_str(&crate::edit::emit_string_key(key));
            let value = value.to_yaml_fragment(indent + 2, line_ending)?;
            if value.contains('\n') || value.starts_with(' ') {
                output.push(':');
                output.push_str(line_ending);
                output.push_str(&value);
            } else {
                output.push_str(": ");
                output.push_str(&value);
            }
        }
        Ok(output)
    }
}

#[doc(hidden)]
pub fn __mapping_overlay_to_yaml_fragment<T>(
    value: &T,
    indent: usize,
    line_ending: &str,
) -> Result<String, YamlError>
where
    T: ToYamlDoc,
{
    mapping_fields_to_yaml_fragment(indent, line_ending, |doc| value.apply_to_yaml_doc(doc))
}

#[doc(hidden)]
pub fn __mapping_fields_to_yaml_fragment<F>(
    indent: usize,
    line_ending: &str,
    apply: F,
) -> Result<String, YamlError>
where
    F: FnOnce(&mut YamlDoc) -> Result<(), YamlError>,
{
    mapping_fields_to_yaml_fragment(indent, line_ending, apply)
}

fn mapping_fields_to_yaml_fragment<F>(
    indent: usize,
    line_ending: &str,
    apply: F,
) -> Result<String, YamlError>
where
    F: FnOnce(&mut YamlDoc) -> Result<(), YamlError>,
{
    let mut doc = YamlDoc::parse("root:\n  __rty_placeholder: null\n")?;
    let root = doc.get_path(&["root"])?.ok_or_else(|| {
        YamlError::new(Diagnostic::new(
            DiagnosticKind::Emitter,
            "temporary nested mapping root was not created",
            Span::empty(0),
        ))
    })?;
    let mut nested = doc.rerooted_at_mapping(root)?;
    apply(&mut nested)?;
    doc.queue_edits_from(&nested)?;
    let rendered = doc.to_string();
    let nested_indent = "  ";
    let output_indent = " ".repeat(indent);
    let mut output = String::new();

    for line in rendered.lines().skip(1) {
        if line.trim_start().starts_with("__rty_placeholder:") {
            continue;
        }
        if !output.is_empty() {
            output.push_str(line_ending);
        }
        if line.is_empty() {
            continue;
        }
        let line = line.strip_prefix(nested_indent).unwrap_or(line);
        output.push_str(&output_indent);
        output.push_str(line);
    }

    Ok(output)
}

#[doc(hidden)]
pub fn __read_mapping_overlay<T>(doc: &YamlDoc, node: NodeId) -> Result<T, YamlError>
where
    T: FromYamlDoc,
{
    let nested = doc.rerooted_at_mapping(node)?;
    T::from_yaml_doc(&nested)
}

#[doc(hidden)]
pub fn __write_mapping_overlay<T>(
    value: &T,
    doc: &mut YamlDoc,
    node: Option<NodeId>,
) -> Result<NodeId, YamlError>
where
    T: ToYamlDoc,
{
    let node = node.ok_or_else(missing_write_node_error)?;
    let mut nested = doc.rerooted_at_mapping(node)?;
    value.apply_to_yaml_doc(&mut nested)?;
    doc.queue_edits_from(&nested)?;
    Ok(node)
}

#[doc(hidden)]
pub fn __read_mapping_fields<T, F>(doc: &YamlDoc, node: NodeId, read: F) -> Result<T, YamlError>
where
    F: FnOnce(&YamlDoc) -> Result<T, YamlError>,
{
    let nested = doc.rerooted_at_mapping(node)?;
    read(&nested)
}

#[doc(hidden)]
pub fn __write_mapping_fields<F>(
    doc: &mut YamlDoc,
    node: NodeId,
    write: F,
) -> Result<NodeId, YamlError>
where
    F: FnOnce(&mut YamlDoc) -> Result<(), YamlError>,
{
    let mut nested = doc.rerooted_at_mapping(node)?;
    write(&mut nested)?;
    doc.queue_edits_from(&nested)?;
    Ok(node)
}

#[doc(hidden)]
pub fn __read_tagged_yaml_value<T>(doc: &YamlDoc, node: NodeId) -> Result<T, YamlError>
where
    T: YamlValue,
{
    let nested = doc.rerooted_without_tag(node)?;
    T::read_yaml(&nested, node)
}

#[doc(hidden)]
pub fn __write_tagged_yaml_value<T>(
    value: &T,
    doc: &mut YamlDoc,
    node: NodeId,
) -> Result<NodeId, YamlError>
where
    T: YamlValue,
{
    let mut nested = doc.rerooted_without_tag(node)?;
    value.write_yaml(&mut nested, Some(node))?;
    doc.queue_edits_from(&nested)?;
    Ok(node)
}

#[doc(hidden)]
pub fn __tag_yaml_fragment(
    tag: &str,
    payload: String,
    indent: usize,
    line_ending: &str,
) -> Result<String, YamlError> {
    if tag.is_empty()
        || !tag
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Emitter,
                format!("`{tag}` is not a valid local YAML enum tag"),
                Span::empty(0),
            )
            .with_expected("an ASCII alphanumeric, underscore, or hyphen tag name"),
        ));
    }
    let fragment = parse_typed_fragment(&payload)?;
    let is_block_collection = matches!(
        fragment.document().semantic_kind(fragment.root()),
        Some(
            SemanticKind::Mapping {
                style: crate::CollectionStyle::Block
            } | SemanticKind::Sequence {
                style: crate::CollectionStyle::Block
            }
        )
    );
    if is_block_collection || payload.contains('\n') || payload.starts_with(' ') {
        Ok(format!(
            "{}!{tag}{line_ending}{payload}",
            " ".repeat(indent)
        ))
    } else {
        Ok(format!("!{tag} {payload}"))
    }
}

#[doc(hidden)]
pub fn __sequence_fields_to_yaml_fragment(
    fields: &[String],
    indent: usize,
    line_ending: &str,
) -> String {
    if fields.is_empty() {
        return "[]".to_owned();
    }
    let indent_text = " ".repeat(indent);
    let mut output = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            output.push_str(line_ending);
        }
        push_block_sequence_item(&mut output, &indent_text, field, line_ending);
    }
    output
}

#[doc(hidden)]
pub fn __read_yaml_document<T>(doc: &YamlDoc) -> Result<T, YamlError>
where
    T: YamlValue,
{
    let node = doc
        .document_root(0)?
        .ok_or_else(|| missing_document_value_error(doc))?;
    T::read_yaml(doc, node)
}

#[doc(hidden)]
pub fn __write_yaml_document<T>(value: &T, doc: &mut YamlDoc) -> Result<(), YamlError>
where
    T: YamlValue,
{
    let node = doc
        .document_root(0)?
        .ok_or_else(|| missing_document_value_error(doc))?;
    value.write_yaml(doc, Some(node))?;
    Ok(())
}

#[doc(hidden)]
pub fn __replace_yaml_value<T>(
    value: &T,
    doc: &mut YamlDoc,
    node: NodeId,
) -> Result<NodeId, YamlError>
where
    T: ToYamlFragment,
{
    let yaml = value.to_yaml_fragment(0, doc.preferred_line_ending())?;
    replace_typed_yaml(doc, node, yaml)
}

fn replace_typed_yaml(
    doc: &mut YamlDoc,
    node: NodeId,
    mut yaml: String,
) -> Result<NodeId, YamlError> {
    if let Some(anchor) = doc.anchor(node).map(str::to_owned) {
        preserve_fragment_root_anchor(&mut yaml, &anchor)?;
    }
    if let Some(comment) = trailing_inline_comment(doc, node)? {
        yaml.push(' ');
        yaml.push_str(&comment);
    }
    let fragment = parse_typed_fragment(&yaml)?;
    let replacement =
        if doc.raw_tag(node).is_some() && fragment.document().raw_tag(fragment.root()).is_none() {
            doc.queue_fragment_replacement_whole(node, &fragment)
        } else {
            doc.queue_fragment_replacement(node, &fragment)
        };
    replacement.map_err(YamlEditError::into_yaml_error)?;
    Ok(node)
}

#[doc(hidden)]
pub fn __typed_node_error(
    doc: &YamlDoc,
    node: NodeId,
    message: impl Into<String>,
    expected: &[&str],
) -> YamlError {
    let span = doc.node(node).map_or(Span::empty(0), Node::span);
    let mut diagnostic = Diagnostic::new(DiagnosticKind::Typed, message, span);
    diagnostic
        .expected
        .extend(expected.iter().map(|value| (*value).to_owned()));
    YamlError::new(diagnostic).with_position_from(&doc.source)
}

fn preserve_fragment_root_anchor(yaml: &mut String, anchor: &str) -> Result<(), YamlError> {
    let properties = parse_node_properties(yaml, Span::from_usize(0, yaml.len()))?;
    if let Some(existing) = properties.anchor {
        yaml.replace_range(existing.start as usize..existing.end as usize, anchor);
    } else if let Some(tag) = properties.tag {
        yaml.insert_str(tag.end as usize, &format!(" &{anchor}"));
    } else {
        yaml.insert_str(0, &format!("&{anchor} "));
    }
    Ok(())
}

fn trailing_inline_comment(doc: &YamlDoc, node: NodeId) -> Result<Option<String>, YamlError> {
    let span = doc.expect_node(node)?.span();
    let comment = doc.tokens()?.into_iter().rfind(|token| {
        token.kind == crate::TokenKind::Comment
            && token.span.start >= span.start
            && token.span.end <= span.end
    });
    let Some(comment) = comment else {
        return Ok(None);
    };
    let before = doc
        .source()
        .slice(Span::new(span.start, comment.span.start));
    if before.contains(['\n', '\r']) {
        return Ok(None);
    }
    Ok(Some(doc.source().slice(comment.span).to_owned()))
}

fn parse_typed_fragment(yaml: &str) -> Result<YamlFragment, YamlError> {
    YamlFragment::parse(yaml).map_err(|error| {
        YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Emitter,
                format!("typed YAML fragment is invalid: {error}"),
                Span::empty(0),
            )
            .with_expected("one valid YAML value"),
        )
    })
}

fn missing_write_node_error() -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Typed,
            "cannot insert a standalone YAML value without collection context",
            Span::empty(0),
        )
        .with_expected("an existing YAML node"),
    )
}

fn missing_document_value_error(doc: &YamlDoc) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Typed,
            "document does not contain a YAML value",
            Span::empty_from_usize(doc.as_source().len()),
        )
        .with_expected("a scalar, sequence, or mapping document root"),
    )
}

fn missing_required_field_error(key: &str) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Typed,
            format!("missing required field `{key}`"),
            Span::empty(0),
        )
        .with_expected(key),
    )
}

fn write_existing_scalar(
    doc: &mut YamlDoc,
    node: Option<NodeId>,
    value: &str,
    string_value: bool,
) -> Result<NodeId, YamlError> {
    let node = node.ok_or_else(missing_write_node_error)?;
    if let Some(block_scalar) = doc
        .node(node)
        .filter(|node| matches!(node.kind, NodeKind::LiteralScalar | NodeKind::FoldedScalar))
    {
        let replacement = format_block_scalar_replacement(doc, block_scalar, value)?;
        doc.queue_edit(block_scalar.span, replacement)?;
        return Ok(node);
    }
    let (span, style) = doc.scalar_replacement_target(node)?;
    let requires_quotes = style == crate::ScalarStyle::Plain
        && string_value
        && (!crate::edit::safe_plain_string(value)
            || doc.is_flow_context(node) && value.contains(['[', ']', '{', '}', ',']));
    let replacement = if requires_quotes {
        crate::fragment::quote_string(value)
    } else {
        match format_scalar_value(value, style) {
            Ok(replacement) => replacement,
            Err(_) if string_value => crate::fragment::quote_string(value),
            Err(error) => return Err(error),
        }
    };
    doc.queue_edit(span, replacement)?;
    Ok(node)
}

fn format_block_scalar_replacement(
    doc: &YamlDoc,
    scalar: &Node,
    value: &str,
) -> Result<String, YamlError> {
    validate_yaml_chars(value)?;
    let text = doc.source.slice(scalar.span);
    let (header_start, header_end, header_value_start) =
        block_scalar_header_line(text, scalar.span)?;
    let header = &text[..header_end];
    let header_line = &text[header_start..header_end];
    let header_info = parse_block_scalar_header(
        header_line[header_value_start..].trim_start(),
        scalar.span.start as usize + header_start + header_value_start,
    )?;
    let content_indent = doc
        .block_scalar_content_indent(scalar)
        .unwrap_or_else(|| doc.node_indent(scalar) + header_info.indent.unwrap_or(2));
    let indent_text = " ".repeat(content_indent);
    let line_ending = doc.preferred_line_ending();
    let mut output = String::new();
    output.push_str(header);
    output.push_str(line_ending);
    for line in value.split('\n') {
        if line.is_empty() {
            output.push_str(line_ending);
        } else {
            output.push_str(&indent_text);
            output.push_str(line);
            output.push_str(line_ending);
        }
    }
    if matches!(header_info.chomp, BlockChomp::Strip) {
        while output.ends_with(line_ending) {
            let new_len = output.len() - line_ending.len();
            output.truncate(new_len);
        }
    }
    Ok(output)
}

fn block_scalar_header_line(text: &str, span: Span) -> Result<(usize, usize, usize), YamlError> {
    let mut line_start = 0;
    while line_start < text.len() {
        let line_end = text[line_start..]
            .find(['\r', '\n'])
            .map_or(text.len(), |offset| line_start + offset);
        let line = &text[line_start..line_end];
        let properties = parse_node_properties(
            line,
            Span::from_usize(
                span.start as usize + line_start,
                span.start as usize + line_end,
            ),
        )?;
        let value = line[properties.value_start..].trim_start();
        if value.starts_with('|') || value.starts_with('>') {
            let leading = line[properties.value_start..].len() - value.len();
            return Ok((line_start, line_end, properties.value_start + leading));
        }

        line_start = line_end;
        if text[line_start..].starts_with("\r\n") {
            line_start += 2;
        } else if text[line_start..].starts_with(['\r', '\n']) {
            line_start += 1;
        } else {
            break;
        }
    }

    Err(YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Emitter,
            "could not find block scalar header",
            span,
        )
        .with_expected("| or > block scalar header"),
    ))
}

fn push_block_sequence_item(
    output: &mut String,
    indent_text: &str,
    fragment: &str,
    line_ending: &str,
) {
    output.push_str(indent_text);
    if fragment.contains('\n') || fragment.starts_with(' ') {
        output.push('-');
        output.push_str(line_ending);
        output.push_str(fragment);
    } else {
        output.push_str("- ");
        output.push_str(fragment);
    }
}

fn sequence_yaml_fragment<T>(
    values: &[T],
    indent: usize,
    line_ending: &str,
) -> Result<String, YamlError>
where
    T: ToYamlFragment,
{
    if values.is_empty() {
        return Ok("[]".to_owned());
    }
    let indent_text = " ".repeat(indent);
    let mut output = String::new();
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push_str(line_ending);
        }
        let value = value.to_yaml_fragment(indent + 2, line_ending)?;
        push_block_sequence_item(&mut output, &indent_text, &value, line_ending);
    }
    Ok(output)
}

fn missing_collection_item_error(doc: &YamlDoc, node: &Node, collection: &str) -> YamlError {
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Typed,
            format!("{collection} entry does not contain a value"),
            node.span,
        )
        .with_expected("a YAML value node"),
    )
    .with_position_from(&doc.source)
}

fn format_block_sequence_tail<T>(
    doc: &YamlDoc,
    indent: usize,
    values: &[T],
) -> Result<String, YamlError>
where
    T: ToYamlFragment,
{
    let indent_text = " ".repeat(indent);
    let line_ending = doc.preferred_line_ending();
    let mut output = String::new();

    for value in values {
        let value = value.to_yaml_fragment(indent + 2, line_ending)?;
        push_block_sequence_item(&mut output, &indent_text, &value, line_ending);
        output.push_str(line_ending);
    }

    Ok(output)
}

fn format_value_as_flow_fragment<T>(doc: &YamlDoc, value: &T) -> Result<String, YamlError>
where
    T: ToYamlFragment,
{
    let fragment = value.to_yaml_fragment(0, doc.preferred_line_ending())?;
    let fragment = parse_typed_fragment(&fragment)?;
    fragment.render_flow(doc).map_err(|error| {
        YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Emitter,
                format!("typed YAML fragment cannot be rendered in flow style: {error}"),
                Span::empty(0),
            )
            .with_expected("a YAML value representable in flow style"),
        )
    })
}

fn typed_parse_error(doc: &YamlDoc, node: NodeId, type_name: &str, value: &str) -> YamlError {
    let span = doc.node(node).map_or(Span::empty(0), |node| node.span);
    YamlError::new(
        Diagnostic::new(
            DiagnosticKind::Typed,
            format!("could not parse `{value}` as {type_name}"),
            span,
        )
        .with_expected(type_name),
    )
    .with_position_from(&doc.source)
}

fn resolved_scalar_at(doc: &YamlDoc, node: NodeId) -> Result<ResolvedScalar, YamlError> {
    let Some(SemanticKind::Scalar { style }) = doc.semantic_kind(node) else {
        return Err(typed_parse_error(doc, node, "scalar", ""));
    };
    let value = doc.scalar_value(node)?;
    let tag = doc.resolved_tag(node)?;
    resolve_scalar(&value, style, tag.as_deref())
        .map_err(|_| typed_parse_error(doc, node, "YAML 1.2 core scalar", &value))
}

impl YamlValue for String {
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        doc.scalar_value(node).map(Cow::into_owned)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        if let Some(node) = node
            && Self::read_yaml(doc, node)? == *self
        {
            return Ok(node);
        }
        write_existing_scalar(doc, node, self, true)
    }
}

impl YamlValue for bool {
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        let value = doc.scalar_value(node)?;
        match resolved_scalar_at(doc, node)? {
            ResolvedScalar::Bool(value) => Ok(value),
            ResolvedScalar::String if matches!(value.as_ref(), "true" | "True" | "TRUE") => {
                Ok(true)
            }
            ResolvedScalar::String if matches!(value.as_ref(), "false" | "False" | "FALSE") => {
                Ok(false)
            }
            _ => Err(typed_parse_error(doc, node, "bool", &value)),
        }
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        if let Some(node) = node
            && Self::read_yaml(doc, node)? == *self
        {
            return Ok(node);
        }
        write_existing_scalar(doc, node, if *self { "true" } else { "false" }, false)
    }
}

macro_rules! impl_yaml_unsigned {
    ($($type:ty),* $(,)?) => {
        $(
            impl YamlValue for $type {
                fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
                    let value = doc.scalar_value(node)?;
                    match resolved_scalar_at(doc, node)? {
                        ResolvedScalar::Number(number) => number
                            .as_u128()
                            .and_then(|number| <$type>::try_from(number).ok()),
                        ResolvedScalar::String => value.parse::<$type>().ok(),
                        _ => None,
                    }
                        .ok_or_else(|| typed_parse_error(doc, node, stringify!($type), &value))
                }

                fn write_yaml(
                    &self,
                    doc: &mut YamlDoc,
                    node: Option<NodeId>,
                ) -> Result<NodeId, YamlError> {
                    if let Some(node) = node
                        && Self::read_yaml(doc, node)? == *self
                    {
                        return Ok(node);
                    }
                    write_existing_scalar(doc, node, &self.to_string(), false)
                }
            }
        )*
    };
}

macro_rules! impl_yaml_signed {
    ($($type:ty),* $(,)?) => {
        $(
            impl YamlValue for $type {
                fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
                    let value = doc.scalar_value(node)?;
                    match resolved_scalar_at(doc, node)? {
                        ResolvedScalar::Number(number) => number
                            .as_i128()
                            .and_then(|number| <$type>::try_from(number).ok()),
                        ResolvedScalar::String => value.parse::<$type>().ok(),
                        _ => None,
                    }
                        .ok_or_else(|| typed_parse_error(doc, node, stringify!($type), &value))
                }

                fn write_yaml(
                    &self,
                    doc: &mut YamlDoc,
                    node: Option<NodeId>,
                ) -> Result<NodeId, YamlError> {
                    if let Some(node) = node
                        && Self::read_yaml(doc, node)? == *self
                    {
                        return Ok(node);
                    }
                    write_existing_scalar(doc, node, &self.to_string(), false)
                }
            }
        )*
    };
}

macro_rules! impl_yaml_float {
    ($($type:ty),* $(,)?) => {
        $(
            impl YamlValue for $type {
                fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
                    let value = doc.scalar_value(node)?;
                    let number = match resolved_scalar_at(doc, node)? {
                        ResolvedScalar::Number(number) => number.as_f64(),
                        ResolvedScalar::NonFinite(NonFiniteFloat::PositiveInfinity) => {
                            Some(f64::INFINITY)
                        }
                        ResolvedScalar::NonFinite(NonFiniteFloat::NegativeInfinity) => {
                            Some(f64::NEG_INFINITY)
                        }
                        ResolvedScalar::NonFinite(NonFiniteFloat::NaN) => Some(f64::NAN),
                        ResolvedScalar::String => value.parse::<f64>().ok(),
                        _ => None,
                    }
                    .ok_or_else(|| typed_parse_error(doc, node, stringify!($type), &value))?;
                    let converted = number as $type;
                    if number.is_finite() && !converted.is_finite() {
                        return Err(typed_parse_error(doc, node, stringify!($type), &value));
                    }
                    Ok(converted)
                }

                fn write_yaml(
                    &self,
                    doc: &mut YamlDoc,
                    node: Option<NodeId>,
                ) -> Result<NodeId, YamlError> {
                    if let Some(node) = node {
                        let current = Self::read_yaml(doc, node)?;
                        if current == *self || current.is_nan() && self.is_nan() {
                            return Ok(node);
                        }
                    }
                    let value = if self.is_nan() {
                        ".nan".to_owned()
                    } else if *self == <$type>::INFINITY {
                        ".inf".to_owned()
                    } else if *self == <$type>::NEG_INFINITY {
                        "-.inf".to_owned()
                    } else {
                        self.to_string()
                    };
                    write_existing_scalar(doc, node, &value, false)
                }
            }
        )*
    };
}

impl_yaml_unsigned!(u8, u16, u32, u64, u128, usize);
impl_yaml_signed!(i8, i16, i32, i64, i128, isize);
impl_yaml_float!(f32, f64);

impl YamlValue for char {
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        let value = doc.scalar_value(node)?;
        let mut characters = value.chars();
        let Some(character) = characters.next() else {
            return Err(typed_parse_error(doc, node, "char", &value));
        };
        if characters.next().is_some() {
            return Err(typed_parse_error(doc, node, "char", &value));
        }
        Ok(character)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        if let Some(node) = node
            && Self::read_yaml(doc, node)? == *self
        {
            return Ok(node);
        }
        write_existing_scalar(doc, node, &self.to_string(), true)
    }
}

impl<T> YamlValue for Box<T>
where
    T: YamlValue,
{
    fn read_yaml_field(doc: &YamlDoc, node: Option<NodeId>, key: &str) -> Result<Self, YamlError> {
        T::read_yaml_field(doc, node, key).map(Box::new)
    }

    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        T::read_yaml(doc, node).map(Box::new)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        (**self).write_yaml(doc, node)
    }
}

impl<T> YamlValue for Option<T>
where
    T: YamlValue,
{
    fn read_yaml_field(doc: &YamlDoc, node: Option<NodeId>, _key: &str) -> Result<Self, YamlError> {
        match node {
            Some(node) => Self::read_yaml(doc, node),
            None => Ok(None),
        }
    }

    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        if matches!(resolved_scalar_at(doc, node), Ok(ResolvedScalar::Null)) {
            Ok(None)
        } else {
            T::read_yaml(doc, node).map(Some)
        }
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        if let Some(value) = self {
            value.write_yaml(doc, node)
        } else {
            let node = node.ok_or_else(missing_write_node_error)?;
            if matches!(resolved_scalar_at(doc, node), Ok(ResolvedScalar::Null)) {
                return Ok(node);
            }
            if doc.raw_tag(node).is_some() {
                return replace_typed_yaml(doc, node, "null".to_owned());
            }
            let null = YamlFragment::parse("null").expect("static null fragment is valid");
            doc.queue_fragment_replacement(node, &null)
                .map_err(YamlEditError::into_yaml_error)?;
            Ok(node)
        }
    }
}

fn read_yaml_sequence<T>(doc: &YamlDoc, node: NodeId) -> Result<Vec<T>, YamlError>
where
    T: YamlValue,
{
    let sequence = doc.expect_node(node)?;
    let mut values = Vec::new();

    match sequence.kind {
        NodeKind::BlockSequence => {
            for entry in doc.children(node) {
                let entry_node = doc.expect_node(entry)?;
                let Some(value_node) = doc.children(entry).next() else {
                    return Err(missing_collection_item_error(doc, entry_node, "sequence"));
                };
                values.push(T::read_yaml(doc, value_node)?);
            }
        }
        NodeKind::FlowSequence => {
            for value_node in doc.sequence_items(node) {
                values.push(T::read_yaml(doc, value_node)?);
            }
        }
        _ => {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    format!("expected sequence, found {:?}", sequence.kind),
                    sequence.span,
                )
                .with_expected("BlockSequence or FlowSequence"),
            )
            .with_position_from(&doc.source));
        }
    }

    Ok(values)
}

fn write_yaml_sequence<T>(
    values: &[T],
    doc: &mut YamlDoc,
    node: Option<NodeId>,
) -> Result<NodeId, YamlError>
where
    T: YamlValue + ToYamlFragment,
{
    let node = node.ok_or_else(missing_write_node_error)?;
    let sequence = doc.expect_node(node)?;
    if !matches!(
        sequence.kind,
        NodeKind::BlockSequence | NodeKind::FlowSequence
    ) {
        return Err(YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Typed,
                format!("expected sequence, found {:?}", sequence.kind),
                sequence.span,
            )
            .with_expected("BlockSequence or FlowSequence"),
        )
        .with_position_from(&doc.source));
    }
    let sequence_kind = sequence.kind;
    let tail_indent = doc.node_indent(sequence);
    let insertion_offset = doc.sequence_insertion_offset(sequence);
    let items = doc.sequence_items(node).collect::<Vec<_>>();
    let common_len = items.len().min(values.len());
    for (value_node, value) in items.iter().copied().take(common_len).zip(values) {
        value.write_yaml(doc, Some(value_node))?;
    }

    if values.len() > items.len() {
        if sequence_kind == NodeKind::FlowSequence {
            let sequence = doc.expect_node(node)?;
            let close = closing_delimiter_offset(doc, sequence.span, ']')
                .map_err(crate::YamlEditError::into_yaml_error)?;
            let mut replacement = String::new();
            for (index, value) in values[items.len()..].iter().enumerate() {
                if !items.is_empty() || index > 0 {
                    replacement.push_str(", ");
                }
                replacement.push_str(&format_value_as_flow_fragment(doc, value)?);
            }
            doc.queue_edit(Span::empty_from_usize(close), replacement)?;
        } else {
            let replacement = format_block_sequence_tail(doc, tail_indent, &values[items.len()..])?;
            doc.queue_edit(Span::empty_from_usize(insertion_offset), replacement)?;
        }
        return Ok(node);
    }

    if values.len() < items.len() {
        let entries = items[values.len()..]
            .iter()
            .copied()
            .map(|item| doc.containing_entry(item).unwrap_or(item))
            .collect::<Vec<_>>();
        doc.remove_collection_entries(node, &entries)?;
    }

    Ok(node)
}

impl<T> YamlValue for Vec<T>
where
    T: YamlValue + ToYamlFragment,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        read_yaml_sequence(doc, node)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        write_yaml_sequence(self, doc, node)
    }
}

impl<T, const N: usize> YamlValue for [T; N]
where
    T: YamlValue + ToYamlFragment,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        let values = read_yaml_sequence(doc, node)?;
        let actual = values.len();
        values.try_into().map_err(|_| {
            let span = doc.node(node).map_or(Span::empty(0), |node| node.span);
            YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    format!("expected sequence with {N} items, found {actual}"),
                    span,
                )
                .with_expected(format!("{N} sequence items")),
            )
            .with_position_from(&doc.source)
        })
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        write_yaml_sequence(self, doc, node)
    }
}

impl<T> YamlValue for std::collections::BTreeMap<String, T>
where
    T: YamlValue + ToYamlFragment,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        let mapping = doc.expect_node(node)?;
        let mut values = std::collections::BTreeMap::new();

        match mapping.kind {
            NodeKind::BlockMapping => {
                let mapping_indent = doc.node_indent(mapping);
                for entry in doc.children(node) {
                    let entry_node = doc.expect_node(entry)?;
                    let mut children = doc.children(entry);
                    let Some(key_node) = children.next() else {
                        continue;
                    };
                    let key = doc.scalar_value(key_node)?.into_owned();
                    let value_node = if let Some(value_node) = children.next() {
                        value_node
                    } else {
                        doc.find_nested_collection_after(entry_node, mapping_indent)
                            .ok_or_else(|| {
                                missing_collection_item_error(doc, entry_node, "mapping")
                            })?
                    };
                    values.insert(key, T::read_yaml(doc, value_node)?);
                }
            }
            NodeKind::FlowMapping => {
                for entry in doc.children(node) {
                    let entry_node = doc.expect_node(entry)?;
                    let mut children = doc.children(entry);
                    let Some(key_node) = children.next() else {
                        continue;
                    };
                    let key = doc.scalar_value(key_node)?.into_owned();
                    let value_node = children
                        .next()
                        .ok_or_else(|| missing_collection_item_error(doc, entry_node, "mapping"))?;
                    values.insert(key, T::read_yaml(doc, value_node)?);
                }
            }
            _ => {
                return Err(YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Typed,
                        format!("expected mapping, found {:?}", mapping.kind),
                        mapping.span,
                    )
                    .with_expected("BlockMapping or FlowMapping"),
                )
                .with_position_from(&doc.source));
            }
        }

        Ok(values)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        let node = node.ok_or_else(missing_write_node_error)?;
        let mapping = doc.expect_node(node)?;
        if !matches!(mapping.kind, NodeKind::BlockMapping | NodeKind::FlowMapping) {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    format!("expected mapping, found {:?}", mapping.kind),
                    mapping.span,
                )
                .with_expected("BlockMapping or FlowMapping"),
            )
            .with_position_from(&doc.source));
        }
        for (key, value) in self {
            if let Some(value_node) = doc.get_mapping_value(node, key)? {
                value.write_yaml(doc, Some(value_node))?;
            } else {
                doc.insert_mapping_value_with_comment(
                    node,
                    key,
                    value,
                    MappingEntryStyle::Inherit,
                    None,
                )?;
            }
        }
        let allowed_keys = self.keys().map(String::as_str).collect::<Vec<_>>();
        doc.retain_mapping_entries(node, &allowed_keys)?;
        Ok(node)
    }
}

impl<T, S> YamlValue for HashMap<String, T, S>
where
    T: YamlValue + ToYamlFragment,
    S: BuildHasher + Default,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        let mapping = doc.expect_node(node)?;
        let mut values = HashMap::with_hasher(S::default());

        match mapping.kind {
            NodeKind::BlockMapping => {
                let mapping_indent = doc.node_indent(mapping);
                for entry in doc.children(node) {
                    let entry_node = doc.expect_node(entry)?;
                    let mut children = doc.children(entry);
                    let Some(key_node) = children.next() else {
                        continue;
                    };
                    let key = doc.scalar_value(key_node)?.into_owned();
                    let value_node = if let Some(value_node) = children.next() {
                        value_node
                    } else {
                        doc.find_nested_collection_after(entry_node, mapping_indent)
                            .ok_or_else(|| {
                                missing_collection_item_error(doc, entry_node, "mapping")
                            })?
                    };
                    values.insert(key, T::read_yaml(doc, value_node)?);
                }
            }
            NodeKind::FlowMapping => {
                for entry in doc.children(node) {
                    let entry_node = doc.expect_node(entry)?;
                    let mut children = doc.children(entry);
                    let Some(key_node) = children.next() else {
                        continue;
                    };
                    let key = doc.scalar_value(key_node)?.into_owned();
                    let value_node = children
                        .next()
                        .ok_or_else(|| missing_collection_item_error(doc, entry_node, "mapping"))?;
                    values.insert(key, T::read_yaml(doc, value_node)?);
                }
            }
            _ => {
                return Err(YamlError::new(
                    Diagnostic::new(
                        DiagnosticKind::Typed,
                        format!("expected mapping, found {:?}", mapping.kind),
                        mapping.span,
                    )
                    .with_expected("BlockMapping or FlowMapping"),
                )
                .with_position_from(&doc.source));
            }
        }

        Ok(values)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        let node = node.ok_or_else(missing_write_node_error)?;
        let mapping = doc.expect_node(node)?;
        if !matches!(mapping.kind, NodeKind::BlockMapping | NodeKind::FlowMapping) {
            return Err(YamlError::new(
                Diagnostic::new(
                    DiagnosticKind::Typed,
                    format!("expected mapping, found {:?}", mapping.kind),
                    mapping.span,
                )
                .with_expected("BlockMapping or FlowMapping"),
            )
            .with_position_from(&doc.source));
        }
        let mut entries = self.iter().collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(key, _)| *key);
        for (key, value) in entries {
            if let Some(value_node) = doc.get_mapping_value(node, key)? {
                value.write_yaml(doc, Some(value_node))?;
            } else {
                doc.insert_mapping_value_with_comment(
                    node,
                    key,
                    value,
                    MappingEntryStyle::Inherit,
                    None,
                )?;
            }
        }
        let allowed_keys = self.keys().map(String::as_str).collect::<Vec<_>>();
        doc.retain_mapping_entries(node, &allowed_keys)?;
        Ok(node)
    }
}
