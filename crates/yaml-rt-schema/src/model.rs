use std::collections::{HashMap, HashSet};

use crate::value::{Map, Number, Value};
use yaml_rt_core::{NodeId, ResolvedScalar, SemanticKind, Span, YamlDoc, resolve_scalar};

use crate::Error;

pub(crate) struct Instance {
    pub value: Value,
    pub spans: HashMap<String, Span>,
}

pub(crate) fn from_document(doc: &YamlDoc, document: usize) -> Result<Instance, Error> {
    let root = doc.document_root(document).map_err(Error::source)?;
    from_node(doc, root)
}

pub(crate) fn from_node(doc: &YamlDoc, root: Option<NodeId>) -> Result<Instance, Error> {
    let mut converter = Converter {
        doc,
        spans: HashMap::new(),
        active: HashSet::new(),
        budget: doc.source().limits().max_expanded_nodes,
        depth_limit: doc.source().limits().max_collection_depth,
        alias_chain_limit: doc.source().limits().max_alias_chain,
    };
    let value = converter.convert(root, String::new(), 0)?;
    Ok(Instance {
        value,
        spans: converter.spans,
    })
}

struct Converter<'a> {
    doc: &'a YamlDoc,
    spans: HashMap<String, Span>,
    active: HashSet<NodeId>,
    budget: usize,
    depth_limit: usize,
    alias_chain_limit: usize,
}

impl Converter<'_> {
    fn convert(
        &mut self,
        node: Option<NodeId>,
        path: String,
        depth: usize,
    ) -> Result<Value, Error> {
        if depth > self.depth_limit {
            return Err(Error::instance(path, "YAML expansion limit exceeded", None));
        }
        self.spend(&path, node)?;
        let Some(mut node) = node else {
            return Ok(Value::Null);
        };
        let mut aliases = HashSet::new();
        while matches!(self.doc.semantic_kind(node), Some(SemanticKind::Alias)) {
            if !aliases.insert(node) {
                return Err(self.fail(&path, node, "cyclic YAML alias chain"));
            }
            if aliases.len() > self.alias_chain_limit {
                return Err(self.fail(&path, node, "YAML alias chain limit exceeded"));
            }
            self.spend(&path, Some(node))?;
            node = self
                .doc
                .resolve_alias(node)
                .ok_or_else(|| self.fail(&path, node, "unresolved YAML alias"))?;
        }
        let span = self.doc.node(node).map(|n| n.span());
        if let Some(span) = span {
            self.spans.insert(path.clone(), span);
        }
        match self.doc.semantic_kind(node) {
            Some(SemanticKind::Scalar { style }) => {
                let text = self.doc.scalar_value(node).map_err(Error::source)?;
                let tag = self.doc.resolved_tag(node).map_err(Error::source)?;
                match resolve_scalar(&text, style, tag.as_deref()).map_err(Error::source)? {
                    ResolvedScalar::Null => Ok(Value::Null),
                    ResolvedScalar::Bool(value) => Ok(Value::Bool(value)),
                    ResolvedScalar::String => Ok(Value::String(text.into_owned())),
                    ResolvedScalar::Number(value) => {
                        let number = Number::from_yaml(value.to_string());
                        Ok(Value::Number(number))
                    }
                    ResolvedScalar::NonFinite(_) => {
                        Err(self.fail(&path, node, "non-finite YAML number is not JSON-compatible"))
                    }
                }
            }
            Some(SemanticKind::Sequence { .. }) => {
                self.collection_tag(node, "tag:yaml.org,2002:seq", &path)?;
                if !self.active.insert(node) {
                    return Err(self.fail(&path, node, "recursive YAML alias graph"));
                }
                let result = self
                    .doc
                    .sequence_items(node)
                    .enumerate()
                    .map(|(index, child)| {
                        self.convert(Some(child), format!("{path}/{index}"), depth + 1)
                    })
                    .collect::<Result<Vec<_>, _>>();
                self.active.remove(&node);
                result.map(Value::Array)
            }
            Some(SemanticKind::Mapping { .. }) => {
                self.collection_tag(node, "tag:yaml.org,2002:map", &path)?;
                if !self.active.insert(node) {
                    return Err(self.fail(&path, node, "recursive YAML alias graph"));
                }
                let mut result = Map::new();
                for (key, value) in self.doc.mapping_entries(node) {
                    let name = self.string_key(key, &path)?;
                    if result.contains_key(&name) {
                        return Err(self.fail(&path, key, "duplicate mapping key"));
                    }
                    let child_path = format!("{path}/{}", escape(&name));
                    result.insert(name, self.convert(Some(value), child_path, depth + 1)?);
                }
                self.active.remove(&node);
                Ok(Value::Object(result))
            }
            _ => Err(self.fail(&path, node, "unknown YAML semantic node")),
        }
    }

    fn string_key(&mut self, mut node: NodeId, path: &str) -> Result<String, Error> {
        let mut aliases = HashSet::new();
        while matches!(self.doc.semantic_kind(node), Some(SemanticKind::Alias)) {
            if !aliases.insert(node) {
                return Err(self.fail(path, node, "cyclic YAML alias key"));
            }
            if aliases.len() > self.alias_chain_limit {
                return Err(self.fail(path, node, "YAML alias chain limit exceeded"));
            }
            self.spend(path, Some(node))?;
            node = self
                .doc
                .resolve_alias(node)
                .ok_or_else(|| self.fail(path, node, "unresolved YAML alias key"))?;
        }
        let Some(SemanticKind::Scalar { style }) = self.doc.semantic_kind(node) else {
            return Err(self.fail(path, node, "non-string mapping key"));
        };
        let text = self.doc.scalar_value(node).map_err(Error::source)?;
        let tag = self.doc.resolved_tag(node).map_err(Error::source)?;
        if resolve_scalar(&text, style, tag.as_deref()).map_err(Error::source)?
            != ResolvedScalar::String
        {
            return Err(self.fail(path, node, "non-string mapping key"));
        }
        Ok(text.into_owned())
    }

    fn collection_tag(&self, node: NodeId, expected: &str, path: &str) -> Result<(), Error> {
        let tag = self.doc.resolved_tag(node).map_err(Error::source)?;
        if tag.as_deref().is_some_and(|tag| tag != expected) {
            return Err(self.fail(
                path,
                node,
                "custom-tagged collection is not JSON-compatible",
            ));
        }
        Ok(())
    }

    fn spend(&mut self, path: &str, node: Option<NodeId>) -> Result<(), Error> {
        self.budget = self.budget.checked_sub(1).ok_or_else(|| {
            Error::instance(
                path.to_owned(),
                "YAML expansion limit exceeded",
                node.and_then(|node| self.doc.node(node).map(|node| node.span())),
            )
        })?;
        Ok(())
    }

    fn fail(&self, path: &str, node: NodeId, message: &str) -> Error {
        Error::instance(
            path.to_owned(),
            message,
            self.doc.node(node).map(|n| n.span()),
        )
    }
}

pub(crate) fn escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::from_document;
    use yaml_rt_core::{ResourceLimits, YamlDoc};

    #[test]
    fn conversion_uses_document_expansion_and_alias_limits() {
        let expansion_limits = ResourceLimits {
            max_expanded_nodes: 4,
            ..ResourceLimits::default()
        };
        let doc = YamlDoc::parse_with_limits(
            "source: &source [one, two]\nroot: [*source, *source]\n",
            expansion_limits,
        )
        .unwrap();
        let error = from_document(&doc, 0).err().expect("conversion is limited");
        assert!(error.to_string().contains("expansion limit"));

        let alias_limits = ResourceLimits {
            max_alias_chain: 0,
            ..ResourceLimits::default()
        };
        let doc = YamlDoc::parse_with_limits("source: &source one\nroot: *source\n", alias_limits)
            .unwrap();
        let error = from_document(&doc, 0).err().expect("alias is limited");
        assert!(error.to_string().contains("alias chain limit"));
    }
}
