//! YouTube without a transcript from the extension: metadata and captions via `yt-dlp`, and as a
//! last resort the audio track (`yt-dlp -x`, converted by `ffmpeg`) for local transcription.

use super::captions::{CaptionCue, parse_json3, parse_vtt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// `dQw4w9WgXcQ`: 11 characters of `[A-Za-z0-9_-]`.
pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Video id from the usual URL shapes (watch, youtu.be, shorts, embed, live; www/m/music hosts).
pub fn video_id_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let id = if host == "youtu.be" {
        path.split('/').next().map(str::to_string)
    } else if host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtube-nocookie.com"
        || host.ends_with(".youtube-nocookie.com")
    {
        let mut segs = path.split('/');
        match segs.next() {
            Some("watch") => query.split('&').find_map(|kv| kv.strip_prefix("v=")).map(str::to_string),
            Some("shorts" | "embed" | "live" | "v") => segs.next().map(str::to_string),
            _ => None,
        }
    } else {
        None
    };
    id.map(|i| i.split(['#', '&']).next().unwrap_or("").to_string()).filter(|i| valid_video_id(i))
}

pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// Link to a moment of the video.
pub fn timestamp_url(id: &str, ms: u64) -> String {
    format!("https://www.youtube.com/watch?v={id}&t={}s", ms / 1000)
}

/// A command-line tool: `LN_<NAME>` overrides, then `PATH`, then Homebrew's folders (apps
/// started from the Dock get a minimal `PATH`).
pub fn find_tool(name: &str) -> Option<PathBuf> {
    let var = format!("LN_{}", name.to_ascii_uppercase().replace('-', ""));
    if let Some(p) = std::env::var_os(&var) {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into(), "/usr/bin".into()])
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// What `yt-dlp -J` says about a video.
#[derive(Debug, Clone, Default)]
pub struct VideoInfo {
    pub title: Option<String>,
    pub channel: Option<String>,
    pub duration_s: Option<f64>,
    pub description: Option<String>,
    /// Original language of the video, when YouTube knows it.
    pub language: Option<String>,
    /// Languages of captions uploaded by the author.
    pub subtitles: Vec<String>,
    /// Languages of automatic captions (the original plus machine translations).
    pub automatic: Vec<String>,
    /// The `-J` output, reused for the downloads (`--load-info-json`).
    pub info_file: PathBuf,
}

/// Runs a command with a time limit; returns stdout, or the last lines of stderr as error.
fn run(cmd: &mut Command, limit: Duration) -> Result<Vec<u8>, String> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    tracing::info!("clip: running {name} {:?}", cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>());
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("{name}: {e}"))?;
    // Read both pipes on threads so a chatty process can't block on a full pipe.
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let out_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut v);
        v
    });
    let err_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut err, &mut v);
        v
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name}: timed out after {} s", limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("{name}: {e}")),
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
    if status.success() {
        Ok(stdout)
    } else {
        let tail: Vec<&str> = stderr.lines().filter(|l| l.contains("ERROR")).collect();
        let msg = if tail.is_empty() { stderr.lines().last().unwrap_or("").to_string() } else { tail.join(" ") };
        tracing::warn!("clip: {name} failed: {stderr}");
        Err(format!("{name}: {}", msg.trim()))
    }
}

pub fn fetch_info(ytdlp: &Path, video_id: &str, dir: &Path) -> Result<VideoInfo, String> {
    let out = run(
        Command::new(ytdlp).args(["-J", "--no-playlist", "--skip-download", "--no-warnings"]).arg(watch_url(video_id)),
        Duration::from_secs(120),
    )?;
    let info_file = dir.join("info.json");
    std::fs::write(&info_file, &out).map_err(|e| e.to_string())?;
    let mut info = parse_info(&out).ok_or_else(|| "yt-dlp: unreadable video information".to_string())?;
    info.info_file = info_file;
    Ok(info)
}

pub fn parse_info(json: &[u8]) -> Option<VideoInfo> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string).filter(|x| !x.trim().is_empty());
    let keys = |k: &str| -> Vec<String> {
        v.get(k).and_then(|x| x.as_object()).map(|o| o.keys().filter(|k| *k != "live_chat").cloned().collect()).unwrap_or_default()
    };
    Some(VideoInfo {
        title: s("title"),
        channel: s("channel").or_else(|| s("uploader")),
        duration_s: v.get("duration").and_then(|x| x.as_f64()),
        description: s("description"),
        language: s("language"),
        subtitles: keys("subtitles"),
        automatic: keys("automatic_captions"),
        info_file: PathBuf::new(),
    })
}

fn base(lang: &str) -> String {
    lang.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase()
}

/// Which caption track to download: `(language key, automatic)`. Author captions in a preferred
/// language first, then YouTube's own recognition in the original language, then automatic
/// captions in a preferred language, then any author captions.
pub fn choose_subtitles(info: &VideoInfo, prefs: &[String]) -> Option<(String, bool)> {
    let find = |list: &[String], want: &str| list.iter().find(|k| base(k) == want && !k.contains("-orig")).cloned();
    let mut order: Vec<String> = Vec::new();
    for p in info.language.iter().chain(prefs) {
        let b = base(p);
        if !b.is_empty() && !order.contains(&b) {
            order.push(b);
        }
    }
    for p in &order {
        if let Some(k) = find(&info.subtitles, p) {
            return Some((k, false));
        }
    }
    // The untranslated recognition is marked `-orig` when YouTube offers translations.
    if let Some(k) = info.automatic.iter().find(|k| k.ends_with("-orig")) {
        return Some((k.clone(), true));
    }
    if let Some(lang) = &info.language
        && let Some(k) = find(&info.automatic, &base(lang))
    {
        return Some((k, true));
    }
    for p in &order {
        if let Some(k) = find(&info.automatic, p) {
            return Some((k, true));
        }
    }
    info.subtitles.first().map(|k| (k.clone(), false))
}

/// Downloads one caption track (json3, else VTT) and parses it.
pub fn download_captions(ytdlp: &Path, info: &VideoInfo, key: &str, automatic: bool, dir: &Path) -> Result<Vec<CaptionCue>, String> {
    let subs = dir.join("subs");
    std::fs::create_dir_all(&subs).map_err(|e| e.to_string())?;
    run(
        Command::new(ytdlp)
            .arg("--load-info-json")
            .arg(&info.info_file)
            .args(["--skip-download", "--no-warnings", if automatic { "--write-auto-subs" } else { "--write-subs" }])
            .args(["--sub-format", "json3/vtt", "--sub-langs", key, "-o"])
            .arg(subs.join("v.%(ext)s")),
        Duration::from_secs(120),
    )?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&subs).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    files.sort();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let cues = match f.extension().and_then(|e| e.to_str()) {
            Some("json3") => parse_json3(&text).unwrap_or_default(),
            Some("vtt") => parse_vtt(&text),
            _ => continue,
        };
        if !cues.is_empty() {
            return Ok(cues);
        }
    }
    Err("yt-dlp: no captions downloaded".into())
}

/// Downloads the audio track as 16 kHz mono WAV.
pub fn download_audio(ytdlp: &Path, ffmpeg: &Path, info: &VideoInfo, dir: &Path) -> Result<PathBuf, String> {
    let ffmpeg_dir = ffmpeg.parent().unwrap_or(Path::new("/"));
    run(
        Command::new(ytdlp)
            .arg("--load-info-json")
            .arg(&info.info_file)
            .args(["--no-warnings", "-f", "bestaudio/best", "-x", "--audio-format", "wav"])
            .args(["--postprocessor-args", "ExtractAudio:-ar 16000 -ac 1"])
            .arg("--ffmpeg-location")
            .arg(ffmpeg_dir)
            .arg("-o")
            .arg(dir.join("audio.%(ext)s")),
        Duration::from_secs(60 * 60),
    )?;
    let wav = dir.join("audio.wav");
    if wav.exists() { Ok(wav) } else { Err("yt-dlp: no audio file was written".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_urls() {
        for u in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?feature=share&v=dQw4w9WgXcQ&t=42s",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ#t=1",
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=x",
        ] {
            assert_eq!(video_id_from_url(u).as_deref(), Some("dQw4w9WgXcQ"), "{u}");
        }
        for u in ["https://example.com/watch?v=dQw4w9WgXcQ", "https://www.youtube.com/watch?v=short", "ftp://youtu.be/dQw4w9WgXcQ"] {
            assert_eq!(video_id_from_url(u), None, "{u}");
        }
        assert!(valid_video_id("a-b_c1234XY"));
        assert!(!valid_video_id("a b_c1234XY"));
        assert_eq!(timestamp_url("dQw4w9WgXcQ", 62_900), "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=62s");
    }

    #[test]
    fn caption_choice() {
        let info = |lang: Option<&str>, subs: &[&str], auto: &[&str]| VideoInfo {
            language: lang.map(str::to_string),
            subtitles: subs.iter().map(|s| s.to_string()).collect(),
            automatic: auto.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let prefs = vec!["de".to_string(), "en".to_string()];
        // Author captions in a preferred language win.
        assert_eq!(choose_subtitles(&info(None, &["fr", "en-US"], &["en"]), &prefs), Some(("en-US".into(), false)));
        // Otherwise the original recognition, not a translation.
        assert_eq!(choose_subtitles(&info(Some("ja"), &[], &["de", "en", "ja-orig", "ja"]), &prefs), Some(("ja-orig".into(), true)));
        assert_eq!(choose_subtitles(&info(Some("ja"), &[], &["de", "ja"]), &prefs), Some(("ja".into(), true)));
        assert_eq!(choose_subtitles(&info(None, &[], &["fr", "en"]), &prefs), Some(("en".into(), true)));
        assert_eq!(choose_subtitles(&info(None, &["fr"], &[]), &prefs), Some(("fr".into(), false)));
        assert_eq!(choose_subtitles(&info(None, &[], &[]), &prefs), None);
    }

    #[test]
    fn info_parsing() {
        let j = br#"{"title":"Me at the zoo","uploader":"jawed","duration":19,"language":null,
            "subtitles":{"en":[{"ext":"json3"}],"live_chat":[]},"automatic_captions":{"en":[],"de":[]}}"#;
        let i = parse_info(j).unwrap();
        assert_eq!(i.title.as_deref(), Some("Me at the zoo"));
        assert_eq!(i.channel.as_deref(), Some("jawed"));
        assert_eq!(i.duration_s, Some(19.0));
        assert_eq!(i.subtitles, vec!["en".to_string()]);
        assert_eq!(i.automatic.len(), 2);
    }
}
