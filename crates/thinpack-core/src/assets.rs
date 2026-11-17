use std::collections::BTreeMap;

#[derive(Debug)]
pub struct Assets {
    packed: bool,
    entries: BTreeMap<String, Vec<u8>>,
}

impl Assets {
    #[must_use]
    pub const fn host() -> Self {
        Self {
            packed: false,
            entries: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn packed() -> Self {
        Self {
            packed: true,
            entries: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: String, contents: Vec<u8>) {
        self.entries.insert(key, contents);
    }

    #[must_use]
    pub const fn is_packed(&self) -> bool {
        self.packed
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.entries.get(key).map(Vec::as_slice)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}
