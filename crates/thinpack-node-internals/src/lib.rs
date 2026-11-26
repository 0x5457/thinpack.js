mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::ops::OPS;

const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
    "thinpack:node-internals",
    thinpack_js::js!("primordials.js"),
)];

const BUILTINS: &[ExtensionFileSource] = &[
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/assert",
        thinpack_js::js!("assert.js"),
    ),
    // `assert` and `util.diff` both require the diff helper. It lives here because
    // internals is linked unconditionally, while `assert` is an optional capability.
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/assert/myers_diff",
        thinpack_js::js!("myers_diff.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/binding/config",
        thinpack_js::js!("binding_config.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/binding/constants",
        thinpack_js::js!("binding_constants.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/binding/buffer",
        thinpack_js::js!("binding_buffer.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/binding/types",
        thinpack_js::js!("binding_types.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/binding/util",
        thinpack_js::js!("binding_util.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/constants",
        thinpack_js::js!("constants.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/encoding",
        thinpack_js::js!("encoding.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/worker/js_transferable",
        thinpack_js::js!("worker_js_transferable.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/errors",
        thinpack_js::js!("errors.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/options",
        thinpack_js::js!("options.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/streams/utils",
        thinpack_js::js!("streams_utils.js"),
    ),
    ExtensionFileSource::new_compressed("node:thinpack/internal/tty", thinpack_js::js!("tty.js")),
    ExtensionFileSource::new_compressed("node:thinpack/internal/util", thinpack_js::js!("util.js")),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/util/colors",
        thinpack_js::js!("util_colors.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/util/debuglog",
        thinpack_js::js!("util_debuglog.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/util/comparisons",
        thinpack_js::js!("comparisons.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/util/inspect",
        thinpack_js::js!("util_inspect.js"),
    ),
    ExtensionFileSource::new_compressed("node:thinpack/internal/url", thinpack_js::js!("url.js")),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/net/address",
        thinpack_js::js!("net_address.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/validators",
        thinpack_js::js!("validators.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/abort_controller",
        thinpack_js::js!("abort_controller_module.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/async_hooks",
        thinpack_js::js!("async_hooks.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/process/task_queues",
        thinpack_js::js!("process_task_queues.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/events/abort_listener",
        thinpack_js::js!("events_abort_listener.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/event_target",
        thinpack_js::js!("event_target.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/fixed_queue",
        thinpack_js::js!("fixed_queue.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/deps/minimatch/index",
        thinpack_js::js!("minimatch.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/deps/minimatch/ast",
        thinpack_js::js!("minimatch_ast.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/deps/minimatch/brace-expressions",
        thinpack_js::js!("minimatch_brace_expressions.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/deps/minimatch/escape",
        thinpack_js::js!("minimatch_escape.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/deps/minimatch/unescape",
        thinpack_js::js!("minimatch_unescape.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/deps/minimatch/assert-valid-pattern",
        thinpack_js::js!("minimatch_assert_valid_pattern.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/deps/brace-expansion",
        thinpack_js::js!("brace_expansion.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/deps/balanced-match",
        thinpack_js::js!("balanced_match.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/querystring",
        thinpack_js::js!("querystring.js"),
    ),
    ExtensionFileSource::new_compressed(
        "node:thinpack/internal/util/types",
        thinpack_js::js!("util_types.js"),
    ),
];

#[must_use]
pub fn init() -> Extension {
    Extension {
        name: "thinpack_node_internals",
        ops: Cow::Borrowed(OPS),
        js_files: Cow::Borrowed(STARTUP),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
