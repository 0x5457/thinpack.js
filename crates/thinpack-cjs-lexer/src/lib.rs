//! Node's `merve` lexer for `CommonJS` named exports.
//!
//! Vendored from `deps/merve` at the Node tag this repository tracks (MIT). Node calls
//! the same lexer from `src/node_cjs_lexer.cc`; using it keeps the export set identical
//! to `cjs-module-lexer` instead of maintaining a JavaScript tokenizer.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::os::raw::c_char;

#[repr(C)]
#[derive(Clone, Copy)]
struct MerveString {
    data: *const c_char,
    length: usize,
}

#[repr(C)]
struct MerveErrorLoc {
    line: u32,
    column: u32,
}

// SAFETY: these are the C functions exported by the vendored `merve.cpp`; the
// signatures mirror `merve_c.h`.
unsafe extern "C" {
    fn merve_parse_commonjs(
        input: *const c_char,
        length: usize,
        out_err: *mut MerveErrorLoc,
    ) -> *mut c_void;
    fn merve_is_valid(result: *mut c_void) -> bool;
    fn merve_free(result: *mut c_void);
    fn merve_get_exports_count(result: *mut c_void) -> usize;
    fn merve_get_export_name(result: *mut c_void, index: usize) -> MerveString;
    fn merve_get_reexports_count(result: *mut c_void) -> usize;
    fn merve_get_reexport_name(result: *mut c_void, index: usize) -> MerveString;
}

/// The named exports and re-export specifiers a `CommonJS` source exposes.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    pub exports: BTreeSet<String>,
    pub reexports: Vec<String>,
}

/// Parse `source` the way Node's `cjs-module-lexer` does.
///
/// Invalid source produces an empty analysis, matching the previous tokenizer: the
/// module will fail when it executes, not while its exports are discovered.
#[must_use]
pub fn analyze(source: &str) -> Analysis {
    let mut error = MerveErrorLoc { line: 0, column: 0 };
    // A few token checks in the vendored lexer read one byte past `end`. A NUL
    // sentinel keeps that read inside this allocation; without it, the same source
    // can parse or fail depending on the bytes after the Rust buffer (the pinned
    // `pi` app's `partial-json` copy failed when it landed next to non-NUL data).
    let mut terminated = Vec::with_capacity(source.len() + 1);
    terminated.extend_from_slice(source.as_bytes());
    terminated.push(0);
    // SAFETY: the pointer/length pair describes `source` for the duration of the call,
    // and the returned handle is freed before this function returns.
    let handle =
        unsafe { merve_parse_commonjs(terminated.as_ptr().cast(), source.len(), &raw mut error) };
    // SAFETY: `handle` is either null or a live result owned by this function.
    if handle.is_null() || !unsafe { merve_is_valid(handle) } {
        // SAFETY: freeing a null handle is a no-op; a live invalid result is owned here.
        unsafe { merve_free(handle) };
        return Analysis::default();
    }

    let mut analysis = Analysis::default();
    // SAFETY: `handle` is a valid analysis result.
    let exports = unsafe { merve_get_exports_count(handle) };
    for index in 0..exports {
        // SAFETY: `index` is below the count just read, and the handle stays live.
        if let Some(name) = unsafe { string_at(merve_get_export_name(handle, index)) } {
            analysis.exports.insert(name);
        }
    }
    // SAFETY: `handle` is a valid analysis result.
    let reexports = unsafe { merve_get_reexports_count(handle) };
    for index in 0..reexports {
        // SAFETY: `index` is below the count just read, and the handle stays live.
        if let Some(name) = unsafe { string_at(merve_get_reexport_name(handle, index)) } {
            analysis.reexports.push(name);
        }
    }
    // SAFETY: the handle was created by `merve_parse_commonjs` and is freed once here.
    unsafe { merve_free(handle) };
    analysis
}

/// # Safety
///
/// `value` must come from a live `merve_analysis` handle.
unsafe fn string_at(value: MerveString) -> Option<String> {
    if value.data.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a live handle, whose strings stay valid until it is
    // freed; `length` is the byte length of that string.
    let bytes = unsafe { std::slice::from_raw_parts(value.data.cast::<u8>(), value.length) };
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::analyze;

    #[test]
    fn finds_named_exports_and_reexports() {
        let analysis = analyze(
            "module.exports.a = 1;\nmodule.exports.b = 2;\nmodule.exports = require('./cjs');\n",
        );
        assert!(analysis.exports.contains("a"));
        assert!(analysis.exports.contains("b"));
        assert_eq!(analysis.reexports, vec!["./cjs"]);
    }

    #[test]
    fn invalid_source_has_no_exports() {
        assert!(analyze("function (").exports.is_empty());
    }
}
