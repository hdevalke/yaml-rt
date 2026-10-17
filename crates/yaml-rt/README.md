# yaml-rt

[![Crates.io](https://img.shields.io/crates/v/yaml-rt.svg)](https://crates.io/crates/yaml-rt)
[![docs.rs](https://docs.rs/yaml-rt/badge.svg)](https://docs.rs/yaml-rt/latest/yaml_rt/)
[![CI](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml/badge.svg?branch=main&event=push)](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml)
[![Playground](https://img.shields.io/badge/playground-try_it_online-blue)](https://hdevalke.github.io/yaml-rt/)

YAML 1.2.2 parsing and editing for Rust with byte-identical output for untouched
documents and localized patches for changes. Use this facade for configuration
editors and migration tools that need to preserve comments and formatting.

## Installation

Requires Rust 1.96 or newer.

```toml
[dependencies]
yaml-rt = "0.3.2"
```

| Feature | Default | Effect |
| --- | --- | --- |
| `derive` | Yes | Re-exports the `YamlRt` derive macro |
| `serde` | No | Re-exports Serde conversion APIs and the dynamic `Value` model |

Use `default-features = false` for just the core APIs. Enable `features = ["serde"]`
for conventional serialization and deserialization; those conversions discard
source presentation.

## Lossless editing

```rust
use yaml_rt::{YamlDoc, YamlError};

fn main() -> Result<(), YamlError> {
    let mut doc = YamlDoc::parse("port: 8080 # user choice\n")?;
    doc.set_scalar(&["port"], "9090")?;
    assert_eq!(doc.to_string(), "port: 9090 # user choice\n");
    doc.commit_edits()?;
    Ok(())
}
```

`to_string()` previews pending patches; `commit_edits()` reparses the result as
the baseline for further edits. The lossless CST remains the source of truth.

Queued edits such as `set_scalar` do not update `as_source()`, semantic reads,
or lookups until `commit_edits()` succeeds. `to_string()` includes those pending
edits. Transactional operations such as `replace_at`, patch application, and
sequence edits commit their changes before returning successfully. Reacquire
node handles after a commit or transactional edit because reparsing can change
node IDs.
Core APIs also include JSON Pointer lookup, fragments, transactional YAML/JSON
patches, sequence editing, and source-aware diagnostics.

## Typed overlays

```rust
use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlError, YamlRt};

#[derive(YamlRt)]
struct Config {
    port: u16,
}

fn main() -> Result<(), YamlError> {
    let mut doc = YamlDoc::parse("port: 8080 # user choice\nextra: keep\n")?;
    let mut config = Config::from_yaml_doc(&doc)?;
    config.port = 9090;
    config.apply_to_yaml_doc(&mut doc)?;
    assert_eq!(doc.to_string(), "port: 9090 # user choice\nextra: keep\n");
    Ok(())
}
```

Overlays preserve unknown struct fields by default and patch modeled values in
place. They support nested structs, wrappers, collections, tuple structs, and
locally tagged enums. See the
[derive guide](https://github.com/hdevalke/yaml-rt/tree/main/crates/yaml-rt-derive#readme)
for attributes and representation details.

Sequence matching is positional, typed mapping keys are strings, and borrowed
overlay fields are unsupported. This editor does not canonicalize YAML or
propagate an anchor edit into copies at alias sites. Parser resource limits are
enabled by default; configure `ResourceLimits` for your input requirements.

JSONPath and JSON Schema are available from separate crates rather than facade
re-exports. For command-line editing, install
[yaml-rt-cli](https://crates.io/crates/yaml-rt-cli).

## Links and license

[API documentation](https://docs.rs/yaml-rt/) ·
[Repository](https://github.com/hdevalke/yaml-rt)

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
