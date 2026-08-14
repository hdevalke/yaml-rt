use std::collections::HashSet;
use std::fmt;

use yaml_rt_core::{
    NodeId, ResolvedScalar, SemanticKind, YamlDoc, YamlScalarStyle, resolve_scalar,
};
use yaml_rt_rfc9535::{JsonPath, QueryMatches};

const MAX_VALUE_DEPTH: usize = 1024;
const MAP_TAG: &str = "tag:yaml.org,2002:map";
const SEQ_TAG: &str = "tag:yaml.org,2002:seq";

pub(crate) fn run_query(
    doc: &YamlDoc,
    document: usize,
    source: &str,
) -> Result<String, QueryCommandError> {
    let matches = query_matches(doc, document, source)?;
    let budget = doc.as_source().len().saturating_mul(100).max(10_000);
    let mut output = String::new();
    for matched in matches {
        write_json_string(matched.pointer().as_str(), &mut output)?;
        output.push_str(": ");
        let mut writer = JsonWriter::new(doc, budget);
        writer.write_node(matched.node(), &mut output, 0)?;
        output.push('\n');
    }
    Ok(output)
}

pub(crate) fn query_matches(
    doc: &YamlDoc,
    document: usize,
    source: &str,
) -> Result<QueryMatches, QueryCommandError> {
    JsonPath::parse(source)
        .map_err(QueryCommandError::display)?
        .query(doc, document)
        .map_err(QueryCommandError::display)
}

#[derive(Debug)]
pub(crate) struct QueryCommandError(String);

impl QueryCommandError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn display(error: impl fmt::Display) -> Self {
        Self::new(error.to_string())
    }
}

impl fmt::Display for QueryCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for QueryCommandError {}

struct JsonWriter<'a> {
    doc: &'a YamlDoc,
    remaining: usize,
    active: HashSet<NodeId>,
}

impl<'a> JsonWriter<'a> {
    fn new(doc: &'a YamlDoc, budget: usize) -> Self {
        Self {
            doc,
            remaining: budget,
            active: HashSet::new(),
        }
    }

    fn write_node(
        &mut self,
        node: Option<NodeId>,
        output: &mut impl fmt::Write,
        depth: usize,
    ) -> Result<(), QueryCommandError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(QueryCommandError::new(
                "JSON-compatible value nesting limit exceeded",
            ));
        }
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or_else(|| QueryCommandError::new("YAML alias expansion limit exceeded"))?;
        let Some(mut node) = node else {
            output
                .write_str("null")
                .map_err(QueryCommandError::display)?;
            return Ok(());
        };
        let mut aliases = HashSet::new();
        while matches!(self.doc.semantic_kind(node), Some(SemanticKind::Alias)) {
            if !aliases.insert(node) {
                return Err(QueryCommandError::new("cyclic YAML alias chain"));
            }
            node = self.doc.resolve_alias(node).ok_or_else(|| {
                QueryCommandError::new(format!(
                    "unresolved YAML alias `*{}`",
                    self.doc.alias_name(node).unwrap_or_default()
                ))
            })?;
        }
        match self.doc.semantic_kind(node) {
            Some(SemanticKind::Scalar { style }) => self.write_scalar(node, style, output),
            Some(SemanticKind::Sequence { .. }) => {
                self.validate_collection_tag(node, SEQ_TAG)?;
                if !self.active.insert(node) {
                    return Err(QueryCommandError::new(
                        "recursive YAML alias graph is not JSON-compatible",
                    ));
                }
                output.write_char('[').map_err(QueryCommandError::display)?;
                for (index, item) in self.doc.sequence_items(node).enumerate() {
                    if index != 0 {
                        output.write_char(',').map_err(QueryCommandError::display)?;
                    }
                    self.write_node(Some(item), output, depth + 1)?;
                }
                output.write_char(']').map_err(QueryCommandError::display)?;
                self.active.remove(&node);
                Ok(())
            }
            Some(SemanticKind::Mapping { .. }) => {
                self.validate_collection_tag(node, MAP_TAG)?;
                if !self.active.insert(node) {
                    return Err(QueryCommandError::new(
                        "recursive YAML alias graph is not JSON-compatible",
                    ));
                }
                let mut keys = HashSet::new();
                output.write_char('{').map_err(QueryCommandError::display)?;
                for (index, (key, value)) in self.doc.mapping_entries(node).enumerate() {
                    let key = self.string_key(key)?;
                    if !keys.insert(key.clone()) {
                        return Err(QueryCommandError::new(format!(
                            "mapping contains duplicate key `{key}`"
                        )));
                    }
                    if index != 0 {
                        output.write_char(',').map_err(QueryCommandError::display)?;
                    }
                    write_json_string(&key, output)?;
                    output.write_char(':').map_err(QueryCommandError::display)?;
                    self.write_node(Some(value), output, depth + 1)?;
                }
                output.write_char('}').map_err(QueryCommandError::display)?;
                self.active.remove(&node);
                Ok(())
            }
            _ => Err(QueryCommandError::new("unknown semantic YAML node")),
        }
    }

    fn write_scalar(
        &self,
        node: NodeId,
        style: YamlScalarStyle,
        output: &mut impl fmt::Write,
    ) -> Result<(), QueryCommandError> {
        let value = self
            .doc
            .scalar_value(node)
            .map_err(QueryCommandError::display)?;
        let tag = self
            .doc
            .resolved_tag(node)
            .map_err(QueryCommandError::display)?;
        match resolve_scalar(&value, style, tag.as_deref()).map_err(QueryCommandError::display)? {
            ResolvedScalar::Null => output.write_str("null").map_err(QueryCommandError::display),
            ResolvedScalar::Bool(value) => output
                .write_str(if value { "true" } else { "false" })
                .map_err(QueryCommandError::display),
            ResolvedScalar::Number(value) => {
                write!(output, "{value}").map_err(QueryCommandError::display)
            }
            ResolvedScalar::String => write_json_string(&value, output),
            ResolvedScalar::NonFinite(_) => Err(QueryCommandError::new(
                "non-finite YAML numbers are not JSON-compatible",
            )),
        }
    }

    fn string_key(&self, node: NodeId) -> Result<String, QueryCommandError> {
        let mut node = node;
        let mut seen = HashSet::new();
        while matches!(self.doc.semantic_kind(node), Some(SemanticKind::Alias)) {
            if !seen.insert(node) {
                return Err(QueryCommandError::new("cyclic YAML alias key"));
            }
            node = self
                .doc
                .resolve_alias(node)
                .ok_or_else(|| QueryCommandError::new("unresolved YAML alias key"))?;
        }
        let Some(SemanticKind::Scalar { style }) = self.doc.semantic_kind(node) else {
            return Err(QueryCommandError::new("mapping contains a non-string key"));
        };
        let value = self
            .doc
            .scalar_value(node)
            .map_err(QueryCommandError::display)?;
        let tag = self
            .doc
            .resolved_tag(node)
            .map_err(QueryCommandError::display)?;
        if resolve_scalar(&value, style, tag.as_deref()).map_err(QueryCommandError::display)?
            != ResolvedScalar::String
        {
            return Err(QueryCommandError::new("mapping contains a non-string key"));
        }
        Ok(value.into_owned())
    }

    fn validate_collection_tag(
        &self,
        node: NodeId,
        expected: &str,
    ) -> Result<(), QueryCommandError> {
        let tag = self
            .doc
            .resolved_tag(node)
            .map_err(QueryCommandError::display)?;
        if tag.as_deref().is_some_and(|tag| tag != expected) {
            return Err(QueryCommandError::new(format!(
                "custom-tagged collection `{}` is not JSON-compatible",
                tag.as_deref().unwrap_or_default()
            )));
        }
        Ok(())
    }
}

fn write_json_string(value: &str, output: &mut impl fmt::Write) -> Result<(), QueryCommandError> {
    output.write_char('"').map_err(QueryCommandError::display)?;
    for character in value.chars() {
        match character {
            '"' => output
                .write_str("\\\"")
                .map_err(QueryCommandError::display)?,
            '\\' => output
                .write_str("\\\\")
                .map_err(QueryCommandError::display)?,
            '\u{0008}' => output
                .write_str("\\b")
                .map_err(QueryCommandError::display)?,
            '\u{000c}' => output
                .write_str("\\f")
                .map_err(QueryCommandError::display)?,
            '\n' => output
                .write_str("\\n")
                .map_err(QueryCommandError::display)?,
            '\r' => output
                .write_str("\\r")
                .map_err(QueryCommandError::display)?,
            '\t' => output
                .write_str("\\t")
                .map_err(QueryCommandError::display)?,
            character if character <= '\u{001f}' => {
                write!(output, "\\u{:04x}", u32::from(character))
                    .map_err(QueryCommandError::display)?;
            }
            character => output
                .write_char(character)
                .map_err(QueryCommandError::display)?,
        }
    }
    output.write_char('"').map_err(QueryCommandError::display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_pointer_value_lines() {
        let doc = YamlDoc::parse("'a/b~c': {hex: 0x10, text: \"x\\ny\"}\n").unwrap();
        assert_eq!(
            run_query(&doc, 0, "$['a/b~c']").unwrap(),
            "\"/a~1b~0c\": {\"hex\":16,\"text\":\"x\\ny\"}\n"
        );
    }

    #[test]
    fn renders_empty_document_root_as_null() {
        let doc = YamlDoc::parse("---\n").unwrap();
        assert_eq!(run_query(&doc, 0, "$").unwrap(), "\"\": null\n");
    }
}
