use crate::{Error, ModelFile, ModelSpec, Result};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, PartialEq)]
pub struct DownloadProgress {
    pub model_id: &'static str,
    pub done: u64,
    pub total: u64,
}

impl DownloadProgress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { self.done as f32 / self.total as f32 }
    }
}

pub fn model_path(models_dir: &Path, spec: &ModelSpec, file: &ModelFile) -> PathBuf {
    models_dir.join(spec.id).join(file.name)
}

/// All files present with the right size (the hash is checked when downloading).
pub fn is_installed(models_dir: &Path, spec: &ModelSpec) -> bool {
    spec.files.iter().all(|f| std::fs::metadata(model_path(models_dir, spec, f)).map(|m| m.len() == f.size).unwrap_or(false))
}

pub struct Downloader {
    http: reqwest::Client,
    models_dir: PathBuf,
}

impl Downloader {
    pub fn new(models_dir: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .user_agent(concat!("live-notes/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http");
        Downloader { http, models_dir }
    }

    /// Downloads all files of a model. Partial files (`.part`) are resumed with HTTP Range.
    /// The file only gets its final name after the SHA-256 matched.
    pub async fn download(&self, spec: &'static ModelSpec, mut progress: impl FnMut(DownloadProgress)) -> Result<()> {
        let total = spec.total_size();
        let mut done_before = 0u64;
        for f in spec.files {
            let dest = model_path(&self.models_dir, spec, f);
            if std::fs::metadata(&dest).map(|m| m.len() == f.size).unwrap_or(false) {
                done_before += f.size;
                progress(DownloadProgress { model_id: spec.id, done: done_before, total });
                continue;
            }
            self.download_file(spec, f, &dest, done_before, total, &mut progress).await?;
            done_before += f.size;
        }
        Ok(())
    }

    async fn download_file(
        &self,
        spec: &'static ModelSpec,
        f: &ModelFile,
        dest: &Path,
        base: u64,
        total: u64,
        progress: &mut impl FnMut(DownloadProgress),
    ) -> Result<()> {
        tokio::fs::create_dir_all(dest.parent().unwrap()).await?;
        let part = dest.with_extension("part");
        let mut have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
        if have > f.size {
            tokio::fs::remove_file(&part).await?;
            have = 0;
        }
        let url = std::env::var("LN_MODEL_BASE_URL").map(|b| format!("{}/{}", b.trim_end_matches('/'), f.name)).unwrap_or_else(|_| f.url.to_string());
        let mut req = self.http.get(&url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = req.send().await?;
        let status = resp.status();
        if have > 0 && status == reqwest::StatusCode::OK {
            // Server ignored the range: start over.
            have = 0;
        } else if !status.is_success() {
            return Err(Error::Status { file: f.name.into(), status: status.as_u16() });
        }
        let mut file = tokio::fs::OpenOptions::new().create(true).append(have > 0).write(true).truncate(have == 0).open(&part).await?;
        let mut stream = resp.bytes_stream();
        let mut done = have;
        let mut last_report = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            done += chunk.len() as u64;
            if done - last_report > 1 << 20 || done == f.size {
                last_report = done;
                progress(DownloadProgress { model_id: spec.id, done: base + done, total });
            }
        }
        file.flush().await?;
        drop(file);
        let part2 = part.clone();
        let hash = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
            let mut h = Sha256::new();
            let mut fh = std::fs::File::open(&part2)?;
            std::io::copy(&mut fh, &mut h)?;
            Ok(format!("{:x}", h.finalize()))
        })
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))??;
        if hash != f.sha256 {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(Error::Checksum(f.name.into()));
        }
        tokio::fs::rename(&part, dest).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelKind, ModelSpec};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const DATA: &[u8] = b"hello model weights";
    // sha256("hello model weights")
    static FILES: [ModelFile; 1] = [ModelFile {
        name: "m.bin",
        url: "http://unused/m.bin",
        size: 19,
        sha256: "c5d0f5bf4eba2ff06d53dc2fb2f3d6763e0fb5a6c86d01e0c2c0d06a2c1b2d55",
    }];

    fn spec_with(sha: &'static str) -> &'static ModelSpec {
        let files: &'static [ModelFile] = Box::leak(Box::new([ModelFile { sha256: sha, ..FILES[0] }]));
        Box::leak(Box::new(ModelSpec {
            id: "test",
            display: "Test",
            kind: ModelKind::Vad,
            license: "MIT",
            license_url: "",
            needs_terms: false,
            files,
            ram_mb: 1,
            min_ram_gb: 1,
        }))
    }

    fn sha(d: &[u8]) -> &'static str {
        Box::leak(format!("{:x}", Sha256::digest(d)).into_boxed_str())
    }

    #[tokio::test]
    async fn downloads_resumes_and_verifies() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/m.bin")).and(header("range", "bytes=6-")).respond_with(ResponseTemplate::new(206).set_body_bytes(&DATA[6..])).mount(&s).await;
        Mock::given(method("GET")).and(path("/m.bin")).respond_with(ResponseTemplate::new(200).set_body_bytes(DATA)).mount(&s).await;
        // SAFETY: tests in this crate do not read the variable concurrently.
        unsafe { std::env::set_var("LN_MODEL_BASE_URL", s.uri()) };
        let dir = tempfile::tempdir().unwrap();
        let spec = spec_with(sha(DATA));
        let dl = Downloader::new(dir.path().to_path_buf());
        // Simulate an interrupted download.
        let dest = model_path(dir.path(), spec, &spec.files[0]);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(dest.with_extension("part"), &DATA[..6]).unwrap();
        let mut last = None;
        dl.download(spec, |p| last = Some(p)).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), DATA);
        assert!(is_installed(dir.path(), spec));
        assert_eq!(last.unwrap().fraction(), 1.0);

        let bad = spec_with("0000000000000000000000000000000000000000000000000000000000000000");
        let dir2 = tempfile::tempdir().unwrap();
        let dl2 = Downloader::new(dir2.path().to_path_buf());
        assert!(matches!(dl2.download(bad, |_| {}).await, Err(Error::Checksum(_))));
        assert!(!is_installed(dir2.path(), bad));
    }
}
