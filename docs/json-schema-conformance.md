# JSON Schema conformance status

`yaml-rt-schema` targets JSON Schema 2020-12. The core parser remains
independent of schema dependencies.

The crate's `tests/official.rs` harness reads a checkout of the official
[JSON Schema Test Suite](https://github.com/json-schema-org/JSON-Schema-Test-Suite)
from the pinned `third_party/json-schema-test-suite` submodule by default.
`JSON_SCHEMA_TEST_SUITE_DIR` can override the root:

```sh
JSON_SCHEMA_TEST_SUITE_DIR=/path/to/JSON-Schema-Test-Suite \
  cargo test -p yaml-rt-schema --test official -- --nocapture
```

With suite commit `ab079cc2bace029fdbb483be28a6ade526bcfbc2`, all 1,265
selected core cases pass. The core harness excludes `refRemote.json` because
the CLI does not fetch remote schemas, and `vocabulary.json` because custom
meta-schemas are outside the release scope. The harness preloads three official
remote fixtures as in-memory resources to exercise reference and dynamic scope
behavior without network access.

With format assertions enabled, all 847 selected optional format cases pass.
The format harness excludes `unknown.json`, which tests annotation behavior,
and `ecmascript-regex.json`, whose regex grammar differs from the Rust `regex`
crate. ECMAScript regular-expression compatibility remains a known gap. The
CLI uses the built-in 2020-12 dialect, which treats `format` as an annotation.
The library's `with_format_assertions(true)` option enables assertions explicitly;
custom meta-schemas and their vocabularies are not supported.

`yaml-rt-schema` uses yaml-rt-core for JSON and YAML parsing, an internal
JSON value writer, and decimal digits for exact numeric checks. It uses `regex`
for patterns, `url` for reference resolution, `iri-string` for URI/IRI syntax,
`idna` for internationalized domain names, and `jiff` for date/time syntax.
These dependencies are confined to the schema crate; `yaml-rt-core` remains
dependency-free. The URI/IDNA parsers preserve full standard format coverage.
