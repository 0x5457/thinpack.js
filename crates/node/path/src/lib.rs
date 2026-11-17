use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

// `glob` feature input: node-path.js loads the minimatch shims it adds.
use thinpack_node_internals as _;

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new("node:path", deno_core::ascii_str_include!("node-path.js")),
        ExtensionFileSource::new(
            "node:path/posix",
            deno_core::ascii_str!("module.exports = require('node:path').posix;\n"),
        ),
        ExtensionFileSource::new(
            "node:path/win32",
            deno_core::ascii_str!("module.exports = require('node:path').win32;\n"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/path/glob",
            deno_core::ascii_str_include!("glob_pattern.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_path",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
