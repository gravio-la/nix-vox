# Nix (flake) for Vox

This repository includes a `flake.nix` that provides a Rust dev shell, several build variants of the `vox` binary (Piper, Kokoro, or Qwen3 TTS), and matching `nix run` app targets. The whole layout is self-contained so you can later move **only** the Nix files (and whatever paths they reference) into a separate repo and point them at this project as a source input, without changing how Vox itself is developed.

## Requirements

- [Nix](https://nixos.org/) with flakes enabled (`experimental-features = nix-command flakes`).

## Quick reference

| What you want | Command |
|----------------|---------|
| Run the **default** build (Piper TTS) | `nix run .#` or `nix run .#default` |
| Run the **Kokoro** build | `nix run .#vox-kokoro` |
| Run the **Qwen3** build | `nix run .#vox-qwen3` |
| Build default package | `nix build .#` |
| Build Kokoro package | `nix build .#vox-kokoro` |
| Build Qwen3 package | `nix build .#vox-qwen3` |
| Developer shell (Rust, ONNX Runtime env, Piper-oriented paths) | `nix develop` |

After `nix build`, the binary is at `result/bin/vox` (both packages install the same executable name `vox`; the store path distinguishes them).

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

## Why multiple packages (`default`, `vox-kokoro`, `vox-qwen3`)

Piper and Kokoro cannot be linked into **one** binary in this tree (you get a duplicate symbol such as `ph_list2`). Qwen3 is shipped as its **own** output as well (no Piper/Kokoro in that derivation), so you can switch TTS stacks without rebuilding unrelated engines.

The flake builds **three** main package variants from the same crate with different Cargo features:

- **`packages.<system>.default`** — `cli,server,piper,pocket,chatterbox` (Piper TTS).
- **`packages.<system>.vox-kokoro`** — `cli,server,kokoro,pocket,chatterbox` (Kokoro TTS; no Piper).
- **`packages.<system>.vox-qwen3`** — `cli,server,qwen3,pocket,chatterbox` (Qwen3 TTS; no Piper/Kokoro).

Apps mirror that: `default`, `vox-kokoro`, and `vox-qwen3` each run the corresponding package’s `vox`.

Qwen3 models are large and not bundled; download them separately (see upstream Qwen3-TTS / `VOX_QWEN3_MODEL_PATH` in the main docs).

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
