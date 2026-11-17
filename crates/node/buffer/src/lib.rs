use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "thinpack:node/buffer",
            deno_core::ascii_str_include!("buffer.js"),
        ),
        ExtensionFileSource::new(
            "thinpack:node/buffer-globals",
            deno_core::ascii_str_include!("buffer_globals.js"),
        ),
    ];
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "node:buffer",
            deno_core::ascii_str_include!("node-buffer.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/buffer",
            deno_core::ascii_str_include!("node-internal-buffer.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/blob",
            deno_core::ascii_str_include!("node-internal-blob.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/file",
            deno_core::ascii_str_include!("node-internal-file.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/blob",
            deno_core::ascii_str_include!("binding_blob.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/webidl",
            deno_core::ascii_str_include!("node-internal-webidl.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/v8/startup_snapshot",
            deno_core::ascii_str_include!("startup_snapshot.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_buffer",
        deps: &["thinpack_node_internals"],
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
