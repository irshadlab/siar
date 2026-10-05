{
  description = "SIAR - Survival Identity & Autonomous Routing Communications System";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, flake-compat }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ rust-overlay.overlays.default ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        inherit (pkgs) lib;
        inherit (pkgs.stdenv.hostPlatform) isLinux isDarwin;

        # Modern nixfmt package
        nixfmtPkg = pkgs.nixfmt or pkgs.nixfmt-rfc-style;

        # Rust toolchain matching rust-toolchain.toml (stable Rust via rust-overlay)
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Linux-specific build requirements for Desktop & Media
        desktopLinuxNativeBuildInputs = with pkgs; [
          pkg-config
          wrapGAppsHook3
          gobject-introspection
          desktop-file-utils
        ];

        desktopLinuxBuildInputs = with pkgs; [
          webkitgtk_4_1
          gtk3
          glib
          glib-networking
          cairo
          pango
          gdk-pixbuf
          libsoup_3
          dav1d
          libopus
          alsa-lib
          openssl
          dbus
          gsettings-desktop-schemas
          xdotool
        ];

        # Core headless / CLI Linux dependencies
        coreLinuxBuildInputs = with pkgs; [
          openssl
          dav1d
          libopus
          alsa-lib
          dbus
        ];

        # Deterministic Darwin SDK dependencies (without impure tryEval)
        darwinFrameworks = with pkgs; lib.optionals isDarwin (
          if pkgs ? apple-sdk_11_0 then [
            pkgs.apple-sdk_11_0
          ] else if pkgs ? apple-sdk then [
            pkgs.apple-sdk
          ] else if pkgs ? darwin && pkgs.darwin ? apple_sdk then [
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
            pkgs.darwin.apple_sdk.frameworks.CoreFoundation
            pkgs.darwin.apple_sdk.frameworks.AppKit
          ] else [ ]
        );

        darwinBuildInputs = with pkgs; [
          libopus
          dav1d
          openssl
          libiconv
        ] ++ darwinFrameworks;

        # Common dependencies for all workspace cargo artifacts
        commonNativeBuildInputs = with pkgs; [
          pkg-config
        ];

        commonBuildInputs = with pkgs; [
          openssl
          dav1d
          libopus
        ]
        ++ lib.optionals isLinux desktopLinuxBuildInputs
        ++ lib.optionals isDarwin darwinBuildInputs;

        # ======================================================================
        # Hermetic & Bit-for-Bit Reproducible Source Filtering
        # ======================================================================
        # Whitelist only the code, manifests, and assets required for the build.
        # Excludes all local build artifacts (target/, dist/, apps/android/app/build),
        # documentation, version control metadata, and editor temporaries so that
        # non-code changes or untracked local builds never alter the derivation hash.
        isIgnoredPath = path:
          let
            rel = lib.removePrefix (toString ./.) (toString path);
          in
            lib.hasPrefix "/target" rel
            || lib.hasPrefix "/dist" rel
            || lib.hasPrefix "/docs" rel
            || lib.hasPrefix "/wiki" rel
            || lib.hasPrefix "/sys-arch" rel
            || lib.hasPrefix "/fuzz" rel
            || lib.hasPrefix "/.git" rel
            || lib.hasPrefix "/.github" rel
            || lib.hasPrefix "/.direnv" rel
            || lib.hasPrefix "/result" rel
            || lib.hasInfix "/build/" rel
            || lib.hasInfix "/.gradle/" rel
            || lib.hasSuffix ".md" rel
            || lib.hasSuffix ".tar.gz" rel
            || lib.hasSuffix ".sh" rel;

        sourceFilter = path: type:
          let
            rel = lib.removePrefix (toString ./.) (toString path);
          in
            if isIgnoredPath path then
              false
            else if type == "directory" then
              rel == ""
              || rel == "/apps" || lib.hasPrefix "/apps/" rel
              || rel == "/crates" || lib.hasPrefix "/crates/" rel
              || rel == "/assets" || lib.hasPrefix "/assets/" rel
            else
              (craneLib.filterCargoSources path type)
              || (builtins.match ".*\\.(png|jpg|jpeg|svg|ico|css|sql|json|txt|proto|c|h|toml|lock)$" path != null);

        src = lib.cleanSourceWith {
          src = ./.;
          filter = sourceFilter;
        };

        # ======================================================================
        # Common Hermetic Derivation Arguments
        # ======================================================================
        commonArgs = {
          inherit src;
          pname = "siar-workspace";
          version = "0.1.0";
          strictDeps = true;
          nativeBuildInputs = commonNativeBuildInputs;
          buildInputs = commonBuildInputs;

          # Deterministic codegen and timestamps
          CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "1";
          CARGO_PROFILE_RELEASE_INCREMENTAL = "false";
          SOURCE_DATE_EPOCH = "315532800"; # 1980-01-01 00:00:00 UTC
          TZ = "UTC";
          LC_ALL = "C.UTF-8";

          # Belt-and-suspenders network lockdown: cargoExtraArgs' --offline
          # (below) stops cargo's own registry/index/git calls, but does
          # nothing about a build.rs that tries to phone home directly, or
          # about cargo silently rewriting Cargo.lock. --frozen refuses to
          # touch the lockfile at all; CARGO_NET_OFFLINE covers any cargo
          # subcommand invoked without --offline explicitly.
          CARGO_NET_OFFLINE = "true";

          # Enforce strict offline, lockfile-frozen cargo build in sandbox
          cargoExtraArgs = "--offline --frozen";

          # NOTE on path remapping: this used to set RUSTFLAGS here to
          # "--remap-path-prefix=${src}=/build/siar", but that was a no-op.
          # ${src} is the /nix/store/<hash>-source path known at *eval*
          # time; stdenv's unpackPhase copies a directory src into the
          # sandbox as ./source under $NIX_BUILD_TOP before anything is
          # compiled, so rustc only ever sees $NIX_BUILD_TOP/source/... in
          # file!()/debug-info output — never the literal nix store path.
          # The remap therefore never matched anything, and no build
          # actually became more path-independent from it. The real
          # substitution is done in preBuild below, once $NIX_BUILD_TOP is
          # actually known.
          preBuild = ''
            export RUSTFLAGS="--remap-path-prefix=$NIX_BUILD_TOP/source=/build/siar''${RUSTFLAGS:+ $RUSTFLAGS}"
          '';
        };

        # Pre-built Cargo dependency artifacts (shared cached layer)
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # ======================================================================
        # Package Derivations
        # ======================================================================
        siar-cli = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "siar";
          cargoExtraArgs = "--package siar-cli --bin siar --offline";
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = if isLinux then coreLinuxBuildInputs else commonBuildInputs;
          meta = with lib; {
            description = "SIAR CLI - Sovereign, Interoperable, Asynchronous, Resilient CLI client";
            homepage = "https://github.com/irshadali5/siar";
            license = licenses.agpl3Plus;
            mainProgram = "siar";
          };
        });

        siar-emergency-node = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "siar-emergency-node";
          cargoExtraArgs = "--package siar-emergency-node --bin siar-emergency-node --offline";
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = if isLinux then coreLinuxBuildInputs else commonBuildInputs;
          meta = with lib; {
            description = "SIAR Emergency DTN Mesh Node daemon";
            homepage = "https://github.com/irshadali5/siar";
            license = licenses.agpl3Plus;
            mainProgram = "siar-emergency-node";
          };
        });

        siar-desktop = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "siar-desktop";
          cargoExtraArgs = "--package siar-desktop --bin siar-desktop --offline";
          nativeBuildInputs = [ pkgs.pkg-config ] ++ lib.optionals isLinux desktopLinuxNativeBuildInputs;
          buildInputs = commonBuildInputs;

          # Install freedesktop desktop specification entry and icons
          postInstall = lib.optionalString isLinux ''
            if [ -d "${./assets/branding}" ]; then
              install -Dm644 ${./assets/branding/icon.png} $out/share/pixmaps/siar.png
              for sz in 16 32 64 128 256 512; do
                if [ -f "${./assets/branding}/icon-''${sz}.png" ]; then
                  install -Dm644 "${./assets/branding}/icon-''${sz}.png" "$out/share/icons/hicolor/''${sz}x''${sz}/apps/siar.png"
                fi
              done
            fi

            # Install desktop launcher entry
            mkdir -p $out/share/applications
            cat << 'DESKTOPEOF' > $out/share/applications/siar.desktop
[Desktop Entry]
Name=SIAR
GenericName=Decentralized Secure Messenger
Comment=Sovereign Infrastructure for Autonomous Resilience (P2P / Mesh / E2EE Messenger)
Exec=siar-desktop
Icon=siar
Terminal=false
Type=Application
Categories=Network;InstantMessaging;Chat;P2P;
StartupWMClass=siar-desktop
Keywords=chat;messaging;mesh;p2p;encrypted;e2ee;iroh;dtn;
DESKTOPEOF
            chmod 644 $out/share/applications/siar.desktop
          '';

          meta = with lib; {
            description = "SIAR Desktop GUI client (Dioxus)";
            homepage = "https://github.com/irshadali5/siar";
            license = licenses.agpl3Plus;
            mainProgram = "siar-desktop";
          };
        });

        siar-all = pkgs.symlinkJoin {
          name = "siar-all";
          paths = [ siar-cli siar-emergency-node ] ++ lib.optionals isLinux [ siar-desktop ];
        };

        # ======================================================================
        # Hermetic Flake Checks
        # ======================================================================
        checks = {
          inherit siar-cli siar-emergency-node;

          clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace --all-targets -- -D warnings";
          });

          fmt = craneLib.cargoFmt {
            inherit src;
          };

          test = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--workspace";
          });

          deny = craneLib.cargoDeny {
            inherit src;
          };
        } // lib.optionalAttrs isLinux {
          inherit siar-desktop;
        };
      in
      {
        inherit checks;

        packages = {
          default = siar-cli;
          siar = siar-cli;
          siar-cli = siar-cli;
          siar-emergency-node = siar-emergency-node;
          siar-desktop = siar-desktop;
          all = siar-all;
        };

        apps = {
          default = flake-utils.lib.mkApp {
            drv = siar-cli;
            name = "siar";
          };
          siar = flake-utils.lib.mkApp {
            drv = siar-cli;
            name = "siar";
          };
          siar-emergency-node = flake-utils.lib.mkApp {
            drv = siar-emergency-node;
            name = "siar-emergency-node";
          };
          siar-desktop = flake-utils.lib.mkApp {
            drv = siar-desktop;
            name = "siar-desktop";
          };
        };

        # ======================================================================
        # Hermetic Development Shell
        # ======================================================================
        devShells.default = craneLib.devShell {
          inherit checks;

          packages = with pkgs; [
            rustToolchain
            cargo-deny
            cargo-audit
            cargo-fuzz
            nixfmtPkg
          ]
          ++ commonNativeBuildInputs
          ++ commonBuildInputs;

          shellHook = ''
            export RUST_BACKTRACE=1
            export TZ="UTC"
            export LC_ALL="C.UTF-8"
            export PKG_CONFIG_PATH="${lib.makeSearchPathOutput "dev" "lib/pkgconfig" commonBuildInputs}:''${PKG_CONFIG_PATH:-}"
            ${lib.optionalString isLinux ''
              export LD_LIBRARY_PATH="${lib.makeLibraryPath commonBuildInputs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
              export GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules"
            ''}
            echo "================================================================="
            echo "  SIAR Hermetic Development Environment (Nix Flake)"
            echo "================================================================="
            echo "  Rust:         $(rustc --version 2>/dev/null || echo 'managed by toolchain')"
            echo "  Cargo:        $(cargo --version 2>/dev/null || echo 'managed by toolchain')"
            echo "  Platform:     ${system}"
            echo "  Codegen:      Deterministic (codegen-units=1, remapped paths)"
            echo "  Networking:   glib-networking TLS module active"
            echo ""
            echo "  Build packages:"
            echo "    nix build .#siar-cli"
            echo "    nix build .#siar-desktop"
            echo "    nix build .#siar-emergency-node"
            echo "    nix build .#all"
            echo ""
            echo "  Run checks:"
            echo "    nix flake check"
            echo "================================================================="
          '';
        };

        formatter = nixfmtPkg;
      }
    );
}
