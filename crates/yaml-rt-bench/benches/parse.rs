use std::fmt::Write;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
#[cfg(feature = "fyaml-baseline")]
use fyaml::Document;
#[cfg(feature = "saphyr-baseline")]
use saphyr::{LoadableYamlNode, Yaml};
use yaml_rt_core::YamlDoc;

struct Fixture {
    name: &'static str,
    input: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "small_config",
        input: r#"host: localhost
ports:
  - 8080
  - 9090
enabled: true
"#,
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
        input: r#"literal: |
  first line
  second line

  indented paragraph
folded: >
  folded line
  continues here

  next paragraph
"#,
    },
    Fixture {
        name: "multi_document",
        input: r#"---
name: first
items:
  - one
  - two
---
name: second
settings: {mode: fast, retries: 3}
...
"#,
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
}

fn bench_input(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: impl std::fmt::Display,
    input: &str,
) {
    group.bench_with_input(BenchmarkId::new("rty", &name), input, |bencher, input| {
        bencher.iter(|| {
            let doc = YamlDoc::parse(black_box(input)).expect("RTY fixture should parse");
            black_box(doc);
        });
    });

    #[cfg(feature = "fyaml-baseline")]
    group.bench_with_input(BenchmarkId::new("fyaml", &name), input, |bencher, input| {
        bencher.iter(|| {
            let doc = Document::parse_str(black_box(input)).expect("fyaml fixture should parse");
            black_box(doc);
        });
    });

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

criterion_group!(benches, bench_parse);
criterion_main!(benches);
