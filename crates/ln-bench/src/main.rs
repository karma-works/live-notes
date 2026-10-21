//! `ln-bench` — checks with real models.
//!
//!   ln-bench download                 download the first-run models into $LN_DATA_DIR
//!   ln-bench asr <file.wav>...        VAD + language choice + Whisper, prints segments and speed
//!   ln-bench embed                    cross-lingual similarity with EmbeddingGemma via llama-server

use ln_asr::{AsrEngine, ChannelSegmenter, EnergyVad, PostProcessor, VoiceDetector};
use ln_types::{AppPaths, Channel, ChineseScript, Lang};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = AppPaths::from_env();
    let models = paths.models_dir();
    match args.first().map(String::as_str) {
        Some("download") => download(&models),
        Some("asr") => asr(&models, &args[1..]),
        Some("embed") => embed(&models),
        _ => eprintln!("usage: ln-bench download | asr <wav>... | embed"),
    }
}

fn download(models: &std::path::Path) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dl = ln_models::Downloader::new(models.to_path_buf());
    for id in ["silero-vad-v5", "whisper-base-q8_0", "embeddinggemma-300m", "whisper-large-v3-turbo-q5_0"] {
        let spec = ln_models::by_id(id).unwrap();
        let mut last = -1i32;
        let r = rt.block_on(dl.download(spec, |p| {
            let pct = (p.fraction() * 100.0) as i32;
            if pct / 10 != last / 10 {
                last = pct;
                eprintln!("{id}: {pct}%");
            }
        }));
        println!("{id}: {}", if r.is_ok() { "ok".to_string() } else { format!("{r:?}") });
    }
}

fn model_file(models: &std::path::Path, id: &str) -> std::path::PathBuf {
    let spec = ln_models::by_id(id).unwrap();
    ln_models::model_path(models, spec, &spec.files[0])
}

fn asr(models: &std::path::Path, files: &[String]) {
    let whisper = model_file(models, "whisper-large-v3-turbo-q5_0");
    let t0 = Instant::now();
    let lid = model_file(models, "whisper-base-q8_0");
    let lid = lid.exists().then_some(lid);
    let tr = ln_asr::WhisperTranscriber::load_with_lid(&whisper, "whisper-large-v3-turbo", lid.as_deref()).expect("whisper model");
    eprintln!("model loaded in {:.1}s", t0.elapsed().as_secs_f32());
    let mut engine = AsrEngine::new(Box::new(tr), PostProcessor::new(ChineseScript::Simplified), Lang::main_languages());
    let silero = model_file(models, "silero-vad-v5");
    for f in files {
        let (pcm, rate) = ln_asr::wav::read_mono(std::path::Path::new(f)).expect("wav");
        let vad: Box<dyn VoiceDetector> = match ln_asr::SileroVad::new(&silero) {
            Ok(v) => Box::new(v),
            Err(_) => Box::new(EnergyVad::default()),
        };
        let mut seg = ChannelSegmenter::new(Channel::Others, vad);
        let mut pending = Vec::new();
        for c in pcm.chunks(rate as usize / 10) {
            pending.extend(seg.push(c, rate));
        }
        pending.extend(seg.flush());
        let audio_s = pcm.len() as f32 / rate as f32;
        let t = Instant::now();
        println!("== {f} ({audio_s:.1}s audio, {} sections)", pending.len());
        for p in &pending {
            match engine.process(p, None) {
                Ok(Some(s)) => println!("[{:>6.1}s] ({} {:.2}) {}", s.start_ms as f32 / 1000.0, s.lang, s.lang_confidence, s.text),
                Ok(None) => println!("[{:>6.1}s] (dropped)", p.start_ms as f32 / 1000.0),
                Err(e) => println!("error: {e}"),
            }
        }
        let el = t.elapsed().as_secs_f32();
        if let Some(p) = pending.first() {
            let tt = engine.transcriber_mut();
            let a = Instant::now();
            let _ = tt.detect(&p.pcm);
            let d = a.elapsed().as_secs_f32();
            let a = Instant::now();
            let _ = tt.transcribe(&p.pcm, Some(&Lang::new("de")), &[], "");
            println!("-- detect {d:.2}s, transcribe {:.2}s (first section)", a.elapsed().as_secs_f32());
        }
        println!("-- {el:.1}s processing, real-time factor {:.2}", el / audio_s.max(0.1));
    }
}

fn embed(models: &std::path::Path) {
    let file = model_file(models, "embeddinggemma-300m");
    let mut server = ln_models::LlamaServer::start(ln_models::ServerRole::Embeddings, &file, None, 2048).expect("llama-server");
    let provider = ln_types::Provider { base_url: ln_models::ServerRole::Embeddings.base_url(), ..ln_types::Provider::local_llama() };
    let c = ln_ai::Client::new(provider, None);
    let docs = [
        "Das Budget für das nächste Quartal wird nächste Woche besprochen.",
        "我们下周再讨论下季度的预算。",
        "Le budget du prochain trimestre sera discuté la semaine prochaine.",
        "Milch, Brot und Käse einkaufen.",
        "Der Hund schläft im Garten.",
    ];
    let query = "task: search result | query: budget next quarter";
    let rt = tokio::runtime::Runtime::new().unwrap();
    let inputs: Vec<String> = std::iter::once(query.to_string()).chain(docs.iter().map(|d| format!("title: none | text: {d}"))).collect();
    let t = Instant::now();
    let v = rt.block_on(c.embeddings("embeddinggemma", &inputs)).expect("embeddings");
    println!("{} embeddings of dim {} in {:.2}s", v.len(), v[0].len(), t.elapsed().as_secs_f32());
    let norm = |x: &[f32]| x[..256].iter().map(|a| a * a).sum::<f32>().sqrt();
    let cos = |a: &[f32], b: &[f32]| a[..256].iter().zip(&b[..256]).map(|(x, y)| x * y).sum::<f32>() / (norm(a) * norm(b));
    for (d, e) in docs.iter().zip(&v[1..]) {
        println!("{:.3}  {d}", cos(&v[0], e));
    }
    server.stop();
}
