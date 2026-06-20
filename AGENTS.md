# Agent instructions for RTY

RTY is a Rust 2024 workspace for a minimal-dependency YAML 1.2.2 round-trip
parser. Keep every change aligned with the roadmap in `README.md`.

## Project goals

- Target YAML 1.2.2.
- Keep passing the YAML Test Suite tag `v2022-01-17` in the core parser.
- Preserve presentation details needed for round-trip editing: comments,
  whitespace, line endings, scalar styles, directives, tags, anchors, aliases,
  document markers, and original spelling where practical.
- Guarantee byte-identical output for untouched YAML and minimal diffs for edited
  YAML.
- Treat the lossless CST as the source of truth. Typed Rust structs are overlays
  that read from and patch the document.

## Workspace responsibilities

- `crates/yaml-rt-core`: no dependencies. Owns source handling, lexer, token
  stream, lossless CST, semantic YAML graph, diagnostics, editor APIs, and the
  patch-based emitter.
- `crates/yaml-rt-derive`: may use `syn`, `quote`, and `proc-macro2`. Owns the
  `YamlRoundTrip` derive macro and YAML field/struct attributes.
- `crates/yaml-rt`: facade crate. Re-exports public core APIs and
  `yaml_rt_derive::YamlRoundTrip`.
- `tests/`: integration tests, including the typed overlay usefulness target and
  YAML Test Suite harness tests.

## Coding guidelines

- Do not add dependencies to `yaml-rt-core` without explicit approval and a
  README explanation. Parser functionality must not depend on external YAML
  libraries.
- Prefer small, well-tested modules for source handling, scanning, parsing,
  composition, schema resolution, editing, and emission.
- Keep lossless syntax concerns separate from semantic/schema-resolved values.
- Preserve spans for all user-visible syntax and diagnostics.
- Store spans or IDs in nodes instead of `&str` references to avoid lifetime-heavy
  public APIs.
- Avoid implementing YAML syntax with broad ad-hoc string splitting; YAML is
  context-sensitive and needs explicit context/indentation tracking.
- Never put try/catch blocks around imports.

## Testing guidelines

- Run `cargo test --workspace` for Rust changes.
- Run `cargo fmt --all -- --check` for formatting changes.
- Add focused unit tests before broad conformance tests when introducing tricky
  YAML grammar behavior.
- Core parser changes should add or update YAML Test Suite `v2022-01-17` coverage
  once the harness exists.
- Track expected YAML Test Suite failures explicitly. The list should normally
  stay empty and only grow for deliberate temporary regressions or unsupported
  future fixture categories.

## Documentation guidelines

- Update `README.md` when the roadmap, architecture, supported YAML features,
  public APIs, derive attributes, or conformance status changes.
- Document any dependency additions and why they are compatible with the
  minimal-dependency goal.
