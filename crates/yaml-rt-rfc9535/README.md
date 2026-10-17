# yaml-rt-rfc9535

[![Crates.io](https://img.shields.io/crates/v/yaml-rt-rfc9535.svg)](https://crates.io/crates/yaml-rt-rfc9535)
[![docs.rs](https://docs.rs/yaml-rt-rfc9535/badge.svg)](https://docs.rs/yaml-rt-rfc9535/latest/yaml_rt_rfc9535/)
[![CI](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml/badge.svg?branch=main&event=push)](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml)

Native RFC 9535 JSONPath parsing and evaluation over yaml-rt documents. Queries
operate on the semantic document without converting it into an intermediate
JSON value tree. Parse a query once and reuse it across documents.

## Installation

Requires Rust 1.96 or newer. This crate is independently usable and is not
re-exported by the `yaml-rt` facade.

```toml
[dependencies]
yaml-rt-core = "0.3.2"
yaml-rt-rfc9535 = "0.3.2"
```

## Query a document

```rust
use yaml_rt_core::YamlDoc;
use yaml_rt_rfc9535::JsonPath;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let doc = YamlDoc::parse("users: [{name: Ada}, {name: Linus}]\n")?;
    let path = JsonPath::parse("$.users[*].name")?;
    let matches = path.query(&doc, 0)?;
    let pointers = matches.iter()
        .map(|matched| matched.pointer().as_str())
        .collect::<Vec<_>>();
    assert_eq!(pointers, ["/users/0/name", "/users/1/name"]);
    Ok(())
}
```

The document index selects one document in a YAML stream. Matches retain
nodelist order and provide node handles and RFC 6901 JSON Pointers for use with
the core document APIs. Queries support selectors, filters, and standard
functions; `regex` supplies the `match()` and `search()` implementations.

## YAML compatibility and errors

Evaluation uses YAML 1.2 core scalar resolution. Before selecting any nodes,
`query` validates the entire selected document against the JSON data model.
Non-string or duplicate mapping keys, non-finite numbers, and unsupported custom
tags anywhere in that document cause an error, even outside the query target.
Alias resolution and resource limits can also produce errors.

This is the crate's strict JSON-compatible input contract;
[RFC 9535 defines queries over valid JSON values](https://www.rfc-editor.org/rfc/rfc9535.html#section-1.3).
For example, `$.port` fails on `port: 8080\nunrelated: .inf\n`.
Use a JSON Pointer for a known path when the rest of the YAML need not fit the
JSON data model:

```rust
use yaml_rt_core::{JsonPointer, YamlDoc};
use yaml_rt_rfc9535::{ErrorKind, JsonPath};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let doc = YamlDoc::parse("port: 8080\nunrelated: .inf\n")?;
    let error = JsonPath::parse("$.port")?.query(&doc, 0).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::DataModel);
    let port = doc.resolve_pointer(0, &JsonPointer::parse("/port")?)?;
    assert_eq!(doc.scalar_value(port)?, "8080");
    Ok(())
}
```

Structured errors distinguish query syntax, typing, regular expressions, data
model compatibility, alias, limit, document, and semantic failures. Query byte
offsets and YAML source spans are included where available.

For shell queries and multi-target edits, use
[yaml-rt-cli](https://crates.io/crates/yaml-rt-cli).

## Links and license

[API documentation](https://docs.rs/yaml-rt-rfc9535/) ·
[Repository](https://github.com/hdevalke/yaml-rt)

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
