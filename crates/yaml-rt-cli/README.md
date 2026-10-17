# yaml-rt-cli

[![Crates.io](https://img.shields.io/crates/v/yaml-rt-cli.svg)](https://crates.io/crates/yaml-rt-cli)
[![docs.rs](https://docs.rs/yaml-rt-cli/badge.svg)](https://docs.rs/yaml-rt-cli/latest/yaml_rt_cli/)
[![CI](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml/badge.svg?branch=main&event=push)](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml)
[![Playground](https://img.shields.io/badge/playground-try_it_online-blue)](https://hdevalke.github.io/yaml-rt/)

The `yaml-rt` command-line query and editor for YAML 1.2.2. Query with RFC 9535
JSONPath and edit with RFC 6901 JSON Pointers while preserving unrelated
comments, whitespace, scalar spelling, and collection styles.

## Installation

Requires Rust 1.96 or newer to build.

```sh
cargo install yaml-rt-cli --locked
```

The package name is `yaml-rt-cli`; the installed binary is `yaml-rt`.
Try commands in the [browser playground](https://hdevalke.github.io/yaml-rt/).

## Query and edit

For a first edit, save this as `config.yaml`:

```yaml
# local development
server:
  port: 8080 # user choice
```

```sh
yaml-rt replace /server/port --value 9090 config.yaml
```

The command prints the following YAML, leaving the file unchanged:

```yaml
# local development
server:
  port: 9090 # user choice
```

Use `--in-place` to save the change. The additional commands below illustrate
other operations; adjust paths and input files to match your configuration.

```sh
# Validate YAML, optionally against a JSON or YAML schema.
yaml-rt validate config.yaml
yaml-rt validate --schema config.schema.json config.yaml

# Infer a permissive JSON Schema from one document.
yaml-rt schema config.yaml --output config.schema.json

# Print JSON Pointer/value pairs for matching nodes.
yaml-rt query '$.services[*].port' config.yaml

# Read a value, or emit each query match as a YAML document.
yaml-rt get /server/port config.yaml
yaml-rt get --query '$.services[*].port' config.yaml

# Print edited YAML or replace the file atomically.
yaml-rt replace /server/port --value 9090 config.yaml
yaml-rt replace --query '$.services[*].port' --value 9090 --in-place config.yaml

# Rename a key while preserving its entry position and comments.
yaml-rt rename-key /server/old-name --to new-name config.yaml

# Insert a complete YAML node from another file.
yaml-rt add /server/tls --value-file tls.yaml config.yaml

# Select the second document in a stream (zero-based).
yaml-rt get /name --doc 1 stream.yaml
```

Operations are `validate`, `schema`, `query`, `get`, `add`, `remove`, `replace`,
`rename-key`, `move`, `copy`, `test`, and `patch`. Use `yaml-rt help <operation>`
for arguments. `get`, `add`, `remove`, `replace`, `rename-key`, and `test` accept
`--query` instead of a positional pointer. Query mutations apply transactionally
to all selected targets; mutations and `test` fail if nothing matches.

## Transactional patches

Save this YAML as `changes.yaml`:

```yaml
- op: test
  path: /server/port
  value: 8080
- op: replace
  path: /server/port
  value: 9090
```

```sh
yaml-rt patch --patch-file changes.yaml --in-place config.yaml
```

Patch input can be YAML or JSON and follows RFC 6902-style operations with full
YAML node values. Operations run in order; any failure rolls back the entire
patch for that document. `--patch` accepts inline input. `--patch-file -` reads
stdin when the target document does not also read stdin.

## Files, batches, and output

Input can be a file, a directory, or `-` for stdin. Omitted input scans the
current directory. Directories recursively include `.yaml` and `.yml` files
(case-insensitive), including hidden paths, in sorted relative-path order;
symlinks are skipped.

Single-file edits print YAML unless `--output FILE` or `--in-place` is supplied.
Batch edits require `--in-place` and replace each successful file atomically.
Failures do not stop remaining files; any failure produces an unsuccessful exit.
Batch read output identifies files with headers, and `--output` collects that
output into one file. Successful `validate` produces no output.

Parse errors include source excerpts, file names, lines, and columns. Terminal
colors follow stderr interactivity and `NO_COLOR`.

## Limits

JSONPath validates the entire selected document for JSON compatibility before
selecting nodes. For example, `get --query '$.server.port'` fails if any unrelated
field contains `.inf`; `get /server/port config.yaml` can still read that known
path. Schema validation also requires JSON-compatible YAML. Schema references
may use local files; network fetches are unavailable. Key rename supports plain
and quoted string keys, but not complex, alias, or block-scalar keys. Copy and
move reject ambiguous or invalid anchor ownership. Parser resource limits are
enabled by default. Untouched documents remain byte-identical; this tool does
not canonicalize formatting.

## Links and license

[API documentation](https://docs.rs/yaml-rt-cli/) ·
[Repository](https://github.com/hdevalke/yaml-rt)

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
