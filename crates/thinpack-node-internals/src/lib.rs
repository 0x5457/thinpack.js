mod ops;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::ops::OPS;

const STARTUP: &[ExtensionFileSource] = &[ExtensionFileSource::new(
    "thinpack:node-internals",
    deno_core::ascii_str_include!("primordials.js"),
)];

const BUILTINS: &[ExtensionFileSource] = &[
    ExtensionFileSource::new(
        "node:thinpack/internal/assert",
        deno_core::ascii_str_include!("assert.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/binding/config",
        deno_core::ascii_str_include!("binding_config.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/binding/constants",
        deno_core::ascii_str_include!("binding_constants.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/binding/buffer",
        deno_core::ascii_str_include!("binding_buffer.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/binding/types",
        deno_core::ascii_str_include!("binding_types.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/binding/util",
        deno_core::ascii_str_include!("binding_util.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/constants",
        deno_core::ascii_str_include!("constants.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/encoding",
        deno_core::ascii_str_include!("encoding.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/worker/js_transferable",
        deno_core::ascii_str_include!("worker_js_transferable.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/errors",
        deno_core::ascii_str_include!("errors.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/options",
        deno_core::ascii_str_include!("options.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/streams/utils",
        deno_core::ascii_str_include!("streams_utils.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/util",
        deno_core::ascii_str_include!("util.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/util/colors",
        deno_core::ascii_str_include!("util_colors.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/util/debuglog",
        deno_core::ascii_str_include!("util_debuglog.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/util/comparisons",
        deno_core::ascii_str_include!("comparisons.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/util/inspect",
        deno_core::ascii_str_include!("util_inspect.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/url",
        deno_core::ascii_str_include!("url.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/net/address",
        deno_core::ascii_str_include!("net_address.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/validators",
        deno_core::ascii_str_include!("validators.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/abort_controller",
        deno_core::ascii_str_include!("abort_controller_module.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/async_hooks",
        deno_core::ascii_str_include!("async_hooks.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/process/task_queues",
        deno_core::ascii_str_include!("process_task_queues.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/events/abort_listener",
        deno_core::ascii_str_include!("events_abort_listener.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/event_target",
        deno_core::ascii_str_include!("event_target.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/fixed_queue",
        deno_core::ascii_str_include!("fixed_queue.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new(
        "node:thinpack/internal/deps/minimatch/index",
        deno_core::ascii_str_include!("minimatch.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new(
        "node:thinpack/internal/deps/minimatch/ast",
        deno_core::ascii_str_include!("minimatch_ast.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new(
        "node:thinpack/internal/deps/minimatch/brace-expressions",
        deno_core::ascii_str_include!("minimatch_brace_expressions.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new(
        "node:thinpack/internal/deps/minimatch/escape",
        deno_core::ascii_str_include!("minimatch_escape.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new(
        "node:thinpack/internal/deps/minimatch/unescape",
        deno_core::ascii_str_include!("minimatch_unescape.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new(
        "node:thinpack/internal/deps/minimatch/assert-valid-pattern",
        deno_core::ascii_str_include!("minimatch_assert_valid_pattern.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new(
        "node:thinpack/internal/deps/brace-expansion",
        deno_core::ascii_str_include!("brace_expansion.js"),
    ),
    #[cfg(feature = "glob")]
    ExtensionFileSource::new(
        "node:thinpack/internal/deps/balanced-match",
        deno_core::ascii_str_include!("balanced_match.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/querystring",
        deno_core::ascii_str_include!("querystring.js"),
    ),
    ExtensionFileSource::new(
        "node:thinpack/internal/util/types",
        deno_core::ascii_str_include!("util_types.js"),
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
