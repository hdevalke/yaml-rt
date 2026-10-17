# yaml-rt-wasm

[![CI](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml/badge.svg?branch=main&event=push)](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml)
[![Playground](https://img.shields.io/badge/playground-try_it_online-blue)](https://hdevalke.github.io/yaml-rt/)

Filesystem-free command engine and WebAssembly bindings for the
[yaml-rt browser playground](https://hdevalke.github.io/yaml-rt/).
This workspace crate is unpublished (`publish = false`). Documents are processed
locally in the browser without uploading them to a server.

## Native command engine

The Rust API accepts a `CommandRequest` and returns a `CommandResult` through
`execute`. Requests carry source text, the document index, command selectors,
values or patches, and optional input/output schemas. Results include edited
YAML, command output, matched pointers, validation results, and diagnostics.

For a local Rust consumer, use a path dependency (adjust to your checkout):

```toml
[dependencies]
yaml-rt-wasm = { path = "../yaml-rt/crates/yaml-rt-wasm" }
```

```rust
use yaml_rt_wasm::{CommandRequest, execute};

fn main() {
    let result = execute(&CommandRequest {
        source: "port: 8080 # keep\n".to_owned(),
        command: "replace".to_owned(),
        selector_kind: Some("pointer".to_owned()),
        selector: Some("/port".to_owned()),
        value: Some("9090".to_owned()),
        ..Default::default()
    });
    assert!(result.ok, "{:?}", result.message);
    assert_eq!(result.output_yaml, "port: 9090 # keep\n");
}
```

The engine supports query, get, editing, transactional patches, validation, and
schema generation. Input schema failures do not stop repair commands. Output
schemas check the edited document or returned values. `export_snapshot` and
`import_snapshot` support portable playground scenarios.

## Build browser bindings

From the repository root, with Rust 1.96 or newer and `wasm-pack` 0.15.0:

```sh
rustup target add wasm32-unknown-unknown
wasm-pack build crates/yaml-rt-wasm --release --target web --out-dir ../../playground/pkg
```

This produces JavaScript bindings and the WebAssembly binary in `playground/pkg`.
The `run_command` export adapts browser arguments to the native engine. See the
[playground source](https://github.com/hdevalke/yaml-rt/tree/main/playground)
for integration and the
[Pages workflow](https://github.com/hdevalke/yaml-rt/blob/main/.github/workflows/pages.yml)
for the full editor build and deployment process.

Run native engine tests from the repository root:

```sh
cargo test -p yaml-rt-wasm --locked
```

Browser schemas are supplied as text; references requiring separate files or
network access are unavailable. JSONPath and schema checks require
JSON-compatible YAML, and the core parser's resource limits remain enabled.

## Links and license

[API documentation](https://github.com/hdevalke/yaml-rt/tree/main/crates/yaml-rt-wasm/src) ·
[Repository](https://github.com/hdevalke/yaml-rt)

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
