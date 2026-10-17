# yaml-rt-core

Dependency-free YAML 1.2.2 parser and lossless editor. This crate owns the source
model, lexer, concrete syntax tree, semantic graph, diagnostics, editing APIs,
and patch-based emitter. Use it directly when you need the foundational APIs
without derive macros or Serde integration.

## Installation

Requires Rust 1.96 or newer. There are no third-party dependencies.

```toml
[dependencies]
yaml-rt-core = "0.3.2"
```

## Parse and edit

```rust
use yaml_rt_core::{YamlDoc, YamlError};

fn main() -> Result<(), YamlError> {
    let input = "port: 8080 # keep this comment\n";
    let mut doc = YamlDoc::parse(input)?;
    assert_eq!(doc.to_string(), input);
    doc.set_scalar(&["port"], "9090")?;
    assert_eq!(doc.to_string(), "port: 9090 # keep this comment\n");
    doc.commit_edits()?;
    Ok(())
}
```

Untouched YAML emits byte-for-byte. Edits retain unrelated source bytes and aim
for the smallest practical diff. Comments, whitespace, line endings, scalar and
collection styles, directives, tags, anchors, aliases, document markers, and
original spelling are retained where practical. `to_string()` renders pending
patches; `commit_edits()` establishes a fresh parsed baseline.

Queued edits such as `set_scalar` do not update `as_source()`, semantic reads,
or lookups until `commit_edits()` succeeds. `to_string()` includes those pending
edits. Transactional operations such as `replace_at`, patch application, and
sequence edits commit their changes before returning successfully. Reacquire
node handles after a commit or transactional edit because reparsing can change
node IDs.

The public APIs include CST and semantic node handles, RFC 6901 `JsonPointer`,
`YamlFragment`, transactional `YamlPatch` operations with full YAML values,
mapping-key rename, and sequence editing. Syntax and diagnostics carry spans.

## Edit with a JSON Pointer

JSON Pointers address mapping members and sequence elements. Transactional edits
validate the resulting YAML and leave the document unchanged on failure.

```rust
use yaml_rt_core::{JsonPointer, YamlDoc, YamlFragment};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut doc = YamlDoc::parse("services: [{port: 8080}] # keep\n")?;
    let pointer = JsonPointer::parse("/services/0/port")?;
    doc.replace_at(0, &pointer, &YamlFragment::parse("9090")?)?;
    assert_eq!(doc.as_source(), "services: [{port: 9090}] # keep\n");
    let port = doc.resolve_pointer(0, &pointer)?;
    assert_eq!(doc.scalar_value(port)?, "9090");
    Ok(())
}
```

The first argument to `replace_at` and `resolve_pointer` is the zero-based
document index. In a pointer, escape `/` as `~1` and `~` as `~0` in mapping keys;
an empty pointer selects the document root.

## Diagnostics

```rust
use yaml_rt_core::{DiagnosticColor, YamlDoc};

fn main() {
    let input = "items: [a, , b]\n";
    let error = YamlDoc::parse(input).unwrap_err();
    let rendered = error.render(input)
        .with_source_name("config.yaml")
        .with_color(DiagnosticColor::Never)
        .to_string();
    assert!(rendered.contains("--> config.yaml:1:"));
}
```

## Limits and compatibility

`YamlDoc::parse` uses conservative `ResourceLimits`: 64 MiB of source, one
million lines and CST nodes, collection nesting of 1,024 levels, alias chains of
256 jumps, and one million expanded semantic nodes per operation. Customize
these with `YamlDoc::parse_with_limits`. `ResourceLimits::unbounded()` is for
trusted input and still enforces internal `u32` span and node-ID bounds.

The parser is tested against all 402 cases in the YAML Test Suite tag
`data-2022-01-17`, with an empty expected-failure list.

JSON Pointer requires unambiguous string mapping keys. Key rename supports
plain and quoted string keys, but not block-scalar, alias, or complex keys.
Editing an anchored node retains aliases rather than rewriting alias sites.
This is a round-trip editor rather than a canonical formatter.

## Links and license

[API documentation](https://docs.rs/yaml-rt-core/) ·
[Repository](https://github.com/hdevalke/yaml-rt)

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
