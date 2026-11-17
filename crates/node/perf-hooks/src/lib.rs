use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

mod ops;

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "thinpack:node-perf-globals",
        deno_core::ascii_str_include!("perf_globals.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:perf_hooks",
        deno_core::ascii_str_include!("perf_hooks.js"),
    )];

    Extension {
        name: "thinpack_node_perf_hooks",
        deps: &["thinpack_node_internals"],
        ops: Cow::Borrowed(OPS),
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
