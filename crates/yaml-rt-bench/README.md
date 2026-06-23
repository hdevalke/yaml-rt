# yaml-rt-bench

Parse-only benchmarks for RTY against ordinary YAML loader baselines.

RTY preserves lossless round-trip state such as source spans, comments, trivia,
events, CST nodes, and semantic graph links. `fyaml` and `saphyr` are included as
useful parser baselines, but they are not feature-equivalent to RTY's
round-trip model. Treat benchmark results as contextual parse-throughput data,
not as a complete product comparison.

Run with:

```sh
cargo bench -p yaml-rt-bench --bench parse
```
