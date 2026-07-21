# yaml-rt-bench

Parse-only benchmarks for RTY against ordinary YAML loader baselines.

RTY preserves lossless round-trip state such as source spans, comments, trivia,
events, CST nodes, and semantic graph links. `fyaml` and `saphyr` are included as
useful parser baselines, but they are not feature-equivalent to RTY's
round-trip model. Treat benchmark results as contextual parse-throughput data,
not as a complete product comparison.

Run with:

```sh
cargo bench -p yaml-rt-bench --features baselines --bench parse
```

## RTY-only perf profiling

The `profile_parse` bench avoids third-party parser dependencies and is intended
for Linux `perf` runs against RTY parser hot paths. It discovers all non-error
YAML Test Suite `in.yaml` fixtures and parses each fixture as a separate
document by default. A synthesized stream mode is available, but it can expose
cross-fixture directive and tag-scope interactions.

Quick smoke runs:

```sh
cargo bench -p yaml-rt-bench --bench profile_parse -- --repeat 1 --mode cases
```

Build a release binary without running it:

```sh
cargo bench -p yaml-rt-bench --bench profile_parse --no-run
```

Then run `perf` against the printed `target/release/deps/profile_parse-*`
binary:

```sh
perf record -F 999 --call-graph dwarf -- target/release/deps/profile_parse-<hash> --repeat 1000 --mode cases
perf report
```

Use `--mode stream` when you specifically want to investigate behavior on one
large concatenated input.
