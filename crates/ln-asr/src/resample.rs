use rubato::{FftFixedIn, Resampler};

/// Streaming resampler from any device rate to 16 kHz mono.
pub struct StreamResampler {
    inner: Option<FftFixedIn<f32>>,
    pending: Vec<f32>,
    chunk: usize,
    rate_in: u32,
}

impl StreamResampler {
    pub fn new(rate_in: u32) -> Self {
        let chunk = 1024;
        let inner = if rate_in == crate::RATE {
            None
        } else {
            Some(FftFixedIn::new(rate_in as usize, crate::RATE as usize, chunk, 2, 1).expect("resampler"))
        };
        StreamResampler { inner, pending: Vec::new(), chunk, rate_in }
    }

    pub fn rate_in(&self) -> u32 {
        self.rate_in
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        let Some(r) = self.inner.as_mut() else { return samples.to_vec() };
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.pending.len() >= self.chunk {
            let block: Vec<f32> = self.pending.drain(..self.chunk).collect();
            if let Ok(mut v) = r.process(&[block], None) {
                out.append(&mut v[0]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_48k_to_16k() {
        let mut r = StreamResampler::new(48_000);
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut out = Vec::new();
        for c in input.chunks(480) {
            out.extend(r.push(c));
        }
        // One second in, about one second out (minus the last partial block and FFT delay).
        assert!(out.len() > 14_500 && out.len() <= 16_000, "{}", out.len());
        let mut same = StreamResampler::new(16_000);
        assert_eq!(same.push(&[1.0, 2.0]), vec![1.0, 2.0]);
    }
}
