//! Sample rate conversion using `rubato`.

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use tracing::info;

use crate::error::VoxError;
use crate::types::AudioChunk;

/// Resamples audio to a target sample rate and converts stereo to mono.
///
/// Resampling is driven by each chunk's [`AudioChunk::sample_rate`] (the device may run at
/// 48 kHz while the pipeline was configured for 16 kHz). If capture falls back to the
/// device default rate, we still downsample correctly so Silero VAD sees true 16 kHz frames.
pub struct AudioResampler {
    resampler: Option<SincFixedIn<f32>>,
    /// Source rate the current `resampler` was built for (`None` if passthrough).
    active_source_rate: Option<u32>,
    target_rate: u32,
}

impl AudioResampler {
    /// Create a resampler. The `source_rate` argument is a legacy hint only; actual conversion
    /// uses [`AudioChunk::sample_rate`] on each [`Self::process`] call.
    pub fn new(source_rate: u32, target_rate: u32) -> Result<Self, VoxError> {
        let _ = source_rate;
        Ok(Self {
            resampler: None,
            active_source_rate: None,
            target_rate,
        })
    }

    fn ensure_resampler(&mut self, source_rate: u32) -> Result<(), VoxError> {
        if self.active_source_rate == Some(source_rate) && self.resampler.is_some() {
            return Ok(());
        }

        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            oversampling_factor: 128,
            interpolation: SincInterpolationType::Cubic,
            window: WindowFunction::BlackmanHarris2,
        };

        let ratio = self.target_rate as f64 / source_rate as f64;
        let chunk_size = 1024;

        let r = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk_size, 1)
            .map_err(|e| VoxError::Audio(format!("failed to create resampler: {e}")))?;

        self.resampler = Some(r);
        self.active_source_rate = Some(source_rate);
        info!(
            from_hz = source_rate,
            to_hz = self.target_rate,
            "audio resampler active (mic rate differs from VAD 16 kHz)"
        );
        Ok(())
    }

    /// Resample an audio chunk. Also converts stereo to mono if needed.
    pub fn process(&mut self, chunk: &AudioChunk) -> Result<AudioChunk, VoxError> {
        // Step 1: Convert to mono if stereo
        let mono_samples = if chunk.channels > 1 {
            stereo_to_mono(&chunk.samples, chunk.channels)
        } else {
            chunk.samples.clone()
        };

        let source_rate = chunk.sample_rate;

        // Step 2: Resample when the capture rate differs from VAD/STT target rate
        let resampled = if source_rate == self.target_rate {
            self.resampler = None;
            self.active_source_rate = None;
            mono_samples
        } else {
            self.ensure_resampler(source_rate)?;
            let resampler = self
                .resampler
                .as_mut()
                .expect("resampler set by ensure_resampler");
            let input_frames_max = resampler.input_frames_max();
            let mut output = Vec::new();

            let mut offset = 0;
            while offset < mono_samples.len() {
                let end = (offset + input_frames_max).min(mono_samples.len());
                let mut input_chunk = mono_samples[offset..end].to_vec();

                if input_chunk.len() < input_frames_max {
                    input_chunk.resize(input_frames_max, 0.0);
                }

                let input = vec![input_chunk];
                let result = resampler
                    .process(&input, None)
                    .map_err(|e| VoxError::Audio(format!("resample error: {e}")))?;

                if let Some(channel) = result.into_iter().next() {
                    if end - offset < input_frames_max {
                        let valid_ratio = (end - offset) as f64 / input_frames_max as f64;
                        let valid_output = (channel.len() as f64 * valid_ratio) as usize;
                        output.extend_from_slice(&channel[..valid_output]);
                    } else {
                        output.extend(channel);
                    }
                }

                offset += input_frames_max;
            }

            output
        };

        Ok(AudioChunk {
            samples: resampled,
            sample_rate: self.target_rate,
            channels: 1,
        })
    }
}

/// Convert interleaved multi-channel audio to mono by averaging channels.
fn stereo_to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels as usize;
    let frame_count = samples.len() / ch;
    let mut mono = Vec::with_capacity(frame_count);
    for i in 0..frame_count {
        let mut sum = 0.0f32;
        for c in 0..ch {
            sum += samples[i * ch + c];
        }
        mono.push(sum / ch as f32);
    }
    mono
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_same_rate() {
        let mut r = AudioResampler::new(16000, 16000).unwrap();
        let chunk = AudioChunk {
            samples: vec![0.1, 0.2, 0.3, 0.4],
            sample_rate: 16000,
            channels: 1,
        };
        let out = r.process(&chunk).unwrap();
        assert_eq!(out.samples, chunk.samples);
        assert_eq!(out.sample_rate, 16000);
        assert_eq!(out.channels, 1);
    }

    #[test]
    fn stereo_to_mono_conversion() {
        let samples = vec![1.0, 0.0, 0.5, 0.5, 0.0, 1.0];
        let mono = stereo_to_mono(&samples, 2);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 0.5).abs() < 1e-6);
        assert!((mono[1] - 0.5).abs() < 1e-6);
        assert!((mono[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn resampler_creates_for_different_rates() {
        let mut r = AudioResampler::new(44100, 16000).unwrap();
        let chunk = AudioChunk {
            samples: vec![0.5; 4410],
            sample_rate: 44100,
            channels: 1,
        };
        let out = r.process(&chunk).unwrap();
        assert!(!out.samples.is_empty());
        assert_eq!(out.sample_rate, 16000);
    }

    #[test]
    fn resample_produces_fewer_samples_when_downsampling() {
        let mut r = AudioResampler::new(44100, 16000).unwrap();
        let chunk = AudioChunk {
            samples: vec![0.5; 44100], // 1 second at 44100 Hz
            sample_rate: 44100,
            channels: 1,
        };
        let out = r.process(&chunk).unwrap();
        // Should produce ~16000 samples (within 5% tolerance for resampler edge effects)
        let ratio = out.samples.len() as f64 / 16000.0;
        assert!(
            ratio > 0.95 && ratio < 1.05,
            "expected ~16000 samples, got {}",
            out.samples.len()
        );
        assert_eq!(out.sample_rate, 16000);
    }

    #[test]
    fn resample_produces_more_samples_when_upsampling() {
        let mut r = AudioResampler::new(8000, 16000).unwrap();
        let chunk = AudioChunk {
            samples: vec![0.5; 8000], // 1 second at 8000 Hz
            sample_rate: 8000,
            channels: 1,
        };
        let out = r.process(&chunk).unwrap();
        let ratio = out.samples.len() as f64 / 16000.0;
        assert!(
            ratio > 0.95 && ratio < 1.05,
            "expected ~16000 samples, got {}",
            out.samples.len()
        );
        assert_eq!(out.sample_rate, 16000);
    }

    #[test]
    fn stereo_input_converted_to_mono() {
        let mut r = AudioResampler::new(16000, 16000).unwrap();
        // Stereo: left=1.0, right=0.0 => mono should be 0.5
        let chunk = AudioChunk {
            samples: vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            sample_rate: 16000,
            channels: 2,
        };
        let out = r.process(&chunk).unwrap();
        assert_eq!(out.channels, 1);
        assert_eq!(out.samples.len(), 4);
        for s in &out.samples {
            assert!((s - 0.5).abs() < 1e-6, "expected 0.5, got {s}");
        }
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let mut r = AudioResampler::new(16000, 16000).unwrap();
        let chunk = AudioChunk {
            samples: vec![],
            sample_rate: 16000,
            channels: 1,
        };
        let out = r.process(&chunk).unwrap();
        assert!(out.samples.is_empty());
    }

    #[test]
    fn resampler_output_values_in_range() {
        let mut r = AudioResampler::new(44100, 16000).unwrap();
        // Sine wave that stays in [-1, 1]
        let samples: Vec<f32> = (0..4410)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 44100.0).sin())
            .collect();
        let chunk = AudioChunk {
            samples,
            sample_rate: 44100,
            channels: 1,
        };
        let out = r.process(&chunk).unwrap();
        for s in &out.samples {
            assert!(s.abs() <= 1.5, "sample out of range: {s}"); // allow small overshoot from sinc
        }
    }
}
