# Changelog

### v0.2.4 (2026-09-03)

#### Features

* **playground:** add validate command (9246c8d)
* **playground:** improve diagnostics and editor diffs (2d50873)

#### Fixes

* replace_at when replacing a block sequence with a flow-sequence fragment
(4014aec)
* preserve yaml alias diagnostic spans (daf632a)

### v0.2.3 (2026-08-25)

#### Features

* add rustc-style YAML diagnostics (fa60d51)
* **cli:** add short command aliases (1c56a26)
* **cli:** add validate subcommand (839c610)
* **yaml-rt-derive:** support unit and tuple structs (58cf31d)
* **yaml-rt-derive:** support rename_all for struct fields (f68a02c)

#### Fixes

* diagnostic newline clippy warning (85efd11)
* extracted sequence node indentation (a5da1e5)
* sequential YAML patch preservation (db046ea)

### v0.2.2 (2026-08-20)

#### Features

* add WebAssembly command playground (6a64221)

#### Fixes

* **yaml-rt-core:** preserve multiline flow layout on insertion (5b0e10a)
* **yaml-rt-core:** preserve multiline flow layout on removal (b558097)
* **yaml-rt-core:** preserve empty block collections after removal (5f6ab30)
* **yaml-rt-core:** remove compact sequence mapping fields safely (2a307ca)
* **yaml-rt-core:** use fixed-size source scan chunks (777b4da)
* **ci:** enable GitHub Pages during deployment (cd3887e)
* **playground:** copy results on insecure local origins (ff42699)
* **playground:** make the result pane reflect command outcomes (f3e1ea0)
* **yaml-rt-core:** render commented scalars safely in flow collections
(06af88d)
* **yaml-rt-core:** remove complete block collection entries (d93796a)
* **yaml-rt-core:** indent compact mapping insertions correctly (35fe0f0)

### v0.2.1 (2026-08-17)

#### Features

* **yaml-rt-cli:** add mapping key renaming (27122b6)
* **yaml-rt-cli:** recurse into directory (dd842f7)
* add yaml_serde compatible generic `Value` model (595533d)

#### Fixes

* **yaml-rt-cli:** normalize path output (2fc77e0)
* range check conversion for all integer and floating points numbers
(21b2485)

## v0.2.0 (2026-08-15)

### Features

* **derive:** support catch-all map flattening (730ad31)
* **cli:** add JSONPath targeting to single-path commands (869d374)
* add transactional YAML and JSON patch support (17bcf9c)

### v0.1.2 (2026-08-10)

#### Features

* add yaml-rt query command (59760fa)

#### Fixes

* validate numeric conversions (40183f0)
* replace speculative flow parsing with a one-pass state machine (3f4e1fb)
* limit collection depth (21a92d6)

### v0.1.1 (2026-08-03)

#### Features

* **derive:** support tagged enum payload variants (18fed39)
* **derive:** support scalar unit enums and variant naming (610b9ba)
* **derive:** add transparent newtype overlays (794eb75)
* **core:** support typed overlays at arbitrary document roots (44c1140)
* **derive:** add yaml with representation adapters (1e10b95)
* **core:** support common config value shapes (c29ee4b)
* **core:** patch typed flow collections incrementally (d8c001e)

#### Fixes

* **derive:** emit empty enum collection payloads (62df8a6)
* **derive:** support generic named structs (4a4e0cd)
* **core:** make typed scalars and options lossless (3089f56)
* **core:** remove private flow fragment bound (6f7f173)

## v0.1.0 (2026-07-24)

### ⚠ BREAKING CHANGE

* SemanticKind is now a small Copy value and no longer embeds tag, anchor, or alias strings. Use raw_tag, resolved_tag, anchor, alias_name, and resolve_alias on YamlDoc for property and alias views.
* Nested block collections now report their logical MappingEntry or SequenceEntry as the CST parent. Flow sequence values are also wrapped in SequenceEntry nodes. Consumers that inspect concrete CST parents or children must traverse these wrappers.
* scalar_value() now returns Cow<str> so plain source-backed scalars can be borrowed without allocation.
* YamlDoc::events() returns the YamlEvents iterator instead of a borrowed event slice. Callers that need random access must collect it.
* SemanticGraph, GraphNode, GraphNodeId, GraphKind, graph(), graph_node(), graph_node_cst(), get_graph_path(), and document_graph() are removed. Use NodeId with semantic_kind(), documents(), mapping_entries(), sequence_items(), and get_path().
* Node no longer exposes a concrete Vec<NodeId> child layout. Traverse children with YamlDoc::children and parents with Node::parent.
* YamlDoc no longer retains a token vector, parse_cst now accepts only Source, and callers request owned tokens with YamlDoc::tokens.
* YamlDoc and Node storage fields are no longer public. Use YamlDoc and Node accessors instead.


### Features

* **cli:** prepare yaml-rt for distribution (a22bf45)
* **cli:** add yaml-rt command and safe output handling (f48bf01)
* **core:** add lossless pointer-addressed YAML edits (fabf107)
* **core:** implement RFC 6901 and RFC 9512 pointer resolution (09af983)
* **core:** add core-schema scalar resolution and semantic equality (da63d63)
* add serde feature (308a7bf)
* minimal diff block sequence resizing (604278c)
* style preserving edits for nested values (b487a8a)
* nested flow collection replacement (6108bd2)
* stream document creation api (10417ea)
* support nested collection fragments in block sequences (9f6aa37)
* add directive metadata and editing apis (ced5d13)
* preserve anchors and tags when replacing collections (7e2c477)
* add document selection APIs for multi-document streams (a42d89e)
* support anchored and tagged scalar edits (39c51c8)
* harden typed editing and document-commit-flow (ac44533)
* parse tag shorthand and alias semantics (713d17a)
* implemented the flow node properties and implicit collection keys (51c474b)
* flow collection keys and explicit flow entries (943eb90)
* improve block scalar parsing and folding (c719a48)
* decode escape sequences in double-quoted scalars (46eeffc)
* parse explicit keys (db54f1e)
* flow plain-scalar and implicit mapping (c2d5b2b)
* add folding decoding (5f94593)
* parse block collection node properties (36f61cc)
* block plain scalar continuations (a110ca1)
* parse nested block sequence entry (de4baff)
* directives and multi-document stream parsing (4082a3a)
* parse aliases tags and anchors (1163942)
* support empty scalar nodes (19fb08f)
* add event-backed semantic graph (7976908)
* add folded scalar parsing (d75cd09)
* add literal block scalar parsing (35cd3fb)
* add single-line flow collections (e42b5c9)
* initial rty workspace (90e96ff)

### Fixes

* **core:** satisfy current Clippy wildcard lint (36315d4)
* **core:** make packaged tests self-contained (1a411e9)
* **parser:** restrict alias events to plain scalars (7e54f86)
* a fuzz-discovered panic in percent-decoded tag suffix handling (!%sҦ)
(0f60a3a)
* avoiding invalid UTF-8 boundary spans for compact same-line document content
(094352b)
* fuzzed flow mapping key panic (68a0777)
* allow split flow mapping separators (f3fc9b3)
* reject directives after implicit document content (3ac24a8)
* reject tabs that enable nested block structure (cdaf23c)
* reject invalid node property placement (be79ee5)
* reject invalid block scalar forms (cf34fb2)
* reject invalid double quoted scalars (6a9661b)
* reject invalid flow collection forms (bc81045)
* fold explicit key plain scalar continuations (f5e106c)
* preserve block scalar whitespace in events (cdd2133)
* handle quote characters inside scalar content (a202f7c)
* support complex mapping keys in block and flow contexts (10bf1b9)
* directive leniency and marker-like scalar (985da9d)
* block collection attachment (50abc58)
* empty stream/end-only document (33f5712)
* directive leniency and marker-like scalar (598781d)
* reject invalid scalar termination or orphaned block content (d4f588d)
* root indentation and root plain scalar continuation (612172e)
* validate more in the parser (a846409)
* multiline flow with comment character (7aebfcb)
* split line node properties (69c85db)
* handle tab prefixed content (7af7537)
