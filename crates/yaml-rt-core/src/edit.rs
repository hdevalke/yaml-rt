use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::fragment::indent_text;
use crate::pointer::parse_sequence_index;
use crate::{
    CollectionStyle, Diagnostic, DiagnosticKind, FragmentError, JsonPointer, NodeId, PointerError,
    ResolvedScalar, SemanticKind, SemanticValueError, Span, YamlDoc, YamlError, YamlFragment,
    YamlScalarStyle, resolve_scalar, semantically_equal, strip_inline_comment,
};

/// Failure while applying a pointer-addressed YAML edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlEditError {
    message: String,
    source_span: Option<Span>,
}

impl YamlEditError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source_span: None,
        }
    }

    fn with_source_span(mut self, source_span: Span) -> Self {
        self.source_span = Some(source_span);
        self
    }

    /// Returns the relevant YAML source span for the edit failure.
    #[must_use]
    pub const fn source_span(&self) -> Option<Span> {
        self.source_span
    }

    pub(crate) fn into_yaml_error(self) -> YamlError {
        let span = self.source_span.unwrap_or_else(|| Span::empty(0));
        YamlError::new(
            Diagnostic::new(DiagnosticKind::Emitter, self.message, span)
                .with_expected("a source-preserving YAML edit"),
        )
    }
}

impl fmt::Display for YamlEditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for YamlEditError {}

impl From<PointerError> for YamlEditError {
    fn from(error: PointerError) -> Self {
        let source_span = error.source_span();
        let mut converted = Self::new(error.to_string());
        if let Some(span) = source_span {
            converted = converted.with_source_span(span);
        }
        converted
    }
}

impl From<FragmentError> for YamlEditError {
    fn from(error: FragmentError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<YamlError> for YamlEditError {
    fn from(error: YamlError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<SemanticValueError> for YamlEditError {
    fn from(error: SemanticValueError) -> Self {
        Self::new(error.to_string())
    }
}

enum AddLocation {
    Root(NodeId),
    Mapping {
        mapping: NodeId,
        existing: Option<NodeId>,
        key: String,
    },
    Sequence {
        sequence: NodeId,
        index: usize,
    },
}

#[derive(Clone, Copy)]
struct RenameTarget {
    mapping: NodeId,
    key: NodeId,
}

impl YamlDoc {
    /// Applies RFC 6902 `add` semantics at a JSON Pointer destination.
    ///
    /// # Errors
    ///
    /// Returns an error when the document or destination does not exist, the
    /// destination cannot accept the value, or the edit cannot be emitted.
    pub fn add_at(
        &mut self,
        document: usize,
        pointer: &JsonPointer,
        value: &YamlFragment,
    ) -> Result<(), YamlEditError> {
        self.transaction(|work| {
            let location = work.resolve_add_location(document, pointer)?;
            work.queue_add(location, value)
        })
    }

    /// Removes an existing value. Removing a document root is unsupported.
    ///
    /// # Errors
    ///
    /// Returns an error when the document or target does not exist, the target
    /// is a document root, or removing it would invalidate the document.
    pub fn remove_at(
        &mut self,
        document: usize,
        pointer: &JsonPointer,
    ) -> Result<(), YamlEditError> {
        if pointer.is_root() {
            return Err(YamlEditError::new(
                "removing a YAML document root is not supported",
            ));
        }
        self.transaction(|work| {
            let target = work.resolve_pointer(document, pointer)?;
            work.queue_value_removal(target)
        })
    }

    /// Replaces an existing value while preserving its surrounding syntax.
    ///
    /// # Errors
    ///
    /// Returns an error when the document or target does not exist or the
    /// replacement cannot be emitted at that location.
    pub fn replace_at(
        &mut self,
        document: usize,
        pointer: &JsonPointer,
        value: &YamlFragment,
    ) -> Result<(), YamlEditError> {
        self.transaction(|work| {
            let target = work.resolve_pointer(document, pointer)?;
            work.queue_fragment_replacement(target, value)
        })
    }

    /// Moves a value using RFC 6902 remove-then-add semantics.
    ///
    /// # Errors
    ///
    /// Returns an error when either pointer is invalid for the document, the
    /// move would be recursive, or anchors, aliases, or syntax prevent it.
    pub fn move_at(
        &mut self,
        document: usize,
        from: &JsonPointer,
        path: &JsonPointer,
    ) -> Result<(), YamlEditError> {
        if from == path {
            return Ok(());
        }
        if from.is_proper_prefix_of(path) {
            return Err(YamlEditError::new(
                "move source must not be a proper prefix of its destination",
            ));
        }
        let mut work = self.clone();
        let source = work.resolve_pointer(document, from)?;
        if work.anchor_has_external_alias(source) {
            return Err(YamlEditError::new(
                "cannot move anchored subtree because an alias outside it depends on that anchor",
            ));
        }
        let fragment = YamlFragment::from_document_node(&work, source)?;
        if from.is_root() {
            return Err(YamlEditError::new(
                "a document root cannot be moved into another location",
            ));
        }
        work.queue_value_removal(source)?;
        work.commit_edits()?;
        let location = work.resolve_add_location(document, path)?;
        work.queue_add(location, &fragment)?;
        work.commit_edits()?;
        *self = work;
        Ok(())
    }

    /// Deep-copies a value using RFC 6902 `copy` semantics.
    ///
    /// # Errors
    ///
    /// Returns an error when either pointer is invalid for the document, the
    /// source contains an anchor, or the copied value cannot be inserted.
    pub fn copy_at(
        &mut self,
        document: usize,
        from: &JsonPointer,
        path: &JsonPointer,
    ) -> Result<(), YamlEditError> {
        let source = self.resolve_pointer(document, from)?;
        let fragment = YamlFragment::from_document_node(self, source)?;
        if fragment.contains_anchor() {
            return Err(YamlEditError::new(format!(
                "cannot copy {:?}: subtree contains an anchor",
                from.as_str()
            )));
        }
        self.add_at(document, path, &fragment)
    }

    /// Compares a pointer-selected target with a YAML value.
    ///
    /// # Errors
    ///
    /// Returns an error when the document or target does not exist or either
    /// value cannot be compared using RFC 6902 equality.
    pub fn test_at(
        &self,
        document: usize,
        pointer: &JsonPointer,
        value: &YamlFragment,
    ) -> Result<bool, YamlEditError> {
        let target = self.resolve_pointer(document, pointer)?;
        semantically_equal(self, target, value.document(), value.root()).map_err(Into::into)
    }

    /// Renames the mapping key that owns a pointer-selected value.
    ///
    /// The operation is transactional and changes only the key scalar spelling.
    /// The pointer must select a mapping member; document roots and sequence
    /// elements do not have a key to rename.
    ///
    /// # Errors
    ///
    /// Returns an error when the pointer does not select a supported string key,
    /// the destination key would collide with another mapping member, or the
    /// edited document cannot be emitted.
    pub fn rename_key_at(
        &mut self,
        document: usize,
        pointer: &JsonPointer,
        new_key: &str,
    ) -> Result<(), YamlEditError> {
        self.rename_keys_at(document, std::slice::from_ref(pointer), new_key)
    }

    /// Renames the mapping keys that own several pointer-selected values.
    ///
    /// Targets are resolved against the original document and duplicate source
    /// key nodes are edited once. All collision checks and edits are applied as
    /// one transaction. An empty pointer slice succeeds without changing the
    /// document.
    ///
    /// # Errors
    ///
    /// Returns an error when any pointer does not select a supported string key,
    /// any affected mapping would contain duplicate final keys, or the edited
    /// document cannot be emitted.
    pub fn rename_keys_at(
        &mut self,
        document: usize,
        pointers: &[JsonPointer],
        new_key: &str,
    ) -> Result<(), YamlEditError> {
        if pointers.is_empty() {
            return Ok(());
        }

        let mut work = self.clone();
        let mut seen_keys = HashSet::new();
        let mut targets = Vec::new();
        for pointer in pointers {
            let target = work.resolve_rename_target(document, pointer)?;
            if seen_keys.insert(target.key) {
                targets.push(target);
            }
        }

        work.validate_rename_targets(&targets, new_key)?;
        for target in targets {
            work.queue_key_rename(target.key, new_key)?;
        }
        work.commit_edits()?;
        *self = work;
        Ok(())
    }

    fn transaction(
        &mut self,
        operation: impl FnOnce(&mut YamlDoc) -> Result<(), YamlEditError>,
    ) -> Result<(), YamlEditError> {
        let mut work = self.clone();
        operation(&mut work)?;
        work.commit_edits()?;
        *self = work;
        Ok(())
    }

    fn resolve_rename_target(
        &self,
        document: usize,
        pointer: &JsonPointer,
    ) -> Result<RenameTarget, YamlEditError> {
        let Some((parent_pointer, token)) = pointer.parent() else {
            return Err(YamlEditError::new(
                "a YAML document root does not have a mapping key to rename",
            ));
        };
        let token_index = pointer.tokens().len().saturating_sub(1);
        let mut parent = self.resolve_pointer(document, &parent_pointer)?;
        parent = self.resolve_aliases_for_pointer(parent, pointer, token_index)?;
        if !matches!(
            self.semantic_kind(parent),
            Some(SemanticKind::Mapping { .. })
        ) {
            return Err(YamlEditError::new(format!(
                "JSON Pointer {:?} does not select a mapping member",
                pointer.as_str()
            )));
        }
        let matched = self
            .mapping_match(parent, token, pointer, token_index)?
            .ok_or_else(|| {
                YamlEditError::new(format!(
                    "mapping has no member {:?} to rename",
                    token.as_str()
                ))
            })?;
        Ok(RenameTarget {
            mapping: parent,
            key: matched.key,
        })
    }

    fn validate_rename_targets(
        &self,
        targets: &[RenameTarget],
        new_key: &str,
    ) -> Result<(), YamlEditError> {
        crate::validate_yaml_chars(new_key)?;
        let mut targets_by_mapping = HashMap::<NodeId, HashSet<NodeId>>::new();
        for target in targets {
            self.validate_rename_key_node(target.key)?;
            targets_by_mapping
                .entry(target.mapping)
                .or_default()
                .insert(target.key);
        }

        for (mapping, renamed_keys) in targets_by_mapping {
            let mut final_keys = HashSet::new();
            for (key, _) in self.mapping_entries(mapping) {
                let decoded = if renamed_keys.contains(&key) {
                    new_key.to_owned()
                } else {
                    self.string_mapping_key(key)?
                };
                if !final_keys.insert(decoded.clone()) {
                    return Err(YamlEditError::new(format!(
                        "renaming a mapping key to {new_key:?} would create duplicate key {decoded:?}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_rename_key_node(&self, key: NodeId) -> Result<(), YamlEditError> {
        if self
            .node(key)
            .is_none_or(|node| node.kind() != crate::NodeKind::Scalar)
            || !matches!(self.semantic_kind(key), Some(SemanticKind::Scalar { .. }))
        {
            return Err(YamlEditError::new(
                "mapping-key rename supports only plain, single-quoted, and double-quoted string keys",
            ));
        }
        if !scalar_is_string(self, key)? {
            return Err(YamlEditError::new(
                "mapping-key rename target is not a string scalar key",
            ));
        }
        Ok(())
    }

    fn string_mapping_key(&self, key: NodeId) -> Result<String, YamlEditError> {
        let mut resolved = key;
        let mut seen = HashSet::new();
        while matches!(self.semantic_kind(resolved), Some(SemanticKind::Alias)) {
            if !seen.insert(resolved) {
                let mut error = YamlEditError::new("cyclic YAML alias key");
                if let Some(span) = self.node(resolved).map(|node| node.span()) {
                    error = error.with_source_span(span);
                }
                return Err(error);
            }
            let alias = resolved;
            resolved = self.resolve_alias(alias).ok_or_else(|| {
                let mut error = YamlEditError::new("unresolved YAML alias key");
                if let Some(span) = self.node(alias).map(|node| node.span()) {
                    error = error.with_source_span(span);
                }
                error
            })?;
        }
        if !scalar_is_string(self, resolved)? {
            return Err(YamlEditError::new(
                "affected mapping contains a non-string key",
            ));
        }
        self.scalar_value(resolved)
            .map(std::borrow::Cow::into_owned)
            .map_err(Into::into)
    }

    fn queue_key_rename(&mut self, key: NodeId, new_key: &str) -> Result<(), YamlEditError> {
        let current = self.scalar_value(key)?;
        if current == new_key {
            return Ok(());
        }
        let (span, style) = self.scalar_replacement_target(key)?;
        let tag = self.resolved_tag(key)?;
        let replacement = match style {
            crate::ScalarStyle::Plain if safe_plain_key_with_tag(new_key, tag.as_deref()) => {
                new_key.to_owned()
            }
            crate::ScalarStyle::Plain => crate::fragment::quote_string(new_key),
            style => crate::format_scalar_value(new_key, style)
                .unwrap_or_else(|_| crate::fragment::quote_string(new_key)),
        };
        self.queue_edit(span, replacement)?;
        Ok(())
    }

    fn resolve_add_location(
        &self,
        document: usize,
        pointer: &JsonPointer,
    ) -> Result<AddLocation, YamlEditError> {
        let Some((parent_pointer, token)) = pointer.parent() else {
            let root = self
                .document_root(document)?
                .ok_or_else(|| YamlEditError::new("selected document has no root node"))?;
            return Ok(AddLocation::Root(root));
        };
        let mut parent = self.resolve_pointer(document, &parent_pointer)?;
        parent = self.resolve_aliases_for_pointer(
            parent,
            pointer,
            pointer.tokens().len().saturating_sub(1),
        )?;
        match self.semantic_kind(parent) {
            Some(SemanticKind::Mapping { .. }) => {
                let existing = self
                    .mapping_match(
                        parent,
                        token,
                        pointer,
                        pointer.tokens().len().saturating_sub(1),
                    )?
                    .map(|entry| entry.value);
                Ok(AddLocation::Mapping {
                    mapping: parent,
                    existing,
                    key: token.as_str().to_owned(),
                })
            }
            Some(SemanticKind::Sequence { .. }) => {
                let length = self.sequence_items(parent).count();
                let parsed = parse_sequence_index(
                    token,
                    pointer,
                    pointer.tokens().len().saturating_sub(1),
                    true,
                )?;
                let index = if parsed == usize::MAX { length } else { parsed };
                if index > length {
                    return Err(YamlEditError::new(format!(
                        "sequence index {index} is out of bounds for insertion into length {length}"
                    )));
                }
                Ok(AddLocation::Sequence {
                    sequence: parent,
                    index,
                })
            }
            _ => Err(YamlEditError::new(format!(
                "add parent {:?} is not a mapping or sequence",
                parent_pointer.as_str()
            ))),
        }
    }

    fn queue_add(
        &mut self,
        location: AddLocation,
        value: &YamlFragment,
    ) -> Result<(), YamlEditError> {
        match location {
            AddLocation::Root(root) => self.queue_fragment_replacement(root, value),
            AddLocation::Mapping {
                existing: Some(existing),
                ..
            } => self.queue_fragment_replacement(existing, value),
            AddLocation::Mapping {
                mapping,
                existing: None,
                key,
            } => self.queue_mapping_insert(mapping, &key, value),
            AddLocation::Sequence { sequence, index } => {
                self.queue_sequence_insert(sequence, index, value)
            }
        }
    }

    pub(crate) fn queue_fragment_replacement(
        &mut self,
        target: NodeId,
        value: &YamlFragment,
    ) -> Result<(), YamlEditError> {
        if self
            .node(target)
            .is_some_and(|node| node.kind() == crate::NodeKind::Scalar)
            && matches!(
                value.document().semantic_kind(value.root()),
                Some(SemanticKind::Scalar { .. })
            )
            && value.document().raw_tag(value.root()).is_none()
            && value.document().anchor(value.root()).is_none()
        {
            let mut replacement = value.to_yaml()?;
            if !replacement.contains(['\n', '\r']) {
                let (span, target_style) = self.scalar_replacement_target(target)?;
                if scalar_is_string(self, target)?
                    && scalar_is_string(value.document(), value.root())?
                {
                    let decoded = value.document().scalar_value(value.root())?;
                    if let Ok(styled) = crate::format_scalar_value(&decoded, target_style) {
                        replacement = styled;
                    }
                }
                self.queue_edit(span, replacement)?;
                return Ok(());
            }
        }
        self.queue_fragment_replacement_whole(target, value)
    }

    pub(crate) fn queue_fragment_replacement_whole(
        &mut self,
        target: NodeId,
        value: &YamlFragment,
    ) -> Result<(), YamlEditError> {
        let target_is_flow = matches!(
            self.semantic_kind(target),
            Some(
                SemanticKind::Mapping {
                    style: CollectionStyle::Flow
                } | SemanticKind::Sequence {
                    style: CollectionStyle::Flow
                }
            )
        );
        let replacement = if target_is_flow || self.is_flow_context(target) {
            value.render_flow_for_replacement(self, target)?
        } else {
            let yaml = value.prepared_for_replacement(self, target)?.to_yaml()?;
            indent_continuation_lines(&yaml, self.node_indent(self.expect_node(target)?))
        };
        self.replace_node_text(target, replacement)?;
        Ok(())
    }

    pub(crate) fn queue_value_removal(&mut self, target: NodeId) -> Result<(), YamlEditError> {
        let Some(entry) = self.containing_entry(target) else {
            self.remove_node(target)?;
            return Ok(());
        };
        let Some(collection) = self.node(entry).and_then(super::syntax::Node::parent) else {
            self.remove_node(entry)?;
            return Ok(());
        };
        let flow = matches!(
            self.semantic_kind(collection),
            Some(
                SemanticKind::Mapping {
                    style: CollectionStyle::Flow
                } | SemanticKind::Sequence {
                    style: CollectionStyle::Flow
                }
            )
        );
        if !flow {
            let span = self.block_collection_entry_removal_span(collection, entry)?;
            let empties_collection = self
                .children(collection)
                .filter(|node| self.containing_entry_child(*node))
                .count()
                == 1;
            let empty = empties_collection
                .then(|| match self.semantic_kind(collection) {
                    Some(SemanticKind::Mapping { .. }) => Some("{}"),
                    Some(SemanticKind::Sequence { .. }) => Some("[]"),
                    _ => None,
                })
                .flatten()
                .unwrap_or_default();
            if empty.is_empty() {
                self.queue_edit(span, String::new())?;
            } else {
                let (span, replacement) =
                    self.empty_block_collection_edit(collection, span, empty)?;
                self.queue_edit(span, replacement)?;
            }
            return Ok(());
        }

        let entries = self
            .children(collection)
            .filter(|node| self.containing_entry_child(*node))
            .collect::<Vec<_>>();
        let index = entries
            .iter()
            .position(|candidate| *candidate == entry)
            .ok_or_else(|| YamlEditError::new("flow collection entry is missing"))?;
        let entry_span = self.expect_node(entry)?.span;
        if entries.get(index + 1).is_none() && self.flow_collection_is_multiline(collection)? {
            if let Some(previous) = index.checked_sub(1).and_then(|index| entries.get(index)) {
                let separator = self.flow_separator_after(*previous, entry)?;
                self.queue_edit(separator, String::new())?;
            }
            let span = self.multiline_flow_terminal_entry_span(collection, entry)?;
            self.queue_edit(span, String::new())?;
            return Ok(());
        }
        let span = if let Some(next) = entries.get(index + 1).copied() {
            Span::new(entry_span.start, self.expect_node(next)?.span.start)
        } else if index > 0 {
            let previous = self.expect_node(entries[index - 1])?.span;
            Span::new(previous.end, entry_span.end)
        } else {
            entry_span
        };
        self.queue_edit(span, String::new())?;
        Ok(())
    }

    fn flow_collection_is_multiline(&self, collection: NodeId) -> Result<bool, YamlEditError> {
        Ok(self
            .source()
            .slice(self.expect_node(collection)?.span)
            .contains(['\r', '\n']))
    }

    fn flow_entry_value_end(&self, entry: NodeId) -> Result<usize, YamlEditError> {
        self.semantic_children(entry)
            .last()
            .and_then(|value| self.node(value))
            .map(|value| value.span.end as usize)
            .ok_or_else(|| YamlEditError::new("flow collection entry does not contain a value"))
    }

    fn flow_separator_after(&self, previous: NodeId, next: NodeId) -> Result<Span, YamlEditError> {
        let start = self.flow_entry_value_end(previous)?;
        let end = self.expect_node(next)?.span.start as usize;
        let separator = self.source().as_str()[start..end]
            .find(',')
            .map(|relative| start + relative)
            .ok_or_else(|| YamlEditError::new("flow collection entries have no separator"))?;
        Ok(Span::from_usize(separator, separator + 1))
    }

    fn multiline_flow_terminal_entry_span(
        &self,
        collection: NodeId,
        entry: NodeId,
    ) -> Result<Span, YamlEditError> {
        let collection_node = self.expect_node(collection)?;
        let entry_node = self.expect_node(entry)?;
        let delimiter = match collection_node.kind() {
            crate::NodeKind::FlowMapping => '}',
            crate::NodeKind::FlowSequence => ']',
            _ => return Err(YamlEditError::new("expected a flow collection")),
        };
        let close = closing_delimiter_offset(self, collection_node.span, delimiter)?;
        let content_end = self.flow_entry_value_end(entry)?;
        let line_end = self.source().as_str()[content_end..close]
            .find(['\r', '\n'])
            .map(|relative| content_end + relative)
            .unwrap_or(close);
        let entry_line = self.line_start_offset(entry_node.span.start as usize);
        let collection_line = self.line_start_offset(collection_node.span.start as usize);
        if entry_line == collection_line || line_end == close {
            return Ok(Span::from_usize(entry_node.span.start as usize, line_end));
        }
        let bytes = self.source().as_str().as_bytes();
        let mut end = line_end;
        if bytes.get(end) == Some(&b'\r') {
            end += 1;
            if bytes.get(end) == Some(&b'\n') {
                end += 1;
            }
        } else if bytes.get(end) == Some(&b'\n') {
            end += 1;
        }
        Ok(Span::from_usize(entry_line, end))
    }

    fn empty_block_collection_edit(
        &self,
        collection: NodeId,
        removal_span: Span,
        empty: &str,
    ) -> Result<(Span, String), YamlEditError> {
        let collection_node = self.expect_node(collection)?;
        let removed = self.source().slice(removal_span);
        let trailing_line_ending = if removed.ends_with("\r\n") {
            "\r\n"
        } else if removed.ends_with('\n') {
            "\n"
        } else if removed.ends_with('\r') {
            "\r"
        } else {
            ""
        };
        let empty_with_break = format!("{empty}{trailing_line_ending}");
        let value_start = self
            .children(collection)
            .find(|child| self.containing_entry_child(*child))
            .map(|entry| {
                self.expect_node(entry).map(|entry| {
                    usize::max(
                        entry.span.start as usize,
                        collection_node.span.start as usize,
                    )
                })
            })
            .transpose()?
            .unwrap_or(self.node_value_start(collection)?);
        let Some(entry) = collection_node.parent() else {
            return Ok((
                Span::from_usize(value_start, removal_span.end as usize),
                empty_with_break,
            ));
        };
        let entry_node = self.expect_node(entry)?;
        if !matches!(
            entry_node.kind(),
            crate::NodeKind::MappingEntry | crate::NodeKind::SequenceEntry
        ) || self.line_start_offset(value_start)
            == self.line_start_offset(entry_node.span.start as usize)
        {
            return Ok((
                Span::from_usize(value_start, removal_span.end as usize),
                empty_with_break,
            ));
        }

        let prefix_span = Span::from_usize(entry_node.span.start as usize, value_start);
        let prefix = self.source().slice(prefix_span);
        let line_break = prefix.find(['\r', '\n']).ok_or_else(|| {
            YamlEditError::new("multiline block collection has no preceding line break")
        })?;
        let line_break_end = if prefix.as_bytes().get(line_break) == Some(&b'\r')
            && prefix.as_bytes().get(line_break + 1) == Some(&b'\n')
        {
            line_break + 2
        } else {
            line_break + 1
        };
        let head = &prefix[..line_break];
        let uncommented = strip_inline_comment(head);
        let content = uncommented.trim_end_matches([' ', '\t']);
        let comment = &head[uncommented.len()..];
        let between = prefix[line_break_end..].trim_end_matches([' ', '\t']);
        let mut replacement = format!("{content} {empty}");
        if !comment.is_empty() {
            replacement.push(' ');
            replacement.push_str(comment.trim_start());
        }
        if !between.is_empty() {
            replacement.push_str(&prefix[line_break..line_break_end]);
            replacement.push_str(between);
        }
        replacement.push_str(trailing_line_ending);
        Ok((
            Span::new(entry_node.span.start, removal_span.end),
            replacement,
        ))
    }

    fn containing_entry_child(&self, node: NodeId) -> bool {
        self.node(node).is_some_and(|node| {
            matches!(
                node.kind(),
                crate::NodeKind::MappingEntry | crate::NodeKind::SequenceEntry
            )
        })
    }

    pub(crate) fn queue_mapping_insert(
        &mut self,
        mapping: NodeId,
        key: &str,
        value: &YamlFragment,
    ) -> Result<(), YamlEditError> {
        let Some(SemanticKind::Mapping { style }) = self.semantic_kind(mapping) else {
            return Err(YamlEditError::new(
                "mapping insertion target is not a mapping",
            ));
        };
        let key = emit_string_key(key);
        match style {
            CollectionStyle::Flow => {
                let mapping_node = self.expect_node(mapping)?;
                let close = closing_delimiter_offset(self, mapping_node.span, '}')?;
                let entries = self
                    .children(mapping)
                    .filter(|node| self.containing_entry_child(*node))
                    .collect::<Vec<_>>();
                let value = value.render_flow(self)?;
                if self.queue_multiline_flow_append(
                    mapping,
                    close,
                    &entries,
                    &format!("{key}: {value}"),
                )? {
                    return Ok(());
                }
                let has_pending_entry = self.edits.iter().any(|edit| {
                    edit.span == Span::empty_from_usize(close) && !edit.replacement.is_empty()
                });
                let prefix = if self.mapping_entries(mapping).next().is_some() || has_pending_entry
                {
                    ", "
                } else {
                    ""
                };
                self.queue_edit(
                    Span::empty_from_usize(close),
                    format!("{prefix}{key}: {value}"),
                )?;
            }
            CollectionStyle::Block => {
                let mapping_node = self.expect_node(mapping)?;
                let indent = self.block_mapping_entry_indent(mapping);
                let offset = self.mapping_insertion_offset(mapping_node);
                let preserve_unterminated_eof = insertion_at_unterminated_eof(self, offset);
                let mut insertion = insertion_prefix(self, offset);
                let value = value.prepared(self)?.to_yaml()?;
                let line_ending = self.preferred_line_ending().to_owned();
                insertion.push_str(&format_block_mapping_entry(
                    &key,
                    &value,
                    indent,
                    &line_ending,
                ));
                if preserve_unterminated_eof && !value.contains(['\n', '\r']) {
                    insertion.truncate(insertion.len() - line_ending.len());
                }
                self.queue_edit(Span::empty_from_usize(offset), insertion)?;
            }
        }
        Ok(())
    }

    pub(crate) fn queue_mapping_insert_before(
        &mut self,
        mapping: NodeId,
        before_entry: NodeId,
        key: &str,
        value: &YamlFragment,
    ) -> Result<(), YamlEditError> {
        let Some(SemanticKind::Mapping { style }) = self.semantic_kind(mapping) else {
            return Err(YamlEditError::new(
                "mapping insertion target is not a mapping",
            ));
        };
        if style != CollectionStyle::Flow {
            return Err(YamlEditError::new(
                "ordered flow insertion requires a flow mapping",
            ));
        }
        let entry = self.expect_node(before_entry)?;
        let key = emit_string_key(key);
        let value = value.render_flow(self)?;
        if self.flow_collection_is_multiline(mapping)? {
            let content_start = self.flow_entry_content_start(before_entry)?;
            let line_start = self.line_start_offset(content_start);
            if line_start != self.line_start_offset(self.expect_node(mapping)?.span.start as usize)
            {
                let indent = &self.source().as_str()[line_start..content_start];
                self.queue_edit(
                    Span::empty_from_usize(line_start),
                    format!("{indent}{key}: {value},{}", self.preferred_line_ending()),
                )?;
                return Ok(());
            }
        }
        self.queue_edit(Span::empty(entry.span.start), format!("{key}: {value}, "))?;
        Ok(())
    }

    pub(crate) fn queue_sequence_insert(
        &mut self,
        sequence: NodeId,
        index: usize,
        value: &YamlFragment,
    ) -> Result<(), YamlEditError> {
        let Some(SemanticKind::Sequence { style }) = self.semantic_kind(sequence) else {
            return Err(YamlEditError::new(
                "sequence insertion target is not a sequence",
            ));
        };
        let items = self.sequence_items(sequence).collect::<Vec<_>>();
        match style {
            CollectionStyle::Flow => {
                let sequence_span = self.expect_node(sequence)?.span;
                let entries = self
                    .children(sequence)
                    .filter(|node| self.containing_entry_child(*node))
                    .collect::<Vec<_>>();
                let close = closing_delimiter_offset(self, sequence_span, ']')?;
                let value = value.render_flow(self)?;
                if index == items.len()
                    && self.queue_multiline_flow_append(sequence, close, &entries, &value)?
                {
                    return Ok(());
                }
                if let Some(entry) = entries.get(index).copied()
                    && self.flow_collection_is_multiline(sequence)?
                {
                    let content_start = self.flow_entry_content_start(entry)?;
                    let line_start = self.line_start_offset(content_start);
                    if line_start != self.line_start_offset(sequence_span.start as usize) {
                        let indent = &self.source().as_str()[line_start..content_start];
                        self.queue_edit(
                            Span::empty_from_usize(line_start),
                            format!("{indent}{value},{}", self.preferred_line_ending()),
                        )?;
                        return Ok(());
                    }
                }
                let offset = if let Some(item) = items.get(index).copied() {
                    self.expect_node(item)?.span.start as usize
                } else {
                    close
                };
                let insertion = if items.is_empty() {
                    value
                } else if index < items.len() {
                    format!("{value}, ")
                } else {
                    format!(", {value}")
                };
                self.queue_edit(Span::empty_from_usize(offset), insertion)?;
            }
            CollectionStyle::Block => {
                let sequence_node = self.expect_node(sequence)?;
                let indent = self.node_indent(sequence_node);
                let offset = if let Some(item) = items.get(index).copied() {
                    let entry = self.containing_entry(item).unwrap_or(item);
                    self.line_start_offset(self.expect_node(entry)?.span.start as usize)
                } else {
                    self.sequence_insertion_offset(sequence_node)
                };
                let preserve_unterminated_eof = insertion_at_unterminated_eof(self, offset);
                let mut insertion = insertion_prefix(self, offset);
                let value = value.prepared(self)?.to_yaml()?;
                let line_ending = self.preferred_line_ending().to_owned();
                insertion.push_str(&format_block_sequence_entry(&value, indent, &line_ending));
                if preserve_unterminated_eof && !value.contains(['\n', '\r']) {
                    insertion.truncate(insertion.len() - line_ending.len());
                }
                self.queue_edit(Span::empty_from_usize(offset), insertion)?;
            }
        }
        Ok(())
    }

    fn flow_entry_content_start(&self, entry: NodeId) -> Result<usize, YamlEditError> {
        self.semantic_children(entry)
            .next()
            .and_then(|value| self.node(value))
            .map(|value| value.span.start as usize)
            .ok_or_else(|| YamlEditError::new("flow collection entry does not contain a value"))
    }

    fn queue_multiline_flow_append(
        &mut self,
        collection: NodeId,
        close: usize,
        entries: &[NodeId],
        rendered: &str,
    ) -> Result<bool, YamlEditError> {
        if !self.flow_collection_is_multiline(collection)? {
            return Ok(false);
        }
        let close_line = self.line_start_offset(close);
        let indent = if let Some(last) = entries.last().copied() {
            let content_start = self.flow_entry_content_start(last)?;
            if self.line_start_offset(content_start) == close_line {
                return Ok(false);
            }
            self.source().as_str()[self.line_start_offset(content_start)..content_start].to_owned()
        } else {
            let close_indent = self.source().as_str()[close_line..close]
                .bytes()
                .take_while(|byte| *byte == b' ')
                .count();
            " ".repeat(close_indent + 2)
        };
        if let Some(last) = entries.last().copied() {
            let separator = Span::empty_from_usize(self.flow_entry_value_end(last)?);
            self.queue_edit(separator, ",".to_owned())?;
        }
        self.queue_edit(
            Span::empty_from_usize(close_line),
            format!("{indent}{rendered}{}", self.preferred_line_ending()),
        )?;
        Ok(true)
    }

    pub(crate) fn is_flow_context(&self, mut node: NodeId) -> bool {
        while let Some(parent) = self.node(node).and_then(super::syntax::Node::parent) {
            if matches!(
                self.semantic_kind(parent),
                Some(
                    SemanticKind::Mapping {
                        style: CollectionStyle::Flow
                    } | SemanticKind::Sequence {
                        style: CollectionStyle::Flow
                    }
                )
            ) {
                return true;
            }
            node = parent;
        }
        false
    }

    fn anchor_has_external_alias(&self, root: NodeId) -> bool {
        let Some(root_span) = self.node(root).map(super::syntax::Node::span) else {
            return false;
        };
        let anchored = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, _)| NodeId::from_usize(index))
            .filter(|node| {
                self.anchor(*node).is_some()
                    && self.node(*node).is_some_and(|node| {
                        node.span().start >= root_span.start && node.span().end <= root_span.end
                    })
            })
            .collect::<Vec<_>>();
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, _)| NodeId::from_usize(index))
            .filter(|node| matches!(self.semantic_kind(*node), Some(SemanticKind::Alias)))
            .any(|alias| {
                let outside = self.node(alias).is_some_and(|node| {
                    node.span().start < root_span.start || node.span().end > root_span.end
                });
                outside
                    && self
                        .resolve_alias(alias)
                        .is_some_and(|target| anchored.contains(&target))
            })
    }

    pub(crate) fn line_start_offset(&self, offset: usize) -> usize {
        let offset = u32::try_from(offset).unwrap_or(u32::MAX);
        match self.source.line_starts().binary_search(&offset) {
            Ok(index) => self.source.line_starts()[index] as usize,
            Err(index) => self.source.line_starts()[index.saturating_sub(1)] as usize,
        }
    }
}

pub(crate) fn closing_delimiter_offset(
    doc: &YamlDoc,
    span: Span,
    delimiter: char,
) -> Result<usize, YamlEditError> {
    let source = doc.source.slice(span);
    let relative = source
        .rfind(delimiter)
        .ok_or_else(|| YamlEditError::new(format!("missing `{delimiter}` delimiter")))?;
    Ok(span.start as usize + relative)
}

fn insertion_prefix(doc: &YamlDoc, offset: usize) -> String {
    if insertion_at_unterminated_eof(doc, offset) {
        doc.preferred_line_ending().to_owned()
    } else {
        String::new()
    }
}

fn insertion_at_unterminated_eof(doc: &YamlDoc, offset: usize) -> bool {
    offset == doc.source.len()
        && !doc
            .source
            .as_str()
            .as_bytes()
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
}

fn format_block_mapping_entry(key: &str, value: &str, indent: usize, ending: &str) -> String {
    let prefix = " ".repeat(indent);
    if !value.contains(['\n', '\r']) {
        return format!("{prefix}{key}: {value}{ending}");
    }
    let value = indent_text(value, indent + 2);
    let mut output = format!("{prefix}{key}:{ending}{value}");
    if !output.ends_with(['\n', '\r']) {
        output.push_str(ending);
    }
    output
}

fn format_block_sequence_entry(value: &str, indent: usize, ending: &str) -> String {
    let prefix = " ".repeat(indent);
    if !value.contains(['\n', '\r']) {
        return format!("{prefix}- {value}{ending}");
    }
    let value = indent_text(value, indent + 2);
    let mut output = format!("{prefix}-{ending}{value}");
    if !output.ends_with(['\n', '\r']) {
        output.push_str(ending);
    }
    output
}

fn indent_continuation_lines(value: &str, indent: usize) -> String {
    if indent == 0 {
        return value.to_owned();
    }
    let prefix = " ".repeat(indent);
    let mut output = String::with_capacity(value.len());
    let mut after_break = false;
    for character in value.chars() {
        if after_break && !matches!(character, '\r' | '\n') {
            output.push_str(&prefix);
            after_break = false;
        }
        output.push(character);
        if character == '\n' {
            after_break = true;
        } else if character != '\r' {
            after_break = false;
        }
    }
    output
}

pub(crate) fn emit_string_key(value: &str) -> String {
    if safe_plain_string(value) {
        value.to_owned()
    } else {
        crate::fragment::quote_string(value)
    }
}

pub(crate) fn safe_plain_string(value: &str) -> bool {
    if !safe_plain_string_syntax(value) {
        return false;
    }
    matches!(
        resolve_scalar(value, YamlScalarStyle::Plain, None),
        Ok(ResolvedScalar::String)
    )
}

fn safe_plain_string_syntax(value: &str) -> bool {
    if value.is_empty()
        || value.trim() != value
        || value.contains(['\n', '\r', '\t', ':', '#', '[', ']', '{', '}', ','])
        || value.starts_with(['-', '?', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`'])
    {
        return false;
    }
    true
}

fn safe_plain_key_with_tag(value: &str, tag: Option<&str>) -> bool {
    const STRING_TAG: &str = "tag:yaml.org,2002:str";

    safe_plain_string_syntax(value)
        && (tag == Some(STRING_TAG)
            || matches!(
                resolve_scalar(value, YamlScalarStyle::Plain, None),
                Ok(ResolvedScalar::String)
            ))
}

fn scalar_is_string(doc: &YamlDoc, node: NodeId) -> Result<bool, YamlEditError> {
    let Some(SemanticKind::Scalar { style }) = doc.semantic_kind(node) else {
        return Ok(false);
    };
    let value = doc.scalar_value(node)?;
    let tag = doc.resolved_tag(node)?;
    Ok(matches!(
        resolve_scalar(&value, style, tag.as_deref()),
        Ok(ResolvedScalar::String)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointer(value: &str) -> JsonPointer {
        JsonPointer::parse(value).unwrap()
    }

    fn fragment(value: &str) -> YamlFragment {
        YamlFragment::parse(value).unwrap()
    }

    #[test]
    fn renames_block_flow_and_explicit_mapping_keys_losslessly() {
        let input = "old: 1 # keep\nflow: {old: 2}\nexplicit:\n  ? 'old'\n  : 3\n";
        let mut doc = YamlDoc::parse(input).unwrap();
        doc.rename_keys_at(
            0,
            &[
                pointer("/old"),
                pointer("/flow/old"),
                pointer("/explicit/old"),
            ],
            "true",
        )
        .unwrap();

        assert_eq!(
            doc.as_source(),
            "\"true\": 1 # keep\nflow: {\"true\": 2}\nexplicit:\n  ? 'true'\n  : 3\n"
        );
    }

    #[test]
    fn rename_key_quotes_plain_names_that_are_not_safe_strings() {
        for new_key in ["true", "", "a: b", "line\nbreak"] {
            let mut doc = YamlDoc::parse("old: value\n").unwrap();
            doc.rename_key_at(0, &pointer("/old"), new_key).unwrap();
            assert_eq!(
                doc.as_source(),
                format!("{}: value\n", crate::fragment::quote_string(new_key))
            );
        }

        let mut flow = YamlDoc::parse("{old: value}\n").unwrap();
        flow.rename_key_at(0, &pointer("/old"), "a,b").unwrap();
        assert_eq!(flow.as_source(), "{\"a,b\": value}\n");
    }

    #[test]
    fn rename_key_preserves_quoted_styles_properties_comments_and_line_endings() {
        let mut single = YamlDoc::parse("'old': value\n").unwrap();
        single.rename_key_at(0, &pointer("/old"), "Bob's").unwrap();
        assert_eq!(single.as_source(), "'Bob''s': value\n");

        let mut double = YamlDoc::parse("\"old\": value\n").unwrap();
        double
            .rename_key_at(0, &pointer("/old"), "new \"key\"")
            .unwrap();
        assert_eq!(double.as_source(), "\"new \\\"key\\\"\": value\n");

        let mut tagged = YamlDoc::parse("!!str &key old: value # keep\r\n").unwrap();
        tagged.rename_key_at(0, &pointer("/old"), "true").unwrap();
        assert_eq!(tagged.as_source(), "!!str &key true: value # keep\r\n");
    }

    #[test]
    fn rename_keys_resolves_all_targets_before_editing_and_deduplicates_alias_routes() {
        let mut nested = YamlDoc::parse("parent:\n  old: 1\nold: 2\n").unwrap();
        nested
            .rename_keys_at(0, &[pointer("/parent/old"), pointer("/parent")], "renamed")
            .unwrap();
        assert_eq!(nested.as_source(), "renamed:\n  renamed: 1\nold: 2\n");

        let mut aliases = YamlDoc::parse("base: &base\n  name: Ada\ncopy: *base\n").unwrap();
        aliases
            .rename_keys_at(
                0,
                &[pointer("/base/name"), pointer("/copy/name")],
                "display-name",
            )
            .unwrap();
        assert_eq!(
            aliases.as_source(),
            "base: &base\n  display-name: Ada\ncopy: *base\n"
        );
    }

    #[test]
    fn rename_keys_rejects_collisions_transactionally() {
        let input = "a: 1\nb: 2\n";
        let mut existing = YamlDoc::parse(input).unwrap();
        let error = existing.rename_key_at(0, &pointer("/a"), "b").unwrap_err();
        assert!(error.to_string().contains("duplicate key \"b\""));
        assert_eq!(existing.as_source(), input);

        let mut selected = YamlDoc::parse(input).unwrap();
        let error = selected
            .rename_keys_at(0, &[pointer("/a"), pointer("/b")], "x")
            .unwrap_err();
        assert!(error.to_string().contains("duplicate key \"x\""));
        assert_eq!(selected.as_source(), input);
    }

    #[test]
    fn rename_keys_treats_duplicate_and_unchanged_targets_as_no_ops() {
        let input = "\"old\": value\n";
        let mut doc = YamlDoc::parse(input).unwrap();
        doc.rename_keys_at(0, &[pointer("/old"), pointer("/old")], "old")
            .unwrap();
        doc.rename_keys_at(0, &[], "ignored").unwrap();
        assert_eq!(doc.as_source(), input);
    }

    #[test]
    fn rename_key_rejects_non_members_and_unsupported_key_forms() {
        let mut root = YamlDoc::parse("key: value\n").unwrap();
        assert!(
            root.rename_key_at(0, &pointer(""), "new")
                .unwrap_err()
                .to_string()
                .contains("document root")
        );
        assert_eq!(root.as_source(), "key: value\n");

        let mut sequence = YamlDoc::parse("- value\n").unwrap();
        assert!(
            sequence
                .rename_key_at(0, &pointer("/0"), "new")
                .unwrap_err()
                .to_string()
                .contains("does not select a mapping member")
        );

        let mut block = YamlDoc::parse("? >\n  old\n: value\n").unwrap();
        assert!(
            block
                .rename_key_at(0, &pointer("/old\n"), "new")
                .unwrap_err()
                .to_string()
                .contains("plain, single-quoted, and double-quoted")
        );

        let mut alias = YamlDoc::parse("name: &key target\n? *key\n: value\n").unwrap();
        let error = alias
            .rename_key_at(0, &pointer("/target"), "new")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("plain, single-quoted, and double-quoted"),
            "{error}"
        );

        let mut complex = YamlDoc::parse("? [a, b]\n: value\n").unwrap();
        assert!(
            complex
                .rename_key_at(0, &pointer("/anything"), "new")
                .is_err()
        );
    }

    #[test]
    fn adds_replaces_and_removes_block_mapping_values() {
        let mut doc = YamlDoc::parse("server:\n  host: localhost # keep\n").unwrap();
        doc.add_at(0, &pointer("/server/port"), &fragment("8080"))
            .unwrap();
        assert_eq!(
            doc.as_source(),
            "server:\n  host: localhost # keep\n  port: 8080\n"
        );
        doc.replace_at(0, &pointer("/server/host"), &fragment("example.com"))
            .unwrap();
        assert_eq!(
            doc.as_source(),
            "server:\n  host: example.com # keep\n  port: 8080\n"
        );
        doc.remove_at(0, &pointer("/server/port")).unwrap();
        assert_eq!(doc.as_source(), "server:\n  host: example.com # keep\n");
    }

    #[test]
    fn replaces_compact_and_expanded_block_sequences_with_flow_fragments() {
        for input in [
            "releases:\n  - artifact: client\n    channel: stable\ntail: retained\n",
            "releases:\n  -\n    artifact: client\n    channel: stable\ntail: retained\n",
        ] {
            let mut doc = YamlDoc::parse(input).unwrap();
            doc.replace_at(
                0,
                &pointer("/releases"),
                &fragment("[{artifact: server, channel: preview}]"),
            )
            .unwrap();

            assert_eq!(
                doc.as_source(),
                "releases:\n  [{artifact: server, channel: preview}]\ntail: retained\n"
            );
            let reparsed = YamlDoc::parse(doc.as_source()).unwrap();
            let artifact = reparsed
                .resolve_pointer(0, &pointer("/releases/0/artifact"))
                .unwrap();
            let channel = reparsed
                .resolve_pointer(0, &pointer("/releases/0/channel"))
                .unwrap();
            assert_eq!(reparsed.scalar_value(artifact).unwrap(), "server");
            assert_eq!(reparsed.scalar_value(channel).unwrap(), "preview");
        }
    }

    #[test]
    fn collection_replacement_keeps_the_owning_entry_comment() {
        let mut doc = YamlDoc::parse(
            "jobs: # scheduling note\n  - # obsolete job note\n    command: build\n    retries: 2\nafter: unchanged\n",
        )
        .unwrap();

        doc.replace_at(
            0,
            &pointer("/jobs"),
            &fragment("[{command: test, retries: 1}]"),
        )
        .unwrap();

        assert_eq!(
            doc.as_source(),
            "jobs: # scheduling note\n  [{command: test, retries: 1}]\nafter: unchanged\n"
        );
        doc.commit_edits().unwrap();
    }

    #[test]
    fn inserts_into_compact_sequence_entry_mappings_at_the_key_column() {
        let input = "services:\n  - name: api\n    port: 8080\n";

        let mut scalar = YamlDoc::parse(input).unwrap();
        scalar
            .add_at(0, &pointer("/services/0/enabled"), &fragment("true"))
            .unwrap();
        assert_eq!(
            scalar.as_source(),
            "services:\n  - name: api\n    port: 8080\n    enabled: true\n"
        );
        scalar.commit_edits().unwrap();

        let mut nested = YamlDoc::parse(input).unwrap();
        nested
            .add_at(
                0,
                &pointer("/services/0/tls"),
                &fragment("{enabled: true, mode: strict}"),
            )
            .unwrap();
        assert_eq!(
            nested.as_source(),
            "services:\n  - name: api\n    port: 8080\n    tls: {enabled: true, mode: strict}\n"
        );
        nested.commit_edits().unwrap();
    }

    #[test]
    fn adds_string_keys_without_changing_their_schema_type() {
        let mut doc = YamlDoc::parse("{}\n").unwrap();
        doc.add_at(0, &pointer("/true"), &fragment("value"))
            .unwrap();
        assert_eq!(doc.as_source(), "{\"true\": value}\n");
        assert!(
            doc.resolve_pointer(0, &pointer("/true")).is_ok(),
            "{}",
            doc.as_source()
        );
    }

    #[test]
    fn inserts_block_and_flow_sequence_items() {
        let mut block = YamlDoc::parse("items:\n  - a\n  - c\n").unwrap();
        block
            .add_at(0, &pointer("/items/1"), &fragment("b"))
            .unwrap();
        block
            .add_at(0, &pointer("/items/-"), &fragment("d"))
            .unwrap();
        assert_eq!(block.as_source(), "items:\n  - a\n  - b\n  - c\n  - d\n");

        let mut flow = YamlDoc::parse("items: [a, c]\n").unwrap();
        flow.add_at(0, &pointer("/items/1"), &fragment("b"))
            .unwrap();
        assert_eq!(flow.as_source(), "items: [a, b, c]\n");
    }

    #[test]
    fn inserts_into_multiline_flow_collections_without_collapsing_layout() {
        let mut mapping = YamlDoc::parse("map: {\n  a: 1 # keep\n}\ntail: keep\n").unwrap();
        mapping
            .add_at(0, &pointer("/map/b"), &fragment("2"))
            .unwrap();
        assert_eq!(
            mapping.as_source(),
            "map: {\n  a: 1, # keep\n  b: 2\n}\ntail: keep\n"
        );
        mapping.commit_edits().unwrap();

        let mut empty_mapping = YamlDoc::parse("outer:\n  map: {\n  }\ntail: keep\n").unwrap();
        empty_mapping
            .add_at(0, &pointer("/outer/map/a"), &fragment("1"))
            .unwrap();
        assert_eq!(
            empty_mapping.as_source(),
            "outer:\n  map: {\n    a: 1\n  }\ntail: keep\n"
        );
        empty_mapping.commit_edits().unwrap();

        let mut sequence = YamlDoc::parse("items: [\r\n  a,\r\n  c\r\n]\r\n").unwrap();
        sequence
            .add_at(0, &pointer("/items/1"), &fragment("b"))
            .unwrap();
        sequence
            .add_at(0, &pointer("/items/-"), &fragment("d"))
            .unwrap();
        assert_eq!(
            sequence.as_source(),
            "items: [\r\n  a,\r\n  b,\r\n  c,\r\n  d\r\n]\r\n"
        );
        sequence.commit_edits().unwrap();

        let mut empty_sequence = YamlDoc::parse("items: [\n]\ntail: keep\n").unwrap();
        empty_sequence
            .add_at(0, &pointer("/items/-"), &fragment("a"))
            .unwrap();
        assert_eq!(empty_sequence.as_source(), "items: [\n  a\n]\ntail: keep\n");
        empty_sequence.commit_edits().unwrap();

        let mut compact = YamlDoc::parse("map: {a: 1}\nitems: [a]\n").unwrap();
        compact
            .add_at(0, &pointer("/map/b"), &fragment("2"))
            .unwrap();
        compact
            .add_at(0, &pointer("/items/-"), &fragment("b"))
            .unwrap();
        assert_eq!(compact.as_source(), "map: {a: 1, b: 2}\nitems: [a, b]\n");
    }

    #[test]
    fn removes_complete_multiline_block_collection_entries() {
        let input = "items:\n  - name: first\n    enabled: true\n  - name: second\n    enabled: false\ntail: keep\n";
        let mut sequence = YamlDoc::parse(input).unwrap();
        sequence.remove_at(0, &pointer("/items/0")).unwrap();
        assert_eq!(
            sequence.as_source(),
            "items:\n  - name: second\n    enabled: false\ntail: keep\n"
        );
        sequence.commit_edits().unwrap();

        let mut last_sequence = YamlDoc::parse(input).unwrap();
        last_sequence.remove_at(0, &pointer("/items/1")).unwrap();
        assert_eq!(
            last_sequence.as_source(),
            "items:\n  - name: first\n    enabled: true\ntail: keep\n"
        );
        last_sequence.commit_edits().unwrap();

        let input = "server:\n  host: localhost\n  tls:\n    enabled: true\ntail: keep\n";
        let mut mapping = YamlDoc::parse(input).unwrap();
        mapping.remove_at(0, &pointer("/server")).unwrap();
        assert_eq!(mapping.as_source(), "tail: keep\n");
        mapping.commit_edits().unwrap();
    }

    #[test]
    fn removes_fields_from_compact_sequence_entry_mappings() {
        let input = "services:\n  - name: api\n    port: 8080 # public endpoint\n    enabled: TRUE\n  - {name: worker, port: 8081, enabled: false}\ntail: keep\n";

        let mut first = YamlDoc::parse(input).unwrap();
        first.remove_at(0, &pointer("/services/0/name")).unwrap();
        assert_eq!(
            first.as_source(),
            "services:\n  - port: 8080 # public endpoint\n    enabled: TRUE\n  - {name: worker, port: 8081, enabled: false}\ntail: keep\n"
        );
        first.commit_edits().unwrap();
        assert!(
            first
                .resolve_pointer(0, &pointer("/services/0/port"))
                .is_ok()
        );

        let mut middle = YamlDoc::parse(input).unwrap();
        middle.remove_at(0, &pointer("/services/0/port")).unwrap();
        assert_eq!(
            middle.as_source(),
            "services:\n  - name: api\n    enabled: TRUE\n  - {name: worker, port: 8081, enabled: false}\ntail: keep\n"
        );
        middle.commit_edits().unwrap();

        let mut last = YamlDoc::parse(input).unwrap();
        last.remove_at(0, &pointer("/services/0/enabled")).unwrap();
        assert_eq!(
            last.as_source(),
            "services:\n  - name: api\n    port: 8080 # public endpoint\n  - {name: worker, port: 8081, enabled: false}\ntail: keep\n"
        );
        last.commit_edits().unwrap();
        assert!(
            last.resolve_pointer(0, &pointer("/services/1/name"))
                .is_ok()
        );

        let mut only = YamlDoc::parse("services:\n  - name: api\ntail: keep\n").unwrap();
        only.remove_at(0, &pointer("/services/0/name")).unwrap();
        assert_eq!(only.as_source(), "services:\n  - {}\ntail: keep\n");
        only.commit_edits().unwrap();
        let item = only.resolve_pointer(0, &pointer("/services/0")).unwrap();
        assert!(matches!(
            only.semantic_kind(item),
            Some(SemanticKind::Mapping { .. })
        ));
    }

    #[test]
    fn removes_terminal_entries_without_collapsing_multiline_flow_layout() {
        let mut mapping =
            YamlDoc::parse("map: {\n  a: 1, # keep\n  b: 2\n}\ntail: keep\n").unwrap();
        mapping.remove_at(0, &pointer("/map/b")).unwrap();
        assert_eq!(
            mapping.as_source(),
            "map: {\n  a: 1 # keep\n}\ntail: keep\n"
        );
        mapping.commit_edits().unwrap();
        assert!(mapping.resolve_pointer(0, &pointer("/map/a")).is_ok());

        let mut sequence =
            YamlDoc::parse("items: [\r\n  a,\r\n  b\r\n]\r\ntail: keep\r\n").unwrap();
        sequence.remove_at(0, &pointer("/items/1")).unwrap();
        assert_eq!(
            sequence.as_source(),
            "items: [\r\n  a\r\n]\r\ntail: keep\r\n"
        );
        sequence.commit_edits().unwrap();

        let mut only = YamlDoc::parse("map: {\n  only: value\n}\ntail: keep\n").unwrap();
        only.remove_at(0, &pointer("/map/only")).unwrap();
        assert_eq!(only.as_source(), "map: {\n}\ntail: keep\n");
        only.commit_edits().unwrap();

        let mut compact = YamlDoc::parse("map: {a: 1, b: 2}\n").unwrap();
        compact.remove_at(0, &pointer("/map/b")).unwrap();
        assert_eq!(compact.as_source(), "map: {a: 1}\n");
    }

    #[test]
    fn removes_multiple_block_sequence_entries_transactionally() {
        let input = "items:\n  - name: first\n    enabled: true\n  - name: second\n    enabled: false\ntail: keep\n";
        let mut doc = YamlDoc::parse(input).unwrap();
        doc.remove_at(0, &pointer("/items/1")).unwrap();
        doc.remove_at(0, &pointer("/items/0")).unwrap();
        assert_eq!(doc.as_source(), "items: []\ntail: keep\n");
        doc.commit_edits().unwrap();
    }

    #[test]
    fn block_removal_owns_only_attached_same_indented_comments() {
        let mut mapping = YamlDoc::parse(
            "root:\r\n  keep: one\r\n  # first\r\n  # second\r\n  remove: two # inline\r\n  tail: three\r\n",
        )
        .unwrap();
        mapping.remove_at(0, &pointer("/root/remove")).unwrap();
        assert_eq!(
            mapping.as_source(),
            "root:\r\n  keep: one\r\n  tail: three\r\n"
        );

        let mut separated = YamlDoc::parse(
            "root:\n # different indent\n\n  # separated\n  remove: value\n  tail: keep\n",
        )
        .unwrap();
        separated.remove_at(0, &pointer("/root/remove")).unwrap();
        assert_eq!(
            separated.as_source(),
            "root:\n # different indent\n\n  tail: keep\n"
        );

        let mut sequence = YamlDoc::parse(
            "items:\n  - keep\n  # attached\n  - remove\n  # belongs to last\n  - last\n",
        )
        .unwrap();
        sequence.remove_at(0, &pointer("/items/1")).unwrap();
        assert_eq!(
            sequence.as_source(),
            "items:\n  - keep\n  # belongs to last\n  - last\n"
        );

        let mut edges = YamlDoc::parse(
            "root:\n    # different indentation\n  # first comment\n  first:\n    # nested comment\n    value: one\n  middle: keep\n  # last comment\n  last: three\n",
        )
        .unwrap();
        edges.remove_at(0, &pointer("/root/first")).unwrap();
        assert_eq!(
            edges.as_source(),
            "root:\n    # different indentation\n  middle: keep\n  # last comment\n  last: three\n"
        );
        edges.commit_edits().unwrap();
        edges.remove_at(0, &pointer("/root/last")).unwrap();
        assert_eq!(
            edges.as_source(),
            "root:\n    # different indentation\n  middle: keep\n"
        );
    }

    #[test]
    fn removals_preserve_empty_block_collection_types() {
        let mut nested =
            YamlDoc::parse("server:\n  host: localhost\nitems:\n  - only\ntail: keep\n").unwrap();
        nested.remove_at(0, &pointer("/server/host")).unwrap();
        nested.remove_at(0, &pointer("/items/0")).unwrap();
        assert_eq!(nested.as_source(), "server: {}\nitems: []\ntail: keep\n");
        nested.commit_edits().unwrap();
        assert!(matches!(
            nested.semantic_kind(nested.resolve_pointer(0, &pointer("/server")).unwrap()),
            Some(SemanticKind::Mapping { .. })
        ));
        assert!(matches!(
            nested.semantic_kind(nested.resolve_pointer(0, &pointer("/items")).unwrap()),
            Some(SemanticKind::Sequence { .. })
        ));

        let mut root_mapping = YamlDoc::parse("only: value\n").unwrap();
        root_mapping.remove_at(0, &pointer("/only")).unwrap();
        assert_eq!(root_mapping.as_source(), "{}\n");
        root_mapping.commit_edits().unwrap();

        let mut root_sequence = YamlDoc::parse("- only\n").unwrap();
        root_sequence.remove_at(0, &pointer("/0")).unwrap();
        assert_eq!(root_sequence.as_source(), "[]\n");
        root_sequence.commit_edits().unwrap();

        let mut anchored =
            YamlDoc::parse("defaults: &defaults\n  retries: 3\nmirror: *defaults\n").unwrap();
        anchored
            .remove_at(0, &pointer("/defaults/retries"))
            .unwrap();
        assert_eq!(
            anchored.as_source(),
            "defaults: &defaults {}\nmirror: *defaults\n"
        );
        anchored.commit_edits().unwrap();
        let mirror = anchored.resolve_pointer(0, &pointer("/mirror")).unwrap();
        let resolved = anchored.resolve_alias(mirror).unwrap();
        assert!(matches!(
            anchored.semantic_kind(resolved),
            Some(SemanticKind::Mapping { .. })
        ));

        let mut commented =
            YamlDoc::parse("server: # keep\r\n  host: localhost\r\ntail: keep\r\n").unwrap();
        commented.remove_at(0, &pointer("/server/host")).unwrap();
        assert_eq!(commented.as_source(), "server: {} # keep\r\ntail: keep\r\n");
        commented.commit_edits().unwrap();
    }

    #[test]
    fn moves_can_temporarily_empty_block_collections() {
        let mut mapping = YamlDoc::parse("server:\n  host: localhost\ntail: keep\n").unwrap();
        mapping
            .move_at(0, &pointer("/server/host"), &pointer("/server/name"))
            .unwrap();
        assert_eq!(
            mapping.as_source(),
            "server: {name: localhost}\ntail: keep\n"
        );

        let mut sequence = YamlDoc::parse("items:\n  - only\ntail: keep\n").unwrap();
        sequence
            .move_at(0, &pointer("/items/0"), &pointer("/items/-"))
            .unwrap();
        assert_eq!(sequence.as_source(), "items: [only]\ntail: keep\n");
    }

    #[test]
    fn mutations_are_transactional() {
        let input = "items: [a]\n";
        let mut doc = YamlDoc::parse(input).unwrap();
        assert!(doc.add_at(0, &pointer("/items/4"), &fragment("x")).is_err());
        assert_eq!(doc.as_source(), input);
    }

    #[test]
    fn move_uses_remove_then_add_sequence_indices() {
        let mut doc = YamlDoc::parse("[a, b, c]\n").unwrap();
        doc.move_at(0, &pointer("/0"), &pointer("/2")).unwrap();
        assert_eq!(doc.as_source(), "[b, c, a]\n");
    }

    #[test]
    fn move_and_copy_strip_inline_comments_when_rendering_flow_values() {
        let input = "value: 8080 # public endpoint\ntarget: {}\n";
        let mut moved = YamlDoc::parse(input).unwrap();
        moved
            .move_at(0, &pointer("/value"), &pointer("/target/moved"))
            .unwrap();
        assert_eq!(moved.as_source(), "target: {moved: 8080}\n");
        moved.commit_edits().unwrap();

        let input = "value: \"a # b\" # keep here\ntarget: {}\n";
        let mut copied = YamlDoc::parse(input).unwrap();
        copied
            .copy_at(0, &pointer("/value"), &pointer("/target/copied"))
            .unwrap();
        assert_eq!(
            copied.as_source(),
            "value: \"a # b\" # keep here\ntarget: {copied: \"a # b\"}\n"
        );
        copied.commit_edits().unwrap();
    }

    #[test]
    fn copy_rejects_anchors_and_test_is_semantic() {
        let mut doc = YamlDoc::parse("one: &one {value: 1}\ntwo: null\n").unwrap();
        assert!(
            doc.copy_at(0, &pointer("/one"), &pointer("/two"))
                .unwrap_err()
                .to_string()
                .contains("anchor")
        );
        assert!(
            doc.test_at(0, &pointer("/one/value"), &fragment("1.0"))
                .unwrap()
        );
    }
}
