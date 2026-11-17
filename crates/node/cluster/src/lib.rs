mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    // Runs before user code: a process spawned by a cluster primary installs its IPC channel
    // and `process.send` here, exactly where Node sets up the child side of the channel.
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "thinpack:node/cluster-bootstrap",
        deno_core::ascii_str_include!("cluster_bootstrap.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new("node:cluster", deno_core::ascii_str_include!("cluster.js")),
        ExtensionFileSource::new(
            "node:thinpack/internal/cluster/primary",
            deno_core::ascii_str_include!("primary.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/cluster/child",
            deno_core::ascii_str_include!("child.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/cluster/worker",
            deno_core::ascii_str_include!("worker.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/cluster/channel",
            deno_core::ascii_str_include!("channel.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/cluster/worker_process",
            deno_core::ascii_str_include!("worker_process.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_cluster",
        deps: &[
            "thinpack_node_events",
            "thinpack_node_net",
            "thinpack_node_path",
            "thinpack_node_process",
            "thinpack_node_stream",
        ],
        ops: Cow::Borrowed(OPS),
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
