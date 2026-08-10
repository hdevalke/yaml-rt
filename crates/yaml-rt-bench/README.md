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
```

The positional arguments are mapping entries, parse iterations, and measurement
mode. `full` (the default) retains a complete `YamlDoc`; `cst` retains only an
owned `Source` and its CST arena. The `parse_phases` Criterion group separates
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
perf record -F 999 --call-graph dwarf -- target/release/deps/profile_parse-<hash> --repeat 1000 --mode cases
perf report
```

Use `--mode stream` when you specifically want to investigate behavior on one
large concatenated input.
