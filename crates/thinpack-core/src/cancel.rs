use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock};

use tokio::sync::oneshot;

pub struct CancelTable {
    senders: OnceLock<Mutex<HashMap<u32, oneshot::Sender<()>>>>,
}

impl fmt::Debug for CancelTable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CancelTable")
            .field("pending", &self.senders().len())
            .finish()
    }
}

impl Default for CancelTable {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelTable {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            senders: OnceLock::new(),
        }
    }

    fn senders(&self) -> MutexGuard<'_, HashMap<u32, oneshot::Sender<()>>> {
        self.senders
            .get_or_init(Default::default)
            .lock()
            .expect("cancel table")
    }

    pub fn register(&self, id: u32) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
        self.senders().insert(id, sender);
        receiver
    }

    pub fn remove(&self, id: u32) {
        self.senders().remove(&id);
    }

    #[must_use]
    pub fn cancel(&self, id: u32) -> bool {
        let Some(sender) = self.senders().remove(&id) else {
            return false;
        };
        sender.send(()).is_ok()
    }
}
