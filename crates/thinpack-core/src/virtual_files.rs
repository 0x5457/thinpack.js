use std::rc::Rc;

#[derive(Debug)]
pub struct VirtualStat {
    pub len: u64,
    pub is_file: bool,
    pub is_dir: bool,
}

#[derive(Debug)]
pub struct VirtualEntry {
    pub name: String,
    pub is_dir: bool,
}

pub trait VirtualFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    fn stat(&self, path: &str) -> Option<VirtualStat>;
    fn entries(&self, path: &str) -> Option<Vec<VirtualEntry>>;
}

pub type VirtualFilesRef = Rc<dyn VirtualFiles>;
