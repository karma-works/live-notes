use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub include_subdirs: bool,
    /// Extra glob patterns from the user (gitignore syntax).
    pub excludes: Vec<String>,
    pub max_file_size: u64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { include_subdirs: true, excludes: vec![], max_file_size: 50 * 1024 * 1024 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
}

/// Directories never worth indexing (SPEC §5 F4).
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "Library",
    ".livenotes",
    "__pycache__",
    ".venv",
    "venv",
    ".Trash",
    "build",
    "dist",
    ".cache",
    ".idea",
];

/// Lists indexable files: respects `.gitignore`/`.ignore`, skips hidden files, big files and
/// unsupported types.
pub fn scan(root: &Path, opts: &ScanOptions) -> Vec<ScannedFile> {
    let mut b = ignore::WalkBuilder::new(root);
    b.hidden(true).git_ignore(true).git_global(false).git_exclude(true).ignore(true).require_git(false).parents(false).follow_links(false);
    if !opts.include_subdirs {
        b.max_depth(Some(1));
    }
    if !opts.excludes.is_empty() {
        let mut ov = ignore::overrides::OverrideBuilder::new(root);
        for e in &opts.excludes {
            let _ = ov.add(&format!("!{e}"));
        }
        if let Ok(o) = ov.build() {
            b.overrides(o);
        }
    }
    b.filter_entry(|e| !(e.file_type().is_some_and(|t| t.is_dir()) && e.file_name().to_str().is_some_and(|n| SKIP_DIRS.contains(&n))));
    let mut out = Vec::new();
    for entry in b.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let p = entry.path();
        if crate::kind_of(p).is_none() {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        if md.len() == 0 || md.len() > opts.max_file_size {
            continue;
        }
        let mtime = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        out.push(ScannedFile { path: p.to_path_buf(), size: md.len(), mtime });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_gitignore_and_skip_dirs() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(r.join("sub")).unwrap();
        std::fs::write(r.join("a.md"), "# A").unwrap();
        std::fs::write(r.join("sub/b.txt"), "b").unwrap();
        std::fs::write(r.join("node_modules/x/c.md"), "c").unwrap();
        std::fs::write(r.join("secret.md"), "s").unwrap();
        std::fs::write(r.join(".gitignore"), "secret.md\n").unwrap();
        std::fs::write(r.join("binary.exe"), "x").unwrap();
        std::fs::write(r.join(".hidden.md"), "h").unwrap();
        let names: Vec<String> =
            scan(r, &ScanOptions::default()).iter().map(|f| f.path.strip_prefix(r).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["a.md", "sub/b.txt"]);
        let flat = scan(r, &ScanOptions { include_subdirs: false, ..Default::default() });
        assert_eq!(flat.len(), 1);
        let ex = scan(r, &ScanOptions { excludes: vec!["sub/**".into()], ..Default::default() });
        assert_eq!(ex.len(), 1);
    }
}
