use parking_lot::Mutex;
use std::collections::HashMap;

/// Where API keys live: the macOS keychain (Linux: Secret Service), never a config file.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str) -> crate::Result<()>;
    fn delete(&self, key: &str) -> crate::Result<()>;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
const SERVICE: &str = "works.karma.livenotes";

/// Credentials stored in the operating system’s secret store.
#[derive(Default)]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn get(&self, key: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, key).ok()?.get_password().ok()
    }
    fn set(&self, key: &str, value: &str) -> crate::Result<()> {
        keyring::Entry::new(SERVICE, key).and_then(|e| e.set_password(value)).map_err(|e| crate::Error::Other(e.to_string()))
    }
    fn delete(&self, key: &str) -> crate::Result<()> {
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(crate::Error::Other(e.to_string())),
        }
    }
}

#[cfg(target_os = "linux")]
impl SecretStore for KeychainStore {
    fn get(&self, key: &str) -> Option<String> {
        let result = secret_tool("lookup", key).output().ok()?;
        if !result.status.success() {
            return None;
        }
        let text = String::from_utf8(result.stdout).ok()?;
        Some(text.strip_suffix('\n').unwrap_or(&text).to_owned())
    }

    fn set(&self, key: &str, value: &str) -> crate::Result<()> {
        use std::io::Write;
        use std::process::Stdio;

        let mut child =
            secret_tool("store", key).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(secret_error)?;
        let written = child.stdin.take().expect("secret-tool stdin is piped").write_all(value.as_bytes());
        let result = child.wait_with_output().map_err(secret_error)?;
        written.map_err(secret_error)?;
        if result.status.success() { Ok(()) } else { Err(secret_error(String::from_utf8_lossy(&result.stderr).trim())) }
    }

    fn delete(&self, key: &str) -> crate::Result<()> {
        let result = secret_tool("clear", key).output().map_err(secret_error)?;
        if result.status.success() || (result.status.code() == Some(1) && result.stderr.is_empty()) {
            Ok(())
        } else {
            Err(secret_error(String::from_utf8_lossy(&result.stderr).trim()))
        }
    }
}

#[cfg(target_os = "linux")]
fn secret_tool(operation: &str, key: &str) -> std::process::Command {
    use std::process::{Command, Stdio};

    let mut command = Command::new("secret-tool");
    command.arg(operation).stdin(Stdio::null());
    if operation == "store" {
        command.arg("--label=Live Notes");
    }
    command.args(["service", SERVICE, "account", key]);
    command
}

#[cfg(target_os = "linux")]
fn secret_error(error: impl std::fmt::Display) -> crate::Error {
    crate::Error::Other(format!("Secret Service: {error}"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl SecretStore for KeychainStore {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn set(&self, _: &str, _: &str) -> crate::Result<()> {
        Err(crate::Error::Unsupported)
    }
    fn delete(&self, _: &str) -> crate::Result<()> {
        Err(crate::Error::Unsupported)
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
