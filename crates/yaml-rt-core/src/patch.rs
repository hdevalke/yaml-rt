use std::collections::HashSet;
use std::fmt;

use crate::{
    JsonPointer, NodeId, ResolvedScalar, SemanticKind, Span, YamlDoc, YamlFragment, resolve_scalar,
};

/// A parsed sequence of RFC 6902-style operations over YAML values.
#[derive(Debug, Clone)]
pub struct YamlPatch {
    operations: Vec<YamlPatchOperation>,
    operation_spans: Vec<Option<Span>>,
}

impl YamlPatch {
    /// Creates a patch from programmatically constructed operations.
    #[must_use]
    pub fn new(operations: Vec<YamlPatchOperation>) -> Self {
        let operation_spans = vec![None; operations.len()];
        Self {
            operations,
            operation_spans,
        }
    }

    /// Parses a YAML or JSON patch document.
    ///
    /// # Errors
    ///
    /// Returns an error when the source is invalid YAML or does not have the
    /// required sequence-of-operation-mappings shape.
    pub fn parse(input: &str) -> Result<Self, YamlPatchError> {
        Self::parse_owned(input.to_owned())
    }

    /// Parses an owned YAML or JSON patch document without first copying it.
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as [`Self::parse`].
    pub fn parse_owned(input: String) -> Result<Self, YamlPatchError> {
        let doc = YamlDoc::parse_owned(input).map_err(|error| {
            YamlPatchError::new(
                YamlPatchErrorKind::Syntax,
                None,
                Some(error.diagnostic.span),
                error.to_string(),
            )
        })?;
        if doc.document_count() != 1 {
            return Err(YamlPatchError::structure(
                None,
                None,
                format!(
                    "a YAML patch must contain exactly one document, found {}",
                    doc.document_count()
                ),
            ));
        }
        let root = doc
            .document_root(0)
            .map_err(|error| YamlPatchError::structure(None, None, error.to_string()))?
            .ok_or_else(|| {
                YamlPatchError::structure(None, None, "a YAML patch must have a sequence root")
            })?;
        require_undecorated_collection(&doc, root, SemanticCollection::Sequence, None)?;

        let mut operations = Vec::new();
        let mut operation_spans = Vec::new();
        for (index, node) in doc.sequence_items(root).enumerate() {
            let span = doc.node(node).map(|node| node.span());
            require_undecorated_collection(&doc, node, SemanticCollection::Mapping, Some(index))?;
            operations.push(parse_operation(&doc, node, index)?);
            operation_spans.push(span);
        }
        Ok(Self {
            operations,
            operation_spans,
        })
    }

    /// Returns the operations in application order.
    #[must_use]
    pub fn operations(&self) -> &[YamlPatchOperation] {
        &self.operations
    }

    /// Consumes this patch and returns its operations.
    #[must_use]
    pub fn into_operations(self) -> Vec<YamlPatchOperation> {
        self.operations
    }

    fn operation_span(&self, index: usize) -> Option<Span> {
        self.operation_spans.get(index).copied().flatten()
    }
}

impl PartialEq for YamlPatch {
    fn eq(&self, other: &Self) -> bool {
        self.operations == other.operations
    }
}

impl Eq for YamlPatch {}

/// One YAML patch operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YamlPatchOperation {
    /// Adds a value, replacing an existing mapping member when present.
    Add {
        /// Destination JSON Pointer.
        path: JsonPointer,
        /// Value to add.
        value: YamlFragment,
    },
    /// Removes an existing value.
    Remove {
        /// Target JSON Pointer.
        path: JsonPointer,
    },
    /// Replaces an existing value.
    Replace {
        /// Target JSON Pointer.
        path: JsonPointer,
        /// Replacement value.
        value: YamlFragment,
    },
    /// Moves an existing value.
    Move {
        /// Source JSON Pointer.
        from: JsonPointer,
        /// Destination JSON Pointer.
        path: JsonPointer,
    },
    /// Copies an existing value.
    Copy {
        /// Source JSON Pointer.
        from: JsonPointer,
        /// Destination JSON Pointer.
        path: JsonPointer,
    },
    /// Tests semantic equality without changing the document.
    Test {
        /// Target JSON Pointer.
        path: JsonPointer,
        /// Expected value.
        value: YamlFragment,
    },
}

/// Classification of a YAML patch failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamlPatchErrorKind {
    /// The patch source is not valid YAML.
    Syntax,
    /// The parsed document is not a valid patch document.
    Structure,
    /// A valid operation could not be applied to the target document.
    Application,
}

/// Failure while parsing or applying a YAML patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlPatchError {
    kind: YamlPatchErrorKind,
    operation_index: Option<usize>,
    span: Option<Span>,
    message: String,
}

impl YamlPatchError {
    fn new(
        kind: YamlPatchErrorKind,
        operation_index: Option<usize>,
        span: Option<Span>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            operation_index,
            span,
            message: message.into(),
        }
    }

    fn structure(
        operation_index: Option<usize>,
        span: Option<Span>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            YamlPatchErrorKind::Structure,
            operation_index,
            span,
            message,
        )
    }

    fn application(
        operation_index: Option<usize>,
        span: Option<Span>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            YamlPatchErrorKind::Application,
            operation_index,
            span,
            message,
        )
    }

    /// Returns the phase in which the patch failed.
    #[must_use]
    pub const fn kind(&self) -> YamlPatchErrorKind {
        self.kind
    }

    /// Returns the zero-based operation index, when one operation is involved.
    #[must_use]
    pub const fn operation_index(&self) -> Option<usize> {
        self.operation_index
    }

    /// Returns the relevant patch-source span, when the patch was parsed from text.
    #[must_use]
    pub const fn span(&self) -> Option<Span> {
        self.span
    }
}

impl fmt::Display for YamlPatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(index) = self.operation_index {
            write!(formatter, "patch operation[{index}]: {}", self.message)
        } else {
            write!(formatter, "YAML patch: {}", self.message)
        }
    }
}

impl std::error::Error for YamlPatchError {}

impl YamlDoc {
    /// Applies every operation in a patch transactionally to one YAML document.
    ///
    /// Operations run in sequence and later pointers observe earlier changes.
    /// The receiver is replaced only after every operation succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected document does not exist, an operation
    /// cannot be applied, or a `test` operation compares unequal.
    pub fn apply_patch(
        &mut self,
        document: usize,
        patch: &YamlPatch,
    ) -> Result<(), YamlPatchError> {
        let mut work = self.clone();
        work.document_root(document)
            .map_err(|error| YamlPatchError::application(None, None, error.to_string()))?;
        for (index, operation) in patch.operations.iter().enumerate() {
            let mutates = !matches!(operation, YamlPatchOperation::Test { .. });
            let result = match operation {
                YamlPatchOperation::Add { path, value } => work.add_at(document, path, value),
                YamlPatchOperation::Remove { path } => work.remove_at(document, path),
                YamlPatchOperation::Replace { path, value } => {
                    work.replace_at(document, path, value)
                }
                YamlPatchOperation::Move { from, path } => work.move_at(document, from, path),
                YamlPatchOperation::Copy { from, path } => work.copy_at(document, from, path),
                YamlPatchOperation::Test { path, value } => {
                    match work.test_at(document, path, value) {
                        Ok(true) => Ok(()),
                        Ok(false) => Err(crate::YamlEditError::new(format!(
                            "test failed at {:?}: values are not semantically equal",
                            path.as_str()
                        ))),
                        Err(error) => Err(error),
                    }
                }
            };
            if let Err(error) = result {
                return Err(YamlPatchError::application(
                    Some(index),
                    patch.operation_span(index),
                    error.to_string(),
                ));
            }
            if mutates && let Err(error) = work.commit_edits() {
                return Err(YamlPatchError::application(
                    Some(index),
                    patch.operation_span(index),
                    error.to_string(),
                ));
            }
        }
        *self = work;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SemanticCollection {
    Sequence,
    Mapping,
}

fn require_undecorated_collection(
    doc: &YamlDoc,
    node: NodeId,
    expected: SemanticCollection,
    operation_index: Option<usize>,
) -> Result<(), YamlPatchError> {
    let span = doc.node(node).map(|node| node.span());
    let valid_kind = matches!(
        (expected, doc.semantic_kind(node)),
        (
            SemanticCollection::Sequence,
            Some(SemanticKind::Sequence { .. })
        ) | (
            SemanticCollection::Mapping,
            Some(SemanticKind::Mapping { .. })
        )
    );
    if !valid_kind {
        let expected_name = match expected {
            SemanticCollection::Sequence => "a sequence root",
            SemanticCollection::Mapping => "a mapping",
        };
        let message = if operation_index.is_some() {
            format!("a YAML patch operation must be {expected_name}")
        } else {
            format!("a YAML patch must have {expected_name}")
        };
        return Err(YamlPatchError::structure(operation_index, span, message));
    }
    if doc.raw_tag(node).is_some() || doc.anchor(node).is_some() {
        return Err(YamlPatchError::structure(
            operation_index,
            span,
            "patch structural collections cannot have tags or anchors",
        ));
    }
    Ok(())
}

fn parse_operation(
    doc: &YamlDoc,
    mapping: NodeId,
    index: usize,
) -> Result<YamlPatchOperation, YamlPatchError> {
    let mut names = HashSet::new();
    let mut operation = None;
    let mut path = None;
    let mut from = None;
    let mut value = None;

    for (key, field_value) in doc.mapping_entries(mapping) {
        let name = string_scalar(doc, key, index, "operation member name")?;
        if !names.insert(name.clone()) {
            return Err(YamlPatchError::structure(
                Some(index),
                doc.node(key).map(|node| node.span()),
                format!("duplicate operation member {name:?}"),
            ));
        }
        match name.as_str() {
            "op" => operation = Some(string_scalar(doc, field_value, index, "`op`")?),
            "path" => path = Some(parse_pointer_field(doc, field_value, index, "path")?),
            "from" => from = Some(parse_pointer_field(doc, field_value, index, "from")?),
            "value" => value = Some(parse_value_fragment(doc, field_value, index)?),
            _ => {}
        }
    }

    let span = doc.node(mapping).map(|node| node.span());
    let operation = operation.ok_or_else(|| {
        YamlPatchError::structure(Some(index), span, "patch operation is missing `op`")
    })?;
    let path = path.ok_or_else(|| {
        YamlPatchError::structure(Some(index), span, "patch operation is missing `path`")
    })?;
    match operation.as_str() {
        "add" => Ok(YamlPatchOperation::Add {
            path,
            value: required_value(value, index, span, "add")?,
        }),
        "remove" => Ok(YamlPatchOperation::Remove { path }),
        "replace" => Ok(YamlPatchOperation::Replace {
            path,
            value: required_value(value, index, span, "replace")?,
        }),
        "move" => Ok(YamlPatchOperation::Move {
            from: required_from(from, index, span, "move")?,
            path,
        }),
        "copy" => Ok(YamlPatchOperation::Copy {
            from: required_from(from, index, span, "copy")?,
            path,
        }),
        "test" => Ok(YamlPatchOperation::Test {
            path,
            value: required_value(value, index, span, "test")?,
        }),
        _ => Err(YamlPatchError::structure(
            Some(index),
            span,
            format!("unknown patch operation {operation:?}"),
        )),
    }
}

fn string_scalar(
    doc: &YamlDoc,
    node: NodeId,
    index: usize,
    field: &str,
) -> Result<String, YamlPatchError> {
    let span = doc.node(node).map(|node| node.span());
    let Some(SemanticKind::Scalar { style }) = doc.semantic_kind(node) else {
        return Err(YamlPatchError::structure(
            Some(index),
            span,
            format!("{field} must be a string"),
        ));
    };
    let text = doc
        .scalar_value(node)
        .map_err(|error| YamlPatchError::structure(Some(index), span, error.to_string()))?;
    let tag = doc
        .resolved_tag(node)
        .map_err(|error| YamlPatchError::structure(Some(index), span, error.to_string()))?;
    let resolved = resolve_scalar(&text, style, tag.as_deref())
        .map_err(|error| YamlPatchError::structure(Some(index), span, error.to_string()))?;
    if resolved != ResolvedScalar::String {
        return Err(YamlPatchError::structure(
            Some(index),
            span,
            format!("{field} must be a string"),
        ));
    }
    Ok(text.into_owned())
}

fn parse_pointer_field(
    doc: &YamlDoc,
    node: NodeId,
    index: usize,
    field: &str,
) -> Result<JsonPointer, YamlPatchError> {
    let span = doc.node(node).map(|node| node.span());
    let text = string_scalar(doc, node, index, &format!("`{field}`"))?;
    JsonPointer::parse(&text)
        .map_err(|error| YamlPatchError::structure(Some(index), span, error.to_string()))
}

fn parse_value_fragment(
    doc: &YamlDoc,
    node: NodeId,
    index: usize,
) -> Result<YamlFragment, YamlPatchError> {
    let span = doc.node(node).map(|node| node.span());
    let mut source = doc
        .extract_node(node)
        .map_err(|error| YamlPatchError::structure(Some(index), span, error.to_string()))?;
    if source.trim().is_empty() {
        source = "null".to_owned();
    }
    YamlFragment::parse_owned(source)
        .map_err(|error| YamlPatchError::structure(Some(index), span, error.to_string()))
}

fn required_value(
    value: Option<YamlFragment>,
    index: usize,
    span: Option<Span>,
    operation: &str,
) -> Result<YamlFragment, YamlPatchError> {
    value.ok_or_else(|| {
        YamlPatchError::structure(
            Some(index),
            span,
            format!("{operation} operation is missing `value`"),
        )
    })
}

fn required_from(
    from: Option<JsonPointer>,
    index: usize,
    span: Option<Span>,
    operation: &str,
) -> Result<JsonPointer, YamlPatchError> {
    from.ok_or_else(|| {
        YamlPatchError::structure(
            Some(index),
            span,
            format!("{operation} operation is missing `from`"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_and_json_patch_documents() {
        let yaml = YamlPatch::parse(
            "- op: add\n  path: /enabled\n  value: true\n- op: remove\n  path: /old\n",
        )
        .unwrap();
        let json = YamlPatch::parse(
            r#"[{"op":"add","path":"/enabled","value":true},{"op":"remove","path":"/old"}]"#,
        )
        .unwrap();
        assert_eq!(yaml, json);

        let with_unknown_member =
            YamlPatch::parse("- {op: remove, path: /old, extension: ignored}\n").unwrap();
        assert_eq!(with_unknown_member.operations().len(), 1);
    }

    #[test]
    fn applies_operations_sequentially_and_preserves_presentation() {
        let patch = YamlPatch::parse(
            "- {op: test, path: /items/0, value: a}\n\
             - {op: add, path: /items/1, value: b}\n\
             - {op: replace, path: /host, value: example.com}\n\
             - {op: copy, from: /items/0, path: /items/-}\n\
             - {op: move, from: /items/1, path: /items/0}\n\
             - {op: remove, path: /old}\n",
        )
        .unwrap();
        let mut doc = YamlDoc::parse("host: localhost # keep\nitems: [a]\nold: true\n").unwrap();
        doc.apply_patch(0, &patch).unwrap();
        assert_eq!(
            doc.as_source(),
            "host: example.com # keep\nitems: [b, a, a]\n"
        );
    }

    #[test]
    fn patch_add_indents_compact_sequence_mapping_members() {
        let mut doc = YamlDoc::parse("services:\n  - name: api\n    port: 8080\n").unwrap();
        let patch =
            YamlPatch::parse("- op: add\n  path: /services/0/protocol\n  value: https\n").unwrap();

        doc.apply_patch(0, &patch).unwrap();
        assert_eq!(
            doc.as_source(),
            "services:\n  - name: api\n    port: 8080\n    protocol: https\n"
        );
        doc.commit_edits().unwrap();
    }

    #[test]
    fn a_late_failure_rolls_back_every_operation() {
        let patch = YamlPatch::parse(
            "- {op: replace, path: /value, value: 2}\n\
             - {op: test, path: /value, value: 3}\n",
        )
        .unwrap();
        let input = "value: 1 # unchanged on failure\n";
        let mut doc = YamlDoc::parse(input).unwrap();
        let error = doc.apply_patch(0, &patch).unwrap_err();
        assert_eq!(error.kind(), YamlPatchErrorKind::Application);
        assert_eq!(error.operation_index(), Some(1));
        assert!(error.span().is_some());
        assert_eq!(doc.as_source(), input);
    }

    #[test]
    fn later_removals_observe_earlier_structural_changes() {
        let input = "groups:\n  first: [a, b, c]\n  second: [d, e]\ntail: keep\n";
        for patch in [
            "- {op: remove, path: /groups/first/0}\n- {op: remove, path: /groups/first/1}\n- {op: remove, path: /groups/second}\n",
            "- {op: remove, path: /groups/second}\n- {op: remove, path: /groups/first/0}\n- {op: remove, path: /groups/first/1}\n",
        ] {
            let mut doc = YamlDoc::parse(input).unwrap();
            doc.apply_patch(0, &YamlPatch::parse(patch).unwrap())
                .unwrap();
            assert_eq!(doc.as_source(), "groups:\n  first: [b]\ntail: keep\n");
        }
    }

    // Regression examples adapted from:
    // https://verrchu.github.io/blog/2-respectful-yaml-patching-in-rust/
    #[test]
    fn respectfully_patches_the_article_asset_groups() {
        let input = "# outer comment\nasset_groups:\n  group_abc:    # group_abc comment\n    - BTC\n    - ETH\n    - SOL\n  # group_xyz outer comment\n  group_xyz:\n    -  DOGE       # asset comment\n    - PEPE\n  default:\n    # default group inner comment\n    - 1INCH\n    - ATOM\n    - LINK\n";
        let listing = YamlPatch::parse(
            "- {op: add, path: /asset_groups/default/2, value: BNB}\n- {op: add, path: /asset_groups/default/-, value: XRP}\n",
        )
        .unwrap();
        let mut listed = YamlDoc::parse(input).unwrap();
        listed.apply_patch(0, &listing).unwrap();
        assert_eq!(
            listed.as_source(),
            "# outer comment\nasset_groups:\n  group_abc:    # group_abc comment\n    - BTC\n    - ETH\n    - SOL\n  # group_xyz outer comment\n  group_xyz:\n    -  DOGE       # asset comment\n    - PEPE\n  default:\n    # default group inner comment\n    - 1INCH\n    - ATOM\n    - BNB\n    - LINK\n    - XRP\n"
        );

        let delisting = YamlPatch::parse(
            "- {op: remove, path: /asset_groups/group_abc/2}\n- {op: remove, path: /asset_groups/group_abc/0}\n- {op: remove, path: /asset_groups/group_xyz}\n- {op: remove, path: /asset_groups/default/1}\n",
        )
        .unwrap();
        let mut delisted = YamlDoc::parse(input).unwrap();
        delisted.apply_patch(0, &delisting).unwrap();
        assert_eq!(
            delisted.as_source(),
            "# outer comment\nasset_groups:\n  group_abc:    # group_abc comment\n    - ETH\n  default:\n    # default group inner comment\n    - 1INCH\n    - LINK\n"
        );
    }

    #[test]
    fn article_listing_preserves_an_unterminated_final_line() {
        let input = "asset_groups:\n  default:\n    - 1INCH\n    - ATOM\n    - LINK";
        let patch = YamlPatch::parse(
            "- {op: add, path: /asset_groups/default/2, value: BNB}\n- {op: add, path: /asset_groups/default/-, value: XRP}\n",
        )
        .unwrap();
        let mut doc = YamlDoc::parse(input).unwrap();
        doc.apply_patch(0, &patch).unwrap();
        assert_eq!(
            doc.as_source(),
            "asset_groups:\n  default:\n    - 1INCH\n    - ATOM\n    - BNB\n    - LINK\n    - XRP"
        );
    }

    #[test]
    fn supports_full_yaml_values_and_empty_nulls() {
        let patch = YamlPatch::parse(
            "- op: add\n  path: /tagged\n  value: !local {left: &item .inf, right: *item}\n\
             - op: add\n  path: /empty\n  value:\n",
        )
        .unwrap();
        let mut doc = YamlDoc::parse("{}\n").unwrap();
        doc.apply_patch(0, &patch).unwrap();
        assert_eq!(
            doc.as_source(),
            "{tagged: !local {left: &item .inf, right: *item}, empty: null}\n"
        );
    }

    #[test]
    fn batch_application_keeps_anchor_safety_and_collision_handling() {
        let patch =
            YamlPatch::parse("- op: add\n  path: /new\n  value: &item {value: 2, alias: *item}\n")
                .unwrap();
        let mut doc = YamlDoc::parse("existing: &item {value: 1}\n").unwrap();
        doc.apply_patch(0, &patch).unwrap();
        assert_eq!(
            doc.as_source(),
            "existing: &item {value: 1}\nnew: &item_1 {value: 2, alias: *item_1}\n"
        );

        let unsafe_copy =
            YamlPatch::parse("- {op: copy, from: /existing, path: /copied}\n").unwrap();
        let input = doc.as_source().to_owned();
        let error = doc.apply_patch(0, &unsafe_copy).unwrap_err();
        assert_eq!(error.operation_index(), Some(0));
        assert!(error.to_string().contains("anchor"));
        assert_eq!(doc.as_source(), input);
    }

    #[test]
    fn validates_patch_structure_and_value_alias_scope() {
        for input in [
            "op: add\npath: /x\nvalue: 1\n",
            "---\n[]\n---\n[]\n",
            "!patch []\n",
            "- scalar\n",
            "- op: unknown\n  path: /x\n",
            "- op: add\n  value: 1\n",
            "- op: add\n  path: /x\n",
            "- op: move\n  path: /x\n",
            "- op: add\n  op: remove\n  path: /x\n  value: 1\n",
            "- 1: member\n  op: remove\n  path: /x\n",
            "- op: add\n  path: 1\n  value: 1\n",
            "- op: add\n  path: x\n  value: 1\n",
            "- &operation {op: remove, path: /x}\n",
            "anchor: &outside value\n---\n- {op: add, path: /x, value: *outside}\n",
        ] {
            assert!(YamlPatch::parse(input).is_err(), "{input}");
        }

        let external_alias =
            "- op: add\n  path: /x\n  outside: &outside value\n  value: *outside\n";
        assert!(YamlPatch::parse(external_alias).is_err());

        let syntax = YamlPatch::parse("[").unwrap_err();
        assert_eq!(syntax.kind(), YamlPatchErrorKind::Syntax);
    }

    #[test]
    fn programmatic_and_empty_patches_work_for_selected_documents() {
        let operation = YamlPatchOperation::Replace {
            path: JsonPointer::parse("/name").unwrap(),
            value: YamlFragment::parse("updated").unwrap(),
        };
        let patch = YamlPatch::new(vec![operation.clone()]);
        assert_eq!(patch.operations(), &[operation]);
        assert_eq!(patch.clone().into_operations().len(), 1);

        let mut doc = YamlDoc::parse("---\nname: first\n---\nname: second\n").unwrap();
        doc.apply_patch(1, &patch).unwrap();
        assert_eq!(doc.as_source(), "---\nname: first\n---\nname: updated\n");

        let input = doc.as_source().to_owned();
        doc.apply_patch(0, &YamlPatch::new(Vec::new())).unwrap();
        assert_eq!(doc.as_source(), input);
    }
}
