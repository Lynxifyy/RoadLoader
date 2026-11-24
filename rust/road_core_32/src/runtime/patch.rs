//! Dynamic offset registry.
//!
//! The 64-bit UI resolves a signature against the game binary and sends the
//! address down the command ring; the hook layer reads it back by name when it
//! installs an inline hook. The table has a fixed capacity and stores names
//! inline, so applying an offset never allocates - which matters in a process
//! that is one bad allocation away from being killed.

/// Longest accepted symbol name, including the terminator slot.
pub const MAX_NAME_BYTES: usize = 63;

/// Number of names the registry can hold at once.
pub const CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchError {
    /// A zero-length name.
    EmptyName,
    /// Longer than [`MAX_NAME_BYTES`].
    NameTooLong,
    /// A name with a byte outside `0x21..=0x7e`, e.g. a stray NUL.
    InvalidName,
    /// Address zero, or one that cannot exist in a 32-bit process.
    InvalidAddress,
    /// All [`CAPACITY`] slots are taken.
    Full,
}

impl PatchError {
    pub fn as_str(self) -> &'static str {
        match self {
            PatchError::EmptyName => "empty name",
            PatchError::NameTooLong => "name too long",
            PatchError::InvalidName => "invalid name",
            PatchError::InvalidAddress => "invalid address",
            PatchError::Full => "registry full",
        }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    name: [u8; MAX_NAME_BYTES],
    name_len: u8,
    address: u64,
}

impl Entry {
    fn name(&self) -> &str {
        // SAFETY: bytes came from a validated `&str`, so the slice is UTF-8.
        unsafe { core::str::from_utf8_unchecked(&self.name[..self.name_len as usize]) }
    }
}

/// Name-to-address table for resolved game functions.
pub struct PatchRegistry {
    entries: [Option<Entry>; CAPACITY],
    len: usize,
}

impl Default for PatchRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PatchRegistry {
    pub const fn new() -> Self {
        Self { entries: [None; CAPACITY], len: 0 }
    }

    /// Stores `address` under `name`. Returns `true` when an existing entry was
    /// replaced, `false` when a new one was added.
    pub fn apply(&mut self, name: &str, address: u64) -> Result<bool, PatchError> {
        validate_name(name)?;
        if address == 0 || address > u32::MAX as u64 {
            return Err(PatchError::InvalidAddress);
        }
        if let Some(slot) = self.find(name) {
            self.entries[slot] = Some(Self::entry(name, address));
            return Ok(true);
        }
        let slot = self
            .entries
            .iter()
            .position(|entry| entry.is_none())
            .ok_or(PatchError::Full)?;
        self.entries[slot] = Some(Self::entry(name, address));
        self.len += 1;
        Ok(false)
    }

    pub fn resolve(&self, name: &str) -> Option<u64> {
        self.find(name).map(|slot| self.entries[slot].expect("slot filled").address)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.entries = [None; CAPACITY];
        self.len = 0;
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, u64)> {
        self.entries
            .iter()
            .flatten()
            .map(|entry| (entry.name(), entry.address))
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|entry| {
            entry
                .as_ref()
                .is_some_and(|entry| entry.name() == name)
        })
    }

    fn entry(name: &str, address: u64) -> Entry {
        let mut buffer = [0u8; MAX_NAME_BYTES];
        buffer[..name.len()].copy_from_slice(name.as_bytes());
        Entry { name: buffer, name_len: name.len() as u8, address }
    }
}

fn validate_name(name: &str) -> Result<(), PatchError> {
    if name.is_empty() {
        return Err(PatchError::EmptyName);
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(PatchError::NameTooLong);
    }
    if !name.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(PatchError::InvalidName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_and_resolves() {
        let mut registry = PatchRegistry::new();
        assert_eq!(registry.apply("Level::tick", 0x1234_5678), Ok(false));
        assert_eq!(registry.resolve("Level::tick"), Some(0x1234_5678));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
        assert_eq!(registry.resolve("Actor::render"), None);
    }

    #[test]
    fn reapplying_replaces_in_place() {
        let mut registry = PatchRegistry::new();
        registry.apply("ScreenView::setup", 0x1000).expect("first");
        assert_eq!(registry.apply("ScreenView::setup", 0x2000), Ok(true));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.resolve("ScreenView::setup"), Some(0x2000));
    }

    #[test]
    fn rejects_bad_names_and_addresses() {
        let mut registry = PatchRegistry::new();
        assert_eq!(registry.apply("", 0x1000), Err(PatchError::EmptyName));
        assert_eq!(registry.apply(&"a".repeat(MAX_NAME_BYTES + 1), 0x1000), Err(PatchError::NameTooLong));
        assert_eq!(registry.apply("has space", 0x1000), Err(PatchError::InvalidName));
        assert_eq!(registry.apply("bad\0name", 0x1000), Err(PatchError::InvalidName));
        assert_eq!(registry.apply("ok", 0), Err(PatchError::InvalidAddress));
        assert_eq!(registry.apply("ok", u32::MAX as u64 + 1), Err(PatchError::InvalidAddress));
        assert!(registry.is_empty());
    }

    #[test]
    fn accepts_the_longest_name() {
        let mut registry = PatchRegistry::new();
        let name = "n".repeat(MAX_NAME_BYTES);
        assert_eq!(registry.apply(&name, 0xffff_ffff), Ok(false));
        assert_eq!(registry.resolve(&name), Some(0xffff_ffff));
    }

    #[test]
    fn fills_up_and_reports_full() {
        let mut registry = PatchRegistry::new();
        for index in 0..CAPACITY {
            registry.apply(&format!("fn{index}"), 0x1000 + index as u64).expect("slot");
        }
        assert_eq!(registry.len(), CAPACITY);
        assert_eq!(registry.apply("overflow", 0x1000), Err(PatchError::Full));
        // Replacing an existing name still works when the table is full.
        assert_eq!(registry.apply("fn0", 0x9000), Ok(true));
        assert_eq!(registry.resolve("fn0"), Some(0x9000));
    }

    #[test]
    fn clear_empties_the_table() {
        let mut registry = PatchRegistry::new();
        registry.apply("a", 0x10).expect("apply");
        registry.clear();
        assert!(registry.is_empty());
        assert_eq!(registry.resolve("a"), None);
        assert_eq!(registry.entries().count(), 0);
    }

    #[test]
    fn iterates_entries() {
        let mut registry = PatchRegistry::new();
        registry.apply("a", 0x10).expect("a");
        registry.apply("b", 0x20).expect("b");
        let mut names: Vec<&str> = registry.entries().map(|(name, _)| name).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["a", "b"]);
    }
}
