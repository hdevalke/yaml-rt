use std::ffi::{c_char, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;

pub struct Tree<'input> {
    raw: NonNull<c_void>,
    input: PhantomData<&'input mut [u8]>,
}

impl Drop for Tree<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` comes from one of the parse functions below and is
        // owned by this handle, so it is deleted exactly once.
        unsafe { yaml_rt_rapidyaml_tree_delete(self.raw.as_ptr()) }
    }
}

pub fn parse_in_arena(input: &str) -> Option<Tree<'static>> {
    // SAFETY: Rapid YAML receives a valid pointer and length for the duration
    // of the call and copies the source into the returned tree's arena.
    let raw =
        unsafe { yaml_rt_rapidyaml_parse_in_arena(input.as_ptr().cast::<c_char>(), input.len()) };
    NonNull::new(raw).map(|raw| Tree {
        raw,
        input: PhantomData,
    })
}

pub fn parse_in_place(input: &mut [u8]) -> Option<Tree<'_>> {
    // SAFETY: the mutable buffer remains borrowed for the lifetime of the
    // returned tree, which may retain views into the modified source.
    let raw = unsafe {
        yaml_rt_rapidyaml_parse_in_place(input.as_mut_ptr().cast::<c_char>(), input.len())
    };
    NonNull::new(raw).map(|raw| Tree {
        raw,
        input: PhantomData,
    })
}

unsafe extern "C" {
    fn yaml_rt_rapidyaml_parse_in_arena(data: *const c_char, size: usize) -> *mut c_void;
    fn yaml_rt_rapidyaml_parse_in_place(data: *mut c_char, size: usize) -> *mut c_void;
    fn yaml_rt_rapidyaml_tree_delete(tree: *mut c_void);
}
