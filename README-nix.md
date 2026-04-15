# Nix (flake) for Vox

This repository includes a `flake.nix` that provides a Rust dev shell, several build variants of the `vox` binary (Piper, Kokoro, or Qwen3 TTS), and matching `nix run` app targets.

## Requirements

- [Nix](https://nixos.org/) with flakes enabled (`experimental-features = nix-command flakes`).

## Quick reference

| What you want | Command |
|----------------|---------|
| Run the **default** build (Piper TTS) | `nix run .#` or `nix run .#default` |
| Run the **Kokoro** build | `nix run .#vox-kokoro` |
| Run the **Qwen3** build | `nix run .#vox-qwen3` |
| Run **Qwen3 + CUDA** (Linux only, needs NVIDIA driver) | `nix run .#vox-qwen3-cuda` |
| Build default package | `nix build .#` |
| Build Kokoro package | `nix build .#vox-kokoro` |
| Build Qwen3 package | `nix build .#vox-qwen3` |
| Build Qwen3+CUDA package | `nix build .#vox-qwen3-cuda` (Linux) |
| Developer shell (Rust, ONNX Runtime env, Piper-oriented paths) | `nix develop` |
| Dev shell for Qwen3+CUDA (+ `aplay` for WAV smoke tests) | `nix develop .#qwen3-cuda` (Linux) |
| **Audible** Piper smoke test (plays one sentence) | `nix run .#speak-demo` |
| **CI-style** check: build + synthesize Piper to WAV (no speaker, offline models) | `nix flake check` |

After `nix build`, the binary is at `result/bin/vox` (both packages install the same executable name `vox`; the store path distinguishes them).

`nix flake check` builds the default package and runs **`checks.<system>.vox-piper-speak-wav`**: it prefetches the English Piper (`en-us` / Lessac) ONNX + JSON into the store, points `VOX_MODELS_DIR` at them, runs `vox speak … --output …` to prove synthesis works without network or audio hardware.

For a **local** test that **plays** audio through your default output device, use `nix run .#speak-demo` (may download the same Piper voice on first run if not cached under `~/.local/share/vox/models`).

The `vox speak` command also accepts **`--output path.wav`** (`-o`) to write WAV instead of playing — useful for headless or scripting.

## Examples (`vox speak`)

Use the **app** that matches the backend you need (`default` = Piper, `vox-qwen3` = Qwen3). Pass `-y` to allow automatic model downloads where the backend supports it.

**Piper — German (bundled voice `piper-de`, Thorsten medium)**  
Voice aliases `de`, `german`, and `deutsch` map to the same model.

```bash
nix run .# -- speak \
  "Guten Tag. Das ist ein kurzes Beispiel mit Piper auf Deutsch." \
  --backend piper --voice de -y
```

**Qwen3 — female German (`de_de_female_1`)**  
Requires the Hugging Face weights once (not shipped in the Nix store). Example using `huggingface-cli` from `python3Packages.huggingface-hub`:

```bash
huggingface-cli download Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice \
  --local-dir ~/.cache/huggingface/hub/models--Qwen--Qwen3-TTS-12Hz-0.6B-CustomVoice/snapshots/main
```

```bash
nix run .#vox-qwen3 -- speak \
  "Guten Tag! Ich hoffe, Sie haben einen schönen Tag. Hier spricht eine weibliche deutsche Stimme mit Qwen drei." \
  --backend qwen3 --voice de_de_female_1 -y
```

**Qwen3 — English (default voice mapping)**  
If you omit a Qwen3-specific `--voice`, the CLI default `af_heart` is remapped to `en_us_female_1` for this backend.

```bash
nix run .#vox-qwen3 -- speak "Hello from Qwen3." --backend qwen3 -y
```

## Qwen3 speed: CPU (`vox-qwen3`) vs CUDA (`vox-qwen3-cuda`)

**`CUDA_COMPUTE_CAP`:** NVIDIA’s [**compute capability**](https://docs.nvidia.com/cuda/cuda-c-programming-guide/index.html#compute-capabilities) is a version number for a GPU architecture (the `sm_XX` target for device code). The Candle stack (e.g. **candle-kernels** / bindgen) reads the environment variable `CUDA_COMPUTE_CAP` at **compile time** so `nvcc` knows which architecture to build for, instead of querying `nvidia-smi`—which is unavailable or unreliable in Nix’s build sandbox, Docker, and similar environments ([Candle install notes](https://huggingface.github.io/candle/guide/installation.html)). Values are the capability in **compact form** without a dot: `80` → 8.0 (e.g. Ampere), `89` → 8.9 (e.g. many RTX 40-series). Discover yours from NVIDIA’s GPU list, or on a machine where the GPU is visible: `nvidia-smi --query-gpu=compute_cap --format=csv`. In this repo it is **first set** on the `vox` package in **`flake.nix`** (`env` when CUDA is enabled: default `CUDA_COMPUTE_CAP = "80"`).

The flake package **`vox-qwen3`** uses Cargo feature **`qwen3`** (Candle **CPU**). Quality matches the GPU build, but inference stays on the **CPU**, often with **moderate utilization** (single-threaded or memory-bound parts are normal).

**Linux + NVIDIA:** **`vox-qwen3-cuda`** builds with **`qwen3-cuda`** (`nvcc` and CUDA libraries come from Nix; **CUDA is unfree**, so the flake uses `allowUnfree = true`). The derivation sets **`CUDA_COMPUTE_CAP`** (default **`80`**, i.e. Ampere) so the build does not need `nvidia-smi` in the sandbox. Override when targeting a different architecture, for example:

```bash
CUDA_COMPUTE_CAP=89 nix build .#vox-qwen3-cuda
```

At **runtime** you still need a working **NVIDIA driver**; **`libcuda.so.1`** is loaded from the driver, not from the Nix closure. The **`vox-qwen3-cuda`** wrapper prepends **`LD_LIBRARY_PATH`** with the CUDA toolkit **and** **`/run/opengl-driver/lib`** (where **NixOS** exposes the NVIDIA driver’s `libcuda`). On non-NixOS Linux, if `libcuda` is already on the default linker path, the extra entry is harmless.

Example (after downloading Qwen3 weights as for CPU Qwen3):

```bash
nix run .#vox-qwen3-cuda -- speak "Hello from Qwen3 on CUDA." --backend qwen3 -y -o /tmp/t.wav
aplay /tmp/t.wav
```

Leave **`VOX_QWEN3_DEVICE`** unset or **`auto`** to pick CUDA when this binary is built with the CUDA feature; or set **`cuda`** / **`cuda:0`**.

**Without Nix:** install the driver + CUDA so `nvcc` works, then `cargo build -p vox --release --features cli,server,qwen3-cuda,pocket,chatterbox`.

**Apple Silicon:** on macOS, use **`qwen3-metal`** (`cargo build … --features cli,server,qwen3-metal,pocket,chatterbox`). There is no CUDA flake output on Darwin.

**Runtime override:** `VOX_QWEN3_DEVICE` wins over the config default (`auto`). Values include `auto`, `cpu`, `cuda`, `cuda:N`, and `metal`.

## Why multiple packages (`default`, `vox-kokoro`, `vox-qwen3`)

Piper and Kokoro cannot be linked into **one** binary in this tree (you get a duplicate symbol such as `ph_list2`). Qwen3 is shipped as its **own** output as well (no Piper/Kokoro in that derivation), so you can switch TTS stacks without rebuilding unrelated engines.

The flake builds **three** main package variants from the same crate with different Cargo features:

- **`packages.<system>.default`** — `cli,server,piper,pocket,chatterbox` (Piper TTS).
- **`packages.<system>.vox-kokoro`** — `cli,server,kokoro,pocket,chatterbox` (Kokoro TTS; no Piper).
- **`packages.<system>.vox-qwen3`** — `cli,server,qwen3,pocket,chatterbox` (Qwen3 TTS; no Piper/Kokoro).
- **`packages.x86_64-linux.vox-qwen3-cuda`** (and **`aarch64-linux`** when available) — `cli,server,whisper,silero,qwen3-cuda,piper,pocket,chatterbox`: **Qwen3 CUDA TTS**, **Whisper STT**, **Silero VAD**, **Piper** (fallback + Live Talk), **Chatterbox**, **Pocket**; Linux only.

Apps mirror that: `default`, `vox-kokoro`, `vox-qwen3`, and on Linux `vox-qwen3-cuda` each run the corresponding package’s `vox`.

Qwen3 models are large and not bundled; download them separately (see upstream Qwen3-TTS / `VOX_QWEN3_MODEL_PATH` in the main docs).

### Models for `vox-qwen3-cuda` (STT + VAD + optional Piper)

Everything below is fetched into **`VOX_MODELS_DIR`** (default `~/.local/share/vox/models` on Linux). Use **`nix run .#vox-qwen3-cuda --`** instead of a local `cargo` build.

| Need | Registry name | On disk |
|------|-----------------|---------|
| VAD (WebSocket listen, `vox listen`) | `silero-vad` | `silero_vad.onnx` |
| STT **multilingual** (German, etc.) | **`whisper-tiny`**, or `whisper-base` / `whisper-small` for accuracy | `ggml-tiny.bin`, … |
| STT **English-only** | `whisper-tiny.en` | `ggml-tiny.en.bin` |
| **Speaker diarization** (ECAPA-style embeddings, `vox chat --diarize`, WS) | **`speaker-encoder`** | `speaker_encoder.onnx` |
| Piper DE (tests / fallback voice) | `piper-de` and `piper-de-config` | `piper/de_DE-thorsten-medium.onnx` + `.json` |

Examples:

```bash
nix run .#vox-qwen3-cuda -- models download silero-vad
nix run .#vox-qwen3-cuda -- models download whisper-tiny
nix run .#vox-qwen3-cuda -- models download speaker-encoder
```

Sources are the **official Vox registry** in `src/cli/models.rs` (Silero GitHub, **ggerganov/whisper.cpp** on Hugging Face, Rhasspy Piper voices). Qwen3 weights still come from **Hugging Face** (`Qwen/Qwen3-TTS-…`) into the usual HF cache.

## Environment the wrappers set

- **`VOX_MODELS_DIR`** — if unset, defaults to a sensible per-user models directory (XDG on Linux, `Application Support` on macOS).
- **Piper build only:** **`PIPER_ESPEAKNG_DATA_DIRECTORY`** is set to Nixpkgs’ `espeak-ng` `share` directory so Piper can find `espeak-ng-data` at runtime.

The build uses the **system** ONNX Runtime from Nixpkgs (`ORT_STRATEGY`, `ORT_LIB_LOCATION`, `ORT_PREFER_DYNAMIC_LINK`) so the sandbox does not download ONNX at build time.

## Dev shell

`nix develop` uses `inputsFrom` the **default** (Piper) package, so native dependencies match that variant. It also sets `CMAKE_PREFIX_PATH` for the vendored **sonic** library used when building Piper-related code, and `PIPER_ESPEAKNG_DATA_DIRECTORY` like the Piper package.

For day-to-day work **only** on Kokoro, you can still use this shell and run `cargo build --no-default-features --features cli,server,kokoro,pocket,chatterbox` (adjust flags to match your `Cargo.toml`), or add a second dev shell in the flake if you want it spelled out explicitly.

## Moving the Nix files to another repository

Nothing in Vox’s Rust sources depends on the flake living here. To relocate:

1. Copy `flake.nix` (and `flake.lock` once you have one) into the other repo.
2. Replace `self` / the local source (`builtins.path` filter over the repo root) with a fixed-output fetch of Vox (e.g. `fetchFromGitHub` on a tag) or a flake **input** that pins this repository at a revision. Update `voxSrc` to point at that tree so it still contains `Cargo.toml`, `Cargo.lock`, `src/`, `vendor/`, `python/`, `examples/`, `tests/`, `benches/`, and `.cargo/` as today’s filter expects.
3. Re-run `nix flake update` and `nix build .#` / `nix build .#vox-kokoro` / `nix build .#vox-qwen3` on the platforms you care about.

Keeping this file as **`README-nix.md`** documents the Nix workflow without turning the main README into a Nix manual.

## Platforms

The flake lists `x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, and `aarch64-darwin`. Not every combination may be tested in CI; if something fails on your system, compare with Nixpkgs’ support for `onnxruntime` and the rest of the `buildInputs` on that platform.
