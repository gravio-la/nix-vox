{
  description = "Vox — local-first voice AI (Rust dev shell)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = lib.genAttrs systems;

      # For `builtins.path` `filter`: keep only whitelisted paths under `rootPrefix` (relative to that root).
      filterPathByWhitelist =
        {
          rootPrefix,
          includeFilesByBaseName,
          includeTreeRoots,
          includeRelContains,
        }:
        path: type:
        let
          ps = toString path;
          rel =
            if lib.hasPrefix rootPrefix ps then lib.removePrefix rootPrefix ps else ps;
          bn = baseNameOf rel;
        in
        lib.elem bn includeFilesByBaseName
        || lib.any (d: rel == d || lib.hasPrefix "${d}/" rel) includeTreeRoots
        || lib.any (infix: lib.hasInfix infix rel) includeRelContains;

      # Piper + Kokoro cannot be linked in one binary (`ph_list2` duplicate symbol). Split TTS outputs:
      # `default` = Piper; `vox-kokoro` = Kokoro; `vox-qwen3` = Qwen3 (no Piper/Kokoro in the same drv).
      # `sonicLibFor` feeds espeak-ng CMake when Piper is enabled.
      voxFeaturesPiper = "cli,server,piper,pocket,chatterbox";
      voxFeaturesKokoro = "cli,server,kokoro,pocket,chatterbox";
      voxFeaturesQwen3 = "cli,server,qwen3,pocket,chatterbox";
      # Qwen3 TTS (CUDA) + HTTP server + Whisper STT + Silero VAD + Piper (fallback / Live Talk).
      # `cli` already pulls whisper+silero; we list them explicitly for clarity. Piper is safe with
      # Qwen3 (the Kokoro+Piper `ph_list2` clash does not apply here).
      voxFeaturesQwen3Cuda = "cli,server,whisper,silero,qwen3-cuda,piper,pocket,chatterbox";

      sonicLibFor =
        pkgs:
        pkgs.stdenv.mkDerivation {
          pname = "sonic-waywardgeek";
          version = "0-unstable-fbf75c3";
          src = pkgs.fetchFromGitHub {
            owner = "waywardgeek";
            repo = "sonic";
            rev = "fbf75c3d6d846bad3bb3d456cbc5d07d9fd8c104";
            hash = "sha256-LvwfVBT+y2Q+P5huE2X+Xlsx3WBOae0JCZsY2Lgr+bA=";
          };
          buildPhase =
            if pkgs.stdenv.isDarwin then
              "make libsonic.a libsonic.dylib"
            else
              "make libsonic.a libsonic.so.0.3.0";
          installPhase =
            if pkgs.stdenv.isDarwin then
              ''
                mkdir -p $out/lib $out/include
                install -Dm644 sonic.h $out/include/sonic.h
                install -Dm644 libsonic.a $out/lib/libsonic.a
                install -Dm755 libsonic.dylib $out/lib/libsonic.dylib
              ''
            else
              ''
                mkdir -p $out/lib $out/include
                install -Dm644 sonic.h $out/include/sonic.h
                install -Dm644 libsonic.a $out/lib/libsonic.a
                install -Dm755 libsonic.so.0.3.0 $out/lib/libsonic.so.0.3.0
                ln -sf libsonic.so.0.3.0 $out/lib/libsonic.so
                ln -sf libsonic.so.0.3.0 $out/lib/libsonic.so.0
              '';
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          # CUDA toolchains (cuda_nvcc, etc.) are unfree; needed for vox-qwen3-cuda.
          pkgs = import nixpkgs {
            inherit system;
            config.allowUnfree = true;
          };
          inherit (pkgs) stdenv darwin rustPlatform llvmPackages;
          sonic-lib = sonicLibFor pkgs;

          voxSrc =
            let
              rootStr = toString self;
              rootPrefix = rootStr + "/";
              # Only these paths become derivation `src` (see `filterPathByWhitelist`).
              includeFilesByBaseName = [
                "Cargo.toml"
                "Cargo.lock"
                "build.rs"
              ];
              includeTreeRoots = [
                "src"
                "vendor"
                "python"
                "examples"
                "tests"
                "benches"
                ".cargo"
              ];
              includeRelContains = [
                "/.cargo/"
              ];
            in
            builtins.path {
              path = self;
              name = "vox-src";
              filter = filterPathByWhitelist {
                inherit rootPrefix includeFilesByBaseName includeTreeRoots includeRelContains;
              };
            };

          mkVoxMcp = rustPlatform.buildRustPackage {
            pname = "vox-mcp";
            version = "0.6.0";
            src = voxSrc;
            cargoLock.lockFile = ./Cargo.lock;
            strictDeps = true;
            nativeBuildInputs = with pkgs; [ pkg-config ];
            buildInputs =
              with pkgs;
              [ openssl ]
              ++ lib.optionals stdenv.isLinux [ alsa-lib ]
              ++ lib.optionals stdenv.isDarwin (
                with darwin.apple_sdk.frameworks;
                [
                  Security
                  SystemConfiguration
                ]
              );
            cargoBuildFlags = [
              "-p"
              "vox"
              "--bin"
              "vox-mcp"
              "--no-default-features"
              "--features"
              "mcp"
            ];
            doCheck = false;
            meta = {
              description = "Vox MCP server — expose Vox voice AI to Claude Desktop and other MCP clients";
              homepage = "https://github.com/mrtozner/vox";
              license = with lib.licenses; [
                mit
                asl20
              ];
              mainProgram = "vox-mcp";
              platforms = lib.platforms.unix;
            };
          };

          mkVox =
            {
              pname,
              features,
              withPiper,
              metaDescription,
              withCuda ? false,
            }:
            let
              cudaPkgs = pkgs.cudaPackages;
              # Merged toolkit: headers + nvcc (bindgen_cuda looks for include/cuda.h on CUDA_PATH).
              cudaToolkit = cudaPkgs.cudatoolkit;
            in
            rustPlatform.buildRustPackage {
              inherit pname;
              version = "0.6.0";

              src = voxSrc;

              cargoLock.lockFile = ./Cargo.lock;

              strictDeps = true;

              nativeBuildInputs =
                with pkgs;
                [
                  pkg-config
                  cmake
                  # ninja: pulls a default buildPhase that runs `ninja` on the outer drv (no build.ninja).
                  git
                  clang
                  llvmPackages.libclang
                  autoPatchelfHook
                  makeWrapper
                ]
                ++ lib.optionals (withCuda && stdenv.isLinux) [
                  cudaToolkit
                ];

              buildInputs =
                with pkgs;
                [
                  openssl
                  onnxruntime
                ]
                ++ lib.optionals withPiper [
                  sonic-lib
                  espeak-ng
                ]
                ++ lib.optionals stdenv.isLinux [ alsa-lib ]
                ++ lib.optionals (withCuda && stdenv.isLinux) [
                  cudaToolkit
                ]
                ++ lib.optionals stdenv.isDarwin (
                  with darwin.apple_sdk.frameworks;
                  [
                    CoreAudio
                  ]
                );

              env =
                {
                  LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
                  ORT_STRATEGY = "system";
                  ORT_LIB_LOCATION = "${lib.getLib pkgs.onnxruntime}/lib";
                  ORT_PREFER_DYNAMIC_LINK = "1";
                }
                // lib.optionalAttrs withPiper {
                  CMAKE_PREFIX_PATH = lib.makeSearchPath ":" [ sonic-lib ];
                }
                // lib.optionalAttrs (withCuda && stdenv.isLinux) {
                  # cudarc + bindgen_cuda (candle-kernels): headers and nvcc via merged toolkit
                  CUDA_HOME = "${cudaToolkit}";
                  CUDA_PATH = "${cudaToolkit}";
                  CUDA_ROOT = "${cudaToolkit}";
                  # candle-kernels/bindgen_cuda: avoid nvidia-smi during sandbox builds (no GPU in drv)
                  CUDA_COMPUTE_CAP = "80";
                };

              cargoBuildFlags = [
                "-p"
                "vox"
                "--features"
                features
              ];

              doCheck = false;

              # libcudart etc. are from Nix; libcuda is only on the host (NVIDIA driver).
              autoPatchelfIgnoreMissingDeps = lib.optionals (withCuda && stdenv.isLinux) [
                "libcuda.so.1"
              ];

              postInstall =
                let
                  setModelsDir =
                    if stdenv.isDarwin then
                      ''[ -z "$VOX_MODELS_DIR" ] && export VOX_MODELS_DIR="$HOME/Library/Application Support/vox/models"''
                    else
                      ''[ -z "$VOX_MODELS_DIR" ] && export VOX_MODELS_DIR="''${XDG_DATA_HOME:-$HOME/.local/share}/vox/models"'';
                  # Toolkit libs (libcudart, …) + NixOS NVIDIA driver (libcuda.so.1 under /run/opengl-driver).
                  cudaLibPath = lib.optionalString (withCuda && stdenv.isLinux) (
                    " --prefix LD_LIBRARY_PATH : ${
                      lib.makeSearchPath ":" [
                        (lib.getLib cudaToolkit)
                        "/run/opengl-driver/lib"
                      ]
                    }"
                  );
                in
                if withPiper then
                  ''
                    wrapProgram $out/bin/vox \
                      --run '${setModelsDir}' \
                      --set-default PIPER_ESPEAKNG_DATA_DIRECTORY "${pkgs.espeak-ng}/share"${cudaLibPath}
                  ''
                else
                  ''
                    wrapProgram $out/bin/vox --run '${setModelsDir}'${cudaLibPath}
                  '';

              meta = {
                description = metaDescription;
                homepage = "https://github.com/mrtozner/vox";
                license = with lib.licenses; [
                  mit
                  asl20
                ];
                mainProgram = "vox";
                platforms = lib.platforms.unix;
              };
            };
        in
        {
          default = mkVox {
            pname = "vox";
            features = voxFeaturesPiper;
            withPiper = true;
            metaDescription = "Local-first voice AI (Piper TTS build)";
          };
          "vox-mcp" = mkVoxMcp;
          "vox-kokoro" = mkVox {
            pname = "vox-kokoro";
            features = voxFeaturesKokoro;
            withPiper = false;
            metaDescription = "Local-first voice AI (Kokoro TTS build)";
          };
          "vox-qwen3" = mkVox {
            pname = "vox-qwen3";
            features = voxFeaturesQwen3;
            withPiper = false;
            metaDescription = "Local-first voice AI (Qwen3 TTS build)";
          };
        }
        // lib.optionalAttrs stdenv.isLinux {
          "vox-qwen3-cuda" = mkVox {
            pname = "vox-qwen3-cuda";
            features = voxFeaturesQwen3Cuda;
            withPiper = true;
            withCuda = true;
            metaDescription = "Local-first voice AI (Qwen3 CUDA TTS, Whisper STT, Silero VAD, Piper)";
          };
        }
      );

      apps = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            config.allowUnfree = true;
          };
        in
        {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/vox";
          };
          "vox-kokoro" = {
            type = "app";
            program = "${self.packages.${system}."vox-kokoro"}/bin/vox";
          };
          "vox-qwen3" = {
            type = "app";
            program = "${self.packages.${system}."vox-qwen3"}/bin/vox";
          };
          "vox-mcp" = {
            type = "app";
            program = "${self.packages.${system}."vox-mcp"}/bin/vox-mcp";
          };
          # Audible smoke test (Piper): requires audio output device
          speak-demo = {
            type = "app";
            program = "${
              pkgs.writeShellScript "vox-speak-demo" ''
                exec ${self.packages.${system}.default}/bin/vox speak \
                  "Build test. This is Vox speaking." \
                  --backend piper --voice en-us -y
              ''
            }";
          };
        }
        // lib.optionalAttrs pkgs.stdenv.isLinux {
          "vox-qwen3-cuda" = {
            type = "app";
            program = "${self.packages.${system}."vox-qwen3-cuda"}/bin/vox";
          };
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            config.allowUnfree = true;
          };
          vox = self.packages.${system}.default;
          piperOnnx = pkgs.fetchurl {
            name = "en_US-lessac-medium.onnx";
            url = "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx";
            hash = "sha256-Xv4J5pkCGHgnr2RuGm6dJp3udp+Yd9F7FrG0buqvAZ8=";
          };
          piperJson = pkgs.fetchurl {
            name = "en_US-lessac-medium.onnx.json";
            url = "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json";
            hash = "sha256-7+GcQXvtBV8taZCCSMa6ZQ+hNbyGiw5quz2hgdq2kKA=";
          };
        in
        {
          # Offline: builds default package, prefetches Piper en-us, runs synthesis to WAV (no speaker).
          vox-piper-speak-wav = pkgs.runCommand "vox-piper-speak-wav" { } ''
            export HOME=$(mktemp -d)
            mkdir -p "$HOME/vox-models/piper"
            ln -s ${piperOnnx} "$HOME/vox-models/piper/en_US-lessac-medium.onnx"
            ln -s ${piperJson} "$HOME/vox-models/piper/en_US-lessac-medium.onnx.json"
            export VOX_MODELS_DIR="$HOME/vox-models"
            mkdir -p "$out"
            ${vox}/bin/vox speak "Nix build test. Piper speaks." \
              --backend piper --voice en-us -y \
              --output "$out/speak-check.wav"
            test -s "$out/speak-check.wav"
          '';
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            config.allowUnfree = true;
          };
          inherit (pkgs) lib stdenv darwin llvmPackages;
          sonic-lib = sonicLibFor pkgs;
          cudaPkgs = pkgs.cudaPackages;
          cudaToolkit = cudaPkgs.cudatoolkit;
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.default ];

            packages =
              (with pkgs; [
                rustc
                cargo
                rustfmt
                clippy
                rust-analyzer

                git

                # Python bindings (README: maturin develop)
                python3
                maturin
                python3Packages.pip
              ]);

            env = {
              RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
              RUST_BACKTRACE = "1";
              LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
              ORT_STRATEGY = "system";
              ORT_LIB_LOCATION = "${lib.getLib pkgs.onnxruntime}/lib";
              ORT_PREFER_DYNAMIC_LINK = "1";
              CMAKE_PREFIX_PATH = lib.makeSearchPath ":" [ sonic-lib ];
              PIPER_ESPEAKNG_DATA_DIRECTORY = "${pkgs.espeak-ng}/share";
            };

            shellHook = ''
              echo "Vox dev shell (Rust $(rustc --version | cut -d' ' -f2))"
            '';
          };
        }
        // lib.optionalAttrs stdenv.isLinux {
          qwen3-cuda = pkgs.mkShell {
            name = "vox-qwen3-cuda";
            inputsFrom = [ self.packages.${system}."vox-qwen3-cuda" ];

            packages = with pkgs; [
              rustc
              cargo
              rustfmt
              clippy
              alsa-utils
            ];

            env = {
              RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
              RUST_BACKTRACE = "1";
              LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";
              ORT_STRATEGY = "system";
              ORT_LIB_LOCATION = "${lib.getLib pkgs.onnxruntime}/lib";
              ORT_PREFER_DYNAMIC_LINK = "1";
              CUDA_HOME = "${cudaToolkit}";
              CUDA_PATH = "${cudaToolkit}";
              CUDA_ROOT = "${cudaToolkit}";
              CUDA_COMPUTE_CAP = "80";
            };

            shellHook = ''
              echo "Vox Qwen3+CUDA dev shell (Rust $(rustc --version | cut -d' ' -f2), nvcc: ${cudaToolkit}/bin/nvcc)"
              echo "  nix run .#vox-qwen3-cuda -- speak \"Hello\" --backend qwen3 -y -o /tmp/t.wav"
              echo "  aplay /tmp/t.wav"
            '';
          };
        }
      );
    };
}
