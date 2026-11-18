use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const STARTUP: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("thinpack:node/buffer", thinpack_js::js!("buffer.js")),
        ExtensionFileSource::new_compressed(
            "thinpack:node/buffer-globals",
            thinpack_js::js!("buffer_globals.js"),
        ),
    ];
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:buffer", thinpack_js::js!("node-buffer.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/buffer",
            thinpack_js::js!("node-internal-buffer.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/blob",
            thinpack_js::js!("node-internal-blob.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/file",
            thinpack_js::js!("node-internal-file.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/blob",
            thinpack_js::js!("binding_blob.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/webidl",
            thinpack_js::js!("node-internal-webidl.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/v8/startup_snapshot",
            thinpack_js::js!("startup_snapshot.js"),
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
