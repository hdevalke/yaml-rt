# yaml-rt-schema

JSON Schema 2020-12 validation and permissive schema inference over yaml-rt
YAML documents. Validation retains instance paths and source spans for useful
diagnostics. Schemas may be written in JSON or YAML.

## Installation

Requires Rust 1.96 or newer. This crate is separate from the `yaml-rt` facade.

```toml
[dependencies]
yaml-rt-core = "0.3.2"
yaml-rt-schema = "0.3.2"
```

## Validate and infer

```rust
use yaml_rt_core::YamlDoc;
use yaml_rt_schema::{Schema, generate_schema};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = Schema::parse(
        "type: object\nproperties:\n  port: {type: integer}\nrequired: [port]\n",
        None,
    )?;
    let doc = YamlDoc::parse("port: 8080 # user choice\n")?;
    schema.validate(&doc, 0)?;
    let inferred = generate_schema(&doc, 0)?;
    assert!(inferred.to_pretty_string().contains("integer"));
    Ok(())
}
```

`Schema::validate` selects a document in a YAML stream. `validate_pointer`
checks one value selected by JSON Pointer while retaining source locations.
`validate_json` accepts the crate's own `Value` model. Errors expose instance
and schema paths and source spans where available.

`generate_schema` infers types, object properties, and array items. Its result
is deliberately permissive: properties remain optional and objects remain open.
It does not infer a complete application contract from one example.

## References and supported behavior

`Schema::from_path` loads a schema with relative file references resolved from
its location. When using `Schema::parse`, pass the schema file path as the
optional base path when local references are needed. `register_resource`
registers an available resource under an absolute URI without network access.
References requiring network fetches fail.

Standard format assertions are opt-in through `with_format_assertions(true)`.
The implementation passes the selected official core and format assertion
cases, plus unknown-format annotation cases. Custom vocabularies and ECMAScript
regex compatibility remain outside the supported behavior. See the detailed
[conformance report](https://github.com/hdevalke/yaml-rt/blob/main/docs/json-schema-conformance.md).

YAML instances must fit the JSON data model: non-string or duplicate mapping
keys, non-finite numbers, and recursive aliases fail validation. Validation
reads the document without rewriting its presentation.

Schema-specific dependencies handle patterns, URI/IRI resolution, and date/time
formats; they do not add dependencies to `yaml-rt-core`.

## Links and license

[API documentation](https://docs.rs/yaml-rt-schema/) ·
[Repository](https://github.com/hdevalke/yaml-rt)

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
