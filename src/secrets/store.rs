//! Where secret values live. Values never leave a `Zeroizing` buffer except on a child's stdin.
use crate::error::Result;
use std::collections::BTreeMap;
use std::sync::Mutex;
use zeroize::Zeroizing;

pub type Secret = Zeroizing<Vec<u8>>;

pub trait SecretStore: Send + Sync {
    fn get(&self, project: &str, key: &str) -> Result<Option<Secret>>;
    fn set(&self, project: &str, key: &str, value: &[u8]) -> Result<()>;
    /// True if something was deleted.
    fn delete(&self, project: &str, key: &str) -> Result<bool>;
}

/// Test double.
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<(String, String), Vec<u8>>>);

impl SecretStore for MemoryStore {
    fn get(&self, project: &str, key: &str) -> Result<Option<Secret>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(project.into(), key.into()))
            .cloned()
            .map(Zeroizing::new))
    }
    fn set(&self, project: &str, key: &str, value: &[u8]) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert((project.into(), key.into()), value.to_vec());
        Ok(())
    }
    fn delete(&self, project: &str, key: &str) -> Result<bool> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .remove(&(project.into(), key.into()))
            .is_some())
    }
}
