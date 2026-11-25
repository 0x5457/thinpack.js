mod assets;

mod backend;

mod builtins;
mod cancel;
mod counter;

mod error;

mod isolate;

mod js;

mod runtime;

mod virtual_files;

pub mod worker;

pub use assets::Assets;
pub use backend::{BACKEND, Backend};
pub use builtins::BuiltinSources;
pub use cancel::CancelTable;
pub use counter::IdCounter;
pub use error::Error;
pub use isolate::IsolateInputs;
pub use js::js_string_literal;
pub use runtime::{Runtime, file_url, path_from_url};
pub use virtual_files::{VirtualEntry, VirtualFiles, VirtualFilesRef, VirtualStat};

#[derive(Debug)]
pub struct Invocation(pub String);
