//! The loader thread for `module.register` hooks.
//!
//! Node runs these hooks on a loader thread because `resolve` is requested through
//! an interface that must answer synchronously while a hook may await I/O. Thinpack
//! gives them their own isolate and event loop for the same reason: the main loader
//! blocks until the hook isolate answers, and the hook isolate can drive `fs.promises`,
//! timers and sockets without touching the app's isolate.
//!
//! `module.registerHooks` deliberately does not come here. Those hooks are synchronous
//! and share the app's isolate, as in Node.

use std::fmt;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use deno_core::error::{CoreError, CoreErrorKind};
use deno_core::{
    Extension, JsRuntime, ModuleLoader, ModuleSpecifier, PollEventLoopOptions, RuntimeOptions, v8,
};
use deno_error::JsErrorClass;
use thinpack_core::{Assets, BuiltinSources};

use crate::error::HookError;

/// The pieces a hook isolate needs. The runtime is built on the hook thread because a
/// `JsRuntime` belongs to the thread that created it.
pub struct HookRuntimeParts {
    pub extensions: Vec<Extension>,
    pub builtins: BuiltinSources,
    pub assets: Assets,
    pub loader: Rc<dyn ModuleLoader>,
}

impl fmt::Debug for HookRuntimeParts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookRuntimeParts")
            .field("extensions", &self.extensions.len())
            .finish_non_exhaustive()
    }
}

/// Builds a hook isolate on the hook thread. `rt` supplies the capability extensions and
/// payload, so the loader does not depend on the runtime template.
pub type HookRuntimeFactory = Box<dyn Fn() -> Result<HookRuntimeParts, String> + Send + 'static>;

enum Command {
    Register {
        url: String,
        data: Vec<u8>,
    },
    Resolve {
        specifier: String,
        parent: String,
        attributes: String,
        conditions: String,
    },
    Load {
        url: String,
        format: String,
        attributes: String,
        conditions: String,
    },
}

enum Reply {
    Register(Result<(), HookError>),
    Resolve(Result<String, HookError>),
    Load(Result<(String, String), HookError>),
}

pub(crate) struct HookHost {
    commands: Sender<Command>,
    replies: Receiver<Reply>,
    active: bool,
}

impl HookHost {
    pub(crate) fn start(factory: HookRuntimeFactory) -> Result<Self, HookError> {
        let (commands, command_rx) = mpsc::channel();
        let (reply_tx, replies) = mpsc::channel();
        thread::Builder::new()
            .name("thinpack-esm-hooks".to_owned())
            .spawn(move || run(&factory, &command_rx, &reply_tx))
            .map_err(|error| {
                HookError::new(
                    "Error",
                    format!("cannot start the ESM loader hook thread: {error}"),
                )
            })?;
        Ok(Self {
            commands,
            replies,
            active: false,
        })
    }

    pub(crate) const fn active(&self) -> bool {
        self.active
    }

    pub(crate) fn register(&mut self, url: &str, data: Vec<u8>) -> Result<(), HookError> {
        let reply = self.exchange(Command::Register {
            url: url.to_owned(),
            data,
        })?;
        match reply {
            Reply::Register(result) => {
                result?;
                self.active = true;
                Ok(())
            }
            _ => Err(bridge_failure()),
        }
    }

    pub(crate) fn resolve(
        &self,
        specifier: &str,
        parent: &str,
        attributes: &str,
        conditions: &str,
    ) -> Result<String, HookError> {
        let reply = self.exchange(Command::Resolve {
            specifier: specifier.to_owned(),
            parent: parent.to_owned(),
            attributes: attributes.to_owned(),
            conditions: conditions.to_owned(),
        })?;
        match reply {
            Reply::Resolve(result) => result,
            _ => Err(bridge_failure()),
        }
    }

    pub(crate) fn load(
        &self,
        url: &str,
        format: &str,
        attributes: &str,
        conditions: &str,
    ) -> Result<(String, String), HookError> {
        let reply = self.exchange(Command::Load {
            url: url.to_owned(),
            format: format.to_owned(),
            attributes: attributes.to_owned(),
            conditions: conditions.to_owned(),
        })?;
        match reply {
            Reply::Load(result) => result,
            _ => Err(bridge_failure()),
        }
    }

    fn exchange(&self, command: Command) -> Result<Reply, HookError> {
        self.commands.send(command).map_err(|_| stopped())?;
        self.replies.recv().map_err(|_| stopped())
    }
}

fn bridge_failure() -> HookError {
    HookError::new(
        "Error",
        "the ESM loader hook bridge returned the wrong reply",
    )
}

fn stopped() -> HookError {
    HookError::new("Error", "the ESM loader hook thread stopped")
}

fn run(factory: &HookRuntimeFactory, commands: &Receiver<Command>, replies: &Sender<Reply>) {
    let mut state = None;
    let mut failure = None;
    while let Ok(command) = commands.recv() {
        let reply = match command {
            Command::Register { url, data } => Reply::Register(
                ensure_runtime(&mut state, factory, &mut failure)
                    .and_then(|runtime| runtime.register(&url, &data)),
            ),
            Command::Resolve {
                specifier,
                parent,
                attributes,
                conditions,
            } => Reply::Resolve(ensure_runtime(&mut state, factory, &mut failure).and_then(
                |runtime| runtime.resolve(&specifier, &parent, &attributes, &conditions),
            )),
            Command::Load {
                url,
                format,
                attributes,
                conditions,
            } => Reply::Load(
                ensure_runtime(&mut state, factory, &mut failure)
                    .and_then(|runtime| runtime.load(&url, &format, &attributes, &conditions)),
            ),
        };
        if replies.send(reply).is_err() {
            break;
        }
    }
}

fn ensure_runtime<'a>(
    state: &'a mut Option<HookRuntime>,
    factory: &HookRuntimeFactory,
    failure: &'a mut Option<HookError>,
) -> Result<&'a mut HookRuntime, HookError> {
    if let Some(runtime) = state {
        return Ok(runtime);
    }
    if let Some(error) = failure {
        return Err(error.clone());
    }
    let result = factory()
        .map_err(|message| HookError::new("Error", message))
        .and_then(HookRuntime::new);
    match result {
        Ok(runtime) => Ok(state.insert(runtime)),
        Err(error) => {
            *failure = Some(error.clone());
            Err(error)
        }
    }
}

struct HookRuntime {
    js: JsRuntime,
    tokio: tokio::runtime::Runtime,
    add: v8::Global<v8::Function>,
    resolve: v8::Global<v8::Function>,
    load: v8::Global<v8::Function>,
}

impl HookRuntime {
    fn new(parts: HookRuntimeParts) -> Result<Self, HookError> {
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                HookError::new(
                    "Error",
                    format!("cannot start the ESM loader hook event loop: {error}"),
                )
            })?;
        let mut js = tokio
            .block_on(async {
                JsRuntime::try_new(RuntimeOptions {
                    extensions: parts.extensions,
                    module_loader: Some(parts.loader),
                    shared_array_buffer_store: Some(
                        thinpack_core::worker::shared_array_buffer_store(),
                    ),
                    ..Default::default()
                })
            })
            .map_err(|error| core_error(&error))?;
        {
            let state = js.op_state();
            let mut state = state.borrow_mut();
            state.put(parts.builtins);
            state.put(parts.assets);
        }
        let add = global_function(&mut js, "add")?;
        let resolve = global_function(&mut js, "resolve")?;
        let load = global_function(&mut js, "load")?;
        Ok(Self {
            js,
            tokio,
            add,
            resolve,
            load,
        })
    }

    fn register(&mut self, url: &str, data: &[u8]) -> Result<(), HookError> {
        let specifier = ModuleSpecifier::parse(url)
            .map_err(|error| HookError::new("Error", error.to_string()))?;
        let Self { js, tokio, add, .. } = self;
        let reply = tokio
            .block_on(async {
                let module = js.load_side_es_module(&specifier).await?;
                let evaluation = js.mod_evaluate(module);
                js.run_event_loop(PollEventLoopOptions::default()).await?;
                evaluation.await?;
                let namespace = js.get_module_namespace(module)?;
                let (namespace, data) = {
                    deno_core::scope!(scope, js);
                    let namespace: v8::Local<'_, v8::Value> =
                        v8::Local::<v8::Object>::new(scope, namespace).into();
                    let namespace = v8::Global::new(scope, namespace);
                    (namespace, blob(scope, data))
                };
                let args = vec![namespace, data];
                let call = js.call_with_args(add, &args);
                js.with_event_loop_promise(call, PollEventLoopOptions::default())
                    .await
            })
            .map_err(|error| core_error(&error))?;
        let reply = take_string(js, reply);
        if reply == "ok" {
            Ok(())
        } else {
            Err(describe_reply(&reply))
        }
    }

    fn resolve(
        &mut self,
        specifier: &str,
        parent: &str,
        attributes: &str,
        conditions: &str,
    ) -> Result<String, HookError> {
        let function = self.resolve.clone();
        let reply = self.request(&function, &[specifier, parent, attributes, conditions])?;
        decode_reply(&reply)
    }

    fn load(
        &mut self,
        url: &str,
        format: &str,
        attributes: &str,
        conditions: &str,
    ) -> Result<(String, String), HookError> {
        let function = self.load.clone();
        let reply = self.request(&function, &[url, format, attributes, conditions])?;
        decode_load(&reply)
    }

    fn request(
        &mut self,
        function: &v8::Global<v8::Function>,
        args: &[&str],
    ) -> Result<String, HookError> {
        let Self { js, tokio, .. } = self;
        let args = {
            deno_core::scope!(scope, js);
            args.iter()
                .map(|argument| {
                    let argument: v8::Local<'_, v8::Value> = v8::String::new(scope, argument)
                        .expect("a Rust string is valid UTF-16")
                        .into();
                    v8::Global::new(scope, argument)
                })
                .collect::<Vec<_>>()
        };
        let reply = tokio
            .block_on(async {
                let call = js.call_with_args(function, &args);
                js.with_event_loop_promise(call, PollEventLoopOptions::default())
                    .await
            })
            .map_err(|error| core_error(&error))?;
        Ok(take_string(js, reply))
    }
}

fn global_function(js: &mut JsRuntime, name: &str) -> Result<v8::Global<v8::Function>, HookError> {
    let value = js
        .execute_script(
            "<thinpack:esm-hooks>",
            format!("__thinpack_hook_runtime__.{name}"),
        )
        .map_err(|error| js_error(&error))?;
    deno_core::scope!(scope, js);
    let value = v8::Local::new(scope, value);
    let function = v8::Local::<v8::Function>::try_from(value).map_err(|_| {
        HookError::new(
            "Error",
            format!("the ESM loader hook bridge has no {name} function"),
        )
    })?;
    Ok(v8::Global::new(scope, function))
}

fn take_string(js: &mut JsRuntime, value: v8::Global<v8::Value>) -> String {
    deno_core::scope!(scope, js);
    v8::Local::new(scope, value).to_rust_string_lossy(scope)
}

fn blob(scope: &v8::PinScope<'_, '_>, bytes: &[u8]) -> v8::Global<v8::Value> {
    let buffer = v8::ArrayBuffer::new(scope, bytes.len());
    if !bytes.is_empty() {
        let data = buffer
            .data()
            .expect("a fresh ArrayBuffer has a backing store");
        // SAFETY: the buffer was just allocated with `bytes.len()` bytes and no other
        // handle aliases its backing store yet.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data.as_ptr().cast::<u8>(), bytes.len());
        }
    }
    let view = v8::Uint8Array::new(scope, buffer, 0, bytes.len())
        .expect("the view is in bounds of the buffer");
    let view: v8::Local<'_, v8::Value> = view.into();
    v8::Global::new(scope, view)
}

fn decode_reply(reply: &str) -> Result<String, HookError> {
    reply
        .strip_prefix("ok\0")
        .map(str::to_owned)
        .ok_or_else(|| describe_reply(reply))
}

fn decode_load(reply: &str) -> Result<(String, String), HookError> {
    let rest = reply
        .strip_prefix("ok\0")
        .ok_or_else(|| describe_reply(reply))?;
    let (format, source) = rest
        .split_once('\0')
        .ok_or_else(|| HookError::new("Error", "the load hook bridge returned no source"))?;
    Ok((format.to_owned(), source.to_owned()))
}

fn describe_reply(reply: &str) -> HookError {
    let Some(rest) = reply.strip_prefix("err\0") else {
        return HookError::new(
            "Error",
            format!("the ESM loader hook bridge returned an unexpected reply: {reply}"),
        );
    };
    let mut parts = rest.splitn(4, '\0');
    let class = parts.next().unwrap_or_default();
    let message = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    let stack = parts.next().unwrap_or_default();
    HookError {
        class: if class.is_empty() {
            "Error".to_owned()
        } else {
            class.to_owned()
        },
        message: message.to_owned(),
        code: (!code.is_empty()).then(|| code.to_owned()),
        stack: (!stack.is_empty()).then(|| stack.to_owned()),
    }
}

fn core_error(error: &CoreError) -> HookError {
    match error.0.as_ref() {
        CoreErrorKind::Js(error) => js_error(error),
        _ => HookError::new("Error", error.to_string()),
    }
}

fn js_error(error: &deno_core::error::JsError) -> HookError {
    let code = error
        .additional_properties
        .iter()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.clone());
    HookError {
        class: error
            .name
            .clone()
            .unwrap_or_else(|| JsErrorClass::get_class(error).into_owned()),
        message: error
            .message
            .clone()
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| error.exception_message.clone()),
        code,
        stack: error.stack.clone(),
    }
}
