#![no_main]

use libfuzzer_sys::fuzz_target;
use yaml_rt_core::ResourceLimits;
use yaml_rt_serde::{Value, from_str_with_limits};

fuzz_target!(|yaml: &str| {
    let limits = ResourceLimits {
        max_source_bytes: 64 * 1024,
        max_lines: 16 * 1024,
        max_nodes: 32 * 1024,
        max_collection_depth: 128,
        max_alias_chain: 32,
        max_expanded_nodes: 32 * 1024,
    };
    let _ = from_str_with_limits::<Value>(yaml, limits);
});
