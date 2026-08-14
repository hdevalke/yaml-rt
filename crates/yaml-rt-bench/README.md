# yaml-rt-bench

Parse-only benchmarks for yaml-rt against ordinary YAML loader baselines.

yaml-rt preserves lossless round-trip state such as source spans, comments, trivia,
CST nodes, and semantic metadata. Tokens and events are lazy. Rapid YAML and Saphyr are included
as useful parser baselines, but they are not feature-equivalent to yaml-rt's
round-trip model. Treat benchmark results as contextual parse-throughput data,
not as a complete product comparison.

Run yaml-rt against Saphyr without requiring a C++ compiler or the Rapid YAML submodule:

```sh
cargo bench -p yaml-rt-bench --features saphyr-baseline --bench parse
```

Rapid YAML v0.16.0 is pinned as a Git submodule at commit
`f8ac8dd50f4f7916579d55a05ebf9c6488e52670`. Initialize it and run its
benchmarks with:

```sh
git submodule update --init third_party/rapidyaml
cargo bench -p yaml-rt-bench --features rapidyaml-baseline --bench parse
```

The Rapid YAML baseline requires a C++11 compiler. `rapidyaml-arena` accepts
the same immutable input as yaml-rt and includes Rapid YAML's copy into its
tree arena. `rapidyaml-in-place` measures parsing a mutable buffer; Criterion
creates that buffer outside the timed routine, so this is Rapid YAML's parser
ceiling rather than an end-to-end immutable-input comparison.

Run every available baseline with:

```sh
cargo bench -p yaml-rt-bench --features baselines --bench parse
```

## Rapid YAML native harness

The repository also provides a parent-owned CMake adapter that adds yaml-rt to
Rapid YAML's own Google Benchmark parse executable without changing the pinned
submodule. It builds `yaml-rt-bench` as a release Rust static library and
registers the complete lossless parser as `bm_ryml_yamlrt_arena`. The `ryml_`
prefix lets Rapid YAML's existing result and plotting scripts recognize the
entry; `yamlrt_arena` is the actual implementation and variant name.

Configure the adapter and build the parse executable with:

```sh
cmake -S crates/yaml-rt-bench/rapidyaml \
  -B target/rapidyaml-bm \
  -DCMAKE_BUILD_TYPE=Release
cmake --build target/rapidyaml-bm --config Release --target ryml-bm-parse
```

Run one fixture, all parse fixtures, or generate the existing per-fixture plot:

```sh
cmake --build target/rapidyaml-bm --config Release --target ryml-bm-parse-travis
cmake --build target/rapidyaml-bm --config Release --target ryml-bm-parse-all
cmake --build target/rapidyaml-bm --config Release --target ryml-bm-parse-travis-plot
```

Configuration requires Cargo, a C++ compiler, CMake, and the dependencies used
by Rapid YAML's benchmark build. Plotting additionally requires the Python
packages from Rapid YAML's benchmark tooling:

```sh
uv venv target/rapidyaml-bm/plot-venv
uv pip install --python target/rapidyaml-bm/plot-venv/bin/python \
  -r third_party/rapidyaml/proj/c4proj/bm-xp/requirements.txt
```

Results are written below `target/rapidyaml-bm/rapidyaml/bm/bm-results`.

`bm_ryml_yamlrt_arena` measures immutable-input `YamlDoc::parse`, including the
source copy, complete CST and semantic construction, allocations, and document
destruction. File I/O and build/loading work are outside the timed loop. Unlike
ordinary loader baselines, yaml-rt retains the original source, comments,
trivia, spans, scalar spelling and styles, and the semantic metadata needed for
round-trip edits, so the throughput comparison remains contextual rather than
feature-equivalent.

The `parse_scaling` group generates flat mappings containing 100, 1,000, and
5,000 entries with fixed-width keys and values so bytes per entry stay constant.
The `parse_large` gate compares Rapid YAML's immutable-input arena parser with
1,000- and 5,000-entry flat mappings, a 1,000-item mixed block/flow document,
a shallow flow-heavy document, and a 1,024-item wide flow sequence. Rapid
YAML's in-place results remain informational and are not used for the gate.
Use the standalone allocation profiler to measure allocation count, total
allocated bytes, transient peak live bytes, and retained bytes for the same
shape:

```sh
cargo bench -p yaml-rt-bench --bench profile_alloc -- 1000 100 full
cargo bench -p yaml-rt-bench --bench profile_alloc -- 1000 100 cst
cargo bench -p yaml-rt-bench --bench profile_alloc -- \
  --file third_party/rapidyaml/bm/cases/travis.yml 100 full
cargo bench -p yaml-rt-bench --bench profile_alloc -- \
  --shape mixed-flow 1000 100 full
cargo bench -p yaml-rt-bench --bench profile_alloc -- \
  --shape json 256 100 full
```

The positional arguments are mapping entries, parse iterations, and measurement
mode. `full` (the default) retains a complete `YamlDoc`; `cst` retains only an
owned `Source` and its CST arena. `--file PATH` replaces the generated mapping;
its optional following arguments are parse iterations and mode. `--shape`
accepts `flat`, `mixed-flow`, `flow`, `quoted`, `block`, or `json`, followed by
entry count, iterations, and mode. The
`parse_phases` Criterion group separates
source construction, parsing an already prepared `Source`, end-to-end CST
construction, borrowed-input parsing, and owned-input parsing for the
corresponding 1,000-entry document.

For stable comparisons, run Criterion with its default sampling three times and
compare the median of the three reported medians. Do not use `--quick` for a
performance gate:

```sh
cargo bench -p yaml-rt-bench --features saphyr-baseline --bench parse
cargo bench -p yaml-rt-bench --features saphyr-baseline --bench parse
cargo bench -p yaml-rt-bench --features saphyr-baseline --bench parse
```

The current layouts are 28 bytes for `Node`, 2 bytes for `SemanticKind`, and
104 bytes for the on-demand public `YamlEvent`. The allocation profiler prints
these sizes so future layout changes are visible in benchmark logs.

## Reference baseline

The compact-arena refactor started from this machine-local reference on
2026-07-21. Criterion was run with `--quick`, so these numbers are orientation
data rather than a portable performance guarantee.

| fixture | yaml-rt median | Saphyr median | yaml-rt / Saphyr |
| --- | ---: | ---: | ---: |
| small config | 7.56 us | 3.86 us | 1.96x |
| medium nested | 37.87 us | 18.26 us | 2.07x |
| block scalars | 5.91 us | 3.33 us | 1.78x |
| multi-document | 14.52 us | 7.11 us | 2.04x |
| 100 mapping entries | 184.76 us | 95.91 us | 1.93x |
| 1,000 mapping entries | 2.05 ms | 1.10 ms | 1.85x |
| 5,000 mapping entries | 12.73 ms | 5.72 ms | 2.23x |

The counting allocator reported 7,061 allocations, 1,467,792 allocated bytes,
and 1,126,672 peak live bytes per 1,000-entry yaml-rt parse. Atomic counter updates
make its timing unsuitable for throughput comparison; use Criterion for time.

## Compact-arena result

Final machine-local `--quick` measurements on 2026-07-21:

| fixture | yaml-rt median | Saphyr median | yaml-rt / Saphyr |
| --- | ---: | ---: | ---: |
| small config | 5.02 us | 4.41 us | 1.14x |
| medium nested | 21.26 us | 19.19 us | 1.11x |
| block scalars | 4.17 us | 3.94 us | 1.06x |
| multi-document | 7.21 us | 7.55 us | 0.96x |
| 100 mapping entries | 91.16 us | 99.92 us | 0.91x |
| 1,000 mapping entries | 0.990 ms | 1.029 ms | 0.96x |
| 5,000 mapping entries | 5.598 ms | 5.080 ms | 1.10x |

The geometric mean over the four source fixtures is 1.06x Saphyr. The
5,000/1,000-entry yaml-rt ratio is 5.66x. At 1,000 fixed-width entries, the counting
allocator reports 23 allocations, 611,868 allocated bytes, and 526,432 peak
live bytes: reductions of 99.7%, 58.3%, and 53.3% from the reference baseline.

With a parsed value kept alive, the full document retains 317,360 bytes. The
CST-only path uses 18 allocations, allocates 407,296 bytes, peaks at 363,620
live bytes, and retains 112,948 bytes. The 204,412-byte retained gap isolates
the current semantic storage, while the full/CST allocation and peak gaps also
include semantic construction work. These measurements use a 1,000-entry
fixed-width mapping and the release allocation profiler above.

## Direct compact parser result

Final machine-local results on 2026-07-21 use the median of the three medians
reported by three complete, default-sampling Criterion runs. Lower ratios are
better.

| fixture | yaml-rt median | Saphyr median | yaml-rt / Saphyr |
| --- | ---: | ---: | ---: |
| small config | 2.185 us | 3.713 us | 0.59x |
| medium nested | 14.865 us | 17.990 us | 0.83x |
| block scalars | 2.942 us | 3.395 us | 0.87x |
| multi-document | 4.707 us | 6.709 us | 0.70x |
| 100 mapping entries | 32.057 us | 100.320 us | 0.32x |
| 1,000 mapping entries | 0.31088 ms | 0.98680 ms | 0.32x |
| 5,000 mapping entries | 1.5119 ms | 5.1792 ms | 0.29x |

The geometric mean across the four source fixtures is **0.737x Saphyr**. yaml-rt
is faster on every fixture and generated size. Its 5,000/1,000-entry scaling is
**4.86x**, below the 5.5x gate.

At 1,000 fixed-width entries, a full document uses 9 allocations, allocates
160,912 bytes, peaks at 158,036 live bytes, and retains 157,140 bytes. The
CST-only measurement has the same construction figures and retains 112,948
bytes; the remaining compact semantic state accounts for a 44,192-byte retained
gap.

Compared with the second-stage starting point above, the direct parser reduces
allocation count by 60.9%, allocated bytes by 73.7%, peak live bytes by 70.0%,
and retained bytes by 50.5%. Compared with the original pre-compact reference,
the reductions are 99.9%, 89.0%, and 86.0% for allocation count, allocated
bytes, and peak live bytes respectively.

The final parser constructs semantic metadata directly from parser callbacks,
uses CST wrappers as the semantic topology, reads lines through the source
index, keeps decorated-node properties sparse and span-backed, and specializes
ordinary property-free plain nodes. Tokens are lexed on request and events are
streamed from CST topology without a retained or transient event arena.

## Rapid YAML optimization result

Machine-local results on 2026-08-10 use the median of the medians from three
complete default-sampling Criterion runs. The Rapid YAML comparison is its
immutable-input arena path; lower ratios are better.

| large fixture | yaml-rt median | Rapid YAML arena median | yaml-rt / Rapid YAML |
| --- | ---: | ---: | ---: |
| flat mapping, 1,000 entries | 74.373 us | 77.860 us | 0.955x |
| flat mapping, 5,000 entries | 394.01 us | 435.65 us | 0.904x |
| mixed block/flow, 1,000 items | 2.6553 ms | 0.74309 ms | 3.573x |
| shallow flow, 128 entries | 57.184 us | 25.668 us | 2.228x |
| wide flow sequence, 1,024 items | 77.074 us | 40.460 us | 1.905x |

The flat-mapping geometric mean is **0.929x**, improving the original roughly
1.9x gap to slightly faster than Rapid YAML's arena path. The complete
five-fixture geometric mean is **1.67x**, so it does not meet the 1.25x broad
cohort gate. The remaining mixed block/flow and flow-collection work is a
separate second optimization stage; the lossless and conformance guarantees
were not weakened to meet the target.

The corresponding three-run phase medians for the 1,000-entry flat mapping are:

| phase | median |
| --- | ---: |
| source construction and line analysis | 38.962 us |
| parse prepared `Source` to CST and semantics | 38.189 us |
| end-to-end CST construction | 77.976 us |
| borrowed `YamlDoc::parse` | 80.183 us |
| owned `YamlDoc::parse_owned` | 80.210 us |

The full 1,000-entry allocation profile uses 10 allocations, allocates 168,920
bytes, peaks at 166,044 live bytes, and retains 165,148 bytes. Relative to the
direct compact parser result, peak live and retained bytes both increase by
5.1%, within the 15% and 10% limits. The CST-only path retains 120,956 bytes.
The 308-case non-error YAML Test Suite profile reached 22.13 MiB/s over 1,000
repetitions, compared with 20.84 MiB/s at the start of this pass.

## yaml-rt-only perf profiling

The `profile_parse` bench avoids third-party parser dependencies and is intended
for Linux `perf` runs against yaml-rt parser hot paths. It discovers all non-error
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
taskset -c 0 perf record -e cpu_core/cycles/P -F 1999 --call-graph dwarf -- \
  target/release/deps/profile_parse-<hash> --repeat 1000 --mode cases
perf report
```

Use `--mode stream` when you specifically want to investigate behavior on one
large concatenated input.

To profile exactly the immutable-input comparison used by Rapid YAML's native
harness, build the adapter first and run one fixture on the same performance
core:

```sh
taskset -c 0 perf record -e cpu_core/cycles/P -F 1999 --call-graph dwarf -- \
  target/rapidyaml-bm/rapidyaml/bm/ryml-bm-parse-0.16.0 \
  '--benchmark_filter=bm_ryml_yamlrt_arena$' --benchmark_min_time=3s \
  third_party/rapidyaml/bm/cases/travis.yml OUTPUT_FILE /tmp/yaml-rt-unused.json
perf report
```

`cpu_core/cycles/P` selects performance-core cycles on hybrid Intel systems;
use the equivalent core-specific cycles event on other CPUs. Keep the selected
CPU free of unrelated work and compare the median CPU time from three complete
runs. Perf capture files are local artifacts and must not be committed.

## Profile-guided parse pass (2026-08-11)

The native harness was run three times per fixture on CPU 0 and the median CPU
times were compared with `bm_ryml_yaml_arena`. The pass removed the dominant
inline-comment, mapping-colon, repeated quote-validation, and discarded decode
work from the profiled paths. Representative ratios changed as follows:

| fixture | before | after |
| --- | ---: | ---: |
| 13-fixture geometric mean | 4.784x | 1.888x |
| `travis.yml` | 4.324x | 1.664x |
| `compile_commands.json` | 8.60x | 1.956x |
| double-quoted, single-line | 13.46x | 2.190x |
| double-quoted, multiline | 2.801x | 1.330x |
| single-quoted, multiline | 2.561x | 1.499x |
| plain, multiline | 2.216x | 1.616x |
| literal block, multiline | 2.835x | 2.126x |

The pass meets the `travis.yml` ≤2x gate but does **not** meet the ≤1.5x
13-fixture geometric-mean gate. The generated 1,000-item mixed block/flow
Criterion comparison is approximately 3.12x Rapid YAML arena and also remains
outside its ≤1.5x gate. These misses are recorded rather than weakening
lossless CST, source, comment, semantic, or conformance work.

The 1,000-entry allocation profile retains 173,156 bytes and peaks at 174,052
live bytes, increases of 4.8% from the preceding 165,148-byte retained and
166,044-byte peak measurements. Both remain inside the 10% retained and 15%
peak-live limits. On AC power, three pinned-core runs of the 308-fixture
non-error YAML Test Suite profile measured 31.23, 31.23, and 31.26 MiB/s. The
31.23 MiB/s median is 41.1% above the preceding 22.13 MiB/s baseline, so the
no-regression throughput gate passes. A 14.62 MiB/s battery-powered run was
discarded because its CPU power state did not match the baseline.

## Allocation and layout pass (2026-08-11)

This pass retained only changes that survived powered timing gates. Source
validation now searches each 32-byte chunk for bytes that actually need YAML
printability handling and processes only those positions, avoiding a second
complete scan of every chunk containing a newline. Flow frame and semantic
event vectors are reused for every flow collection in a document. The first
document ID is stored inline, while multi-document streams spill to a normal
vector.

| internal record | before | after |
| --- | ---: | ---: |
| `FlowFrame` | 32 B | 24 B |
| transient scalar facts | 24 B | 12 B |
| simple mapping facts | 16 B | 8 B |
| pending node properties | 48 B | 40 B |
| parser context | 48 B | 40 B |
| `SemanticNode` | 16 B | 12 B |

The three-run native fixture median improved from **1.882x** to **1.740x**
Rapid YAML arena geometrically. `travis.yml` improved from **1.675x** to
**1.591x**. The generated mixed block/flow comparison improved from
approximately **3.12x** to **2.84x**. These improve the preceding result but do
not meet the 1.5x geometric-mean or mixed-flow targets.

The 1,000-entry flat mapping now uses 9 allocations, retains 165,112 bytes,
and peaks at 165,880 live bytes, compared with 10 allocations, 173,156 retained
bytes, and 174,052 peak bytes before the pass. `travis.yml` uses 8 allocations,
retains 19,491 bytes, and peaks at 20,259 bytes. The 1,000-item mixed-flow
profile uses 16 allocations total rather than allocating scratch storage per
collection.

Three AC-powered YAML Test Suite profile runs on CPU 2 measured 29.90, 29.92,
and 29.74 MiB/s. The 29.90 MiB/s median is 4.3% below the preceding 31.23 MiB/s
measurement and remains within the 5% regression gate. The native executable's
text section decreased from 3,624,821 to 3,622,585 bytes.

The following experiments were rejected by their timing gates: safe inline
arrays for every parser stack, fused validation and line-fact construction,
adaptive line-capacity sampling, `u16` next-significant-line deltas, packed
optional property spans, and forced inlining of the validation scanner. The
retained implementation uses safe Rust throughout the new storage code and
adds no dependencies.

## Unified parser control-flow pass (2026-08-11)

This pass retained sequential `LineCursor` iteration, a compact block
collection frame stack, direct semantic registration in the iterative flow
parser, and node-referenced sparse semantic metadata. Flow property parsing is
shared with semantic registration; property-only empty nodes, implicit flow
mappings, and collection keys no longer require a completed-CST traversal.

| internal record | before | after |
| --- | ---: | ---: |
| block parser context | 40 B | 16 B `BlockFrame` plus sparse side state |
| retained `Node` | 28 B | 32 B including semantic reference |
| retained common semantic record | 12 B plus dense slot | encoded in node |
| retained exceptional semantic record | 12 B plus dense slot | 8 B sparse metadata |

Three short native-harness acceptance runs pinned to CPU 2 measured 13-fixture
geometric ratios of **1.772x**, **1.777x**, and **1.774x** Rapid YAML arena
(median **1.774x**). `travis.yml` measured **1.627x**, **1.643x**, and
**1.621x** (median **1.627x**). Both remain within 5% of the preceding retained
stage but do not meet the 1.5x target. The generated 1,000-item mixed block/flow
median was 1.924 ms versus approximately 0.674 ms for Rapid YAML arena, or
about **2.85x**. Dedicated shallow and wide flow Criterion cases improved by
approximately 57% and 64%, respectively, after duplicate flow property parsing
was removed.

The 1,000-entry flat mapping now uses 10 allocations, retains 141,008 bytes,
and peaks at 177,540 live bytes. Relative to the preceding retained stage,
retained memory decreases by 14.6% and peak live memory increases by 7.0%,
inside the 10% retained-growth and 15% peak-growth gates. The 1,000-item mixed
block/flow profile remains at 16 allocations and retains 1,760,599 bytes.
Three CPU-2 YAML Test Suite profile runs measured 28.82, 28.94, and 28.67 MiB/s;
the 28.82 MiB/s median is 3.6% below the preceding 29.90 MiB/s result and stays
inside the 5% regression gate.

A four-byte-offset structural tape was implemented and tested with quote,
comment, multiline-flow, and mapping-separator boundaries, but rejected. On
mixed block/flow it added a complete scan before enough consumers used the tape
and regressed the pinned benchmark from 1.924 ms to 2.22 ms. The parser therefore
retains compact line facts and context-aware scanners; structural-tape code and
allocations are not part of the retained implementation.

## Complete block-machine cutover (2026-08-11)

Production block parsing now runs only through `BlockMachine`. Mapping,
sequence, explicit-entry, nested-child, empty-value, split-property, block
scalar, and indentless-sequence behavior is represented by entry phases and
consume, reprocess, push, and pop transitions. The former nested block-value
dispatch and heterogeneous context searches were deleted after a test-only
dual-engine fingerprint matched CST nodes and links, semantic metadata,
documents, events and CST links, rendering, and positioned diagnostics on
focused fixtures and all 402 YAML Test Suite cases.

The semantic builder now writes derived kind/style flags and sparse metadata
references directly into CST nodes. It no longer keeps dense CST slots or a
transient semantic-node arena, and `finish` validates open frames without a
final CST-wide conversion pass. Ordinary collection lookup restores cached
same-kind links on pop; the remaining reverse searches are exceptional
property and block-scalar recovery paths.

Three short CPU-2 native-harness repetitions produced a **1.720x** 13-fixture
geometric mean against Rapid YAML arena and **1.563x** on `travis.yml`. The
geometric result improves the preceding 1.774x stage by 3.0%; `travis.yml`
improves by 3.9%. The 1.5x target remains unmet, but the architecture gate is
inside its allowed 5% regression bound.

Three pinned mixed block/flow Criterion runs measured yaml-rt at 1.5784,
1.5705, and 1.6201 ms and Rapid YAML arena at 0.68557, 0.69360, and 0.69523 ms.
The median-of-medians ratio is **2.28x**, down from 2.85x in the preceding
retained stage. The final short phase medians for a 1,000-entry flat mapping
were 35.283 us for source construction, 24.214 us for a prepared-source CST,
62.648 us end to end, 62.833 us for borrowed `YamlDoc::parse`, and 62.791 us
for owned input. Shallow flow measured 48.004 us, wide flow 78.969 us, and a
depth-32 nested flow sequence 2.900 us.

Final retained allocation profiles are:

| workload | allocations | peak live | retained |
| --- | ---: | ---: | ---: |
| flat mapping, 1,000 entries | 8 | 141,408 B | 141,024 B |
| mixed block/flow, 1,000 items | 19 | 1,761,247 B | 1,760,767 B |
| flow, 1,000 entries | 20 | 485,002 B | 484,522 B |
| JSON objects, 256 entries | 18 | 127,613 B | 127,133 B |
| block scalars, 1,000 entries | 29 | 1,038,350 B | 1,037,934 B |

No retained-memory or peak-live limit regressed relative to the preceding
direct-semantic stage. Three AC-powered CPU-2 YAML Test Suite profiles measured
27.66, 27.68, and 27.43 MiB/s (median **27.66 MiB/s**), 4.0% below the preceding
28.82 MiB/s result and inside the architectural cutover allowance.

Eager preparation before grammatical dispatch, a special empty-frame loop, and
forced transition inlining were measured and rejected as neutral-to-slower.
The global structural tape remains rejected and was not retried. The retained
implementation uses per-line stack data, preserves the 32-byte node layout,
adds no dependency, and keeps the expected-failure list empty.
