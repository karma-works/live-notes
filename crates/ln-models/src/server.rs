//! The bundled llama-server (llama.cpp). Started on demand, stopped when idle; stopping the
//! process frees the model's memory at once (ADR-0002 §3).

use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerRole {
    /// EmbeddingGemma for search.
    Embeddings,
    /// Gemma 4 for chat and images.
    Chat,
}

impl ServerRole {
    pub fn port(self) -> u16 {
        match self {
            ServerRole::Embeddings => 18431,
            ServerRole::Chat => 18432,
        }
    }
    pub fn base_url(self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port())
    }
}

/// Looks for `llama-server`: inside the app bundle, then `LN_LLAMA_SERVER`, then PATH.
pub fn find_llama_server() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LN_LLAMA_SERVER") {
        return Some(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for cand in [dir.join("llama-server"), dir.join("../Resources/bin/llama-server")] {
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .map(|d| d.join("llama-server"))
        .find(|p| p.exists())
}

pub struct LlamaServer {
    child: Option<Child>,
    pub role: ServerRole,
    pub model: PathBuf,
    last_used: Instant,
}

impl LlamaServer {
    /// Starts the server and waits until `/health` answers (models load from disk, so this can
    /// take a few seconds).
    pub fn start(role: ServerRole, model: &Path, mmproj: Option<&Path>, ctx: u32) -> Result<LlamaServer> {
        let bin = find_llama_server().ok_or(Error::NoServer)?;
        let mut cmd = Command::new(bin);
        cmd.arg("-m").arg(model).arg("--host").arg("127.0.0.1").arg("--port").arg(role.port().to_string());
        cmd.arg("--no-webui").arg("-c").arg(ctx.to_string());
        match role {
            ServerRole::Embeddings => {
                cmd.arg("--embeddings").arg("--pooling").arg("mean").arg("-ub").arg("2048").arg("-b").arg("2048");
            }
            ServerRole::Chat => {
                cmd.arg("--jinja");
                if let Some(p) = mmproj {
                    cmd.arg("--mmproj").arg(p);
                }
            }
        }
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let child = cmd.spawn().map_err(|e| Error::ServerStart(e.to_string()))?;
        let mut s = LlamaServer { child: Some(child), role, model: model.to_path_buf(), last_used: Instant::now() };
        s.wait_ready(Duration::from_secs(120))?;
        Ok(s)
    }

    fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let url = format!("http://127.0.0.1:{}/health", self.role.port());
        while Instant::now() < deadline {
            if let Some(c) = self.child.as_mut()
                && let Ok(Some(st)) = c.try_wait()
            {
                return Err(Error::ServerStart(format!("exited with {st}")));
            }
            if health_ok(&url) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        self.stop();
        Err(Error::ServerStart("timeout".into()))
    }

    pub fn touch(&mut self) {
        self.last_used = Instant::now();
    }

    pub fn idle_for(&self) -> Duration {
        self.last_used.elapsed()
    }

    pub fn is_running(&mut self) -> bool {
        self.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Minimal blocking HTTP GET for the health check (avoids pulling a runtime into this thread).
fn health_ok(url: &str) -> bool {
    use std::io::{Read, Write};
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&host.parse().unwrap(), Duration::from_millis(300)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    if write!(s, "GET /{path} HTTP/1.0\r\nHost: {host}\r\n\r\n").is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    matches!(s.read(&mut buf), Ok(n) if n > 12 && &buf[9..12] == b"200")
}
