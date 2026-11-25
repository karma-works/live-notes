use crate::{AudioChunk, CaptureStream, Error, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const SAMPLE_RATE: u32 = 48_000;
/// How often the system audio stream looks for output devices that appeared after it started.
const RELINK_INTERVAL: Duration = Duration::from_secs(1);

struct PipeWireCapture {
    child: Child,
    reader: Option<JoinHandle<()>>,
    /// Dropping the sender stops the thread that links new output devices.
    linker: Option<(Sender<()>, JoinHandle<()>)>,
}

impl CaptureStream for PipeWireCapture {
    fn stop(&mut self) {
        if let Some((stop, linker)) = self.linker.take() {
            drop(stop);
            let _ = linker.join();
        }
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

/// What the capture stream is connected to.
enum Route {
    /// WirePlumber connects the stream to the default device or to an explicit target.
    Auto { target: Option<std::ffi::OsString> },
    /// One playback stream, linked explicitly.
    Application(String),
    /// The monitors of every output device, linked explicitly and kept up to date.
    AllOutputs,
}

pub fn start_capture(tx: Sender<AudioChunk>, monitor: bool) -> Result<Box<dyn CaptureStream>> {
    let source = if monitor { "system audio" } else { "microphone" };
    let target_var = if monitor { "LN_PIPEWIRE_OUTPUT" } else { "LN_PIPEWIRE_MICROPHONE" };
    let target = std::env::var_os(target_var);
    let route = match monitor.then(|| std::env::var("LN_PIPEWIRE_APPLICATION").ok()).flatten() {
        Some(application) => Route::Application(application),
        // Calls often play on a headset while the default output is a monitor or speakers.
        // The default device's monitor alone would then record silence for the other side.
        None if monitor && target.is_none() => Route::AllOutputs,
        None => Route::Auto { target },
    };
    let channels = if matches!(route, Route::Auto { .. }) { 1 } else { 2 };
    let mut command = Command::new("pw-record");
    command.args(["--raw", "--rate", "48000", "--channels", &channels.to_string(), "--format", "f32"]);

    static NEXT_STREAM: AtomicU64 = AtomicU64::new(0);
    let input_name = format!("live-notes-{}-{}", std::process::id(), NEXT_STREAM.fetch_add(1, Ordering::Relaxed));
    let properties = match &route {
        Route::Application(_) | Route::AllOutputs => {
            // This stream is connected explicitly, so WirePlumber must not route it to a default device.
            command.args(["--target", "0", "--channel-map", "Stereo"]);
            format!("{{ node.name = {input_name} node.autoconnect = false }}")
        }
        Route::Auto { target } => {
            if let Some(target) = target.as_ref() {
                if target.is_empty() || target == "0" {
                    return Err(Error::Other(format!("{target_var} must name a PipeWire node or its serial")));
                }
                command.arg(format!("--target={}", target.to_string_lossy()));
            }
            format!("{{ node.name = {input_name} stream.capture.sink = {monitor} node.dont-fallback = {} }}", target.is_some())
        }
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
    let mut capture = PipeWireCapture { child, reader: Some(reader), linker: None };
    match route {
        Route::Application(application) => link_application(&application, &input_name)?,
        Route::AllOutputs => capture.linker = Some(link_all_outputs(input_name)?),
        Route::Auto { .. } => {}
    }
    if ready_rx.recv_timeout(Duration::from_secs(3)).is_err() {
        return Err(Error::Other(format!("PipeWire {source}: no samples received from the default device")));
    }
    Ok(Box::new(capture))
}

/// A parsed `pw-dump` snapshot of the PipeWire graph.
struct Graph(Vec<Value>);

impl Graph {
    fn load() -> Result<Graph> {
        let dump = super::output(&mut Command::new("pw-dump"))?;
        serde_json::from_str(&dump).map(Graph).map_err(|error| Error::Other(format!("pw-dump: {error}")))
    }

    fn nodes(&self) -> impl Iterator<Item = &Value> {
        self.0.iter().filter(|entry| entry["type"] == "PipeWire:Interface:Node")
    }

    fn find_node(&self, name: &str, media_class: Option<&str>) -> Option<u64> {
        self.nodes().find_map(|entry| {
            let props = &entry["info"]["props"];
            let matches_name = props["node.name"].as_str() == Some(name)
                || props["object.serial"].as_u64().is_some_and(|serial| name.parse() == Ok(serial));
            let matches_class = media_class.is_none_or(|class| props["media.class"].as_str() == Some(class));
            if matches_name && matches_class { entry["id"].as_u64() } else { None }
        })
    }

    /// Ports of a node in one direction, in the order PipeWire lists them.
    fn ports(&self, node: u64, direction: &'static str) -> impl Iterator<Item = &Value> {
        self.0.iter().filter(move |entry| {
            entry["type"] == "PipeWire:Interface:Port"
                && entry["info"]["direction"] == direction
                && entry["info"]["props"]["node.id"].as_u64() == Some(node)
        })
    }

    fn port_ids(&self, node: u64, direction: &'static str) -> Vec<u64> {
        self.ports(node, direction).filter_map(|port| port["id"].as_u64()).collect()
    }

    /// `(output port, input port)` pairs that are already linked.
    fn links(&self) -> HashSet<(u64, u64)> {
        self.0
            .iter()
            .filter(|entry| entry["type"] == "PipeWire:Interface:Link")
            .filter_map(|entry| Some((entry["info"]["output-port-id"].as_u64()?, entry["info"]["input-port-id"].as_u64()?)))
            .collect()
    }
}

/// Links each output port to one of the two capture ports, duplicating mono across both.
fn link_stereo(capture_ports: &[u64], output_ports: &[u64], existing: &HashSet<(u64, u64)>) -> Result<()> {
    for (index, input_port) in capture_ports.iter().enumerate() {
        let output_port = output_ports[index.min(output_ports.len() - 1)];
        if !existing.contains(&(output_port, *input_port)) {
            super::output(Command::new("pw-link").arg(output_port.to_string()).arg(input_port.to_string()))?;
        }
    }
    Ok(())
}

/// Connect to a specific playback stream without routing through the output device's mix.
fn link_application(application: &str, input_name: &str) -> Result<()> {
    let target_missing = || Error::Other(format!("PipeWire playback stream {application:?} is not running"));
    for _ in 0..20 {
        let graph = Graph::load()?;
        let source = graph.find_node(application, Some("Stream/Output/Audio")).ok_or_else(target_missing)?;
        if let Some(input) = graph.find_node(input_name, None) {
            let capture_ports = graph.port_ids(input, "input");
            let playback_ports = graph.port_ids(source, "output");
            if capture_ports.len() == 2 && !playback_ports.is_empty() {
                return link_stereo(&capture_ports, &playback_ports, &graph.links());
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(Error::Other("PipeWire playback capture ports were not created".into()))
}

/// Links the monitor ports of every output device into the capture stream. PipeWire mixes
/// several links into one input port, so audio on any device reaches the "others" channel.
/// Returns the input ports of the capture stream and how many devices are linked.
fn link_output_monitors(graph: &Graph, input: u64) -> Result<(Vec<u64>, usize)> {
    let capture_ports = graph.port_ids(input, "input");
    if capture_ports.len() != 2 {
        return Ok((capture_ports, 0));
    }
    let existing = graph.links();
    let mut linked = 0;
    let sinks = graph
        .nodes()
        .filter(|node| node["info"]["props"]["media.class"].as_str() == Some("Audio/Sink"))
        .filter_map(|node| node["id"].as_u64());
    for sink in sinks {
        let monitors: Vec<u64> = graph
            .ports(sink, "output")
            .filter(|port| port["info"]["props"]["port.monitor"].as_bool() == Some(true))
            .filter_map(|port| port["id"].as_u64())
            .collect();
        if monitors.is_empty() {
            continue;
        }
        // A device may disappear between the dump and the link; the next pass catches up.
        match link_stereo(&capture_ports, &monitors, &existing) {
            Ok(()) => linked += 1,
            Err(error) => tracing::debug!("linking output device {sink}: {error}"),
        }
    }
    Ok((capture_ports, linked))
}

fn link_all_outputs(input_name: String) -> Result<(Sender<()>, JoinHandle<()>)> {
    let mut input = None;
    let mut known = 0;
    for _ in 0..20 {
        let graph = Graph::load()?;
        if let Some(node) = graph.find_node(&input_name, None) {
            let (ports, linked) = link_output_monitors(&graph, node)?;
            if ports.len() == 2 {
                if linked == 0 {
                    return Err(Error::Other("PipeWire system audio: no output device found".into()));
                }
                tracing::info!("system audio: recording {linked} output device(s)");
                input = Some(node);
                known = linked;
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    let input = input.ok_or_else(|| Error::Other("PipeWire system audio capture ports were not created".into()))?;
    let (stop_tx, stop_rx): (Sender<()>, Receiver<()>) = bounded(0);
    let linker = thread::Builder::new()
        .name("pipewire-relink".into())
        .spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = stop_rx.recv_timeout(RELINK_INTERVAL) {
                match Graph::load().and_then(|graph| link_output_monitors(&graph, input)) {
                    Ok((_, linked)) if linked != known => {
                        tracing::info!("system audio: recording {linked} output device(s)");
                        known = linked;
                    }
                    Ok(_) => {}
                    Err(error) => tracing::debug!("system audio relink: {error}"),
                }
            }
        })
        .map_err(|error| Error::Other(format!("PipeWire system audio: {error}")))?;
    Ok((stop_tx, linker))
}
