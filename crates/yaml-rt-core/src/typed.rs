use std::borrow::Cow;

use crate::edit::closing_delimiter_offset;
use crate::{
    BlockChomp, Diagnostic, DiagnosticKind, MappingEntryStyle, Node, NodeId, NodeKind, Span,
    YamlDoc, YamlError, YamlFragment, format_scalar_value, parse_block_scalar_header,
    parse_node_properties, validate_plain_mapping_fragment, validate_yaml_chars,
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
        plain_yaml_fragment(self, "YAML value")
    }
}

impl ToYamlFragment for &str {
    fn to_yaml_fragment(&self, _indent: usize, _line_ending: &str) -> Result<String, YamlError> {
        plain_yaml_fragment(self, "YAML value")
    }
}

impl ToYamlFragment for bool {
    fn to_yaml_fragment(&self, _indent: usize, _line_ending: &str) -> Result<String, YamlError> {
        Ok(if *self { "true" } else { "false" }.to_owned())
    }
}

impl_plain_yaml_fragment!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);

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
        if self.is_empty() {
            return Ok("[]".to_owned());
        }
        let indent_text = " ".repeat(indent);
        let mut output = String::new();
        for (index, value) in self.iter().enumerate() {
            if index > 0 {
                output.push_str(line_ending);
            }
            let value = value.to_yaml_fragment(indent + 2, line_ending)?;
            push_block_sequence_item(&mut output, &indent_text, &value, line_ending);
        }
        Ok(output)
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
            validate_plain_mapping_fragment(key, "mapping key")?;
            if index > 0 {
                output.push_str(line_ending);
            }
            output.push_str(&indent_text);
            output.push_str(key);
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

impl<T> ToYamlFragment for T
where
    T: ToYamlDoc,
{
    fn to_yaml_fragment(&self, indent: usize, line_ending: &str) -> Result<String, YamlError> {
        let mut doc = YamlDoc::parse("root:\n  __rty_placeholder: null\n")?;
        let root = doc.get_path(&["root"])?.ok_or_else(|| {
            YamlError::new(Diagnostic::new(
                DiagnosticKind::Emitter,
                "temporary nested mapping root was not created",
                Span::empty(0),
            ))
        })?;
        let mut nested = doc.rerooted_at_mapping(root)?;
        self.apply_to_yaml_doc(&mut nested)?;
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

fn write_existing_scalar(
    doc: &mut YamlDoc,
    node: Option<NodeId>,
    value: &str,
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
    let replacement = if style == crate::ScalarStyle::Plain
        && doc.is_flow_context(node)
        && value.contains(['[', ']', '{', '}', ','])
    {
        crate::fragment::quote_string(value)
    } else {
        format_scalar_value(value, style)?
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
    let fragment = YamlFragment::parse(&fragment).map_err(|error| {
        YamlError::new(
            Diagnostic::new(
                DiagnosticKind::Emitter,
                format!("typed YAML fragment is invalid: {error}"),
                Span::empty(0),
            )
            .with_expected("one valid YAML value"),
        )
    })?;
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

impl YamlValue for String {
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        doc.scalar_value(node).map(Cow::into_owned)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        write_existing_scalar(doc, node, self)
    }
}

impl YamlValue for bool {
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        let value = doc.scalar_value(node)?;
        match value.as_ref() {
            "true" | "True" | "TRUE" => Ok(true),
            "false" | "False" | "FALSE" => Ok(false),
            _ => Err(typed_parse_error(doc, node, "bool", &value)),
        }
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        write_existing_scalar(doc, node, if *self { "true" } else { "false" })
    }
}

macro_rules! impl_yaml_number {
    ($($type:ty),* $(,)?) => {
        $(
            impl YamlValue for $type {
                fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
                    let value = doc.scalar_value(node)?;
                    value.parse::<$type>().map_err(|_| {
                        typed_parse_error(doc, node, stringify!($type), &value)
                    })
                }

                fn write_yaml(
                    &self,
                    doc: &mut YamlDoc,
                    node: Option<NodeId>,
                ) -> Result<NodeId, YamlError> {
                    write_existing_scalar(doc, node, &self.to_string())
                }
            }
        )*
    };
}

impl_yaml_number!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);

impl<T> YamlValue for Option<T>
where
    T: YamlValue,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        T::read_yaml(doc, node).map(Some)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        if let Some(value) = self {
            value.write_yaml(doc, node)
        } else {
            let node = node.ok_or_else(missing_write_node_error)?;
            let remove_node = doc.containing_entry(node).unwrap_or(node);
            doc.remove_node(remove_node)?;
            Ok(node)
        }
    }
}

impl<T> YamlValue for Vec<T>
where
    T: YamlValue + ToYamlFragment,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
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

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
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
        let common_len = items.len().min(self.len());
        for (value_node, value) in items.iter().copied().take(common_len).zip(self) {
            value.write_yaml(doc, Some(value_node))?;
        }

        if self.len() > items.len() {
            if sequence_kind == NodeKind::FlowSequence {
                let sequence = doc.expect_node(node)?;
                let close = closing_delimiter_offset(doc, sequence.span, ']')
                    .map_err(crate::YamlEditError::into_yaml_error)?;
                let mut replacement = String::new();
                for (index, value) in self[items.len()..].iter().enumerate() {
                    if !items.is_empty() || index > 0 {
                        replacement.push_str(", ");
                    }
                    replacement.push_str(&format_value_as_flow_fragment(doc, value)?);
                }
                doc.queue_edit(Span::empty_from_usize(close), replacement)?;
            } else {
                let replacement =
                    format_block_sequence_tail(doc, tail_indent, &self[items.len()..])?;
                doc.queue_edit(Span::empty_from_usize(insertion_offset), replacement)?;
            }
            return Ok(node);
        }

        if self.len() < items.len() {
            let entries = items[self.len()..]
                .iter()
                .copied()
                .map(|item| doc.containing_entry(item).unwrap_or(item))
                .collect::<Vec<_>>();
            doc.remove_collection_entries(node, &entries)?;
        }

        Ok(node)
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
                    let key = doc.scalar_text(key_node)?.to_owned();
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
                    let key = doc.scalar_text(key_node)?.to_owned();
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

impl<T> YamlValue for T
where
    T: FromYamlDoc + ToYamlDoc,
{
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError> {
        let nested = doc.rerooted_at_mapping(node)?;
        Self::from_yaml_doc(&nested)
    }

    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError> {
        let node = node.ok_or_else(missing_write_node_error)?;
        let mut nested = doc.rerooted_at_mapping(node)?;
        self.apply_to_yaml_doc(&mut nested)?;
        doc.queue_edits_from(&nested)?;
        Ok(node)
    }
}
