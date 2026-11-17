#![allow(
    unused_crate_dependencies,
    reason = "integration tests only link the dependencies they exercise"
)]

#[cfg(not(feature = "backend-v8"))]
use deno_core::{JsRuntime, RuntimeOptions, v8};

#[cfg(not(feature = "backend-v8"))]
fn install(rt: &mut JsRuntime, store: &v8::SharedRef<v8::BackingStore>) {
    let context = rt.main_context();
    v8::scope!(let handle_scope, rt.v8_isolate());
    let context = v8::Local::new(handle_scope, context);
    let scope = &mut v8::ContextScope::new(handle_scope, context);
    let global = context.global(scope);
    let sab = v8::SharedArrayBuffer::with_backing_store(scope, store);
    let key = v8::String::new(scope, "sab").unwrap();
    global.set(scope, key.into(), sab.into());
}

#[cfg(not(feature = "backend-v8"))]
#[test]
fn shared_array_buffer_crosses_isolates() {
    let mut rt_a = JsRuntime::new(RuntimeOptions::default());
    let mut rt_b = JsRuntime::new(RuntimeOptions::default());

    let store = {
        let mut rt = JsRuntime::new(RuntimeOptions::default());
        let context = rt.main_context();
        v8::scope!(let handle_scope, rt.v8_isolate());
        let context = v8::Local::new(handle_scope, context);
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        v8::SharedArrayBuffer::new_backing_store(scope, 8).make_shared()
    };
    install(&mut rt_a, &store);
    install(&mut rt_b, &store);

    rt_a.execute_script("a", "Atomics.store(new Int32Array(sab), 0, 42)")
        .unwrap();
    let value = rt_b
        .execute_script("b", "Atomics.load(new Int32Array(sab), 0)")
        .unwrap();
    let value = {
        let context = rt_b.main_context();
        v8::scope!(let handle_scope, rt_b.v8_isolate());
        let context = v8::Local::new(handle_scope, context);
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        v8::Local::new(scope, value).int32_value(scope).unwrap()
    };
    assert_eq!(value, 42, "shared stores must be visible across isolates");
}

#[cfg(not(feature = "backend-v8"))]
#[test]
fn atomics_wait_notify_across_threads() {
    let store = {
        let mut rt = JsRuntime::new(RuntimeOptions::default());
        let context = rt.main_context();
        v8::scope!(let handle_scope, rt.v8_isolate());
        let context = v8::Local::new(handle_scope, context);
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        v8::SharedArrayBuffer::new_backing_store(scope, 8).make_shared()
    };

    let worker_store = store.clone();
    let worker = std::thread::spawn(move || {
        let mut rt = JsRuntime::new(RuntimeOptions::default());
        install(&mut rt, &worker_store);
        // Slot 0 is the wait word and stays 0; the notify is repeated until the
        // waiter acknowledges on slot 1, so a notify that lands before the wait
        // has started cannot turn the result into "not-equal" or lose the wakeup.
        rt.execute_script(
            "notify",
            "const view = new Int32Array(sab);
             for (;;) {
               Atomics.notify(view, 0, 1);
               if (Atomics.load(view, 1) === 1) break;
             }",
        )
        .unwrap();
    });

    let mut rt = JsRuntime::new(RuntimeOptions::default());
    install(&mut rt, &store);
    let status = rt
        .execute_script(
            "wait",
            "const status = Atomics.wait(new Int32Array(sab), 0, 0, 5000);
             Atomics.store(new Int32Array(sab), 1, 1);
             status",
        )
        .unwrap();
    let status = {
        let context = rt.main_context();
        v8::scope!(let handle_scope, rt.v8_isolate());
        let context = v8::Local::new(handle_scope, context);
        let scope = &mut v8::ContextScope::new(handle_scope, context);
        v8::Local::new(scope, status).to_rust_string_lossy(scope)
    };
    worker.join().unwrap();
    assert_eq!(status, "ok", "Atomics.wait must be woken by another thread");
}
