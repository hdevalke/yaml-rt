use crate::semantic::SemanticStore;
use crate::{NodeId, Parser, Source, Span, YamlError};

/// Lossless syntax node produced by the CST parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Node classification.
    pub(crate) kind: NodeKind,
    /// Original source span for this node.
    pub(crate) span: Span,
    pub(crate) parent: u32,
    pub(crate) first_child: u32,
    pub(crate) last_child: u32,
    pub(crate) next_sibling: u32,
}

pub(crate) const NO_NODE: u32 = u32::MAX;

impl Node {
    /// Returns this node's syntax classification.
    #[must_use]
    pub const fn kind(&self) -> NodeKind {
        self.kind
    }

    /// Returns this node's original source span.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Returns this node's parent, when it is not the stream root.
    #[must_use]
    pub const fn parent(&self) -> Option<NodeId> {
        node_link(self.parent)
    }
}

/// Iterator over a node's children in source order.
#[derive(Debug, Clone)]
pub struct Children<'doc> {
    nodes: &'doc [Node],
    next: u32,
}

impl<'doc> Children<'doc> {
    pub(crate) fn new(nodes: &'doc [Node], parent: NodeId) -> Self {
        let next = nodes
            .get(parent.as_usize())
            .map_or(NO_NODE, |node| node.first_child);
        Self { nodes, next }
    }
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        let id = node_link(self.next)?;
        self.next = self.nodes[id.as_usize()].next_sibling;
        Some(id)
    }
}

pub(crate) const fn node_link(link: u32) -> Option<NodeId> {
    if link == NO_NODE {
        None
    } else {
        Some(NodeId(link))
    }
}

/// Node kinds emitted by the CST parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeKind {
    /// Complete YAML stream.
    Stream,
    /// Content document inside a stream.
    Document,
    /// Explicit document start or end marker.
    DocumentMarker,
    /// YAML directive line.
    Directive,
    /// Block mapping collection.
    BlockMapping,
    /// One mapping entry line.
    MappingEntry,
    /// Block sequence collection.
    BlockSequence,
    /// One block or flow sequence item wrapper.
    SequenceEntry,
    /// Single-line flow sequence collection.
    FlowSequence,
    /// Single-line flow mapping collection.
    FlowMapping,
    /// Literal block scalar collection.
    LiteralScalar,
    /// Folded block scalar collection.
    FoldedScalar,
    /// Scalar syntax span.
    Scalar,
}

/// Semantic YAML event produced by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlEvent {
    /// Event classification.
    pub kind: YamlEventKind,
    /// Source span associated with this event.
    pub span: Span,
    /// CST node that originated this semantic event, when applicable.
    pub(crate) cst: Option<NodeId>,
    pub(crate) content_indent: Option<u32>,
}

/// YAML collection spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionStyle {
    /// Block collection syntax.
    Block,
    /// Flow collection syntax.
    Flow,
}

/// YAML scalar spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamlScalarStyle {
    /// Plain scalar syntax.
    Plain,
    /// Single-quoted scalar syntax.
    SingleQuoted,
    /// Double-quoted scalar syntax.
    DoubleQuoted,
    /// Literal block scalar syntax.
    Literal,
    /// Folded block scalar syntax.
    Folded,
}

/// Semantic YAML event kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YamlEventKind {
    /// Start of a YAML stream.
    StreamStart,
    /// End of a YAML stream.
    StreamEnd,
    /// Start of a YAML document.
    DocumentStart {
        /// Whether the source used an explicit `---` marker.
        explicit: bool,
    },
    /// End of a YAML document.
    DocumentEnd {
        /// Whether the source used an explicit `...` marker.
        explicit: bool,
    },
    /// Start of a sequence node.
    SequenceStart {
        /// Sequence spelling style.
        style: CollectionStyle,
        /// Explicit tag, when present.
        tag: Option<String>,
        /// Explicit anchor, when present.
        anchor: Option<String>,
    },
    /// End of a sequence node.
    SequenceEnd,
    /// Start of a mapping node.
    MappingStart {
        /// Mapping spelling style.
        style: CollectionStyle,
        /// Explicit tag, when present.
        tag: Option<String>,
        /// Explicit anchor, when present.
        anchor: Option<String>,
    },
    /// End of a mapping node.
    MappingEnd,
    /// Scalar node with decoded content.
    Scalar {
        /// Scalar spelling style.
        style: YamlScalarStyle,
        /// Decoded scalar value.
        value: String,
        /// Explicit tag, when present.
        tag: Option<String>,
        /// Explicit anchor, when present.
        anchor: Option<String>,
    },
    /// Alias node.
    Alias {
        /// Alias name without the leading `*`.
        name: String,
    },
}

/// Parses a source buffer into a lossless CST node arena.
///
/// # Errors
///
/// Returns an error when the source contains YAML syntax the parser cannot
/// accept or when parser events cannot be produced from the CST.
pub fn parse_cst(source: &Source) -> Result<Vec<Node>, YamlError> {
    Parser::new(source).parse().map(|parsed| parsed.nodes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedYaml {
    pub(crate) nodes: Vec<Node>,
    pub(crate) semantics: SemanticStore,
}
