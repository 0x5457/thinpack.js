mod ops;

mod virtual_mount;

pub use virtual_mount::VirtualMount;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

// `glob` feature input: the extension's own JS (glob.js) calls the ops it adds.
use thinpack_node_internals as _;

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:fs", thinpack_js::js!("node-fs.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/utils",
            thinpack_js::js!("node-internal-fs-utils.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:fs/promises",
            thinpack_js::js!("node-fs-promises.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/promises",
            thinpack_js::js!("node-internal-fs-promises.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/dir",
            thinpack_js::js!("node-internal-fs-dir.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/read/context",
            thinpack_js::js!("node-internal-fs-read-context.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/cp/cp",
            thinpack_js::js!("node-internal-fs-cp.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/cp/cp-sync",
            thinpack_js::js!("node-internal-fs-cp-sync.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/streams",
            thinpack_js::js!("node-internal-fs-streams.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/fs_dir",
            thinpack_js::js!("binding_fs_dir.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/fs",
            thinpack_js::js!("binding_fs.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/watchers",
            thinpack_js::js!("fs_watchers.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/recursive_watch",
            thinpack_js::js!("fs_recursive_watch.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/rimraf",
            thinpack_js::js!("fs_rimraf.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/process/permission",
            thinpack_js::js!("permission.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/readline/interface",
            thinpack_js::js!("readline_interface.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/fs/glob",
            thinpack_js::js!("glob.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/streams/fast-utf8-stream",
            thinpack_js::js!("fast-utf8-stream.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_fs",
        deps: &[
            "thinpack_node_internals",
            "thinpack_node_buffer",
            "thinpack_node_events",
            "thinpack_node_path",
            "thinpack_node_string_decoder",
            "thinpack_node_stream",
            "thinpack_node_timers",
            "thinpack_node_util",
        ],
        ops: Cow::Borrowed(OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
