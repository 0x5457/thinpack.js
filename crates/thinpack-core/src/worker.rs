use std::collections::{HashMap, VecDeque};
use std::future::poll_fn;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::task::{Poll, Waker};

use deno_core::SharedArrayBufferStore;

use crate::Error;

pub type PortId = u32;

pub type WorkerId = u32;

pub fn shared_array_buffer_store() -> SharedArrayBufferStore {
    static STORE: OnceLock<SharedArrayBufferStore> = OnceLock::new();
    STORE.get_or_init(SharedArrayBufferStore::default).clone()
}

#[derive(Default)]
struct Inbox {
    messages: Mutex<VecDeque<Vec<u8>>>,
    waiters: Mutex<Vec<Waker>>,
    closed: AtomicBool,
}

impl Inbox {
    fn push(&self, message: Vec<u8>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        self.messages.lock().expect("port inbox").push_back(message);
        self.wake();
    }

    fn take(&self) -> Option<Vec<u8>> {
        self.messages.lock().expect("port inbox").pop_front()
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.messages.lock().expect("port inbox").clear();
        self.wake();
    }

    fn wake(&self) {
        for waker in drain_waiters(&self.waiters) {
            waker.wake();
        }
    }

    async fn next(&self) -> Option<Vec<u8>> {
        poll_fn(|context| {
            let mut waiters = self.waiters.lock().expect("port waiters");
            let message = self.messages.lock().expect("port inbox").pop_front();
            if let Some(message) = message {
                waiters.clear();
                return Poll::Ready(Some(message));
            }
            if self.closed.load(Ordering::SeqCst) {
                return Poll::Ready(None);
            }
            waiters.push(context.waker().clone());
            Poll::Pending
        })
        .await
    }
}

/// Drains a waker list without holding its lock across the wake-up calls.
fn drain_waiters(waiters: &Mutex<Vec<Waker>>) -> Vec<Waker> {
    std::mem::take(&mut *waiters.lock().expect("port waiters"))
}

struct Port {
    peer: PortId,
    inbox: Inbox,
}

fn ports() -> MutexGuard<'static, HashMap<PortId, Arc<Port>>> {
    static PORTS: OnceLock<Mutex<HashMap<PortId, Arc<Port>>>> = OnceLock::new();
    PORTS
        .get_or_init(Default::default)
        .lock()
        .expect("port table")
}

pub fn channel() -> (PortId, PortId) {
    static PORTS: crate::IdCounter = crate::IdCounter::new();
    let a = PORTS.next();
    let b = PORTS.next();
    let mut table = ports();
    table.insert(
        a,
        Arc::new(Port {
            peer: b,
            inbox: Inbox::default(),
        }),
    );
    table.insert(
        b,
        Arc::new(Port {
            peer: a,
            inbox: Inbox::default(),
        }),
    );
    (a, b)
}

#[must_use]
pub fn peer(id: PortId) -> Option<PortId> {
    ports().get(&id).map(|port| port.peer)
}

fn lookup(id: PortId) -> Option<Arc<Port>> {
    ports().get(&id).cloned()
}

pub fn send(id: PortId, message: Vec<u8>) {
    if let Some(port) = lookup(id) {
        port.inbox.push(message);
    }
}

#[must_use]
pub fn try_take(id: PortId) -> Option<Vec<u8>> {
    lookup(id)?.inbox.take()
}

pub async fn wait(id: PortId) -> Option<Vec<u8>> {
    match lookup(id) {
        Some(port) => port.inbox.next().await,
        None => None,
    }
}

pub fn close(id: PortId) {
    let port = ports().remove(&id);
    if let Some(port) = port {
        port.inbox.close();
    }
}

#[derive(Debug)]
pub struct WorkerSpec {
    pub script: PathBuf,
    pub data: Vec<u8>,
    pub parent_port: PortId,
    pub parent_peer: PortId,
    pub thread_id: WorkerId,
}

#[derive(Clone, Debug)]
pub struct WorkerContext {
    pub thread_id: WorkerId,
    pub port: PortId,
    pub peer: PortId,
    pub data: Vec<u8>,
}

impl WorkerSpec {
    #[must_use]
    pub fn context(&self) -> WorkerContext {
        WorkerContext {
            thread_id: self.thread_id,
            port: self.parent_port,
            peer: self.parent_peer,
            data: self.data.clone(),
        }
    }
}

type Boot = dyn Fn(WorkerSpec) -> Result<(), Error> + Send + Sync + 'static;

static BOOT: OnceLock<Box<Boot>> = OnceLock::new();

/// # Panics
///
/// Panics when a boot function was already installed.
pub fn install_boot(boot: impl Fn(WorkerSpec) -> Result<(), Error> + Send + Sync + 'static) {
    assert!(
        BOOT.set(Box::new(boot)).is_ok(),
        "worker boot is installed twice"
    );
}

#[derive(Default)]
struct WorkerState {
    parent_port: PortId,
    outcome: Mutex<Option<Result<(), String>>>,
    waiters: Mutex<Vec<Waker>>,
}

impl WorkerState {
    fn finish(&self, outcome: Result<(), String>) {
        *self.outcome.lock().expect("worker outcome") = Some(outcome);
        for waker in drain_waiters(&self.waiters) {
            waker.wake();
        }
    }

    async fn done(&self) {
        poll_fn(|context| {
            let mut waiters = self.waiters.lock().expect("worker waiters");
            if self.outcome.lock().expect("worker outcome").is_some() {
                waiters.clear();
                return Poll::Ready(());
            }
            waiters.push(context.waker().clone());
            Poll::Pending
        })
        .await;
    }

    fn error(&self) -> String {
        match &*self.outcome.lock().expect("worker outcome") {
            Some(Ok(())) | None => String::new(),
            Some(Err(message)) => message.clone(),
        }
    }
}

fn workers() -> MutexGuard<'static, HashMap<WorkerId, Arc<WorkerState>>> {
    static WORKERS: OnceLock<Mutex<HashMap<WorkerId, Arc<WorkerState>>>> = OnceLock::new();
    WORKERS
        .get_or_init(Default::default)
        .lock()
        .expect("workers")
}

pub fn next_thread_id() -> WorkerId {
    static THREADS: crate::IdCounter = crate::IdCounter::new();
    THREADS.next()
}

/// # Errors
///
/// Returns an error when worker support is not installed or the thread cannot be
/// started.
pub fn spawn(spec: WorkerSpec) -> Result<WorkerId, Error> {
    let boot = BOOT
        .get()
        .ok_or_else(|| Error::new("worker support is not installed"))?;
    let id = spec.thread_id;
    let parent_port = spec.parent_port;
    let state = Arc::new(WorkerState {
        parent_port,
        ..Default::default()
    });

    let child = Arc::clone(&state);
    std::thread::Builder::new()
        .name(format!("thinpack-worker-{id}"))
        .spawn(move || {
            let outcome = match boot(spec) {
                Ok(()) => Ok(()),
                Err(error) => Err(error.to_string()),
            };
            child.finish(outcome);
        })
        .map_err(|error| Error::new(format!("cannot start worker thread: {error}")))?;

    workers().insert(id, state);
    Ok(id)
}

pub async fn wait_exit(id: WorkerId) -> bool {
    let state = workers().get(&id).cloned();
    match state {
        Some(state) => {
            state.done().await;
            true
        }
        None => false,
    }
}

#[must_use]
pub fn worker_error(id: WorkerId) -> String {
    workers()
        .get(&id)
        .map(|state| state.error())
        .unwrap_or_default()
}

pub fn terminate(id: WorkerId) {
    let state = workers().get(&id).cloned();
    if let Some(state) = state {
        close(state.parent_port);
    }
}
