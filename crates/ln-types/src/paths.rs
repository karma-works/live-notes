use std::path::{Path, PathBuf};

/// All locations the app reads or writes. `LN_DATA_DIR` and `LN_NOTES_DIR`
/// override the defaults, which keeps tests and benchmarks away from user data.
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// App support directory: index, settings, models, journal.
    pub data_dir: PathBuf,
    /// Default notes folder (`~/Documents/Live Notes`).
    pub default_notes_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl AppPaths {
    pub fn from_env() -> Self {
        let data_dir = std::env::var_os("LN_DATA_DIR").map(PathBuf::from).unwrap_or_else(default_data_dir);
        let default_notes_dir = std::env::var_os("LN_NOTES_DIR").map(PathBuf::from).unwrap_or_else(|| {
            dirs::document_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Documents")).join("Live Notes")
        });
        let logs_dir = if cfg!(target_os = "macos") && std::env::var_os("LN_DATA_DIR").is_none() {
            dirs::home_dir().unwrap_or_default().join("Library/Logs/Live Notes")
        } else {
            data_dir.join("logs")
        };
        AppPaths { data_dir, default_notes_dir, logs_dir }
    }

    /// Paths rooted in one directory, for tests.
    pub fn in_dir(root: &Path) -> Self {
        AppPaths { data_dir: root.join("data"), default_notes_dir: root.join("notes"), logs_dir: root.join("logs") }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data_dir.join("settings.toml")
    }
    pub fn index_db(&self) -> PathBuf {
        self.data_dir.join("index.sqlite")
    }
    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }
    pub fn journal_dir(&self) -> PathBuf {
        self.data_dir.join("journal")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [&self.data_dir, &self.logs_dir, &self.models_dir(), &self.journal_dir()] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

fn default_data_dir() -> PathBuf {
    // macOS: ~/Library/Application Support/Live Notes; Linux: $XDG_DATA_HOME/live-notes
    if cfg!(target_os = "macos") {
        dirs::data_dir().unwrap_or_default().join("Live Notes")
    } else {
        dirs::data_dir().unwrap_or_default().join("live-notes")
    }
}
