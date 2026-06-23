use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use fyaml::Document;
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
        group.bench_with_input(
            BenchmarkId::new("rty", fixture.name),
            fixture.input,
            |bencher, input| {
                bencher.iter(|| {
                    let doc = YamlDoc::parse(black_box(input)).expect("RTY fixture should parse");
                    black_box(doc);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fyaml", fixture.name),
            fixture.input,
            |bencher, input| {
                bencher.iter(|| {
                    let doc =
                        Document::parse_str(black_box(input)).expect("fyaml fixture should parse");
                    black_box(doc);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("saphyr", fixture.name),
            fixture.input,
            |bencher, input| {
                bencher.iter(|| {
                    let docs =
                        Yaml::load_from_str(black_box(input)).expect("saphyr fixture should parse");
                    black_box(docs);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
