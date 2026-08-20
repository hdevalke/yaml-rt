use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use crate::{NodeId, SemanticKind, YamlDoc, YamlError, YamlScalarStyle, strip_inline_comment};

/// A parsed YAML value document containing exactly one root node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlFragment {
    doc: YamlDoc,
    root: NodeId,
}

impl YamlFragment {
    /// Parses an owned YAML value.
    ///
    /// # Errors
    ///
    /// Returns an error when the input is invalid YAML, does not contain exactly
    /// one rooted document, or contains an alias that escapes the value root.
    pub fn parse_owned(input: String) -> Result<Self, FragmentError> {
        let doc = YamlDoc::parse_owned(input).map_err(FragmentError::from)?;
        if doc.document_count() != 1 {
            return Err(FragmentError::new(format!(
                "a YAML value must contain exactly one document, found {}",
                doc.document_count()
            )));
        }
        let root = doc
            .document_root(0)
            .map_err(FragmentError::from)?
            .ok_or_else(|| FragmentError::new("a YAML value must contain one root node"))?;
        let fragment = Self { doc, root };
        fragment.validate_alias_scope()?;
        Ok(fragment)
    }

    /// Parses a borrowed YAML value.
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as [`Self::parse_owned`].
    pub fn parse(input: &str) -> Result<Self, FragmentError> {
        Self::parse_owned(input.to_owned())
    }

    /// Returns the fragment's parsed document.
    #[must_use]
    pub fn document(&self) -> &YamlDoc {
        &self.doc
    }

    /// Returns the fragment root node.
    #[must_use]
    pub const fn root(&self) -> NodeId {
        self.root
    }

    /// Returns the root as minimally de-indented standalone YAML.
    ///
    /// # Errors
    ///
    /// Returns an error when the stored root node is no longer valid.
    pub fn to_yaml(&self) -> Result<String, FragmentError> {
        self.doc
            .extract_node(self.root)
            .map_err(FragmentError::from)
    }

    pub(crate) fn contains_anchor(&self) -> bool {
        self.subtree_nodes()
            .any(|node| self.doc.anchor(node).is_some())
    }

    pub(crate) fn from_document_node(doc: &YamlDoc, root: NodeId) -> Result<Self, FragmentError> {
        doc.node(root)
            .ok_or_else(|| FragmentError::new("fragment source node is missing"))?;
        Ok(Self {
            doc: doc.clone(),
            root,
        })
    }

    pub(crate) fn prepared(&self, target: &YamlDoc) -> Result<Self, FragmentError> {
        self.prepared_with_anchor_exemption(target, None)
    }

    pub(crate) fn prepared_for_replacement(
        &self,
        target: &YamlDoc,
        replaced: NodeId,
    ) -> Result<Self, FragmentError> {
        self.prepared_with_anchor_exemption(target, target.anchor(replaced))
    }

    fn prepared_with_anchor_exemption(
        &self,
        target: &YamlDoc,
        exempt_anchor: Option<&str>,
    ) -> Result<Self, FragmentError> {
        let mut used = target.anchor_names();
        if let Some(anchor) = exempt_anchor {
            used.remove(anchor);
        }
        let mut renamed = BTreeMap::new();
        for node in self.subtree_nodes() {
            let Some(name) = self.doc.anchor(node) else {
                continue;
            };
            if used.insert(name.to_owned()) {
                continue;
            }
            let mut suffix = 1_u64;
            let replacement = loop {
                let candidate = format!("{name}_{suffix}");
                if used.insert(candidate.clone()) {
                    break candidate;
                }
                suffix = suffix.saturating_add(1);
            };
            renamed.insert(name.to_owned(), replacement);
        }
        if renamed.is_empty() {
            return Ok(self.clone());
        }

        let mut doc = self.doc.clone();
        let nodes = self.subtree_nodes().collect::<Vec<_>>();
        for node in nodes {
            let Some(properties) = doc.semantic_properties(node) else {
                continue;
            };
            if let Some(span) = properties.anchor {
                let old = doc.source.slice(span);
                if let Some(new) = renamed.get(old) {
                    doc.queue_edit(span, new.clone())
                        .map_err(FragmentError::from)?;
                }
            }
            if let Some(span) = properties.alias {
                let old = doc.source.slice(span);
                if let Some(new) = renamed.get(old) {
                    doc.queue_edit(span, new.clone())
                        .map_err(FragmentError::from)?;
                }
            }
        }
        doc.commit_edits().map_err(FragmentError::from)?;
        let root = doc
            .document_root(0)
            .map_err(FragmentError::from)?
            .ok_or_else(|| FragmentError::new("prepared fragment lost its root node"))?;
        Ok(Self { doc, root })
    }

    pub(crate) fn render_flow(&self, target: &YamlDoc) -> Result<String, FragmentError> {
        let prepared = self.prepared(target)?;
        prepared.render_node_flow(prepared.root, 0)
    }

    pub(crate) fn render_flow_for_replacement(
        &self,
        target: &YamlDoc,
        replaced: NodeId,
    ) -> Result<String, FragmentError> {
        let prepared = self.prepared_for_replacement(target, replaced)?;
        prepared.render_node_flow(prepared.root, 0)
    }

    fn render_node_flow(&self, node: NodeId, depth: usize) -> Result<String, FragmentError> {
        enum RenderAction {
            Node(NodeId, usize),
            Text(&'static str),
        }

        let mut output = String::new();
        let mut pending = vec![RenderAction::Node(node, depth)];
        while let Some(action) = pending.pop() {
            let RenderAction::Node(node, depth) = action else {
                let RenderAction::Text(text) = action else {
                    unreachable!();
                };
                output.push_str(text);
                continue;
            };
            if depth > 1024 {
                return Err(FragmentError::new(
                    "fragment rendering recursion limit exceeded",
                ));
            }
            match self.doc.semantic_kind(node) {
                Some(SemanticKind::Alias) => {
                    output.push('*');
                    output.push_str(self.doc.alias_name(node).unwrap_or_default());
                }
                Some(SemanticKind::Scalar { style }) => {
                    let prefix = self.property_prefix(node);
                    if matches!(style, YamlScalarStyle::Literal | YamlScalarStyle::Folded)
                        || self
                            .doc
                            .node(node)
                            .is_some_and(|node| node.span().is_empty())
                    {
                        let value = self.doc.scalar_value(node).map_err(FragmentError::from)?;
                        output.push_str(&prefix);
                        output.push_str(&quote_string(&value));
                    } else {
                        let source = self.doc.extract_node(node).map_err(FragmentError::from)?;
                        let source = strip_inline_comment(&source).trim_end();
                        if source.contains(['\n', '\r'])
                            || style == YamlScalarStyle::Plain
                                && source.contains(['[', ']', '{', '}', ','])
                        {
                            let value = self.doc.scalar_value(node).map_err(FragmentError::from)?;
                            output.push_str(&prefix);
                            output.push_str(&quote_string(&value));
                        } else {
                            output.push_str(source);
                        }
                    }
                }
                Some(SemanticKind::Sequence { .. }) => {
                    output.push_str(&self.property_prefix(node));
                    output.push('[');
                    pending.push(RenderAction::Text("]"));
                    let items = self.doc.sequence_items(node).collect::<Vec<_>>();
                    for (index, item) in items.into_iter().enumerate().rev() {
                        pending.push(RenderAction::Node(item, depth + 1));
                        if index > 0 {
                            pending.push(RenderAction::Text(", "));
                        }
                    }
                }
                Some(SemanticKind::Mapping { .. }) => {
                    output.push_str(&self.property_prefix(node));
                    output.push('{');
                    pending.push(RenderAction::Text("}"));
                    let entries = self.doc.mapping_entries(node).collect::<Vec<_>>();
                    for (index, (key, value)) in entries.into_iter().enumerate().rev() {
                        pending.push(RenderAction::Node(value, depth + 1));
                        pending.push(RenderAction::Text(": "));
                        pending.push(RenderAction::Node(key, depth + 1));
                        if index > 0 {
                            pending.push(RenderAction::Text(", "));
                        }
                    }
                }
                Some(SemanticKind::Document) | None => {
                    return Err(FragmentError::new("cannot render unknown YAML node"));
                }
            }
        }
        Ok(output)
    }

    fn property_prefix(&self, node: NodeId) -> String {
        let mut prefix = String::new();
        if let Some(tag) = self.doc.raw_tag(node) {
            prefix.push_str(tag);
            prefix.push(' ');
        }
        if let Some(anchor) = self.doc.anchor(node) {
            prefix.push('&');
            prefix.push_str(anchor);
            prefix.push(' ');
        }
        prefix
    }

    fn subtree_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        let span = self.doc.node(self.root).map(super::syntax::Node::span);
        self.doc
            .nodes
            .iter()
            .enumerate()
            .map(|(index, _)| NodeId::from_usize(index))
            .filter(move |node| {
                let Some(root_span) = span else {
                    return false;
                };
                self.doc.node(*node).is_some_and(|node| {
                    node.span().start >= root_span.start && node.span().end <= root_span.end
                })
            })
    }

    fn validate_alias_scope(&self) -> Result<(), FragmentError> {
        let root_span = self
            .doc
            .node(self.root)
            .map(super::syntax::Node::span)
            .ok_or_else(|| FragmentError::new("fragment root node is missing"))?;
        for node in self.subtree_nodes() {
            if !matches!(self.doc.semantic_kind(node), Some(SemanticKind::Alias)) {
                continue;
            }
            let target = self.doc.resolve_alias(node).ok_or_else(|| {
                FragmentError::new(format!(
                    "unresolved value alias `*{}`",
                    self.doc.alias_name(node).unwrap_or_default()
                ))
            })?;
            let target_span = self
                .doc
                .node(target)
                .map(super::syntax::Node::span)
                .ok_or_else(|| FragmentError::new("alias target is missing"))?;
            if target_span.start < root_span.start || target_span.end > root_span.end {
                return Err(FragmentError::new(
                    "a value alias cannot reference a node outside the value root",
                ));
            }
        }
        Ok(())
    }
}

impl YamlDoc {
    /// Extracts one semantic node as valid standalone YAML where possible.
    ///
    /// # Errors
    ///
    /// Returns an error when `node` does not identify a node in this document.
    pub fn extract_node(&self, node: NodeId) -> Result<String, YamlError> {
        let node = self.expect_node(node)?;
        let source = self.source.slice(node.span);
        let line_start = self.source.as_str()[..node.span.start as usize]
            .rfind(['\n', '\r'])
            .map_or(0, |index| index + 1);
        let base_indent = self.source.as_str()[line_start..node.span.start as usize]
            .bytes()
            .take_while(|byte| *byte == b' ')
            .count();
        Ok(deindent_continuation_lines(source, base_indent))
    }

    pub(crate) fn anchor_names(&self) -> BTreeSet<String> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(index, _)| self.anchor(NodeId::from_usize(index)).map(str::to_owned))
            .collect()
    }
}

fn deindent_continuation_lines(source: &str, indent: usize) -> String {
    if indent == 0 || !source.contains(['\n', '\r']) {
        return source.to_owned();
    }
    let bytes = source.as_bytes();
    let mut output = String::with_capacity(source.len());
    let mut position = 0;
    let mut first = true;
    while position < bytes.len() {
        if !first {
            let mut removed = 0;
            while removed < indent && bytes.get(position) == Some(&b' ') {
                position += 1;
                removed += 1;
            }
        }
        first = false;
        let line_end = source[position..]
            .find(['\n', '\r'])
            .map_or(source.len(), |offset| position + offset);
        output.push_str(&source[position..line_end]);
        position = line_end;
        if bytes.get(position) == Some(&b'\r') {
            output.push('\r');
            position += 1;
            if bytes.get(position) == Some(&b'\n') {
                output.push('\n');
                position += 1;
            }
        } else if bytes.get(position) == Some(&b'\n') {
            output.push('\n');
            position += 1;
        }
    }
    output
}

pub(crate) fn indent_text(source: &str, indent: usize) -> String {
    if indent == 0 || source.is_empty() {
        return source.to_owned();
    }
    let prefix = " ".repeat(indent);
    let mut output = String::with_capacity(source.len() + prefix.len());
    let mut at_line_start = true;
    for character in source.chars() {
        if at_line_start && !matches!(character, '\r' | '\n') {
            output.push_str(&prefix);
            at_line_start = false;
        }
        output.push(character);
        if character == '\n' || character == '\r' {
            at_line_start = true;
        }
    }
    output
}

pub(crate) fn quote_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04X}", u32::from(character))
                    .expect("writing to a String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

/// Failure to parse, validate, or render a YAML fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentError {
    message: String,
}

impl FragmentError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for FragmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FragmentError {}

impl From<YamlError> for FragmentError {
    fn from(error: YamlError) -> Self {
        Self::new(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested_block_sequence(depth: usize) -> String {
        let mut yaml = String::new();
        for level in 0..depth {
            yaml.push_str(&"  ".repeat(level));
            yaml.push_str("-\n");
        }
        yaml.push_str(&"  ".repeat(depth));
        yaml.push_str("value\n");
        yaml
    }

    #[test]
    fn fragment_requires_one_nonempty_document() {
        assert!(YamlFragment::parse("").is_err());
        assert!(YamlFragment::parse("--- a\n--- b\n").is_err());
        assert!(YamlFragment::parse("[a, b]").is_ok());
    }

    #[test]
    fn extraction_deindents_nested_block_nodes() {
        let doc = YamlDoc::parse("outer:\n  one: 1\n  two:\n    - a\n    - b\n").unwrap();
        let node = doc
            .get_mapping_value(doc.document_root_mapping(0).unwrap(), "outer")
            .unwrap()
            .unwrap();
        assert_eq!(
            doc.extract_node(node).unwrap(),
            "one: 1\ntwo:\n  - a\n  - b"
        );
    }

    #[test]
    fn rejects_unresolved_value_aliases() {
        assert!(YamlFragment::parse("*outside").is_err());
    }

    #[test]
    fn flow_rendering_normalizes_block_collections_only() {
        let fragment = YamlFragment::parse("one:\n  - a\n  - b\n").unwrap();
        let target = YamlDoc::parse("target: []\n").unwrap();
        assert_eq!(fragment.render_flow(&target).unwrap(), "{one: [a, b]}");
    }

    #[test]
    fn flow_rendering_preserves_the_existing_depth_limit() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let target = YamlDoc::parse("target: []\n").unwrap();
                let accepted = YamlFragment::parse(&nested_block_sequence(1024)).unwrap();
                assert!(accepted.render_flow(&target).is_ok());

                let rejected = YamlFragment::parse(&nested_block_sequence(1025)).unwrap();
                assert!(
                    rejected
                        .render_flow(&target)
                        .unwrap_err()
                        .to_string()
                        .contains("recursion limit")
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
