// Copyright 2018-2026 the Deno authors. MIT license.
//
// thinpack patch: synchronous ES module loading, for CJS `require(esm)`.
//
// `mod_evaluate_sync` already covers the evaluated and TLA cases; this drives
// `RecursiveModuleLoad` synchronously (our loader always returns `ModuleLoadResponse::Sync`)
// and returns the module namespace. It shares the `ModuleMap` with regular `import`, so
// caching and cycle semantics match.

use std::future::Future;
use std::task::Context;
use std::task::Poll;
use std::task::Waker;

use futures::stream::StreamExt;

use crate::error::CoreError;
use crate::error::CoreErrorKind;
use crate::error::exception_to_err;
use crate::module_specifier::ModuleSpecifier;
use crate::modules::RequestedModuleType;
use crate::modules::recursive_load::RecursiveModuleLoad;
use crate::modules::recursive_load::SideModuleKind;
use crate::runtime::jsrealm::JsRealm;

/// Synchronously loads and evaluates an ES module graph, returning the module namespace.
///
/// Used for CJS `require(esm)`: the loader must be synchronous (`ModuleLoadResponse::Sync`)
/// and the graph must not contain top-level await (`mod_evaluate_sync` reports
/// `CoreErrorKind::TLA`).
pub fn require_esm(
    scope: &mut v8::PinScope,
    specifier: &str,
) -> Result<v8::Global<v8::Object>, CoreError> {
    let specifier = ModuleSpecifier::parse(specifier)?;
    let module_map = JsRealm::module_map_from(scope);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);

    // Already-evaluated modules (or ones being evaluated, e.g. a cycle) yield their namespace
    // directly instead of being evaluated twice.
    if let Some(id) = module_map.get_id(specifier.as_str(), RequestedModuleType::None) {
        match module_map
            .get_handle(id)
            .map(|handle| v8::Local::<v8::Module>::new(scope, handle).get_status())
        {
            Some(v8::ModuleStatus::Evaluated) => {
                return module_map.get_module_namespace(scope, id);
            }
            Some(v8::ModuleStatus::Evaluating) => {
                let handle = module_map.get_handle(id).expect("module exists");
                let module = v8::Local::<v8::Module>::new(scope, handle);
                let namespace = v8::Local::<v8::Object>::try_from(module.get_module_namespace())?;
                return Ok(v8::Global::new(scope, namespace));
            }
            _ => {}
        }
    }

    let load = RecursiveModuleLoad::side(
        specifier.to_string(),
        module_map.clone(),
        SideModuleKind::Sync,
        None,
    );
    let mut load = std::pin::pin!(load);
    let mut load = match load.as_mut().poll(&mut context) {
        Poll::Ready(Ok(load)) => load,
        Poll::Ready(Err(error)) => return Err(error),
        // An async loader cannot be used from a synchronous require.
        Poll::Pending => return Err(CoreErrorKind::TLA.into_box()),
    };

    loop {
        match load.poll_next_unpin(&mut context) {
            Poll::Ready(Some(Ok((request, source)))) => {
                load.register_and_recurse(scope, &request, source)
                    .map_err(|error| error.into_error(scope, false, false))?;
            }
            Poll::Ready(Some(Err(error))) => return Err(error),
            Poll::Ready(None) => break,
            Poll::Pending => return Err(CoreErrorKind::TLA.into_box()),
        }
    }

    let id = load
        .root_module_id()
        .ok_or_else(|| CoreErrorKind::MissingFromModuleMap(specifier.to_string()).into_box())?;

    module_map
        .instantiate_module(scope, id)
        .map_err(|exception| {
            let exception = v8::Local::new(scope, exception);
            CoreErrorKind::Js(exception_to_err(scope, exception, false, false)).into_box()
        })?;
    module_map.mod_evaluate_sync(scope, id)?;
    module_map.get_module_namespace(scope, id)
}
