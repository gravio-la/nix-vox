//! Vox MCP Server — exposes Vox speech capabilities via the Model Context Protocol.
//!
//! Requires a running Vox HTTP server (default: http://localhost:3000).
//! Start with: `vox serve --port 3000`
//!
//! Usage:
//!   vox-mcp                                  # connects to http://localhost:3000
//!   vox-mcp --server-url http://host:PORT    # custom URL
//!
//! Claude Desktop config:
//!   {
//!     "mcpServers": {
//!       "vox": {
//!         "command": "vox-mcp",
//!         "args": ["--server-url", "http://localhost:3000"]
//!       }
//!     }
//!   }

use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    tool, tool_handler, tool_router,
    ServiceExt,
    transport::stdio,
};
use serde_json::json;

// ── Parameter types ──────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SpeakParams {
    /// The text to convert to speech.
    text: String,
    /// Optional voice ID. Use list_voices to see available voices.
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<String>,
    /// If true, play audio through the default output device after synthesis.
    /// Default: false. When false, saves to a temp WAV file only.
    #[serde(default)]
    play: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TranscribeParams {
    /// Absolute path to a WAV file to transcribe.
    file: String,
}

// ── Server ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct VoxMcpServer {
    base_url: String,
    client: reqwest::Client,
    #[allow(dead_code)] // used internally by #[tool_router] and #[tool_handler] macros
    tool_router: ToolRouter<VoxMcpServer>,
}

#[tool_router]
impl VoxMcpServer {
    fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
            tool_router: Self::tool_router(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    // ── Tools ─────────────────────────────────────────────────────────────

    #[tool(description = "Convert text to speech. With play: true, plays audio through speakers. Otherwise saves to a temp WAV file. Use list_voices to see available voice IDs.")]
    async fn speak(
        &self,
        Parameters(SpeakParams { text, voice, play }): Parameters<SpeakParams>,
    ) -> Result<CallToolResult, McpError> {
        let voice_label = voice.clone().unwrap_or_else(|| "default".into());
        let body = json!({ "text": text, "voice": voice });

        let resp = match self.client
            .post(self.url("/v1/synthesize"))
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => return Ok(CallToolResult::error(vec![Content::text(format!(
                "Vox server error {}: {}",
                r.status(),
                r.text().await.unwrap_or_default()
            ))])),
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(format!(
                "Cannot reach Vox server at {} — start it with: vox serve\n{}",
                self.base_url, e
            ))])),
        };

        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to read audio response: {e}"
            ))])),
        };

        if play {
            // Decode WAV and play
            return Self::play_audio(&bytes, &voice_label).await;
        }

        // Default: save to temp file
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let path = std::env::temp_dir().join(format!("vox_{ts}.wav"));

        if let Err(e) = std::fs::write(&path, &bytes) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to write audio file: {e}"
            ))]));
        }

        Ok(CallToolResult::success(vec![Content::text(
            json!({
                "file": path.to_string_lossy(),
                "size_bytes": bytes.len(),
                "voice": voice_label,
            })
            .to_string(),
        )]))
    }

    #[tool(description = "Transcribe speech from a WAV audio file. Returns the transcribed text, duration, and detected language.")]
    async fn transcribe(
        &self,
        Parameters(TranscribeParams { file }): Parameters<TranscribeParams>,
    ) -> Result<CallToolResult, McpError> {
        let bytes = match std::fs::read(&file) {
            Ok(b) => b,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(format!(
                "Cannot read file '{file}': {e}"
            ))])),
        };

        match self.client
            .post(self.url("/v1/transcribe"))
            .header("Content-Type", "audio/wav")
            .body(bytes)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                let json: serde_json::Value = r
                    .json()
                    .await
                    .unwrap_or_else(|_| json!({"error": "failed to parse response"}));
                Ok(CallToolResult::success(vec![Content::text(json.to_string())]))
            }
            Ok(r) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Vox server error {}: {}",
                r.status(),
                r.text().await.unwrap_or_default()
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Cannot reach Vox server at {}: {e}",
                self.base_url
            ))])),
        }
    }

    #[tool(description = "List all available TTS voices with their IDs, names, languages, genders, and backend (kokoro/piper/qwen3/pocket).")]
    async fn list_voices(&self) -> Result<CallToolResult, McpError> {
        self.get_json("/v1/voices").await
    }

    #[tool(description = "Show loaded speech models: STT backend (Whisper/Sherpa/Distil), TTS backend (Kokoro/Piper/Qwen3), and connected Ollama LLMs.")]
    async fn list_models(&self) -> Result<CallToolResult, McpError> {
        self.get_json("/v1/models").await
    }

    #[tool(description = "Get hardware capabilities: GPU availability, system memory, loaded models, and supported Vox features.")]
    async fn get_capabilities(&self) -> Result<CallToolResult, McpError> {
        self.get_json("/v1/capabilities").await
    }

    #[tool(description = "Check if the Vox server is running and get usage statistics (total transcriptions, syntheses, uptime).")]
    async fn server_status(&self) -> Result<CallToolResult, McpError> {
        let health = self.client.get(self.url("/health")).send().await;
        let stats = self.client.get(self.url("/v1/stats")).send().await;

        let running = matches!(&health, Ok(r) if r.status().is_success());
        let stats_json = match stats {
            Ok(r) => r.json::<serde_json::Value>().await.unwrap_or(json!({})),
            Err(_) => json!({}),
        };

        Ok(CallToolResult::success(vec![Content::text(
            json!({
                "running": running,
                "server_url": self.base_url,
                "stats": stats_json,
            })
            .to_string(),
        )]))
    }
}

impl VoxMcpServer {
    async fn get_json(&self, path: &str) -> Result<CallToolResult, McpError> {
        match self.client.get(self.url(path)).send().await {
            Ok(r) if r.status().is_success() => {
                let json: serde_json::Value = r
                    .json()
                    .await
                    .unwrap_or_else(|_| json!({"error": "failed to parse response"}));
                Ok(CallToolResult::success(vec![Content::text(json.to_string())]))
            }
            Ok(r) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Vox server error {}: {}",
                r.status(),
                r.text().await.unwrap_or_default()
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Cannot reach Vox server at {} — start it with: vox serve\n{e}",
                self.base_url
            ))])),
        }
    }

    /// Decode WAV bytes and play through the default audio device.
    async fn play_audio(bytes: &[u8], voice_label: &str) -> Result<CallToolResult, McpError> {
        // Decode WAV inline, similar to server/handlers.rs
        let cursor = Cursor::new(bytes);
        let reader = match hound::WavReader::new(cursor) {
            Ok(r) => r,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to decode WAV: {e}"
            ))])),
        };

        let spec = reader.spec();
        let sample_rate = spec.sample_rate;
        let channels = spec.channels;

        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => match reader.into_samples::<f32>().collect::<Result<Vec<_>, _>>() {
                Ok(s) => s,
                Err(e) => return Ok(CallToolResult::error(vec![Content::text(format!(
                    "WAV decode error: {e}"
                ))])),
            },
            hound::SampleFormat::Int => {
                let bits = spec.bits_per_sample;
                let max_val = (1u32 << (bits - 1)) as f32;
                match reader.into_samples::<i32>().collect::<Result<Vec<_>, _>>() {
                    Ok(samples_i32) => samples_i32.into_iter().map(|s| s as f32 / max_val).collect(),
                    Err(e) => return Ok(CallToolResult::error(vec![Content::text(format!(
                        "WAV decode error: {e}"
                    ))])),
                }
            }
        };

        let duration_ms = if sample_rate > 0 {
            (samples.len() as u64 * 1000) / (sample_rate as u64 * channels as u64)
        } else {
            0
        };

        let audio_chunk = vox::types::AudioChunk {
            samples,
            sample_rate,
            channels,
        };

        // Play in a blocking task to avoid blocking the async runtime
        let play_result: Result<Result<(), String>, tokio::task::JoinError> = tokio::task::spawn_blocking(move || {
            let player = vox::AudioPlayer::new()
                .map_err(|e| format!("Failed to initialize audio player: {e}"))?;
            player.play_blocking(&audio_chunk)
                .map_err(|e| format!("Playback error: {e}"))
        })
        .await;

        match play_result {
            Ok(Ok(())) => Ok(CallToolResult::success(vec![Content::text(
                json!({
                    "played": true,
                    "duration_ms": duration_ms,
                    "voice": voice_label,
                })
                .to_string(),
            )])),
            Ok(Err(e)) => {
                if e.contains("no audio output device") {
                    Ok(CallToolResult::error(vec![Content::text(
                        "No audio output device available. Use play: false and play the returned file manually.".to_string()
                    )]))
                } else {
                    Ok(CallToolResult::error(vec![Content::text(e)]))
                }
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Playback task failed: {e}"
            ))])),
        }
    }
}

// ── ServerHandler ─────────────────────────────────────────────────────────────

#[tool_handler]
impl ServerHandler for VoxMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "Vox is a local-first voice AI framework. \
                Use speak to synthesize text to speech (with optional play: true for direct audio playback), \
                transcribe to convert a WAV file to text, \
                list_voices for available TTS voices, \
                list_models for loaded STT/TTS backends, \
                and server_status to check connectivity. \
                Requires a running Vox server: `vox serve --port 3000`."
                    .to_string(),
            )
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn default_server_url() -> String {
    let port = std::env::var("VOX_PORT")
        .or_else(|_| std::env::var("PORT"))
        .unwrap_or_else(|_| "3000".into());
    format!("http://localhost:{port}")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs must go to stderr — stdout is reserved for the MCP stdio transport.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let server_url = args
        .windows(2)
        .find(|w| w[0] == "--server-url")
        .map(|w| w[1].clone())
        .unwrap_or_else(default_server_url);

    tracing::info!("vox-mcp connecting to {}", server_url);

    let service = VoxMcpServer::new(server_url)
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("MCP server error: {:?}", e))?;

    service.waiting().await?;
    Ok(())
}
