use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use yaml_rt_core::YamlDoc;

struct CountingAllocator;

static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

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
    let input = flat_mapping(entries);

    reset_counters();
    ENABLED.store(true, Ordering::Relaxed);
    let started = Instant::now();
    for _ in 0..iterations {
        let doc = YamlDoc::parse(black_box(&input)).expect("generated mapping should parse");
        black_box(doc);
    }
    let elapsed = started.elapsed();
    ENABLED.store(false, Ordering::Relaxed);

    println!("entries: {entries}");
    println!("input bytes: {}", input.len());
    println!("iterations: {iterations}");
    println!(
        "time: {:.1} us/op",
        elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
    );
    println!(
        "allocations: {:.1}/op",
        ALLOCATIONS.load(Ordering::Relaxed) as f64 / iterations as f64
    );
    println!(
        "allocated bytes: {:.1}/op",
        ALLOCATED_BYTES.load(Ordering::Relaxed) as f64 / iterations as f64
    );
    println!("peak live bytes: {}", PEAK_BYTES.load(Ordering::Relaxed));
    println!("final live bytes: {}", LIVE_BYTES.load(Ordering::Relaxed));
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
        input.push_str("key_");
        input.push_str(&index.to_string());
        input.push_str(": value_");
        input.push_str(&index.to_string());
        input.push('\n');
    }
    input
}
