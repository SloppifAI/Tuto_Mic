{
  description = "Tuto Mic – Unterricht aufnehmen, lokal transkribieren und zusammenfassen";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; config.allowUnfree = true; };

        # Bibliotheken, die winit/eframe/cpal zur Laufzeit per dlopen laden
        runtimeLibs = with pkgs; [
          alsa-lib
          libGL
          libxkbcommon
          vulkan-loader
          wayland
          libx11
          libxcursor
          libxi
          libxrandr
        ];

        common = with pkgs; [
          cargo
          rustc
          clippy
          rustfmt
          rust-analyzer
          # Qualität: Lizenzen/Sicherheitslücken, Tippfehler, ungenutzte Crates, Testrunner, TOML-Format
          cargo-deny
          typos
          cargo-machete
          cargo-nextest
          taplo
          cmake
          pkg-config
          clang
          # Vulkan-Backend von whisper.cpp: Shader-Compiler glslc
          shaderc.bin
        ] ++ runtimeLibs;

        # Aufgebaut wie ein Vulkan-SDK (include/, lib/, bin/glslc): Dort sucht CMakes FindVulkan.
        vulkanSdk = pkgs.symlinkJoin {
          name = "vulkan-sdk";
          paths = with pkgs; [ vulkan-headers vulkan-loader vulkan-loader.dev shaderc.bin ];
        };

        env = {
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib"; # bindgen (whisper-rs-sys)
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}"; # rust-analyzer: Quellen der Standardbibliothek
          # Laufzeit. vulkan-loader findet die Treiber (ICDs) unter NixOS selbst in /run/opengl-driver.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
          VULKAN_SDK = "${vulkanSdk}";
          # whisper-rs-sys linkt unter Linux `vulkan` ohne eigenen Suchpfad
          LIBRARY_PATH = "${vulkanSdk}/lib";
        };
      in
      {
        # Standard: Vulkan (GPU; ohne GPU rechnet whisper.cpp auf der CPU): `nix develop`
        devShells.default = pkgs.mkShell (env // { packages = common; });

        # Zusätzlich CUDA (NVIDIA): `nix develop .#cuda`, dann `cargo run --features cuda`
        devShells.cuda =
          let
            cuda = pkgs.cudaPackages.cudatoolkit;
            # whisper-rs-sys linkt `culibos`, das es ab CUDA 13 nicht mehr gibt -> leerer Platzhalter
            culibosStub = pkgs.runCommand "culibos-stub" { nativeBuildInputs = [ pkgs.binutils ]; } ''
              mkdir -p $out/lib && ar rcs $out/lib/libculibos.a
            '';
          in
          pkgs.mkShell (env // {
            packages = common ++ [ cuda ];
            CUDA_PATH = "${cuda}";
            # nur zur Compile-Zeit: GPU-Kernel nur für die eigene GPU bauen (spart sehr viel Build-Zeit)
            CUDAARCHS = "native";
            # Linker-Suchpfade (whisper-rs-sys sucht CUDA sonst nur unter /usr/local/cuda)
            LIBRARY_PATH = "${cuda}/lib:${cuda}/lib/stubs:${culibosStub}/lib:${env.LIBRARY_PATH}";
            shellHook = ''
              # Laufzeit: echter Treiber (libcuda) aus NixOS + CUDA-Bibliotheken
              export LD_LIBRARY_PATH=/run/opengl-driver/lib:${cuda}/lib:$LD_LIBRARY_PATH
            '';
          });
      });
}
