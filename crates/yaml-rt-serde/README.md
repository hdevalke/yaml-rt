# yaml-rt-serde

[![Crates.io](https://img.shields.io/crates/v/yaml-rt-serde.svg)](https://crates.io/crates/yaml-rt-serde)
[![docs.rs](https://docs.rs/yaml-rt-serde/badge.svg)](https://docs.rs/yaml-rt-serde/latest/yaml_rt_serde/)
[![CI](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml/badge.svg?branch=main&event=push)](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml)

Serde serialization and deserialization backed by yaml-rt's YAML 1.2.2 parser.
Use this crate for typed data conversion or a dynamic YAML value model.
Conversions discard comments, anchors, styles, and original scalar spelling;
use [yaml-rt overlays](https://crates.io/crates/yaml-rt) for lossless editing.

## Installation

Requires Rust 1.96 or newer.

```toml
[dependencies]
yaml-rt-serde = "0.3.2"
serde = { version = "1", features = ["derive"] }
```

The common conversion functions and value types are also available through
`yaml-rt` with its `serde` feature. The free `from_str_with_limits`,
`from_slice_with_limits`, and `from_reader_with_limits` functions require
`yaml-rt-serde` directly; they are not facade re-exports. The corresponding
`Deserializer` constructors are available through either crate.

## Typed conversion

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Service {
    name: String,
    replicas: u16,
}

fn main() -> Result<(), yaml_rt_serde::Error> {
    let service: Service = yaml_rt_serde::from_str("name: api\nreplicas: 3\n")?;
    assert_eq!(service.replicas, 3);
    assert_eq!(yaml_rt_serde::to_string(&service)?, "name: api\nreplicas: 3\n");
    Ok(())
}
```

`from_str`, `from_slice`, and `from_reader` deserialize one YAML document.
Their `_with_limits` counterparts accept custom parser resource limits.
`to_string` and `to_writer` emit deterministic block-style YAML.
`Deserializer` and `Serializer` expose the lower-level Serde adapters.

## Dynamic values

```rust
use yaml_rt_serde::{Value, from_str, from_value, to_value};

fn main() -> Result<(), yaml_rt_serde::Error> {
    let mut value: Value = from_str("service: {name: api, replicas: 2}\n")?;
    value["service"]["replicas"] = Value::from(3);
    assert_eq!(value["service"]["name"], "api");
    let replicas: u16 = from_value(value["service"]["replicas"].clone())?;
    assert_eq!(replicas, 3);
    assert_eq!(to_value(vec![1_u8, 2])?[0], 1);
    Ok(())
}
```

The `yaml_serde`-compatible model includes `Value`, `Number`, `Sequence`,
ordered `Mapping`, indexing, and local tags. `Number` supports `i128` and `u128`.
`to_value` and `from_value` convert typed values without an intermediate YAML
string. Aliases resolve during conversion; `<<` remains an ordinary mapping
key until `Value::apply_merge()` explicitly expands merges.

Enum unit variants use strings; variants with data use local YAML tags.
Presentation preservation requires retaining and editing a `YamlDoc` instead
of serializing converted values.

## Links and license

[API documentation](https://docs.rs/yaml-rt-serde/) ·
[Repository](https://github.com/hdevalke/yaml-rt)

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
