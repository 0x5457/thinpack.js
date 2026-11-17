// `#[op2]` signatures take their arguments by value — the op ABI, not this
// crate, decides — so most op parameters are not consumed in the body.
#![expect(
    clippy::needless_pass_by_value,
    reason = "op2 op arguments are owned by the op ABI"
)]
// `#[op2]` hands each op a `&mut v8::PinScope`, and read-only helpers keep that
// same type so the scope has one spelling across the crate. Spelling the hidden
// lifetimes (as `elided_lifetimes_in_paths` requires) also makes
// `needless_pass_by_ref_mut` flag the ops that only use V8's shared APIs, and an
// `#[op2]` signature cannot carry a lint attribute of its own.
#![expect(
    clippy::needless_pass_by_ref_mut,
    reason = "`&mut v8::PinScope` is the scope type the op layer hands out"
)]
// The `node:sqlite` op arguments are positional, so flag-heavy ops such as
// `open` cannot be split without changing the JavaScript-facing ABI.
#![expect(
    clippy::fn_params_excessive_bools,
    reason = "op arity and order are fixed by the `node:sqlite` JavaScript API"
)]

mod authorizer;

mod backup;

mod callbacks;

mod errors;

mod extras;

mod functions;

mod modules;

mod ops;

mod session;

use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

use crate::callbacks::Pending;
use crate::ops::Handles;

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:sqlite",
        deno_core::ascii_str_include!("sqlite.js"),
    )];

    Extension {
        name: "thinpack_node_sqlite",
        deps: &["thinpack_node_internals"],
        ops: Cow::Borrowed(ops::OPS),
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        op_state_fn: Some(Box::new(|state| {
            state.put(Handles::default());
            state.put(Pending::default());
        })),
        ..Default::default()
    }
}
