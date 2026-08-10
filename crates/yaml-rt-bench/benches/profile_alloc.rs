use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::fmt::Write;
use std::hint::black_box;
use std::mem::size_of;
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
    let entries = env::args().nth(1).map_or(1_000, |value| {
        value.parse().expect("entries must be numeric")
    });
    let iterations = env::args().nth(2).map_or(100, |value| {
        value.parse().expect("iterations must be numeric")
    });
    let mode_arg = env::args().nth(3);
    let mode = Mode::parse(mode_arg.as_deref());
    let input = flat_mapping(entries);

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
    println!("entries: {entries}");
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
        elapsed.as_secs_f64() * 1_000_000.0 / f64::from(iterations)
    );
    println!(
        "allocations: {:.1}/op",
        count_as_f64(allocations) / f64::from(iterations)
    );
    println!(
        "allocated bytes: {:.1}/op",
        count_as_f64(allocated_bytes) / f64::from(iterations)
    );
    println!("repeated peak live bytes: {repeated_peak_bytes}");
    println!("retained allocations: {retained_allocations}");
    println!("retained allocated bytes: {retained_allocated_bytes}");
    println!("retained peak live bytes: {retained_peak_bytes}");
    println!("retained bytes: {retained_bytes}");
}

fn count_as_f64(value: usize) -> f64 {
    // Benchmark reporting is approximate once counters exceed `f64`'s exact
    // integer range, which is acceptable for human-readable rates.
    #[allow(clippy::cast_precision_loss)]
    let converted = value as f64;
    converted
}

fn parse_input(mode: Mode, input: &str) -> Parsed {
    match mode {
        Mode::Full => Parsed::Full(
            YamlDoc::parse(input).expect("generated mapping should parse as a full document"),
        ),
        Mode::Cst => {
            let source = Source::new(input.to_owned()).expect("generated source should be valid");
            let nodes = parse_cst(&source).expect("generated mapping should parse as a CST");
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
