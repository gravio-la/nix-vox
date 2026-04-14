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

      # Piper + Kokoro cannot be linked in one binary (`ph_list2` duplicate symbol). Split TTS outputs:
      # `default` = Piper; `vox-kokoro` = Kokoro; `vox-qwen3` = Qwen3 (no Piper/Kokoro in the same drv).
      # `sonicLibFor` feeds espeak-ng CMake when Piper is enabled.
      voxFeaturesPiper = "cli,server,piper,pocket,chatterbox";
      voxFeaturesKokoro = "cli,server,kokoro,pocket,chatterbox";
      voxFeaturesQwen3 = "cli,server,qwen3,pocket,chatterbox";

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
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs) stdenv darwin rustPlatform llvmPackages;
          sonic-lib = sonicLibFor pkgs;

          voxSrc =
            let
              rootStr = toString self;
              rootPrefix = rootStr + "/";
            in
            builtins.path {
              path = self;
              name = "vox-src";
              filter =
                path: type:
                let
                  ps = toString path;
                  rel =
                    if lib.hasPrefix rootPrefix ps then lib.removePrefix rootPrefix ps else ps;
                  bn = baseNameOf rel;
                in
                bn == "Cargo.toml"
                || bn == "Cargo.lock"
                || bn == "build.rs"
                || rel == "src"
                || lib.hasPrefix "src/" rel
                || rel == "vendor"
                || lib.hasPrefix "vendor/" rel
                || rel == "python"
                || lib.hasPrefix "python/" rel
                || rel == "examples"
                || lib.hasPrefix "examples/" rel
                || rel == "tests"
                || lib.hasPrefix "tests/" rel
                || rel == "benches"
                || lib.hasPrefix "benches/" rel
                || rel == ".cargo"
                || lib.hasPrefix ".cargo/" rel
                || lib.hasInfix "/.cargo/" rel;
            };

          mkVox =
            {
              pname,
              features,
              withPiper,
              metaDescription,
            }:
            rustPlatform.buildRustPackage {
              inherit pname;
              version = "0.6.0";

              src = voxSrc;

              cargoLock.lockFile = ./Cargo.lock;

              strictDeps = true;

              nativeBuildInputs = with pkgs; [
                pkg-config
                cmake
                # ninja: pulls a default buildPhase that runs `ninja` on the outer drv (no build.ninja).
                git
                clang
                llvmPackages.libclang
                autoPatchelfHook
                makeWrapper
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
                };

              cargoBuildFlags = [
                "-p"
                "vox"
                "--features"
                features
              ];

              doCheck = false;

              postInstall =
                let
                  setModelsDir =
                    if stdenv.isDarwin then
                      ''[ -z "$VOX_MODELS_DIR" ] && export VOX_MODELS_DIR="$HOME/Library/Application Support/vox/models"''
                    else
                      ''[ -z "$VOX_MODELS_DIR" ] && export VOX_MODELS_DIR="''${XDG_DATA_HOME:-$HOME/.local/share}/vox/models"'';
                in
                if withPiper then
                  ''
                    wrapProgram $out/bin/vox \
                      --run '${setModelsDir}' \
                      --set-default PIPER_ESPEAKNG_DATA_DIRECTORY "${pkgs.espeak-ng}/share"
                  ''
                else
                  ''
                    wrapProgram $out/bin/vox --run '${setModelsDir}'
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
      );

      apps = forAllSystems (
        system:
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
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs) lib stdenv darwin llvmPackages;
          sonic-lib = sonicLibFor pkgs;
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
      );
    };
}
