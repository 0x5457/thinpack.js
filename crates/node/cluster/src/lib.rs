mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::ops::OPS;

#[must_use]
pub fn init() -> Extension {
    // Runs before user code: a process spawned by a cluster primary installs its IPC channel
    // and `process.send` here, exactly where Node sets up the child side of the channel.
    const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "thinpack:node/cluster-bootstrap",
        thinpack_js::js!("cluster_bootstrap.js"),
    )];
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:cluster", thinpack_js::js!("cluster.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/cluster/primary",
            thinpack_js::js!("primary.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/cluster/child",
            thinpack_js::js!("child.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/cluster/worker",
            thinpack_js::js!("worker.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/cluster/channel",
            thinpack_js::js!("channel.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/cluster/worker_process",
            thinpack_js::js!("worker_process.js"),
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
