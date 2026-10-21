use parking_lot::Mutex;
use std::collections::HashMap;

/// Where API keys live: the macOS keychain (Linux: Secret Service), never a config file.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str) -> crate::Result<()>;
    fn delete(&self, key: &str) -> crate::Result<()>;
}

const SERVICE: &str = "works.karma.livenotes";

/// System keychain via the `keyring` crate.
#[derive(Default)]
pub struct KeychainStore;

impl SecretStore for KeychainStore {
    fn get(&self, key: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, key).ok()?.get_password().ok()
    }
    fn set(&self, key: &str, value: &str) -> crate::Result<()> {
        keyring::Entry::new(SERVICE, key)
            .and_then(|e| e.set_password(value))
            .map_err(|e| crate::Error::Other(e.to_string()))
    }
    fn delete(&self, key: &str) -> crate::Result<()> {
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(crate::Error::Other(e.to_string())),
        }
    }
}

/// In-memory store for tests and the `LN_TEST_*_KEY` variables.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> crate::Result<()> {
        self.0.lock().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> crate::Result<()> {
        self.0.lock().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_store() {
        let s = MemoryStore::default();
        s.set("openrouter", "k").unwrap();
        assert_eq!(s.get("openrouter").as_deref(), Some("k"));
        s.delete("openrouter").unwrap();
        assert!(s.get("openrouter").is_none());
    }
}
