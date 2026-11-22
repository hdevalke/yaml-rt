# yaml-rt-bench

[![CI](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml/badge.svg?branch=main&event=push)](https://github.com/hdevalke/yaml-rt/actions/workflows/ci.yml)

Parse-only benchmarks for yaml-rt, Saphyr 0.4.0, and Rapid YAML 0.16.0. yaml-rt
retains source, trivia, CST nodes, and semantic metadata for round-trip editing.
This unpublished workspace crate requires Rust 1.96 or newer.

## Run the comparison

From the workspace root, initialize the Rapid YAML submodule and run both baselines.
The Rapid YAML build requires a C++11 compiler.

```sh
git submodule update --init third_party/rapidyaml
cargo bench -p yaml-rt-bench --features baselines --bench parse
```

Use `--features saphyr-baseline` to run the Rust-only comparison or
`--features rapidyaml-baseline` to run only the Rapid YAML variants. The arena
variant includes copying the immutable input into Rapid YAML's tree. Criterion
prepares the mutable buffer for the in-place variant before timing begins.

For performance gates, run Criterion three times with default sampling and take
the median of the run medians. Use Rapid YAML's arena variant for the `parse_large`
comparison. `parse_scaling` uses fixed-width flat mappings, and `parse_phases`
measures source construction, CST construction, and full-document parsing.

Past results and optimization notes are in [Benchmark history](HISTORY.md).

## Library comparison (2026-11-22)

yaml-rt at `b4ebb90` was compared with Saphyr 0.4.0 and Rapid YAML 0.16.0 on
an AC-powered Intel Core i5-8250U with Rust 1.100.0. Each result is the median
of three Criterion run medians. The runs were pinned to CPU 3 and used 50
samples, a 1-second warm-up, and a 2-second target measurement per case.
The flat mappings at 1,000 and 5,000 entries appear in two benchmark groups.
The table uses the `parse_scaling` timings for those inputs.

| Fixture | yaml-rt (µs) | Saphyr (µs) | Rapid YAML arena (µs) | Rapid YAML in-place (µs) |
| --- | ---: | ---: | ---: | ---: |
| Small config | 1.99 | 3.72 | 1.07 | 1.08 |
| Medium nested | 9.15 | 17.69 | 3.29 | 3.36 |
| Block scalars | 1.82 | 3.48 | 1.03 | 1.09 |
| Multi-document | 3.82 | 6.73 | 1.72 | 1.75 |
| Flat mapping, 100 entries | 16.85 | 93.64 | 18.10 | 18.01 |
| Flat mapping, 1,000 entries | 160.29 | 935.18 | 176.93 | 171.77 |
| Flat mapping, 5,000 entries | 792.01 | 4,677.58 | 911.58 | 1,130.35 |
| Mixed block/flow, 1,000 items | 3,100.77 | 7,170.29 | 1,453.69 | 1,427.99 |
| Shallow flow, 128 entries | 103.95 | 278.63 | 46.90 | 47.26 |
| Wide flow sequence, 1,024 items | 147.09 | 373.06 | 77.21 | 77.62 |
| Quoted scalars, 256 entries | 269.53 | 637.62 | 93.80 | 95.51 |
| Block scalars, 128 entries | 185.17 | 347.21 | 80.55 | 75.26 |
| Nested configuration, 128 entries | 243.37 | 744.17 | 119.61 | 120.24 |
| JSON objects, 256 entries | 301.96 | 917.41 | 99.77 | 101.98 |
| Mixed flow, 256 entries | 417.97 | 1,059.98 | 166.24 | 166.91 |

Across the 15 distinct inputs, yaml-rt took **0.36×** Saphyr's time,
**1.89×** Rapid YAML arena's time, and **1.85×** Rapid YAML in-place's time
by geometric mean. yaml-rt had lower medians than Saphyr on every input and
lower medians than Rapid YAML on all three flat mapping sizes. Its time was
**0.90×** Rapid YAML arena's on those three sizes by geometric mean and
**1.48×** on the five fixtures measured in `parse_large`.

## Profile yaml-rt

Measure allocations, total allocated bytes, peak live bytes, and retained bytes
with `profile_alloc`. Its positional arguments are entry count, iteration count,
and mode. `full` retains a `YamlDoc` and `cst` retains the source and CST.
`--shape` accepts `flat`, `mixed-flow`, `flow`, `quoted`, `block`, or `json`.
`--file PATH` takes an input file followed by iteration count and mode.

```sh
cargo bench -p yaml-rt-bench --bench profile_alloc -- 1000 100 full
cargo bench -p yaml-rt-bench --bench profile_alloc -- --shape mixed-flow 1000 100 cst
cargo bench -p yaml-rt-bench --bench profile_alloc -- \
  --file third_party/rapidyaml/bm/cases/travis.yml 100 full
```

`profile_parse` parses the non-error YAML Test Suite inputs separately by
default. `--mode stream` combines the fixtures into one stream. Build with
`--no-run` to profile the executable with Linux `perf`.

```sh
cargo bench -p yaml-rt-bench --bench profile_parse -- --repeat 1 --mode cases
cargo bench -p yaml-rt-bench --bench profile_parse --no-run
perf record -g -- PATH_TO_PROFILE_PARSE --repeat 1000 --mode cases
perf report
```

## Rapid YAML native harness

The CMake adapter registers yaml-rt as `bm_ryml_yamlrt_arena` in Rapid YAML's
Google Benchmark executable. It times the immutable-input `YamlDoc::parse`
call and document destruction. The harness requires CMake, Python, a C++
compiler, and Rapid YAML's benchmark dependencies. Build it from the workspace
root.

```sh
cmake -S crates/yaml-rt-bench/rapidyaml -B target/rapidyaml-bm -DCMAKE_BUILD_TYPE=Release
cmake --build target/rapidyaml-bm --config Release --target ryml-bm-parse
cmake --build target/rapidyaml-bm --config Release --target ryml-bm-parse-travis
cmake --build target/rapidyaml-bm --config Release --target ryml-bm-parse-all
```

Results are under `target/rapidyaml-bm/rapidyaml/bm/bm-results`. To generate a
plot, configure CMake with a Python environment that has the plotting packages.

```sh
uv venv target/rapidyaml-bm/plot-venv
uv pip install --python target/rapidyaml-bm/plot-venv/bin/python \
  -r third_party/rapidyaml/proj/c4proj/bm-xp/requirements.txt
cmake -S crates/yaml-rt-bench/rapidyaml -B target/rapidyaml-bm \
  -DPython_EXECUTABLE="$PWD/target/rapidyaml-bm/plot-venv/bin/python"
cmake --build target/rapidyaml-bm --config Release --target ryml-bm-parse-travis-plot
```

To profile the yaml-rt variant on one fixture, filter the executable by its
benchmark name. Keep captures in a local build directory or `/tmp`.

```sh
perf record -g -- target/rapidyaml-bm/rapidyaml/bm/ryml-bm-parse-0.16.0 \
  '--benchmark_filter=bm_ryml_yamlrt_arena$' --benchmark_min_time=3s \
  third_party/rapidyaml/bm/cases/travis.yml OUTPUT_FILE /tmp/yaml-rt-benchmark.json
```

Licensed under either [Apache-2.0](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/hdevalke/yaml-rt/blob/main/LICENSE-MIT), at your option.
