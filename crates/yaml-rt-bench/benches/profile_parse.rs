use std::env;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use yaml_rt_core::YamlDoc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Stream,
    Cases,
}

#[derive(Debug)]
struct Config {
    suite_dir: PathBuf,
    repeat: usize,
    mode: Mode,
}

#[derive(Debug)]
struct Fixture {
    id: String,
    input: String,
}

fn main() {
    let config = Config::parse();
    let fixtures = discover_fixtures(&suite_root(&config.suite_dir)).unwrap_or_else(|error| {
        panic!(
            "failed to discover YAML Test Suite fixtures below {}: {error}",
            config.suite_dir.display()
        )
    });

    assert!(
        !fixtures.is_empty(),
        "no non-error YAML Test Suite fixtures with in.yaml found below {}",
        config.suite_dir.display()
    );

    match config.mode {
        Mode::Stream => run_stream(&fixtures, config.repeat),
        Mode::Cases => run_cases(&fixtures, config.repeat),
    }
}

impl Config {
    fn parse() -> Self {
        let mut suite_dir = default_suite_dir();
        let mut repeat = 100usize;
        let mut mode = Mode::Cases;
        let mut args = env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--suite-dir" => {
                    let value = args
                        .next()
                        .unwrap_or_else(|| panic!("--suite-dir requires a path argument"));
                    suite_dir = PathBuf::from(value);
                }
                "--repeat" => {
                    let value = args
                        .next()
                        .unwrap_or_else(|| panic!("--repeat requires a numeric argument"));
                    repeat = value.parse().unwrap_or_else(|error| {
                        panic!("invalid --repeat value `{value}`: {error}")
                    });
                }
                "--mode" => {
                    let value = args
                        .next()
                        .unwrap_or_else(|| panic!("--mode requires stream or cases"));
                    mode = match value.as_str() {
                        "stream" => Mode::Stream,
                        "cases" => Mode::Cases,
                        _ => panic!("invalid --mode value `{value}`; expected stream or cases"),
                    };
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                "--bench" => {}
                _ => panic!("unknown argument `{arg}`; use --help for usage"),
            }
        }

        Self {
            suite_dir,
            repeat,
            mode,
        }
    }
}

fn print_help() {
    println!(
        "profile_parse [--suite-dir PATH] [--repeat N] [--mode stream|cases]\n\
         \n\
         Defaults:\n\
           --suite-dir ../../third_party/yaml-test-suite from this crate\n\
           --repeat 100\n\
           --mode cases"
    );
}

fn default_suite_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("third_party")
        .join("yaml-test-suite")
}

fn suite_root(path: &Path) -> PathBuf {
    let data = path.join("data");
    if data.is_dir() { data } else { path.to_owned() }
}

fn discover_fixtures(root: &Path) -> std::io::Result<Vec<Fixture>> {
    let mut fixtures = Vec::new();
    discover_fixtures_inner(root, &mut fixtures)?;
    fixtures.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(fixtures)
}

fn discover_fixtures_inner(dir: &Path, fixtures: &mut Vec<Fixture>) -> std::io::Result<()> {
    let input = dir.join("in.yaml");
    if input.is_file() {
        if !dir.join("error").is_file() {
            fixtures.push(Fixture {
                id: case_id(dir),
                input: fs::read_to_string(input)?,
            });
        }
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            discover_fixtures_inner(&entry.path(), fixtures)?;
        }
    }

    Ok(())
}

fn case_id(dir: &Path) -> String {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let parent = dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    if name.len() == 2 && name.bytes().all(|byte| byte.is_ascii_digit()) {
        format!("{parent}:{name}")
    } else {
        name.to_owned()
    }
}

fn run_stream(fixtures: &[Fixture], repeat: usize) {
    let input = suite_stream(fixtures);
    let total_bytes = input.len().saturating_mul(repeat);
    let started = Instant::now();

    for _ in 0..repeat {
        let doc = YamlDoc::parse(black_box(input.as_str())).expect("suite stream should parse");
        black_box(doc);
    }

    report(
        "stream",
        fixtures.len(),
        input.len(),
        repeat,
        total_bytes,
        started,
    );
}

fn run_cases(fixtures: &[Fixture], repeat: usize) {
    let input_bytes = fixtures
        .iter()
        .map(|fixture| fixture.input.len())
        .sum::<usize>();
    let total_bytes = input_bytes.saturating_mul(repeat);
    let started = Instant::now();

    for _ in 0..repeat {
        for fixture in fixtures {
            let doc = YamlDoc::parse(black_box(fixture.input.as_str()))
                .unwrap_or_else(|error| panic!("fixture {} should parse: {error}", fixture.id));
            black_box(doc);
        }
    }

    report(
        "cases",
        fixtures.len(),
        input_bytes,
        repeat,
        total_bytes,
        started,
    );
}

fn suite_stream(fixtures: &[Fixture]) -> String {
    let estimated_len = fixtures.iter().map(|fixture| fixture.input.len() + 5).sum();
    let mut stream = String::with_capacity(estimated_len);

    for fixture in fixtures {
        stream.push_str(&fixture.input);
        if !stream.ends_with('\n') {
            stream.push('\n');
        }
        stream.push_str("...\n");
    }

    stream
}

fn report(
    mode: &str,
    fixtures: usize,
    input_bytes: usize,
    repeat: usize,
    total_bytes: usize,
    started: Instant,
) {
    let elapsed = started.elapsed();
    let mib = count_as_f64(total_bytes) / (1024.0 * 1024.0);
    let throughput = mib / elapsed.as_secs_f64();

    println!("mode: {mode}");
    println!("fixtures: {fixtures}");
    println!("input bytes: {input_bytes}");
    println!("repeat: {repeat}");
    println!("total bytes: {total_bytes}");
    println!("elapsed: {elapsed:.3?}");
    println!("throughput: {throughput:.2} MiB/s");
}

fn count_as_f64(value: usize) -> f64 {
    // Benchmark byte counts are displayed approximately above `f64`'s exact
    // integer range.
    #[expect(
        clippy::cast_precision_loss,
        reason = "human-readable benchmark throughput may approximate byte counts above 53 bits"
    )]
    let converted = value as f64;
    converted
}
