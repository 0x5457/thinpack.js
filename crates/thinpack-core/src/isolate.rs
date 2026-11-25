use std::fmt;

use crate::VirtualFilesRef;

/// The per-isolate inputs a capability may consume while building its extensions.
///
/// The runtime creates one for the main isolate and one for each worker. A host run has no
/// packed files, so a capability that mounts a payload sees `None` and stays on the host.
pub struct IsolateInputs {
    files: Option<VirtualFilesRef>,
}

impl IsolateInputs {
    #[must_use]
    pub const fn new(files: Option<VirtualFilesRef>) -> Self {
        Self { files }
    }

    /// The packed virtual file system, when this isolate runs from a payload.
    #[must_use]
    pub fn files(&self) -> Option<VirtualFilesRef> {
        self.files.clone()
    }
}

impl fmt::Debug for IsolateInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IsolateInputs")
            .field("packed", &self.files.is_some())
            .finish()
    }
}
