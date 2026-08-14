//! Benchmark-only native interface for embedding yaml-rt in external harnesses.

use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;
use std::str;

use yaml_rt_core::YamlDoc;

/// Parses and destroys a complete lossless yaml-rt document.
///
/// This function is intentionally limited to the unpublished benchmark crate.
/// It owns the same source copy as [`YamlDoc::parse`], keeps the parsed document
/// observable, and includes destruction in the measured call.
///
/// # Safety
///
/// When `length` is non-zero, `data` must point to `length` readable bytes and
/// remain valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaml_rt_bench_parse(data: *const u8, length: usize) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = if length == 0 {
            &[]
        } else {
            if data.is_null() {
                return false;
            }
            // SAFETY: the caller guarantees that `data` points to `length`
            // readable bytes for the duration of this call.
            unsafe { slice::from_raw_parts(data, length) }
        };
        let Ok(input) = str::from_utf8(bytes) else {
            return false;
        };
        let Ok(document) = YamlDoc::parse(input) else {
            return false;
        };
        black_box(document);
        true
    }))
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::yaml_rt_bench_parse;

    fn parse(input: &[u8]) -> bool {
        // SAFETY: `input` remains readable for the duration of the call.
        unsafe { yaml_rt_bench_parse(input.as_ptr(), input.len()) }
    }

    #[test]
    fn ffi_parses_valid_yaml() {
        assert!(parse(b"name: yaml-rt\nitems: [one, two]\n"));
    }

    #[test]
    fn ffi_rejects_invalid_yaml() {
        assert!(!parse(b"unterminated: [one, two\n"));
    }

    #[test]
    fn ffi_rejects_invalid_utf8() {
        assert!(!parse(&[0xff, 0xfe]));
    }

    #[test]
    fn ffi_accepts_an_empty_stream() {
        assert!(parse(b""));
    }

    #[test]
    fn ffi_rejects_a_null_non_empty_buffer() {
        // SAFETY: the function rejects a null pointer before reading it.
        assert!(!unsafe { yaml_rt_bench_parse(std::ptr::null(), 1) });
    }
}
