use std::fmt::Write;
use std::hint::black_box;

#[cfg(feature = "rapidyaml-baseline")]
mod rapidyaml;

#[cfg(feature = "rapidyaml-baseline")]
use criterion::BatchSize;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
#[cfg(feature = "saphyr-baseline")]
use saphyr::{LoadableYamlNode, Yaml};
use yaml_rt_core::{Source, YamlDoc, parse_cst};

struct Fixture {
    name: &'static str,
    input: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "small_config",
        input: r"host: localhost
ports:
  - 8080
  - 9090
enabled: true
",
    },
    Fixture {
        name: "medium_nested",
        input: r#"# application settings
server:
  host: "localhost"
  port: 8080
  tls:
    enabled: true
    cert: 'certs/dev.pem'
features:
  - name: metrics
    flags: [http, runtime, "histograms"]
  - name: tracing
    flags: {level: debug, format: json}
limits:
  request-bytes: 1048576
  timeouts:
    connect-ms: 500
    read-ms: 2000
"#,
    },
    Fixture {
        name: "block_scalars",
        input: r"literal: |
  first line
  second line

  indented paragraph
folded: >
  folded line
  continues here

  next paragraph
",
    },
    Fixture {
        name: "multi_document",
        input: r"---
name: first
items:
  - one
  - two
---
name: second
settings: {mode: fast, retries: 3}
...
",
    },
];

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    for fixture in FIXTURES {
        bench_input(&mut group, fixture.name, fixture.input);
    }

    group.finish();

    let generated = [100, 1_000, 5_000].map(|entries| (entries, flat_mapping(entries)));
    let mut scaling = c.benchmark_group("parse_scaling");
    for (entries, input) in &generated {
        bench_input(&mut scaling, entries, input);
    }
    scaling.finish();

    let phase_input = flat_mapping(1_000);
    let mut phases = c.benchmark_group("parse_phases");
    phases.bench_function("source/1000", |bencher| {
        bencher.iter(|| {
            let source = Source::new(black_box(phase_input.clone()))
                .expect("generated source should be valid");
            black_box(source);
        });
    });
    let prepared_source =
        Source::new(phase_input.clone()).expect("generated source should be valid");
    phases.bench_function("prepared-cst/1000", |bencher| {
        bencher.iter(|| {
            let nodes = parse_cst(black_box(&prepared_source))
                .expect("generated mapping should parse as a CST");
            black_box(nodes);
        });
    });
    phases.bench_function("end-to-end-cst/1000", |bencher| {
        bencher.iter(|| {
            let source = Source::new(black_box(phase_input.clone()))
                .expect("generated source should be valid");
            let nodes = parse_cst(&source).expect("generated mapping should parse as a CST");
            black_box((source, nodes));
        });
    });
    phases.bench_function("borrowed-full/1000", |bencher| {
        bencher.iter(|| {
            let doc = YamlDoc::parse(black_box(&phase_input))
                .expect("generated mapping should parse as a full document");
            black_box(doc);
        });
    });
    phases.bench_function("owned-full/1000", |bencher| {
        bencher.iter(|| {
            let doc = YamlDoc::parse_owned(black_box(phase_input.clone()))
                .expect("generated mapping should parse as a full document");
            black_box(doc);
        });
    });
    phases.finish();

    let large_inputs = [
        ("flat_1000", flat_mapping(1_000)),
        ("flat_5000", flat_mapping(5_000)),
        ("mixed_block_flow_1000", mixed_block_flow(1_000)),
        ("shallow_flow_128", shallow_flow_heavy(128)),
        ("wide_flow_1024", wide_flow_sequence(1_024)),
    ];
    let mut large = c.benchmark_group("parse_large");
    for (name, input) in &large_inputs {
        bench_input(&mut large, name, input);
    }
    large.finish();

    let flow_inputs = [
        ("shallow_flow_heavy", shallow_flow_heavy(128)),
        ("wide_flow_1024", wide_flow_sequence(1_024)),
        ("nested_flow_32", nested_flow_sequence(32)),
        ("nested_flow_256", nested_flow_sequence(256)),
        ("nested_flow_1024", nested_flow_sequence(1_024)),
    ];
    let mut flow = c.benchmark_group("parse_flow");
    for (name, input) in &flow_inputs {
        flow.bench_with_input(
            BenchmarkId::new("yaml-rt", name),
            input,
            |bencher, input| {
                bencher.iter(|| {
                    let doc = YamlDoc::parse(black_box(input))
                        .expect("generated flow collection should parse");
                    black_box(doc);
                });
            },
        );
    }
    flow.finish();
}

fn bench_input(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: impl std::fmt::Display,
    input: &str,
) {
    group.bench_with_input(
        BenchmarkId::new("yaml-rt", &name),
        input,
        |bencher, input| {
            bencher.iter(|| {
                let doc = YamlDoc::parse(black_box(input)).expect("yaml-rt fixture should parse");
                black_box(doc);
            });
        },
    );

    #[cfg(feature = "rapidyaml-baseline")]
    {
        group.bench_with_input(
            BenchmarkId::new("rapidyaml-arena", &name),
            input,
            |bencher, input| {
                bencher.iter(|| {
                    let tree = rapidyaml::parse_in_arena(black_box(input))
                        .expect("Rapid YAML should parse the fixture in its arena");
                    black_box(tree);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("rapidyaml-in-place", &name),
            input,
            |bencher, input| {
                bencher.iter_batched(
                    || input.as_bytes().to_vec(),
                    |mut input| {
                        let tree = rapidyaml::parse_in_place(black_box(input.as_mut_slice()))
                            .expect("Rapid YAML should parse the fixture in place");
                        black_box(tree);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    #[cfg(feature = "saphyr-baseline")]
    group.bench_with_input(
        BenchmarkId::new("saphyr", &name),
        input,
        |bencher, input| {
            bencher.iter(|| {
                let docs =
                    Yaml::load_from_str(black_box(input)).expect("saphyr fixture should parse");
                black_box(docs);
            });
        },
    );
}

fn flat_mapping(entries: usize) -> String {
    let mut input = String::with_capacity(entries.saturating_mul(24));
    for index in 0..entries {
        writeln!(input, "key_{index:05}: value_{index:05}")
            .expect("writing to a String cannot fail");
    }
    input
}

fn shallow_flow_heavy(entries: usize) -> String {
    let mut input = String::with_capacity(entries.saturating_mul(32));
    input.push('[');
    for index in 0..entries {
        if index > 0 {
            input.push_str(", ");
        }
        write!(input, "{{key_{index}: [one, two]}}").expect("writing to a String cannot fail");
    }
    input.push(']');
    input
}

fn mixed_block_flow(entries: usize) -> String {
    let mut input = String::with_capacity(entries.saturating_mul(96));
    input.push_str("items:\n");
    for index in 0..entries {
        writeln!(input, "  - name: item_{index:05}").expect("writing to a String cannot fail");
        input.push_str("    enabled: true\n");
        input.push_str("    flags: [http, runtime, histograms]\n");
        input.push_str("    limits: {retries: 3, timeout_ms: 2000}\n");
    }
    input
}

fn wide_flow_sequence(entries: usize) -> String {
    let mut input = String::with_capacity(entries.saturating_mul(8));
    input.push('[');
    for index in 0..entries {
        if index > 0 {
            input.push_str(", ");
        }
        write!(input, "{index}").expect("writing to a String cannot fail");
    }
    input.push(']');
    input
}

fn nested_flow_sequence(depth: usize) -> String {
    let mut input = String::with_capacity(depth.saturating_mul(2).saturating_add(1));
    input.extend(std::iter::repeat_n('[', depth));
    input.push('0');
    input.extend(std::iter::repeat_n(']', depth));
    input
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
