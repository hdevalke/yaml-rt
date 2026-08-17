# yaml-rt

`yaml-rt` is a YAML 1.2.2 parser and editor for Rust that keeps the original
source text intact. It is designed for tools that need to change YAML without
reformatting everything around the change: comments, whitespace, line endings,
scalar styles, directives, tags, anchors, aliases, document markers, and
original spelling are retained where practical.

Untouched documents are emitted byte-for-byte. Edits are applied as localized
source patches, so a changed value normally produces only the diff you asked
for.

Version 0.1 is suitable for production use within the guarantees and
limitations documented below. Public APIs may still evolve between 0.x
releases.

## When to use yaml-rt

Use `yaml-rt` when you are building a configuration editor, migration tool,
formatter-aware automation, or command-line utility where preserving a human's
YAML matters. For ordinary typed data interchange where presentation does not
matter, the optional Serde integration provides a more conventional conversion
API.

## Installation

The facade crate enables typed-overlay derives by default:

```toml
[dependencies]
yaml-rt = "0.1.0"
```

Choose features explicitly when needed:

```toml
[dependencies]
yaml-rt = { version = "0.1.0", default-features = false }
# or
yaml-rt = { version = "0.1.0", features = ["serde"] }
```

Install the command-line editor with:

```sh
cargo install yaml-rt-cli --locked
```

The installed binary is named `yaml-rt`.

## Lossless editing

Parse a document, queue an edit, and render the minimally changed source:

```rust
use yaml_rt::{YamlDoc, YamlError};

fn main() -> Result<(), YamlError> {
    let input = "\
# local development
server:
  host: \"localhost\"
  port: 8080 # keep this comment
";
    let mut doc = YamlDoc::parse(input)?;
    doc.set_scalar(&["server", "port"], "9090")?;

    assert_eq!(
        doc.to_string(),
        "\
# local development
server:
  host: \"localhost\"
  port: 9090 # keep this comment
"
    );
    Ok(())
}
```

`YamlDoc::to_string()` previews the source with pending patches applied.
`YamlDoc::commit_edits()` reparses that result and makes it the new baseline for
subsequent edits.

The lower-level API exposes the lossless concrete syntax tree, semantic node
metadata, JSON Pointer operations, fragments, diagnostics with spans, and
patch-oriented editing primitives.

## Typed round-trip overlays

`YamlRt` maps Rust configuration models onto YAML without turning the
Rust value into the source of truth. Reading does not discard syntax, and
writing patches only modeled values unless configured otherwise.

```rust
use yaml_rt::{FromYamlDoc, ToYamlDoc, YamlDoc, YamlError, YamlRt};

#[derive(Debug, PartialEq, Eq, YamlRt)]
struct Config {
    host: String,

    #[yaml(default = 8080)]
    port: u16,

    #[yaml(rename = "log-level")]
    log_level: String,
}

fn main() -> Result<(), YamlError> {
    let input = "\
host: \"localhost\"
port: 3000 # selected by the user
log-level: info
extra: keep-me
";
    let mut doc = YamlDoc::parse(input)?;
    let mut config = Config::from_yaml_doc(&doc)?;

    config.port = 9090;
    config.apply_to_yaml_doc(&mut doc)?;

    assert_eq!(
        doc.to_string(),
        "\
host: \"localhost\"
port: 9090 # selected by the user
log-level: info
extra: keep-me
"
    );
    Ok(())
}
```

Supported field attributes are:

- `rename = "yaml-key"`
- `alias = "legacy-key"`
- `default` and `default = expression`
- `comment = "Comment for inserted entries."`
- `skip`
- `skip_serializing_if = "path::to::predicate"`
- `flatten`
- `with = "module::path"`

Rust doc comments become comments for newly inserted entries when no explicit
`comment` attribute is present. Struct-level policies control unknown fields
with `preserve_unknown_fields` or `prune_unknown_fields`, and insertion order
with `insert_order = "append"` or `insert_order = "struct"`.

`flatten` accepts both nested derived structs and one catch-all
`BTreeMap<String, T>` or `HashMap<String, T>` across the recursively flattened
field graph. The catch-all map reads every entry not claimed by a canonical
field name, alias, skipped field, or nested flattened struct. Applying the
overlay synchronizes those entries exactly: existing entries are patched in
place, missing map keys are removed from YAML, and new keys are inserted
deterministically. A catch-all entry that collides with a modeled key is
rejected before edits are queued.

Single-field tuple structs are transparent automatically. Their inner value is
represented directly, so `struct Port(u16)` reads and writes `8080`, not a
mapping or tag. The unnamed field may use `yaml(with = "module")`.

Enums use the same YAML representation as `yaml-rt-serde`: unit variants are
strings, and variants with data use local tags.

```rust
# use yaml_rt::YamlRt;
#[derive(YamlRt)]
#[yaml(rename_all = "lowercase")]
enum Mode {
    A,                          // a
    Value(u16),                 // !value 42
    Pair(u8, bool),             // !pair [1, true]
    Server { host: String },    // !server {host: api}
}
```

Enum variants support `rename` and repeated `alias`. Enum-level `rename_all`
accepts `lowercase`, `snake_case`, `kebab-case`, `SCREAMING_SNAKE_CASE`,
`camelCase`, and `PascalCase`, following Serde's variant transformations.
Aliases are accepted when reading; new values emit the canonical name. An
existing alias spelling is retained while the variant stays the same.

Same-variant writes patch newtype payloads, tuple elements, and struct fields
incrementally. That retains scalar spelling, collection style, comments,
anchors, and unknown struct-variant fields. Switching variants replaces the
enum node deterministically, retains its anchor and surrounding entry comment,
and removes comments owned by the old payload.

Common configuration shapes are supported directly:

| Shape | Coverage |
| --- | --- |
| Scalars | `String`, `bool`, `char`, all signed and unsigned integer widths including 128-bit values, `f32`, and `f64` |
| Wrappers | `Option<T>`, `Box<T>`, and fixed arrays `[T; N]` |
| Collections | `Vec<T>`, `BTreeMap<String, T>`, and `HashMap<String, T>` |
| Structs | Named mapping structs and transparent single-field tuple structs |
| Enums | Unit, newtype, tuple, and named-field variants; data variants use local YAML tags |
| Nested models | Derived structs, newtypes, and enums inside the wrappers and collections above |
| Generics | Type, lifetime, and const generics with inferred field bounds and retained user `where` clauses |
| Document roots | Typed scalar, sequence, mapping, newtype, and enum roots |
| Presentation | Existing block and flow collections are patched in their original style |

Sequence elements are matched positionally. Existing elements retain comments,
unknown nested keys, anchors, tags, spelling, and style; appended elements use
deterministic formatting. Typed maps synchronize modeled keys exactly while a
derived struct still preserves unknown fields by default. A semantically
unchanged scalar is not rewritten, so spellings such as `TRUE` and `0x10`
survive. Missing and explicit-null `Option<T>` fields both read as `None`;
writing `None` emits `null` unless `skip_serializing_if` omits the field.

Use `with` when a field has a configuration representation different from its
Rust type. The adapter module declares a supported YAML representation and two
conversions:

```rust
use std::time::Duration;
use yaml_rt::YamlError;

mod duration_seconds {
    use super::{Duration, YamlError};

    pub type Repr = u64;

    pub fn from_yaml(value: Repr) -> Result<Duration, YamlError> {
        Ok(Duration::from_secs(value))
    }

    pub fn to_yaml(value: &Duration) -> Result<Repr, YamlError> {
        Ok(value.as_secs())
    }
}

# use yaml_rt::YamlRt;
#[derive(YamlRt)]
struct Service {
    #[yaml(with = "duration_seconds", rename = "timeout-seconds")]
    timeout: Duration,
}
```

`Repr` must implement `YamlValue + ToYamlFragment` and can be a scalar or a
collection. `rename`, `alias`, `default`, `comment`, and
`skip_serializing_if` continue to apply around a named-field adapter. Adapters
also work on transparent newtype fields and unnamed enum payload fields. See the
[`config_models` example](crates/yaml-rt/examples/config_models.rs) for nested
struct sequences, a flow-style update, optional/null fields, generics, and the
duration adapter together, and the
[`enum_overlays` example](crates/yaml-rt/examples/enum_overlays.rs) for
transparent newtypes and tagged enums.

`YamlRt` is a lossless overlay, not a general-purpose Serde data-model
implementation: it reads from an existing `YamlDoc` and applies localized
patches back to that same source. Serde conversion creates ordinary Rust
values and deterministic YAML when retaining the original presentation is not
required.

## Serde conversion

Enable the `serde` feature when source presentation does not need to survive a
typed conversion:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Service {
    name: String,
    replicas: u16,
}

fn main() -> Result<(), yaml_rt::Error> {
    let service: Service = yaml_rt::from_str("name: api\nreplicas: 3\n")?;
    assert_eq!(
        service,
        Service {
            name: "api".to_owned(),
            replicas: 3,
        }
    );

    assert_eq!(
        yaml_rt::to_string(&service)?,
        "name: api\nreplicas: 3\n"
    );
    Ok(())
}
```

For dynamic YAML, the same feature exposes a `yaml_serde`-compatible value
model with ordered mappings, local tags, indexing, typed conversion, and
explicit merge-key expansion:

```rust
use yaml_rt::{Value, from_str, from_value, to_value};

# fn main() -> Result<(), yaml_rt::Error> {
let mut value: Value = from_str("service: {name: api, replicas: 2}\n")?;
value["service"]["replicas"] = Value::from(3);
assert_eq!(value["service"]["name"], "api");

let replicas: u16 = from_value(value["service"]["replicas"].clone())?;
assert_eq!(replicas, 3);
assert_eq!(to_value(vec![1_u8, 2])?[0], 1);
# Ok(())
# }
```

`Value`, `Number`, `Sequence`, `Mapping`, `Index`, `to_value`, and
`from_value` are available from `yaml-rt-serde` directly and through the
facade. `Number` additionally retains `i128` and `u128` values. Value
conversion resolves aliases and intentionally discards comments, anchors,
styles, and original scalar spelling. YAML `<<` keys remain ordinary mapping
entries until `Value::apply_merge()` is called.

Serde serialization emits deterministic block-style YAML. Use a typed
round-trip overlay instead when comments, quoting, whitespace, or other source
presentation must be retained.

## Command-line query and editor

The CLI queries YAML with RFC 9535 JSONPath and applies JSON Pointer operations
while preserving the rest of the document:

```sh
# Find every service port and print JSON Pointer/value pairs.
yaml-rt query '$.services[*].port' config.yaml

# Read a node.
yaml-rt get /server/port config.yaml

# Read every matching node as a YAML document stream.
yaml-rt get --query '$.services[*].port' config.yaml

# Replace a value and print the edited document.
yaml-rt replace /server/port --value 9090 config.yaml

# Replace every node selected by JSONPath.
yaml-rt replace --query '$.services[*].port' --value 9090 config.yaml

# Edit a file atomically in place.
yaml-rt add /server/debug --value true --in-place config.yaml

# Copy a complete YAML node from a file.
yaml-rt add /server/tls --value-file tls.yaml config.yaml

# Select the second document in a YAML stream.
yaml-rt get /name --doc 1 stream.yaml

# Apply several changes transactionally from YAML or JSON.
yaml-rt patch --patch-file changes.yaml --in-place config.yaml

# Recursively edit every .yaml or .yml file under a directory.
yaml-rt replace /server/port --value 9090 --in-place configs/
```

Available operations are `query`, `get`, `add`, `remove`, `replace`, `move`,
`copy`, `test`, and `patch`. Query results are emitted in nodelist order as one
compact JSON Pointer/value pair per line. JSONPath evaluation uses the YAML 1.2
core schema and rejects YAML values that are not JSON-compatible, including
non-string or duplicate mapping keys and non-finite numbers.

`get`, `add`, `remove`, `replace`, and `test` also accept `--query QUERY` in
place of their positional JSON Pointer. `get --query` emits each match as a
separate `---` YAML document in nodelist order. Query-targeted mutations apply
to all matched nodes transactionally; duplicate targets are edited once, and a
selected ancestor supersedes its selected descendants. A mutation or `test`
fails when the query matches nothing, while `get` succeeds with empty output.

`patch` accepts an RFC 6902-style operation sequence through either
`--patch YAML` or `--patch-file FILE`. JSON patch documents are valid because
JSON is a subset of YAML; YAML syntax additionally permits complete YAML nodes
as operation values. Operations run in order against the selected document,
and the entire patch is rolled back if any operation or `test` fails.

An input may be a single file, a directory, or `-` for standard input. Omitting
the input scans the current directory. Directory inputs are searched
recursively for `.yaml` and `.yml` files using case-insensitive extensions;
hidden paths are included, symbolic links are skipped, and files are processed
in sorted relative-path order. A directory with no matching files succeeds
without output.

Batch `query` and `get --query` output identifies each input containing a match
with an `==> relative/path.yaml <==` header and a blank line between files;
files without matches produce no section. Pointer-based batch `get` identifies
every successful input. The `--output` option writes the same combined stream
to one file. Batch mutations
require `--in-place`; each file is replaced atomically only after its operation
succeeds. Failures are reported with relative paths, processing continues for
the remaining files, and the command exits unsuccessfully if any file or
directory traversal failed.

Single-file mutations write to standard output unless `--output` or
`--in-place` is used. Values passed with `--value` or `--value-file` must be
complete YAML nodes. `--patch-file -` reads the patch from standard input only
when the target YAML does not also use `-`; both inputs cannot use standard
input at once.

Run `yaml-rt help <operation>` for operation-specific arguments.

## Crates and features

| Package | Purpose | Published |
| --- | --- | --- |
| `yaml-rt-core` | Dependency-free source model, parser, CST, semantic graph, diagnostics, editor, and emitter | Yes |
| `yaml-rt-rfc9535` | Native RFC 9535 JSONPath parsing and evaluation over `YamlDoc` | Yes |
| `yaml-rt-derive` | `YamlRt` procedural derive | Yes |
| `yaml-rt-serde` | Serde serializer and deserializer | Yes |
| `yaml-rt` | Facade re-exporting the public APIs | Yes |
| `yaml-rt-cli` | `yaml-rt` command-line editor | Yes |
| `yaml-rt-bench` | Local comparison benchmarks | No |
| `fuzz` | Separate cargo-fuzz workspace | No |

The facade features are:

| Feature | Default | Effect |
| --- | --- | --- |
| `derive` | Yes | Re-exports `yaml_rt_derive::YamlRt` |
| `serde` | No | Re-exports the `yaml-rt-serde` conversion API |

`yaml-rt-core` has no third-party dependencies and retains the foundational RFC
6901 `JsonPointer` API. Its `YamlPatch` and `YamlPatchOperation` APIs parse and
transactionally apply RFC 6902-style operation sequences with full YAML values.
The independently usable `yaml-rt-rfc9535` crate depends on `regex` for the
standard `match()` and `search()` functions. Its JSONPath parser, semantic
evaluator, compatibility validator, and pointer construction operate directly
on `YamlDoc` without a generic JSON value dependency. Compact JSON rendering
remains private to `yaml-rt-cli`; the RFC 9535 crate is not re-exported by the
`yaml-rt` facade.

## YAML 1.2.2 conformance

The parser is tested against all 402 cases in the YAML Test Suite tag
`data-2022-01-17`, including semantic JSON comparisons where fixtures provide
them. The expected-failure list is empty.

The conformance harness is opt-in for ordinary package tests. A complete local
run uses the pinned submodule:

```sh
YAML_TEST_SUITE_RUN_ALL=1 \
YAML_TEST_SUITE_CHECK_JSON=1 \
cargo test -p yaml-rt-core --test yaml_test_suite
```

## Guarantees

- Parsing and emitting an untouched valid document is byte-identical.
- Local edits retain unrelated source bytes and aim for the smallest practical
  diff.
- Batch patches are transactional; a failing operation leaves the target
  document unchanged.
- User-visible syntax and diagnostics carry source spans.
- YAML streams, directives, tags, anchors, aliases, explicit document markers,
  collection styles, scalar styles, comments, whitespace, and line endings are
  represented by the lossless model.
- Typed round-trip overlays preserve unknown fields by default.

## Current limitations

- This is a round-trip editor, not a canonical YAML formatter.
- Editing an anchored node does not propagate the edit through aliases; aliases
  continue to refer to the same anchor in the emitted YAML.
- CLI `copy` and `move` reject cases where anchor ownership would become
  ambiguous or invalid.
- JSON Pointer lookup reports duplicate mapping keys and cannot address
  non-string mapping keys.
- CLI and patch `test` operations compare YAML values using the supported YAML
  1.2 core scalar and collection model; they are not a general tag-aware
  application schema.
- Serde `Value` conversion does not preserve presentation metadata and only
  expands merge keys when `Value::apply_merge()` is requested.
- Typed-overlay `flatten` has intentionally conservative combinations with
  field and struct policies; unsupported combinations produce derive errors.
- Typed mappings use string keys. Borrowed overlay fields, standalone unit
  structs, standalone multi-field tuple structs, and full Serde attribute
  compatibility remain future work.
- Enum data variants use local tags only. Internally tagged, adjacently tagged,
  externally mapped, and untagged enum representations are not implemented.
- Collection identity is positional for sequences. Mapping insertion accepts
  string keys only; newly inserted `HashMap` keys are sorted for deterministic
  output while existing source order is retained.
- Flow collections may be nested up to 1,024 levels. Deeper input is rejected
  with a span-aware parser diagnostic.

These constraints are checked rather than silently producing lossy or
surprising output.

## Stability

The 0.1 line is production-usable for the documented behavior. Patch releases
will preserve source and semantic behavior unless fixing a correctness or
safety issue. Because the project is pre-1.0, public Rust APIs and CLI details
may change in minor releases; such changes are documented in the changelog and
follow Conventional Commits.

## Architecture

The lossless CST remains the source of truth. Semantic information and typed
Rust values are overlays that read from it and queue source patches. See
[`docs/architecture.md`](docs/architecture.md) for the component boundaries and
data flow. Block parsing advances a sequential line cursor over compact entry
and collection frames in one iterative state machine. Flow collections are
parsed left-to-right with a bounded explicit frame stack. Both machines
register semantic metadata directly in CST nodes during recognition, so deeply
nested input does not rely on call-stack recursion, no completed-CST semantic
pass is required, and each CST node is attached in its grammatical context
exactly once.

## Roadmap

Near-term work focuses on expanding ergonomic edit operations, richer
schema-aware scalar handling, broader typed-overlay shapes, sustained fuzzing,
and performance profiling while retaining zero dependencies in the core crate.

## Development

The workspace requires Rust 1.96 or newer and tests the latest stable toolchain.
Useful release-readiness commands are:

```sh
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy \
  -p yaml-rt-core -p yaml-rt-derive -p yaml-rt-serde \
  -p yaml-rt -p yaml-rt-cli \
  --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" \
  cargo doc --workspace --all-features --no-deps
```

See [`RELEASING.md`](RELEASING.md) for the publication process.

## License

Licensed under either the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.
