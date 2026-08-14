use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::fmt::Write;
use std::fs;
use std::hint::black_box;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use yaml_rt_core::{Node, SemanticKind, Source, YamlDoc, YamlEvent, parse_cst};

struct CountingAllocator;

static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
enum Mode {
    Full,
    Cst,
}

impl Mode {
    fn parse(value: Option<&str>) -> Self {
        match value {
            None | Some("full") => Self::Full,
            Some("cst") => Self::Cst,
            Some(value) => panic!("mode must be `full` or `cst`, got `{value}`"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Cst => "cst",
        }
    }
}

enum Parsed {
    Full(YamlDoc),
    Cst { source: Source, nodes: Vec<Node> },
}

impl Parsed {
    fn retained_units(&self) -> usize {
        match self {
            Self::Full(doc) => doc.source().len(),
            Self::Cst { source, nodes } => source.len().saturating_add(nodes.len()),
        }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && ENABLED.load(Ordering::Relaxed) {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if ENABLED.load(Ordering::Relaxed) {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() && ENABLED.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size, Ordering::Relaxed);
            if new_size >= layout.size() {
                update_live(new_size - layout.size());
            } else {
                LIVE_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_pointer
    }
}

fn record_allocation(bytes: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED_BYTES.fetch_add(bytes, Ordering::Relaxed);
    update_live(bytes);
}

fn update_live(bytes: usize) {
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
}

fn main() {
    let config = Config::from_args();
    let input = config.input;
    let iterations = config.iterations;
    let mode = config.mode;

    reset_counters();
    ENABLED.store(true, Ordering::Relaxed);
    let started = Instant::now();
    for _ in 0..iterations {
        let parsed = parse_input(mode, black_box(&input));
        black_box(parsed);
    }
    let elapsed = started.elapsed();
    ENABLED.store(false, Ordering::Relaxed);
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
    let repeated_peak_bytes = PEAK_BYTES.load(Ordering::Relaxed);

    reset_counters();
    ENABLED.store(true, Ordering::Relaxed);
    let retained = parse_input(mode, black_box(&input));
    let retained_allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let retained_allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
    let retained_peak_bytes = PEAK_BYTES.load(Ordering::Relaxed);
    let retained_bytes = LIVE_BYTES.load(Ordering::Relaxed);
    ENABLED.store(false, Ordering::Relaxed);
    black_box(retained.retained_units());

    println!("mode: {}", mode.name());
    println!("workload: {}", config.workload);
    println!("input bytes: {}", input.len());
    println!("iterations: {iterations}");
    println!(
        "type sizes: Node={} SemanticKind={} YamlEvent={}",
        size_of::<Node>(),
        size_of::<SemanticKind>(),
        size_of::<YamlEvent>()
    );
    println!(
        "time: {:.1} us/op",
        elapsed.as_secs_f64() * 1_000_000.0 / count_as_f64(iterations)
    );
    println!(
        "allocations: {:.1}/op",
        count_as_f64(allocations) / count_as_f64(iterations)
    );
    println!(
        "allocated bytes: {:.1}/op",
        count_as_f64(allocated_bytes) / count_as_f64(iterations)
    );
    println!("repeated peak live bytes: {repeated_peak_bytes}");
    println!("retained allocations: {retained_allocations}");
    println!("retained allocated bytes: {retained_allocated_bytes}");
    println!("retained peak live bytes: {retained_peak_bytes}");
    println!("retained bytes: {retained_bytes}");
}

struct Config {
    input: String,
    workload: String,
    iterations: usize,
    mode: Mode,
}

#[derive(Clone, Copy)]
enum Shape {
    Flat,
    MixedFlow,
    Flow,
    Quoted,
    Block,
    Json,
}

impl Shape {
    fn parse(value: &str) -> Self {
        match value {
            "flat" => Self::Flat,
            "mixed-flow" => Self::MixedFlow,
            "flow" => Self::Flow,
            "quoted" => Self::Quoted,
            "block" => Self::Block,
            "json" => Self::Json,
            _ => panic!("shape must be flat, mixed-flow, flow, quoted, block, or json"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::MixedFlow => "mixed-flow",
            Self::Flow => "flow",
            Self::Quoted => "quoted",
            Self::Block => "block",
            Self::Json => "json",
        }
    }

    fn generate(self, entries: usize) -> String {
        match self {
            Self::Flat => flat_mapping(entries),
            Self::MixedFlow => mixed_block_flow(entries),
            Self::Flow => shallow_flow_heavy(entries),
            Self::Quoted => quoted_scalars(entries),
            Self::Block => block_scalars(entries),
            Self::Json => json_objects(entries),
        }
    }
}

impl Config {
    fn from_args() -> Self {
        let mut args = env::args().skip(1).filter(|argument| argument != "--bench");
        let first = args.next();
        if first.as_deref() == Some("--file") {
            let path = args.next().expect("--file requires a UTF-8 input path");
            let requested_path = PathBuf::from(&path);
            let resolved_path = if requested_path.is_relative() && !requested_path.exists() {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(&requested_path)
            } else {
                requested_path
            };
            let input =
                fs::read_to_string(resolved_path).expect("benchmark input must be readable UTF-8");
            let iterations = parse_count(args.next(), 100, "iterations");
            let mode_arg = args.next();
            assert!(
                args.next().is_none(),
                "unexpected allocation-profiler argument"
            );
            return Self {
                input,
                workload: format!("file {path}"),
                iterations,
                mode: Mode::parse(mode_arg.as_deref()),
            };
        }

        if first.as_deref() == Some("--shape") {
            let shape = Shape::parse(
                &args
                    .next()
                    .expect("--shape requires a generated workload name"),
            );
            let entries = parse_count(args.next(), 1_000, "entries");
            let iterations = parse_count(args.next(), 100, "iterations");
            let mode_arg = args.next();
            assert!(
                args.next().is_none(),
                "unexpected allocation-profiler argument"
            );
            return Self {
                input: shape.generate(entries),
                workload: format!("generated {} ({entries} entries)", shape.name()),
                iterations,
                mode: Mode::parse(mode_arg.as_deref()),
            };
        }

        let entries = parse_count(first, 1_000, "entries");
        let iterations = parse_count(args.next(), 100, "iterations");
        let mode_arg = args.next();
        assert!(
            args.next().is_none(),
            "unexpected allocation-profiler argument"
        );
        Self {
            input: flat_mapping(entries),
            workload: format!("generated flat mapping ({entries} entries)"),
            iterations,
            mode: Mode::parse(mode_arg.as_deref()),
        }
    }
}

fn parse_count(value: Option<String>, default: usize, name: &str) -> usize {
    value.map_or(default, |value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be numeric"))
    })
}

fn count_as_f64(value: usize) -> f64 {
    // Benchmark reporting is approximate once counters exceed `f64`'s exact
    // integer range, which is acceptable for human-readable rates.
    #[expect(
        clippy::cast_precision_loss,
        reason = "human-readable benchmark rates may approximate counters above 53 bits"
    )]
    let converted = value as f64;
    converted
}

fn parse_input(mode: Mode, input: &str) -> Parsed {
    match mode {
        Mode::Full => Parsed::Full(
            YamlDoc::parse(input).expect("allocation-profiler input should parse as a document"),
        ),
        Mode::Cst => {
            let source = Source::new(input.to_owned()).expect("profiler source should be valid");
            let nodes = parse_cst(&source).expect("profiler input should parse as a CST");
            Parsed::Cst { source, nodes }
        }
    }
}

fn reset_counters() {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_BYTES.store(0, Ordering::Relaxed);
}

fn flat_mapping(entries: usize) -> String {
    let mut input = String::with_capacity(entries.saturating_mul(24));
    for index in 0..entries {
        writeln!(input, "key_{index:05}: value_{index:05}")
            .expect("writing to a String cannot fail");
    }
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

fn quoted_scalars(entries: usize) -> String {
    let mut input = String::with_capacity(entries.saturating_mul(72));
    for index in 0..entries {
        writeln!(
            input,
            "double_{index}: \"Unicode café value {index}\" # comment"
        )
        .expect("writing to a String cannot fail");
        writeln!(input, "single_{index}: 'presentation # value {index}'")
            .expect("writing to a String cannot fail");
    }
    input
}

fn block_scalars(entries: usize) -> String {
    let mut input = String::with_capacity(entries.saturating_mul(96));
    for index in 0..entries {
        writeln!(input, "literal_{index}: |-").expect("writing to a String cannot fail");
        writeln!(input, "  first line {index}").expect("writing to a String cannot fail");
        input.push_str("  second line\n");
        writeln!(input, "folded_{index}: >").expect("writing to a String cannot fail");
        input.push_str("  folded text\n  continues here\n\n");
    }
    input
}

fn json_objects(entries: usize) -> String {
    let mut input = String::with_capacity(entries.saturating_mul(72));
    input.push('[');
    for index in 0..entries {
        if index > 0 {
            input.push(',');
        }
        write!(
            input,
            "{{\"id\":{index},\"name\":\"item {index}\",\"enabled\":true}}"
        )
        .expect("writing to a String cannot fail");
    }
    input.push(']');
    input
}
