//! Audio capture from system microphone via `cpal`.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::error::VoxError;
use crate::types::AudioChunk;

/// Default chunk size in samples at the pipeline reference rate (16 kHz).
/// Silero VAD uses 512-sample frames (32 ms); we scale this by `actual_rate / 16 kHz`
/// so each capture callback delivers an integer number of **mono** samples that
/// resample to a multiple of the VAD frame size when the device runs at 16/48 kHz.
const DEFAULT_CHUNK_SIZE: usize = 512;

/// Captures audio from the system's default input device and streams
/// it as [`AudioChunk`]s through a tokio mpsc channel.
pub struct AudioCapture {
    stream: cpal::Stream,
}

impl AudioCapture {
    /// Create a new capture from the default input device.
    ///
    /// Returns `Self` and an `mpsc::Receiver<AudioChunk>` that streams audio
    /// chunks of approximately one Silero frame duration at 16 kHz (32 ms when
    /// `sample_rate` is 16000).
    pub fn new(
        sample_rate: u32,
        channels: u16,
    ) -> Result<(Self, mpsc::Receiver<AudioChunk>), VoxError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| VoxError::Audio("no default input device found".into()))?;

        let device_name = device.name().unwrap_or_else(|_| "unknown".into());
        info!(device = %device_name, "using audio input device");

        // Try to find a matching config, otherwise use the device default
        let config = Self::find_config(&device, sample_rate, channels)?;
        let actual_rate = config.sample_rate().0;
        let actual_channels = config.channels();
        info!(
            sample_rate = actual_rate,
            channels = actual_channels,
            "audio capture config"
        );

        // Compute chunk size scaled to actual device sample rate
        let chunk_size = (DEFAULT_CHUNK_SIZE as f64 * actual_rate as f64 / 16000.0) as usize;

        // Large enough that brief ONNX/VAD stalls do not drop chunks (drops sound like silence to VAD).
        let (tx, rx) = mpsc::channel::<AudioChunk>(256);

        let stream_config: StreamConfig = config.clone().into();
        let sample_format = config.sample_format();

        let stream = match sample_format {
            SampleFormat::F32 => {
                let ch = actual_channels;
                let rate = actual_rate;
                device
                    .build_input_stream(
                        &stream_config,
                        Self::make_callback_f32(tx, chunk_size, rate, ch),
                        |err| error!("audio stream error: {}", err),
                        None,
                    )
                    .map_err(|e| VoxError::Audio(format!("failed to build input stream: {e}")))?
            }
            SampleFormat::I16 => {
                let ch = actual_channels;
                let rate = actual_rate;
                device
                    .build_input_stream(
                        &stream_config,
                        Self::make_callback_i16(tx, chunk_size, rate, ch),
                        |err| error!("audio stream error: {}", err),
                        None,
                    )
                    .map_err(|e| VoxError::Audio(format!("failed to build input stream: {e}")))?
            }
            SampleFormat::U8 => {
                let ch = actual_channels;
                let rate = actual_rate;
                device
                    .build_input_stream(
                        &stream_config,
                        Self::make_callback_u8(tx, chunk_size, rate, ch),
                        |err| error!("audio stream error: {}", err),
                        None,
                    )
                    .map_err(|e| VoxError::Audio(format!("failed to build input stream: {e}")))?
            }
            other => {
                return Err(VoxError::Audio(format!(
                    "unsupported sample format: {other:?}"
                )));
            }
        };

        Ok((Self { stream }, rx))
    }

    /// Start capturing audio.
    pub fn start(&self) -> Result<(), VoxError> {
        self.stream
            .play()
            .map_err(|e| VoxError::Audio(format!("failed to start stream: {e}")))
    }

    /// Pause audio capture.
    pub fn stop(&self) -> Result<(), VoxError> {
        self.stream
            .pause()
            .map_err(|e| VoxError::Audio(format!("failed to pause stream: {e}")))
    }

    /// Find a supported stream config, preferring the requested rate/channels.
    fn find_config(
        device: &cpal::Device,
        desired_rate: u32,
        desired_channels: u16,
    ) -> Result<cpal::SupportedStreamConfig, VoxError> {
        // First try the exact desired config
        if let Ok(configs) = device.supported_input_configs() {
            for cfg in configs {
                if cfg.channels() == desired_channels
                    && cfg.min_sample_rate().0 <= desired_rate
                    && cfg.max_sample_rate().0 >= desired_rate
                {
                    return Ok(cfg.with_sample_rate(cpal::SampleRate(desired_rate)));
                }
            }
        }

        // Fallback to device default
        device
            .default_input_config()
            .map_err(|e| VoxError::Audio(format!("no supported input config: {e}")))
    }

    /// Build an f32 input callback that accumulates samples into chunks.
    fn make_callback_f32(
        tx: mpsc::Sender<AudioChunk>,
        chunk_size: usize,
        sample_rate: u32,
        channels: u16,
    ) -> impl FnMut(&[f32], &cpal::InputCallbackInfo) + Send + 'static {
        let mut accumulator: Vec<f32> = Vec::with_capacity(chunk_size);
        move |data: &[f32], _info: &cpal::InputCallbackInfo| {
            accumulator.extend_from_slice(data);
            while accumulator.len() >= chunk_size {
                let chunk_data: Vec<f32> = accumulator.drain(..chunk_size).collect();
                let chunk = AudioChunk {
                    samples: chunk_data,
                    sample_rate,
                    channels,
                };
                Self::try_send_chunk(&tx, chunk);
            }
        }
    }

    /// Build an i16 input callback that converts to f32 and accumulates.
    fn make_callback_i16(
        tx: mpsc::Sender<AudioChunk>,
        chunk_size: usize,
        sample_rate: u32,
        channels: u16,
    ) -> impl FnMut(&[i16], &cpal::InputCallbackInfo) + Send + 'static {
        let mut accumulator: Vec<f32> = Vec::with_capacity(chunk_size);
        move |data: &[i16], _info: &cpal::InputCallbackInfo| {
            // Convert i16 to f32 normalized to [-1.0, 1.0]
            for &sample in data {
                accumulator.push(sample as f32 / i16::MAX as f32);
            }
            while accumulator.len() >= chunk_size {
                let chunk_data: Vec<f32> = accumulator.drain(..chunk_size).collect();
                let chunk = AudioChunk {
                    samples: chunk_data,
                    sample_rate,
                    channels,
                };
                Self::try_send_chunk(&tx, chunk);
            }
        }
    }

    /// Build a u8 input callback (some ALSA/PipeWire devices default to U8).
    fn make_callback_u8(
        tx: mpsc::Sender<AudioChunk>,
        chunk_size: usize,
        sample_rate: u32,
        channels: u16,
    ) -> impl FnMut(&[u8], &cpal::InputCallbackInfo) + Send + 'static {
        let mut accumulator: Vec<f32> = Vec::with_capacity(chunk_size);
        move |data: &[u8], _info: &cpal::InputCallbackInfo| {
            for &sample in data {
                // Unsigned 8-bit centered at 128 → [-1, 1]
                accumulator.push((sample as f32 - 128.0) / 128.0);
            }
            while accumulator.len() >= chunk_size {
                let chunk_data: Vec<f32> = accumulator.drain(..chunk_size).collect();
                let chunk = AudioChunk {
                    samples: chunk_data,
                    sample_rate,
                    channels,
                };
                Self::try_send_chunk(&tx, chunk);
            }
        }
    }

    fn try_send_chunk(tx: &mpsc::Sender<AudioChunk>, chunk: AudioChunk) {
        match tx.try_send(chunk) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("dropped microphone chunk (capture queue full); VAD may miss speech");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}
