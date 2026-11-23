use crate::{AudioChunk, CaptureStream, Error, Result};
use crossbeam_channel::{Sender, bounded};
use serde_json::Value;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const SAMPLE_RATE: u32 = 48_000;

struct PipeWireCapture {
    child: Child,
    reader: Option<JoinHandle<()>>,
}

impl CaptureStream for PipeWireCapture {
    fn stop(&mut self) {
        if let Some(reader) = self.reader.take() {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = reader.join();
        }
    }
}

impl Drop for PipeWireCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_capture(tx: Sender<AudioChunk>, monitor: bool) -> Result<Box<dyn CaptureStream>> {
    let source = if monitor { "system audio" } else { "microphone" };
    let target_var = if monitor { "LN_PIPEWIRE_OUTPUT" } else { "LN_PIPEWIRE_MICROPHONE" };
    let target = std::env::var_os(target_var);
    let application = monitor.then(|| std::env::var("LN_PIPEWIRE_APPLICATION").ok()).flatten();
    let channels = if application.is_some() { 2 } else { 1 };
    let mut command = Command::new("pw-record");
    command.args(["--raw", "--rate", "48000", "--channels", &channels.to_string(), "--format", "f32"]);

    static NEXT_STREAM: AtomicU64 = AtomicU64::new(0);
    let input_name = format!("live-notes-{}-{}", std::process::id(), NEXT_STREAM.fetch_add(1, Ordering::Relaxed));
    let properties = if application.is_some() {
        // This stream is connected explicitly, so WirePlumber must not route it to a default device.
        command.args(["--target", "0", "--channel-map", "Stereo"]);
        format!("{{ node.name = {input_name} node.autoconnect = false }}")
    } else {
        if let Some(target) = target.as_ref() {
            if target.is_empty() || target == "0" {
                return Err(Error::Other(format!("{target_var} must name a PipeWire node or its serial")));
            }
            command.arg(format!("--target={}", target.to_string_lossy()));
        }
        format!("{{ node.name = {input_name} stream.capture.sink = {monitor} node.dont-fallback = {} }}", target.is_some(),)
    };
    command.args(["--properties", &properties]);
    let mut child = command
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| Error::Other(format!("PipeWire {source}: {error}")))?;
    let mut stdout = child.stdout.take().expect("pw-record stdout is piped");
    let (ready_tx, ready_rx) = bounded(1);
    let reader = thread::Builder::new().name(format!("pipewire-{source}")).spawn(move || {
        let mut bytes = vec![0; 960 * 4 * channels];
        let mut ready = Some(ready_tx);
        while stdout.read_exact(&mut bytes).is_ok() {
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
            let frames = bytes.as_chunks::<4>().0.chunks_exact(channels);
            let samples = frames.map(|frame| frame.iter().copied().map(f32::from_ne_bytes).sum::<f32>() / channels as f32).collect();
            // The audio pipeline may stop consuming while this process is being shut down.
            // Keep the reader interruptible and let it exit when the receiver disappears.
            if matches!(
                tx.try_send(AudioChunk { samples, sample_rate: SAMPLE_RATE }),
                Err(crossbeam_channel::TrySendError::Disconnected(_))
            ) {
                break;
            }
        }
    });
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Other(format!("PipeWire {source}: {error}")));
        }
    };
    let capture = PipeWireCapture { child, reader: Some(reader) };
    if let Some(application) = application {
        link_application(&application, &input_name)?;
    }
    if ready_rx.recv_timeout(Duration::from_secs(3)).is_err() {
        return Err(Error::Other(format!("PipeWire {source}: no samples received from the default device")));
    }
    Ok(Box::new(capture))
}

/// Connect to a specific playback stream without routing through the output device's mix.
fn link_application(application: &str, input_name: &str) -> Result<()> {
    let target_missing = || Error::Other(format!("PipeWire playback stream {application:?} is not running"));
    for _ in 0..20 {
        let dump = super::output(&mut Command::new("pw-dump"))?;
        let objects: Vec<Value> = serde_json::from_str(&dump).map_err(|error| Error::Other(format!("pw-dump: {error}")))?;
        let find_node = |name: &str, media_class: Option<&str>| {
            objects.iter().find_map(|entry| {
                let props = &entry["info"]["props"];
                let matches_name = props["node.name"].as_str() == Some(name)
                    || props["object.serial"].as_u64().is_some_and(|serial| name.parse() == Ok(serial));
                let matches_class = media_class.is_none_or(|class| props["media.class"].as_str() == Some(class));
                if entry["type"] == "PipeWire:Interface:Node" && matches_name && matches_class { entry["id"].as_u64() } else { None }
            })
        };
        let source = find_node(application, Some("Stream/Output/Audio")).ok_or_else(target_missing)?;
        if let Some(input) = find_node(input_name, None) {
            let ports = |node, direction: &'static str| {
                objects.iter().filter(move |entry| {
                    entry["type"] == "PipeWire:Interface:Port"
                        && entry["info"]["direction"] == direction
                        && entry["info"]["props"]["node.id"].as_u64() == Some(node)
                })
            };
            let capture_ports: Vec<_> = ports(input, "input").filter_map(|port| port["id"].as_u64()).collect();
            let playback_ports: Vec<_> = ports(source, "output").filter_map(|port| port["id"].as_u64()).collect();
            if capture_ports.len() == 2 && !playback_ports.is_empty() {
                for (index, input_port) in capture_ports.iter().enumerate() {
                    // Duplicate a mono stream across the two capture ports before downmixing.
                    let output_port = playback_ports[index.min(playback_ports.len() - 1)];
                    super::output(Command::new("pw-link").arg(output_port.to_string()).arg(input_port.to_string()))?;
                }
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(Error::Other("PipeWire playback capture ports were not created".into()))
}
