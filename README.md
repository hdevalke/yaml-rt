# RTY: a YAML 1.2.2 round-trip parser

RTY is an early-stage Rust workspace for a minimal-dependency YAML round-trip
parser. The target language is YAML 1.2.2, and the parser core should test
against the YAML Test Suite at tag `v2022-01-17`.

YAML syntax is intentionally treated as tricky from the start. Indentation,
flow-vs-block contexts, scalar styles, chomping/folding, directives, tag
handles, anchors, aliases, document markers, comments, and schema resolution all
interact. A round-trip parser must preserve presentation details that ordinary
loaders discard.

## End goal

The workspace is organized around four public crates:

```text
yaml-rt-core      # no dependencies: parser, lossless CST arena, editor
yaml-rt-derive    # derive macros using syn, quote, and proc-macro2
yaml-rt-serde     # typed Serde loading and dumping
yaml-rt           # public facade crate
```

Target usage:

```rust
use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlRoundTrip};

#[derive(YamlRoundTrip)]
struct Config {
    /// Server hostname.
    host: String,

    #[yaml(default = 8080)]
    port: u16,

    #[yaml(rename = "log-level")]
    log_level: String,
}

let mut doc = YamlDoc::parse(input)?;
let mut cfg = Config::from_yaml_doc(&doc)?;

cfg.port = 9090;

cfg.apply_to_yaml_doc(&mut doc)?;
let output = doc.to_string();
```

`yaml-rt` enables its `derive` feature by default, which re-exports
`YamlRoundTrip`. To use only the core facade without proc-macro dependencies,
disable default features:

```toml
yaml-rt = { version = "...", default-features = false }
```

Enable `derive` explicitly when defaults are disabled:

```toml
yaml-rt = { version = "...", default-features = false, features = ["derive"] }
```

Serde integration uses Serde's normal `Serialize` and `Deserialize` derives. It
is available through the facade's off-by-default `serde` feature:

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
yaml-rt = { version = "...", features = ["serde"] }
```

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Config {
    host: String,
    port: u16,
}

let input = "host: localhost\nport: 8080\n";
let config: Config = yaml_rt::from_str(input)?;
let output = yaml_rt::to_string(&config)?;
# Ok::<(), yaml_rt::Error>(())
```

Serde conversion round-trips typed values, not YAML presentation. Comments,
whitespace, scalar style, source spelling, and anchor identity are not present in
Serde's data model. Use `YamlRoundTrip` and retain the `YamlDoc` when those details
must survive an edit.

## Examples

Run examples from the workspace root with `cargo run -p yaml-rt --example NAME`,
or from `crates/yaml-rt` with `cargo run --example NAME`.

```sh
cargo run -p yaml-rt --example typed_config
cargo run -p yaml-rt --example nested_edit
cargo run -p yaml-rt --example multi_document
```

- `typed_config` shows the basic derive workflow: parse YAML, read a typed
  overlay, update fields, insert a defaulted field, and print the minimal patch.
- `nested_edit` updates a nested struct and resizes a block sequence while
  preserving comments, scalar style, unknown fields, and inline comments.
- `multi_document` reads and writes a selected document in a YAML stream without
  changing the other document.

Required guarantees:

- Untouched YAML re-emits byte-for-byte identically, including original line
  endings and trailing spaces.
- Edited YAML produces minimal diffs.
- Comments are preserved.
- Unknown fields are preserved by default.
- Existing scalar style, indentation, key order, line endings, and surrounding
  trivia are preserved where possible.

## Resolved design decisions

- The public API should expose both a mutable concrete syntax tree and a
  higher-level editable document model.
- The first optimization target is editor and incremental-editing use rather
  than batch-only loading.
- The CST is the source of truth. The typed struct is only an overlay that reads
  from and writes patches back to the document.
- Duplicate mapping keys may be preserved by the lossless CST so diagnostics and
  round-trip inspection remain possible, while semantic views and typed loading
  can reject duplicates by default.
- Duplicate-key diagnostics should include both the duplicate key span and the
  previous key span.
- RTY should eventually support deriving typed overlays for arbitrary Rust
  structs through `YamlRoundTrip` or related macros.

## Architecture

```text
Owned Source + u32 line index
  ↓
Indexed-line parser + unified ContextFrame stack
  ↓
Lossless CST arena ← direct SemanticBuilder callbacks
  + sparse span-backed semantic metadata
  ├─→ lazy tokenization
  ├─→ streaming YAML event state machine
  ├─→ typed derive overlays
  └─→ patch-based emitter
```

The CST is the source of truth. Semantic metadata is keyed by the same `NodeId`
used by syntax and stores only information that cannot be recovered cheaply
from CST topology and source spans. There is no second graph node arena.
Parser callbacks populate the compact semantic side arena directly; parsing
does not construct a transient public-event representation.

Lines are random-access views over `Source::line_starts`, so the parser does not
retain a second line arena. A single `ContextFrame` stack carries indentation,
block collection state, pending properties, semantic collection state, and
block-scalar indentation. Ordinary property-free plain mapping entries use a
narrow specialized path whose separator and value offsets are analyzed once;
all decorated or structurally ambiguous forms continue through the full YAML
validators.

### Core data model

```rust
pub struct YamlDoc {
    source: Source,
    nodes: Vec<Node>,
    semantics: SemanticStore,
    edits: Vec<Edit>,
}

pub struct Source {
    text: String,
    line_starts: Vec<u32>,
}

pub struct Span {
    start: u32,
    end: u32,
}

pub struct Token {
    kind: TokenKind,
    span: Span,
}

pub struct Node {
    kind: NodeKind,
    span: Span,
    parent: u32,
    first_child: u32,
    last_child: u32,
    next_sibling: u32,
}
```

The concrete document and node fields are private. Read source and syntax
through `YamlDoc::source`, `YamlDoc::node`, and the `Node` accessors. Lossless
tokens are generated only when requested through `YamlDoc::tokens`, and events
are derived through the `YamlEvents` iterator returned by `YamlDoc::events`.
Ordinary parsing retains neither vector. `YamlDoc::parse_owned` accepts a
`String` without copying its source buffer.

Nodes store spans instead of `&str` slices and child topology uses fixed-size
arena links instead of one `Vec` per node. `documents`, `mapping_entries`,
`sequence_items`, and `children` expose storage-independent iterators. Plain
single-line scalar values return a borrowed `Cow<str>`; decoding allocates only
for quoted, escaped, folded, block, or multiline content.

Each semantic node is a compact record of at most 16 bytes. Tags, anchors,
aliases, and explicit block indentation live in a sparse property arena as
source spans and therefore cost nothing per undecorated scalar. Custom `%TAG`
bindings and anchor indexes are document-local. `YamlDoc::raw_tag`,
`resolved_tag`, `anchor`, `alias_name`, and `resolve_alias` expose those lazy
views without restoring a graph representation.

`YamlEvents` walks CST topology with an explicit depth-first task stack. It
creates owned `YamlEvent` values, resolves properties, and decodes scalar text
one item at a time. Calling `events()` no longer allocates an event-sized vector
or decodes the complete stream up front.

### Public traits

```rust
pub trait FromYamlDoc: Sized {
    fn from_yaml_doc(doc: &YamlDoc) -> Result<Self, YamlError>;
}

pub trait ToYamlDoc {
    fn apply_to_yaml_doc(&self, doc: &mut YamlDoc) -> Result<(), YamlError>;
}

pub trait YamlValue: Sized {
    fn read_yaml(doc: &YamlDoc, node: NodeId) -> Result<Self, YamlError>;
    fn write_yaml(&self, doc: &mut YamlDoc, node: Option<NodeId>) -> Result<NodeId, YamlError>;
}
```

## Current status

- Rust 2024 workspace scaffold.
- `yaml-rt-core` has no dependencies.
- `yaml-rt-derive` is isolated so `syn`, `quote`, and `proc-macro2` do not leak
  into the parser core.
- `yaml-rt` re-exports the core API; its default `derive` feature re-exports
  the `YamlRoundTrip` derive macro and can be disabled for a proc-macro-free
  facade. Its off-by-default `serde` feature re-exports the typed Serde adapter.
- `yaml-rt-serde` provides `from_str`, `from_slice`, `from_reader`,
  `to_string`, and `to_writer` APIs plus multi-document deserialization and
  serialization. It can be used directly or through `yaml-rt`'s `serde` feature
  and does not add dependencies to the core.
- `YamlDoc::parse` validates source characters, builds one lossless CST arena
  with direct, sparse semantic metadata, and preserves byte-identical output
  for untouched YAML. Tokens and events are produced on demand and are never
  retained by ordinary parsing.
- `yaml-rt-core` passes every discovered YAML Test Suite `data-2022-01-17`
  `in.yaml` case for parsing, byte-identical round-trip output, parser event
  rendering, and semantic view construction.

## Milestone plan

### 1. Workspace skeleton

- [x] Create `crates/yaml-rt-core`.
- [x] Create `crates/yaml-rt-derive`.
- [x] Create `crates/yaml-rt`.
- [x] Create `tests/` for integration and conformance tests.
- [x] Re-export `yaml_rt_core::*` and `yaml_rt_derive::YamlRoundTrip` from the
      facade crate.

### 2. Source, span, diagnostics

Implement before parsing:

- [x] `Source::new(String)`.
- [x] `Source::slice(Span) -> &str`.
- [x] `Source::line_col(offset) -> LineCol`.
- [x] `YamlError`.
- [x] `Diagnostic`.
- [x] `ParseError`.
- [x] Accepted-character validation for YAML 1.2.2.
- [x] Diagnostics with span, line, column, expected tokens, and notes.

### 3. Lexer MVP preserving all bytes

Support these initial inputs and tokens:

```yaml
# comment
---
key: value
list:
  - item
quoted: "hello"
single: 'hello'
...
```

Tokens include whitespace, comments, newlines, document markers, scalars, `:`,
`-`, and flow markers. The first lexer invariant is implemented:

```text
tokens_to_string(tokens, source) == source
```

- [x] Tokenize whitespace, comments, and newlines without losing bytes.
- [x] Tokenize document start/end markers.
- [x] Tokenize plain, single-quoted, and double-quoted scalar chunks for the MVP
      subset.
- [x] Tokenize `:`, `-`, `?`, commas, and flow collection markers.
- [x] Report unterminated quoted scalars with lexer diagnostics.
- [x] Assert the source reconstruction invariant in tests.

### 4. Parser MVP for block mappings and sequences

Parse block mappings and block sequences:

```yaml
host: localhost
ports:
  - 8080
  - 9090
```

Produce a lossless CST. The first parser invariant is implemented:

```text
doc.to_string() == input
```

- [x] Build a root stream/document CST arena.
- [x] Parse block mapping lines into `BlockMapping`, `MappingEntry`, and scalar
      nodes.
- [x] Parse block sequence lines into `BlockSequence`, `SequenceEntry`, and
      scalar nodes.
- [x] Represent empty mapping values and bare sequence entries as empty scalar
      nodes.
- [x] Preserve source spans for all MVP CST nodes.
- [x] Keep byte-identical output for parsed MVP documents.
- [x] Reject tabs in indentation with parser diagnostics.

### 5. Strict diagnostics

Add proper errors for:

- [x] Invalid indentation.
- [x] Tabs in indentation.
- [x] Unterminated quote.
- [x] Unexpected token.
- [x] Empty mapping value.
- [x] Invalid document marker.

Diagnostics include spans, line/column positions when a `Source` is available,
expected tokens, and notes.

### 6. Semantic lookup

Add path APIs:

- [x] `doc.root_mapping()?`
- [x] `doc.get_path(&["server", "port"])`
- [x] `doc.get_mapping_entry(mapping, "port")`
- [x] `doc.get_mapping_value(mapping, "port")`
- [x] `doc.scalar_text(node)`
- [x] `doc.semantic_kind(node)`
- [x] `doc.mapping_entries(mapping)` and `doc.sequence_items(sequence)`

Typed parsing can now bind fields to MVP YAML nodes without losing the CST as the
source of truth.

### 7. Patch writer

Implemented for the MVP block-syntax subset:

- [x] `doc.replace_node_text(node, text)` queues exact-span replacement edits.
- [x] `doc.insert_mapping_entry(mapping, key, value, style)` appends plain
      `key: value` entries while inheriting indentation and line endings.
- [x] `doc.insert_mapping_value_with_comment(mapping, key, value, style, comment)`
      inserts typed scalar, sequence, mapping, and nested struct fragments with
      inherited indentation and line endings.
- [x] `doc.remove_node(node)` removes mapping/sequence entry lines and exact
      spans for other nodes.
- [x] `doc.remove_mapping_entry(mapping, key)` removes an existing mapping entry
      line by key and treats missing keys as a no-op.
- [x] `doc.to_string()` renders the original source plus pending patches.
- [x] `doc.commit_edits()` reparses the rendered YAML into a fresh CST and
      semantic metadata, then clears pending edits.

Patch application rule: sort edits from highest offset to lowest offset before
applying them. The writer validates replacement text as YAML-printable text and
rejects overlapping pending edits so minimal patches remain deterministic.
`to_string()` is a non-mutating preview; `commit_edits()` is the validation
boundary for low-level edits that may render invalid YAML.

### 8. Scalar-preserving edits

Implemented for existing scalar nodes in the MVP block-syntax subset:

```yaml
name: "old"
```

Then:

```rust
doc.set_scalar(&["name"], "new")?;
```

produces:

```yaml
name: "new"
```

- [x] Preserve plain vs single-quoted vs double-quoted style for safe
      single-line replacements.
- [x] Preserve indentation, line endings, existing inline comments, surrounding
      comments, and key order by patching only the scalar spelling span.
- [x] Escape replacement text according to the preserved quoted style.
- [x] Reject plain replacements that would need a style change in this MVP
      writer.

### 9. Full YAML surface

Add support incrementally:

- [x] Single-line flow sequences: `[a, b, c]`, including nested flow sequences
      and typed `Vec<T>` reads. Typed writes can replace the existing flow
      sequence span, including generated nested flow collection values.
- [x] Single-line flow mappings: `{a: 1, b: 2}`, including nested flow
      collections and typed `BTreeMap<String, T>` reads. Typed writes can
      replace the existing flow mapping span, including generated nested flow
      collection values.
- [x] Literal scalars: `|`, including strip/clip/keep chomping and typed
      `String` reads and writes.
- [x] Folded scalars: `>`, including strip/clip/keep chomping and typed
      `String` reads and writes.
- [x] Anchors: `&name` on scalars and flow collections are preserved in lazy
      events and semantic metadata. Anchored scalar and collection values can be
      rewritten while preserving the original anchor spelling and spacing.
- [x] Aliases: `*name`.
- [x] Tags: `!tag`, `!!str`, `!<uri>` on scalars and flow collections are
      preserved in events and semantic metadata. `%TAG` directive resolution is
      supported, and tagged scalar and collection values can be rewritten while
      preserving the original tag spelling and spacing.
- [x] Directives: `%YAML` and `%TAG` are parsed before each document. `%TAG`
      handles are resolved into event and semantic tag metadata. Directive
      metadata can be inspected, inserted, updated, and removed before document
      content.
- [x] Multi-document streams: explicit document starts and ends produce
      per-document events and CST document nodes. Document-selection
      APIs can read, look up, and write a selected document by zero-based index,
      and editor APIs can append new explicit stream documents.
- [x] Explicit keys: `? key`.

At this point the parser becomes serious YAML 1.2.2, not only config YAML.

Current editing limits: semantic alias propagation is not attempted for rewritten
anchors, and inserting new documents at arbitrary stream positions is future
work. Canonical semantic re-emission is not a primary conformance target because
it discards the presentation details RTY is designed to preserve.

Compatibility note: `root_mapping`, `get_path`, `FromYamlDoc::from_yaml_doc`,
and `ToYamlDoc::apply_to_yaml_doc` continue to target the first document.
Use `document_count`, `document_root_mapping`, `get_path_in_document`,
`read_document`, and `write_document` for explicit multi-document workflows.

### 10. Manual typed traits

Implemented for the scalar MVP needed before derive macro work:

- [x] `YamlValue for String`.
- [x] `YamlValue for bool`.
- [x] `YamlValue for integers`.
- [x] `YamlValue for floats`.
- [x] Manual `FromYamlDoc` and `ToYamlDoc` implementations for a `Config`
      fixture in core tests.

Typed containers now have MVP support for existing nodes in the block-syntax
subset:

- [x] `YamlValue for Option<T>`.
- [x] `YamlValue for Vec<T>`.
- [x] `YamlValue for BTreeMap<String, T>`.

`Option<T>` reads present nodes as `Some(T)` and removes the containing mapping
or sequence entry when written as `None`. `Vec<T>` reads block and flow sequences,
patches same-length block sequences item-by-item, replaces different-length block
sequences, and can replace single-line flow sequence spans. `BTreeMap<String, T>`
reads block and flow mappings, patches existing block mapping keys in place,
inserts missing block mapping keys, preserves unknown keys by default, and can
replace single-line flow mapping spans. Parent-aware typed insertion now supports
missing scalar, `Vec<T>`, `BTreeMap<String, T>`, and nested struct fields.

The manual `Config` fixture proves that typed overlays can read decoded scalar
values, apply patch-based scalar updates, insert missing fields, and preserve
unknown fields and existing scalar style before derive macro generation starts.

### 11. Derive macro MVP

Implemented for named-field structs whose fields map to same-named root mapping
keys:

```rust
#[derive(YamlRoundTrip)]
struct Config {
    host: String,
    port: u16,
}
```

- [x] Generate `FromYamlDoc` implementations using `YamlValue::read_yaml`.
- [x] Generate `ToYamlDoc` implementations that patch existing fields and append
      missing fields through the typed mapping writer.
- [x] Validate that the MVP derive preserves unknown fields and existing scalar
      style in facade integration tests.

Attribute handling for defaults, comments, renames, aliases, skips, and flattening
now has MVP coverage. Struct-level unknown-field policy now has MVP coverage.
Custom insertion order now has MVP coverage for root mapping insertions, including
missing nested struct fields.

### 12. Derive attributes

Add attributes in this order:

- [x] `#[yaml(rename = "...")]`.
- [x] `#[yaml(default)]`.
- [x] `#[yaml(default = ...)]`.
- [x] `#[yaml(comment = "...")]`.
- [x] `#[yaml(alias = "...")]`.
- [x] `#[yaml(skip)]`.
- [x] `#[yaml(skip_serializing_if = "...")]`.
- [x] `#[yaml(flatten)]`.

`#[yaml(skip)]` currently fills the Rust field with `Default::default()` and
leaves any matching source key untouched during writes, preserving it as
source-owned YAML. `#[yaml(skip_serializing_if = "...")]` reads normally, then
omits missing fields or removes an existing canonical/alias mapping entry when
the predicate returns `true`. `#[yaml(flatten)]` reads and writes a nested
round-trip struct against the same root mapping, so flattened fields preserve
unknown top-level keys and scalar styles through the nested overlay.

Struct-level attributes:

- [x] `#[yaml(preserve_unknown_fields)]`.
- [x] `#[yaml(prune_unknown_fields)]`.
- [x] `#[yaml(insert_order = "append")]`.
- [x] `#[yaml(insert_order = "struct")]`.

Default behavior:

- `preserve_unknown_fields`.
- `insert_order = "append"`.

`#[yaml(insert_order = "append")]` appends missing fields at the end of the
root mapping. `#[yaml(insert_order = "struct")]` inserts a missing field before
the next existing modeled field in Rust declaration order when possible, falling
back to append insertion otherwise. For this milestone, struct-order insertion
cannot be combined with flattened fields because the derive macro cannot yet
statically enumerate flattened keys.

`#[yaml(prune_unknown_fields)]` queues line-wise removal edits for root mapping
entries whose keys are not modeled by the struct or its aliases. The default
`#[yaml(preserve_unknown_fields)]` behavior keeps those entries byte-for-byte.
For this milestone, `prune_unknown_fields` cannot be combined with flattened
fields because the derive macro cannot yet statically enumerate flattened keys.

### 13. Comments from Rust doc comments

Implemented for derive-inserted missing fields in the MVP block mapping writer.

Convert Rust field docs into insertion metadata:

```rust
/// Server port.
port: u16,
```

When inserting a missing field, this may produce:

```yaml
# Server port.
port: 8080
```

Never overwrite existing YAML comments. Comment priority is:

1. Existing YAML comment.
2. `#[yaml(comment = "...")]`.
3. Rust doc comment.
4. None.

The current derive writer applies priorities 2–4 only when inserting missing
fields; existing fields are patched in place so their comments remain untouched.
When appending a commented field to a mapping that already uses blank lines
between entries, the writer preserves that paragraph style by inserting a blank
line before the generated comment.

### 14. Integration test target

Implemented as `crates/yaml-rt/tests/usefulness_target.rs`. Use this as the main
“done enough to be useful” test:

```rust
#[derive(YamlRoundTrip)]
struct Config {
    /// Server hostname.
    host: String,

    /// Server port.
    #[yaml(default = 8080)]
    port: u16,

    /// Enable debug logging.
    #[yaml(default = false)]
    debug: bool,
}
```

Input:

```yaml
# main server
host: "localhost"

# chosen port
port: 3000

extra: keep-me
```

Edit:

```rust
let mut doc = YamlDoc::parse(input)?;
let mut cfg = Config::from_yaml_doc(&doc)?;

cfg.port = 9090;
cfg.debug = true;

cfg.apply_to_yaml_doc(&mut doc)?;
```

Expected output:

```yaml
# main server
host: "localhost"

# chosen port
port: 9090

extra: keep-me

# Enable debug logging.
debug: true
```

### 15. Serde typed conversion

`yaml-rt-serde` is the conventional typed loading and dumping path for types
implementing `serde::Serialize` and `serde::Deserialize`.

- It supports primitives, options, sequences, tuples, mappings, structs,
  newtypes, and YAML-tagged enum variants.
- Plain scalars use the YAML 1.2 core schema. Plain borrowed strings can be read
  directly from string and byte-slice inputs.
- Deserialization follows anchors and aliases with recursion and repetition
  limits. Serialization does not invent anchors because Serde does not expose
  shared object identity.
- A `Deserializer` iterator and reusable `Serializer` support YAML streams with
  multiple documents.
- Dynamic `Value` APIs, merge-key expansion, UTF-16/UTF-32 input, and applying
  arbitrary Serde values as lossless document patches remain future work.

## Implementation order

Start with this exact order:

1. Source/Span/Diagnostics.
2. Lexer preserving all bytes.
3. CST parser for block mappings/sequences.
4. Byte-identical round-trip.
5. Path lookup.
6. Scalar replacement by span patch.
7. Manual typed `Config` implementation.
8. Derive macro MVP.
9. Comments/defaults/rename.
10. Full YAML features.

The fastest path to value is not “complete YAML first.” It is:

```text
lossless subset → editable subset → typed overlay → full YAML completion
```

That gives RTY a usable library early while keeping the final architecture
correct.

## YAML Test Suite strategy

- [x] Add YAML Test Suite as a pinned submodule at
      `third_party/yaml-test-suite` using the `data-2022-01-17` fixture tag
      generated from upstream `v2022-01-17`.
- [x] Support focused valid/invalid parse and byte-identical round-trip checks
      through `crates/yaml-rt-core/tests/yaml_test_suite.rs`.
- [x] Support parser-produced event stream checks for currently accepted cases.
- [x] Build CST-keyed semantic metadata and derive semantic iterators for the
      accepted subset.
- [x] Support JSON-compatible value fixture checks with
      `YAML_TEST_SUITE_CHECK_JSON=1`.
- [ ] Expand round-trip editing conformance with focused fixtures that prove
      edited YAML preserves untouched comments, whitespace, scalar style,
      indentation, line endings, and key order. YAML Test Suite `emit.yaml`
      fixtures are intentionally not a core target because they validate
      canonical semantic emission rather than lossless round-trip behavior.
- [x] Record expected failures while the parser is incomplete and require the
      list to shrink as phases land. The expected-failure list is currently
      empty.
- [x] Classify failures by source validation, lexer, parser, composer, schema,
      typed overlay, or emitter.
- Keep focused unit tests for tricky grammar behavior before relying on broad
  conformance tests.

Initialize the pinned fixture submodule after cloning:

```sh
git submodule update --init --recursive
```

Run a focused subset against the in-repo submodule:

```sh
YAML_TEST_SUITE_CASES=CASE[,CASE...] \
cargo test -p yaml-rt-core --test yaml_test_suite
```

`YAML_TEST_SUITE_DIR=/path/to/yaml-test-suite-data` can override the in-repo
submodule. Set `YAML_TEST_SUITE_RUN_ALL=1` to run every discovered `in.yaml`
case. The full `in.yaml` suite should pass for the pinned data tag; any expected
failure should be temporary and tracked explicitly in the harness.

To update the suite when upstream publishes a new data tag:

```sh
git -C third_party/yaml-test-suite fetch --tags
git -C third_party/yaml-test-suite checkout data-YYYY-MM-DD
git add third_party/yaml-test-suite .gitmodules
```

Use the upstream data tag corresponding to the desired source tag, for example
`data-2022-01-17` for source tag `v2022-01-17`.

## Tricky YAML areas to handle deliberately

- Indentation is semantic but not uniform across contexts.
- Plain scalars are restricted by following characters and collection context.
- `:` and `?` can be indicators or content depending on surrounding characters.
- Block scalar indentation detection, chomping, and folding are easy to get
  subtly wrong.
- Comments are presentation-only, but round-trip editing must retain them.
- Anchors and aliases are document-scoped semantic relationships indexed from
  CST `NodeId`s, not merely syntax spellings.
- Tags can be verbatim, shorthand, local, or resolved via directives.
- YAML supports streams with multiple documents, not only single documents.
- Schema resolution must not erase original scalar spelling.
- JSON compatibility matters, but YAML is more than JSON plus comments.

## Development

```sh
cargo test --workspace
cargo fmt --all -- --check
```
