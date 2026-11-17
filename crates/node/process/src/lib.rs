mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "thinpack:node/process",
        deno_core::ascii_str_include!("process.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:process",
        deno_core::ascii_str_include!("process_module.js"),
    )];

    Extension {
        name: "thinpack_node_process",
        deps: &["thinpack_io", "thinpack_node_internals"],
        ops: Cow::Borrowed(OPS),
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
