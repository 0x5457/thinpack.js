mod assets;

mod backend;

mod builtins;
mod cancel;
mod counter;

mod error;

mod runtime;

mod virtual_files;

pub mod worker;

pub use assets::Assets;
pub use backend::{BACKEND, Backend};
pub use builtins::BuiltinSources;
pub use cancel::CancelTable;
pub use counter::IdCounter;
pub use error::Error;
pub use runtime::Runtime;
pub use virtual_files::{VirtualEntry, VirtualFiles, VirtualFilesRef, VirtualStat};

#[derive(Debug)]
pub struct Invocation(pub String);
