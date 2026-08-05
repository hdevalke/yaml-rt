# Architecture

yaml-rt separates lossless syntax, semantic interpretation, editing, and typed
data so that no derived representation needs to reconstruct the user's source.
The concrete syntax tree (CST) and original source are always authoritative.

## Workspace boundaries

- `yaml-rt-core` owns source storage, spans, lexing, parsing, the CST, semantic
  metadata, diagnostics, RFC 6901 JSON Pointer lookup, edit operations,
  typed-overlay traits, and patch emission. It intentionally has no third-party
  dependencies.
- `yaml-rt-rfc9535` owns native JSONPath parsing, evaluation, regex functions,
  strict JSON-data-model validation, and located pointer construction. It runs
  directly against core semantic nodes and depends on `regex` only for the
  standard `match()` and `search()` functions.
- `yaml-rt-derive` generates typed round-trip overlay implementations. Its
  procedural macro dependencies do not enter the core parser.
- `yaml-rt-serde` converts between YAML and Serde data models when presentation
  preservation is not required.
- `yaml-rt` is the public facade and feature switchboard.
- `yaml-rt-cli` orchestrates JSONPath queries, renders compact JSON values, and
  exposes patch-oriented editing through JSON Pointer operations.
- `yaml-rt-bench` and the separate `fuzz` workspace are development-only.

## Parse and edit flow

```text
source bytes
    |
    v
lossless tokens --> lossless CST <--- semantic node metadata
                         |
                         +--- typed overlay reads
                         |
edit request ------------+---> ordered source patches
                                      |
                                      v
                              minimally changed YAML
```

### Source and spans

`Source` owns UTF-8 text and its line-start index. The document editor derives
the preferred line-ending style from that source when it needs to insert new
lines. Syntax nodes and diagnostics use byte `Span`s and stable `NodeId`s
instead of borrowing substrings. This keeps the public model lifetime-light and
makes diagnostics and edits refer to exact user-visible syntax.

### Lossless syntax

The lexer retains comments, whitespace, directives, scalar spelling, collection
punctuation, and document markers. The parser builds a CST whose node spans
cover source regions without normalizing them. Rendering an unedited
`YamlDoc` therefore returns the original bytes.

### Semantic metadata

Composition records document roots, mapping entries, sequence items, tags,
anchors, aliases, and resolved scalar kinds alongside the CST. It does not
replace the syntax tree. Schema resolution and JSON Pointer lookup use this
metadata while emission continues to use source spans.

### Editing and emission

Editor methods queue non-overlapping replacements, insertions, and removals.
The emitter applies the ordered patches to the original source. Unaffected
ranges are copied verbatim. `commit_edits` renders and reparses the result,
making the new document a clean baseline and validating the produced YAML.

Collection-aware operations derive indentation, line endings, and surrounding
presentation from their parent syntax. When preserving semantics is uncertain,
the API reports an error rather than performing a lossy rewrite.

### Typed overlays

`FromYamlDoc` reads fields from the semantic view while leaving the document
owned by the caller. `ToYamlDoc` compares and writes field values through the
editor API. The derive crate adds field aliases, defaults, comments, flattening,
unknown-field policy, insertion-order policy, transparent newtypes, and locally
tagged enums. Same-variant enum payloads use the existing scalar, sequence, and
mapping editors; variant changes replace only the selected node. Unknown source
entries remain untouched by default.

Serde conversion is intentionally separate. It represents typed YAML data but
does not promise round-trip presentation preservation.

### Streams and documents

A `YamlDoc` may contain a YAML stream with multiple documents. Document-aware
APIs select roots by zero-based index. Directives and explicit markers remain
part of the lossless syntax, while semantic operations are scoped to the
selected document.

## Conformance and regression strategy

The parser is checked against the pinned YAML Test Suite tag
`data-2022-01-17`. The harness verifies parse acceptance or rejection, event
streams, emitted output, and semantic JSON fixtures where available. Focused
unit tests cover grammar and editing behavior, package tests verify the core
archive without the submodule, and fuzzing exercises parser entry points with
the suite as seed material.
