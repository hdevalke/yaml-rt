# yaml-rt-derive

[![Crates.io](https://img.shields.io/crates/v/yaml-rt-derive.svg)](https://crates.io/crates/yaml-rt-derive)
[![docs.rs](https://docs.rs/yaml-rt-derive/badge.svg)](https://docs.rs/yaml-rt-derive/latest/yaml_rt_derive/)
[![CI](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml/badge.svg?branch=main&event=push)](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml)

The `YamlRt` procedural derive macro for typed, presentation-preserving overlays
on YAML documents. Rust values read from and patch the lossless document; they
do not replace it as the source of truth.

## Installation and usage

Use the macro through `yaml-rt`, which enables the `derive` feature by default
and provides the runtime traits required by generated code. Requires Rust 1.96
or newer.

```toml
[dependencies]
yaml-rt = "0.3.2"
```

```rust
use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlError, YamlRt};

#[derive(YamlRt)]
struct Config {
    #[yaml(rename = "http-port", default = 8080)]
    port: u16,
}

fn main() -> Result<(), YamlError> {
    let mut doc = YamlDoc::parse("http-port: 3000 # keep\nextra: retained\n")?;
    let mut config = Config::from_yaml_doc(&doc)?;
    config.port = 9090;
    config.apply_to_yaml_doc(&mut doc)?;
    assert_eq!(doc.to_string(), "http-port: 9090 # keep\nextra: retained\n");
    Ok(())
}
```

## Models and attributes

Named structs use mappings; single-field tuple structs are transparent;
multi-field tuple structs use fixed-length sequences; unit structs use null.
Unit enum variants use strings and data variants use local YAML tags.
Nested derived models, generics, scalar types, `Option`, `Box`, fixed arrays,
`Vec`, and string-keyed `BTreeMap`/`HashMap` fields are supported.

Field attributes include `rename`, repeated `alias`, `default` or
`default = expression`, `comment`, `skip`, `skip_serializing_if`, `flatten`,
and `with`. Rust doc comments supply comments for newly inserted fields unless
an explicit `comment` is given.

Struct policies include `preserve_unknown_fields` (the default),
`prune_unknown_fields`, and `insert_order = "append"` or `"struct"`.
`rename_all` supports `lowercase`, `snake_case`, `kebab-case`,
`SCREAMING_SNAKE_CASE`, `camelCase`, and `PascalCase`; explicit renames win.
Enum variants accept `rename` and repeated `alias`; named-field variants also
accept `rename_all` for payload keys.

`flatten` supports nested derived structs and one catch-all string-keyed map
across the recursively flattened model. Catch-all maps synchronize their entries
exactly and reject collisions with modeled keys.

`flatten` cannot be combined on the same field with `rename`, `alias`, `default`,
`comment`, `skip`, `skip_serializing_if`, or `with`. Structs containing flattened
fields cannot use `prune_unknown_fields` or `insert_order = "struct"`.
These combinations produce derive errors.

A `with = "module"` adapter defines `Repr`,
`from_yaml(Repr) -> Result<T, YamlError>`, and
`to_yaml(&T) -> Result<Repr, YamlError>`.
`Repr` implements `YamlValue + ToYamlFragment`.

## Preservation and limitations

Unchanged scalars retain their spelling; existing collection styles, comments,
and unknown struct fields survive localized updates. Sequences match by position,
and typed maps synchronize modeled keys exactly. Aliases for field and variant
names are accepted when reading; existing variant alias spelling survives a
same-variant update.

Borrowed overlay fields and full Serde attribute compatibility are unsupported.
Enums do not support internally tagged, adjacently tagged, externally mapped,
or untagged representations. See the
[configuration examples](https://github.com/hdevalke/yaml-rt/tree/main/crates/yaml-rt/examples)
for adapters, nested editing, and enum models.

## Links and license

[API documentation](https://docs.rs/yaml-rt-derive/) ·
[Repository](https://github.com/hdevalke/yaml-rt)

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
