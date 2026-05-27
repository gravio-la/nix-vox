# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Vox** is a local-first voice AI framework written in Rust. It assembles a complete voice pipeline with no cloud dependencies or API keys:

```
Audio In → VAD (Silero) → STT (Whisper/Sherpa/Distil) → [LLM] → [TTS] → Audio Out
```

Core components:
- **VAD** (Voice Activity Detection): Silero ONNX
- **STT** (Speech-to-Text): Whisper, Distil-Whisper, Sherpa-ONNX with streaming support
- **TTS** (Text-to-Speech): Kokoro (57 voices, 9 languages), Qwen3 (state-of-the-art), Piper, Pocket (pure Rust), Chatterbox (voice cloning)
- **Speaker Diarization** (experimental): Real-time speaker identification with ECAPA-TDNN embeddings and SQLite-backed speaker DB
- **Server**: HTTP/WebSocket API with embedded web UI
- **Features**: Voice chat with Ollama LLMs, live barge-in conversations, streaming transcription

## Build & Development

### Basic Commands

```bash
# Build library
cargo build

# Build CLI binary (requires features)
cargo build --features cli

# Run full server with common TTS backends
cargo build --features cli,server,kokoro,piper

# Run tests (all)
cargo test

# Run a single test file
cargo test --test server_tests -- --nocapture

# Benchmark pipeline
cargo bench --bench pipeline_bench

# Run examples
cargo run --example simple_listen --features whisper,silero
cargo run --example voice_assistant --features whisper,silero,kokoro
```

### Nix Development

```bash
# Enter nix shell (sets up Rust + dependencies)
nix flake update && nix develop

# Build with Nix
nix build

# Build Kokoro variant
nix build .#vox-kokoro

# Build Qwen3 CUDA variant
nix build .#vox-qwen3-cuda
```

### Testing Specifics

- **server_tests** and **error_edge_tests**: Require `server` feature
- **streaming_pipeline_tests**: Require `whisper,silero` features
- **diarization_tests**: Require `diarization` feature
- **live_talk_integration**: Requires `server` feature and Ollama running

Full test suite: `cargo test --all-features`

Quick test: `cargo test --features cli,whisper,silero --lib`

## Architecture & Key Modules

### Core Pipeline (src/)

| Module | Purpose |
|--------|---------|
| **engine.rs** | Main Vox engine: builder pattern, audio capture loop, VAD/STT/TTS orchestration |
| **streaming_pipeline.rs** | Concurrent streaming I/O: audio capture → VAD → STT with frame buffering |
| **streaming_chat.rs** | Streaming LLM integration: combine STT + LLM + TTS responses |
| **traits.rs** | Plugin interfaces: `VadBackend`, `SttBackend`, `TtsBackend` for swappable implementations |

### Subsystems

- **audio/**: Resampler (48kHz/44.1kHz → 16kHz), audio I/O via cpal, WAV reading/writing
- **vad/**: Silero VAD (ONNX Runtime) — detects speech boundaries
- **stt/**: Whisper, Distil-Whisper, Sherpa-ONNX backends
- **tts/**: Kokoro, Qwen3, Piper, Pocket, Chatterbox implementations
- **diarization/**: Speaker encoder (ECAPA-TDNN), speaker store (SQLite), voice embedding management
- **server/**: Axum HTTP + WebSocket server, embedded web UI, `/v1/` REST API
- **cli/**: Binary entrypoints (listen, speak, chat, serve, config, benchmark)
- **model_cache.rs**: Model auto-download, verification, platform-specific storage paths
- **system_profile.rs**: Hardware detection (GPU, CPU, memory) for capability reporting

### Important Constants & Utilities

- **Audio**: 16 kHz mono PCM f32 LE is the internal format; resampling happens automatically
- **Speaker DB**: SQLite at `~/.local/share/vox/models/speakers.db` (Linux); queried during diarization
- **Models**: Auto-download on first run; use `vox models list/download/path` to manage

## MCP Server (`vox-mcp`)

Exposes Vox as a [Model Context Protocol](https://modelcontextprotocol.io) server so Claude Desktop, Cursor, and other MCP-compatible tools can call speech tools directly.

**Build & run:**
```bash
# Build
cargo build --bin vox-mcp --features mcp

# Run (Vox HTTP server must already be running)
vox serve --port 3000 &
cargo run --bin vox-mcp --features mcp -- --server-url http://localhost:3000
```

**Claude Desktop config** (`~/.config/claude/claude_desktop_config.json`):
```json
{
  "mcpServers": {
    "vox": {
      "command": "vox-mcp",
      "args": ["--server-url", "http://localhost:3000"]
    }
  }
}
```

**Exposed tools:** `speak`, `transcribe`, `list_voices`, `list_models`, `get_capabilities`, `server_status`

**Implementation:** `src/bin/vox_mcp.rs` — thin HTTP client wrapping the existing REST API using `rmcp` 1.x (official Anthropic Rust MCP SDK). No TypeScript required.

## Feature Flags

**Always enabled by default**: `whisper`, `silero`

**Key flag combinations**:
- `cli,server,kokoro,piper` — Full server with Kokoro + Piper voices
- `cli,server,qwen3` — Server with state-of-the-art Qwen3 TTS
- `cli,distil-whisper,pocket` — Lightweight edge deployment
- `diarization` — Speaker ID (auto-enabled by `server`)
- `qwen3-cuda` / `qwen3-metal` — GPU acceleration (Qwen3 only; macOS auto-enables Metal for `qwen3` + `pocket`)

**TTS backends**: Kokoro + Piper cannot link in the same binary (duplicate `ph_list2` symbol). See flake.nix for variant builds: `default` (Piper), `vox-kokoro`, `vox-qwen3`.

## Common Workflows

### Adding a New STT Backend

1. Implement `SttBackend` trait in `src/stt/mod.rs` (async `transcribe()` method)
2. Add feature flag to Cargo.toml
3. Instantiate in `engine.rs:VoxEngine::init_stt()`
4. Add example in `examples/`
5. Test with `cargo test --features your-backend`

### Adding a New TTS Backend

1. Implement `TtsBackend` trait in `src/tts/mod.rs` (async `synthesize()` method)
2. Add feature flag + dependencies
3. Wire into `engine.rs:VoxEngine::init_tts()`
4. Check for symbol conflicts (e.g., Kokoro + Piper)
5. Test voice selection via CLI and server API

### Debugging Audio Flow

- Use `vox test` to diagnose audio I/O
- Check resampler: `src/audio/resampler.rs`
- VAD frame timing: set `RUST_LOG=debug` for frame logging
- STT latency: use `--nocapture` in tests to see timestamps

### Testing Server Endpoints

```bash
# Start server
cargo run --bin vox --features cli,server,kokoro -- serve --port 3000

# In another terminal:
curl http://localhost:3000/v1/voices
curl http://localhost:3000/v1/capabilities
curl http://localhost:3000/health
```

## Key Files to Know

- **Cargo.toml**: Workspace, features, dependencies. Carefully ordered TTS backends to avoid linker conflicts.
- **flake.nix**: Nix build definitions; defines multiple build outputs (default, kokoro, qwen3, qwen3-cuda).
- **src/bin/vox.rs**: CLI entry point; command dispatch (listen, speak, chat, serve, config, benchmark).
- **src/server/**: Axum server routes, WebSocket handlers, embedded UI HTML.
- **build.rs**: Pre-compile steps (currently minimal; some backends may require build-time setup).

## Vendor Dependencies

- **vendor/qwen3-tts/**: Qwen3 TTS implementation (Candle-based, CUDA/Metal support)
- **vendor/piper-rs/**: Piper TTS Rust wrapper (espeak-ng integration)
- **vendor/cbx/**: Chatterbox voice cloning backend
- **vendor/sherpa-sys/**: FFI bindings to Sherpa-ONNX C++ library

## Development Notes

- **Async runtime**: Tokio with `features = ["full"]`; VAD/STT/TTS are all async
- **Error handling**: Custom `VoxError` enum; use `.context("operation")` from `anyhow` for stack traces
- **Model downloads**: HTTP + progress bar via `indicatif`; auto-retries on network error
- **Speaker database**: Initialized lazily; in-memory speaker cache for ~session; persisted to SQLite on enroll
- **WebSocket**: Four channels: `/v1/listen` (STT), `/v1/speak` (TTS), `/v1/converse` (voice chat), `/v1/live-talk` (barge-in)
- **Nix notes**: Piper needs `libsonic` built separately; Qwen3 CUDA requires unfree packages (set in flake.nix)

## Testing Strategy

- **Integration tests** (tests/*.rs): Full pipeline, VAD+STT+TTS, server endpoints, error handling
- **Benchmarks** (benches/): Pipeline throughput, resampler, VAD, STT, TTS, comparison benches
- **Examples** (examples/): Lightweight demos; good starting point for debugging
- Tests with `required-features` in Cargo.toml are skipped if features not enabled

## Platform Support

- **macOS (Apple Silicon)**: Tested; auto-enables Metal GPU for Qwen3/Pocket
- **macOS (Intel)**: Tested via CI
- **Linux (x86_64)**: Tested via CI
- **Windows (x86_64)**: Tested via CI
- **Raspberry Pi 4+**: Supported via `distil-whisper` + `pocket` (see README for config)
