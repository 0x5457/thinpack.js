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
        ExtensionFileSource::new("node:fs", deno_core::ascii_str_include!("node-fs.js")),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/utils",
            deno_core::ascii_str_include!("node-internal-fs-utils.js"),
        ),
        ExtensionFileSource::new(
            "node:fs/promises",
            deno_core::ascii_str_include!("node-fs-promises.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/promises",
            deno_core::ascii_str_include!("node-internal-fs-promises.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/dir",
            deno_core::ascii_str_include!("node-internal-fs-dir.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/read/context",
            deno_core::ascii_str_include!("node-internal-fs-read-context.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/cp/cp",
            deno_core::ascii_str_include!("node-internal-fs-cp.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/cp/cp-sync",
            deno_core::ascii_str_include!("node-internal-fs-cp-sync.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/streams",
            deno_core::ascii_str_include!("node-internal-fs-streams.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/fs_dir",
            deno_core::ascii_str_include!("binding_fs_dir.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/fs",
            deno_core::ascii_str_include!("binding_fs.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/watchers",
            deno_core::ascii_str_include!("fs_watchers.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/recursive_watch",
            deno_core::ascii_str_include!("fs_recursive_watch.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/rimraf",
            deno_core::ascii_str_include!("fs_rimraf.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/process/permission",
            deno_core::ascii_str_include!("permission.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/readline/interface",
            deno_core::ascii_str_include!("readline_interface.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/fs/glob",
            deno_core::ascii_str_include!("glob.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/streams/fast-utf8-stream",
            deno_core::ascii_str_include!("fast-utf8-stream.js"),
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
