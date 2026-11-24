#![allow(
    unused_crate_dependencies,
    reason = "integration tests only link the dependencies they exercise"
)]
// The system-JSC backend cannot wrap a shared backing store in a SharedArrayBuffer through
// JSC's public C API, and v8x's serializer copies SAB bytes instead of transferring ownership.
// These cases test that engine primitive, so they only run on QuickJS and V8.
#![cfg(not(feature = "backend-jsc"))]

use std::borrow::Cow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use deno_core::{Extension, JsRuntime, OpDecl, RuntimeOptions, op2};

// Both tests below drive the same process-global mailbox and counters, so they
// must not overlap; cargo runs tests in one binary on parallel threads.
static SERIAL: Mutex<()> = Mutex::new(());

static MAILBOX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static RECORDED: AtomicU32 = AtomicU32::new(u32::MAX);
static DELIVERED: AtomicU32 = AtomicU32::new(0);

#[op2(fast)]
fn op_mailbox_send(#[buffer(copy)] data: Vec<u8>) {
    *MAILBOX.lock().unwrap() = data;
}

#[op2]
#[buffer]
fn op_mailbox_take() -> Vec<u8> {
    std::mem::take(&mut *MAILBOX.lock().unwrap())
}

#[op2(fast)]
fn op_record(value: u32) {
    RECORDED.store(value, Ordering::SeqCst);
}

#[op2(fast)]
fn op_deliver(value: u32) {
    DELIVERED.store(value, Ordering::SeqCst);
}

fn mailbox_extension() -> Extension {
    let ops: Vec<OpDecl> = vec![
        op_mailbox_send(),
        op_mailbox_take(),
        op_record(),
        op_deliver(),
    ];
    Extension {
        name: "thinpack_test_mailbox",
        ops: Cow::Owned(ops),
        ..Default::default()
    }
}

fn runtime() -> JsRuntime {
    JsRuntime::try_new(RuntimeOptions {
        extensions: vec![mailbox_extension()],
        shared_array_buffer_store: Some(thinpack_core::worker::shared_array_buffer_store()),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn shared_array_buffer_travels_through_serialize() {
    let _serial = SERIAL.lock().unwrap();
    let mut main = runtime();
    let mut worker = runtime();

    main.execute_script(
        "send",
        "const sab = new SharedArrayBuffer(8);
         Atomics.store(new Int32Array(sab), 0, 7);
         Deno.core.ops.op_mailbox_send(Deno.core.serialize({ sab }));"
            .to_owned(),
    )
    .unwrap();

    worker
        .execute_script(
            "receive",
            "const { sab } = Deno.core.deserialize(Deno.core.ops.op_mailbox_take());
             const view = new Int32Array(sab);
             Deno.core.ops.op_record(Atomics.load(view, 0));
             Atomics.store(view, 0, 9);"
                .to_owned(),
        )
        .unwrap();

    assert_eq!(
        RECORDED.load(Ordering::SeqCst),
        7,
        "the receiver must see the sender's writes"
    );

    main.execute_script(
        "observe",
        "Deno.core.ops.op_record(Atomics.load(new Int32Array(sab), 0));".to_owned(),
    )
    .unwrap();
    assert_eq!(
        RECORDED.load(Ordering::SeqCst),
        9,
        "the sender must see the receiver's writes"
    );
}

#[test]
fn atomics_wait_is_woken_across_isolates() {
    let _serial = SERIAL.lock().unwrap();
    let worker = std::thread::spawn(|| {
        let mut runtime = runtime();
        loop {
            runtime
                .execute_script(
                    "receive",
                    "(function() {
                       const received = Deno.core.ops.op_mailbox_take();
                       if (received.length === 0) {
                         Deno.core.ops.op_record(0);
                       } else {
                         const { sab } = Deno.core.deserialize(received);
                         const view = new Int32Array(sab);
                         Atomics.store(view, 0, 1);
                         Atomics.notify(view, 0, 1);
                         Deno.core.ops.op_deliver(1);
                       }
                     })();"
                        .to_owned(),
                )
                .unwrap();
            if DELIVERED.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    let mut main = runtime();
    main.execute_script(
        "wait",
        "const sab = new SharedArrayBuffer(8);
         Deno.core.ops.op_mailbox_send(Deno.core.serialize({ sab }));
         const view = new Int32Array(sab);
         const status = Atomics.wait(view, 0, 0, 5000);
         Deno.core.ops.op_record(
           Atomics.load(view, 0) === 1 && (status === 'ok' || status === 'not-equal') ? 42 : 0
         );"
        .to_owned(),
    )
    .unwrap();

    assert_eq!(
        RECORDED.load(Ordering::SeqCst),
        42,
        "the wait must be woken by the other isolate"
    );
    worker.join().unwrap();
}
