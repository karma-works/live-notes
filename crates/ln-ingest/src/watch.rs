//! File system watching (FSEvents on macOS, inotify on Linux) through `notify`.

use crossbeam_channel::{Receiver, Sender, unbounded};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Created or modified.
    Changed(PathBuf),
    Removed(PathBuf),
}

pub struct FolderWatcher {
    watcher: RecommendedWatcher,
    pub events: Receiver<WatchEvent>,
}

impl FolderWatcher {
    pub fn new() -> notify::Result<Self> {
        let (tx, rx): (Sender<WatchEvent>, _) = unbounded();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            use notify::EventKind::*;
            for p in ev.paths {
                let e = match ev.kind {
                    Remove(_) => WatchEvent::Removed(p),
                    Create(_) | Modify(_) => {
                        if p.exists() {
                            WatchEvent::Changed(p)
                        } else {
                            WatchEvent::Removed(p)
                        }
                    }
                    _ => continue,
                };
                let _ = tx.send(e);
            }
        })?;
        Ok(FolderWatcher { watcher, events: rx })
    }

    pub fn watch(&mut self, path: &Path, recursive: bool) -> notify::Result<()> {
        self.watcher.watch(path, if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive })
    }

    pub fn unwatch(&mut self, path: &Path) {
        let _ = self.watcher.unwatch(path);
    }
}
