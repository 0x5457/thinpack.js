use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2, v8};
use deno_error::JsErrorBox;

use crate::resources::Resources;

#[op2(nofast, reentrant)]
pub(crate) fn op_sqlite_backup<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] source: u32,
    #[string] path: &str,
    #[string] source_name: &str,
    #[string] target_name: &str,
    #[smi] rate: u32,
    progress: Option<v8::Local<'s, v8::Value>>,
) -> Result<(), JsErrorBox> {
    Resources::backup(
        scope,
        &state,
        source,
        path,
        source_name,
        target_name,
        rate,
        progress,
    )
}
