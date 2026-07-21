# yaml-rt-bench

Parse-only benchmarks for RTY against ordinary YAML loader baselines.

RTY preserves lossless round-trip state such as source spans, comments, trivia,
CST nodes, and semantic metadata. Tokens and events are lazy. `fyaml` and Saphyr are included as
useful parser baselines, but they are not feature-equivalent to RTY's
round-trip model. Treat benchmark results as contextual parse-throughput data,
not as a complete product comparison.

Run RTY against Saphyr without requiring the native fyaml build:

```sh
cargo bench -p yaml-rt-bench --features saphyr-baseline --bench parse
```

Run every available baseline with:

```sh
cargo bench -p yaml-rt-bench --features baselines --bench parse
```

The `parse_scaling` group generates flat mappings containing 100, 1,000, and
5,000 entries with fixed-width keys and values so bytes per entry stay constant.
Use the standalone allocation profiler to measure allocation count, total
allocated bytes, transient peak live bytes, and retained bytes for the same
shape:

```sh
cargo bench -p yaml-rt-bench --bench profile_alloc -- 1000 100 full
cargo bench -p yaml-rt-bench --bench profile_alloc -- 1000 100 cst
```

The positional arguments are mapping entries, parse iterations, and measurement
mode. `full` (the default) retains a complete `YamlDoc`; `cst` retains only an
owned `Source` and its CST arena. The `parse_phases` Criterion group provides
the corresponding 1,000-entry throughput comparison.

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

| fixture | RTY median | Saphyr median | RTY / Saphyr |
| --- | ---: | ---: | ---: |
| small config | 7.56 us | 3.86 us | 1.96x |
| medium nested | 37.87 us | 18.26 us | 2.07x |
| block scalars | 5.91 us | 3.33 us | 1.78x |
| multi-document | 14.52 us | 7.11 us | 2.04x |
| 100 mapping entries | 184.76 us | 95.91 us | 1.93x |
| 1,000 mapping entries | 2.05 ms | 1.10 ms | 1.85x |
| 5,000 mapping entries | 12.73 ms | 5.72 ms | 2.23x |

The counting allocator reported 7,061 allocations, 1,467,792 allocated bytes,
and 1,126,672 peak live bytes per 1,000-entry RTY parse. Atomic counter updates
make its timing unsuitable for throughput comparison; use Criterion for time.

## Compact-arena result

Final machine-local `--quick` measurements on 2026-07-21:

| fixture | RTY median | Saphyr median | RTY / Saphyr |
| --- | ---: | ---: | ---: |
| small config | 5.02 us | 4.41 us | 1.14x |
| medium nested | 21.26 us | 19.19 us | 1.11x |
| block scalars | 4.17 us | 3.94 us | 1.06x |
| multi-document | 7.21 us | 7.55 us | 0.96x |
| 100 mapping entries | 91.16 us | 99.92 us | 0.91x |
| 1,000 mapping entries | 0.990 ms | 1.029 ms | 0.96x |
| 5,000 mapping entries | 5.598 ms | 5.080 ms | 1.10x |

The geometric mean over the four source fixtures is 1.06x Saphyr. The
5,000/1,000-entry RTY ratio is 5.66x. At 1,000 fixed-width entries, the counting
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

| fixture | RTY median | Saphyr median | RTY / Saphyr |
| --- | ---: | ---: | ---: |
| small config | 2.185 us | 3.713 us | 0.59x |
| medium nested | 14.865 us | 17.990 us | 0.83x |
| block scalars | 2.942 us | 3.395 us | 0.87x |
| multi-document | 4.707 us | 6.709 us | 0.70x |
| 100 mapping entries | 32.057 us | 100.320 us | 0.32x |
| 1,000 mapping entries | 0.31088 ms | 0.98680 ms | 0.32x |
| 5,000 mapping entries | 1.5119 ms | 5.1792 ms | 0.29x |

The geometric mean across the four source fixtures is **0.737x Saphyr**. RTY
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
