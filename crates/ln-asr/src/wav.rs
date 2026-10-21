//! WAV helpers: encoding sections for cloud transcription and reading test files.

use std::io::Cursor;

/// 16-bit PCM mono WAV at the given rate.
pub fn encode(samples: &[f32], rate: u32) -> Vec<u8> {
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut cur = Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cur, spec).expect("wav writer");
        for s in samples {
            let _ = w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        }
        let _ = w.finalize();
    }
    cur.into_inner()
}

/// Reads a WAV file as mono f32 plus its sample rate.
pub fn read_mono(path: &std::path::Path) -> Result<(Vec<f32>, u32), hound::Error> {
    let mut r = hound::WavReader::open(path)?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / max)).collect::<Result<_, _>>()?
        }
    };
    Ok((raw.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect(), spec.sample_rate))
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("ln-wav-{}.wav", std::process::id()));
        let s: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        std::fs::write(&dir, super::encode(&s, 16_000)).unwrap();
        let (back, rate) = super::read_mono(&dir).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(back.len(), 1600);
        assert!((back[10] - s[10]).abs() < 1e-3);
        let _ = std::fs::remove_file(dir);
    }
}
