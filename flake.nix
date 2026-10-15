{
  description = "hark! — a music player that respects perfect albums";

  # Pinned to the exact nixpkgs revision this machine's system channel is on
  # (nixos-26.05.6282.2f5a153c270b). This matters: an unpinned `nixpkgs` would
  # resolve to unstable and pull a *different* webkitgtk, which is a multi-hour
  # source build instead of a store path that is already here.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/2f5a153c270b70cb0f8c11f46d96d6d3bc39f4e3";

  outputs = { self, nixpkgs }:
    let
      # x86_64-linux is the dev machine and the only one this flake packages.
      # The darwin shells are kept for completeness; macOS installs a release
      # build from GitHub Releases instead (see RELEASING.md).
      systems = [ "x86_64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # One source of truth for the version, so a release bumps three files
      # rather than four. The release workflow refuses a tag that disagrees.
      version = (builtins.fromTOML (builtins.readFile ./src-tauri/Cargo.toml)).package.version;
    in
    {
      # `nix profile install github:hweikel/hark/v0.1.0#hark` — the tag in the
      # flake ref is the pin. Nothing in here names a version to chase, and
      # `nix profile upgrade` is deliberately a no-op against an immutable tag:
      # moving to a new release is an explicit reinstall at the new tag.
      packages.x86_64-linux =
        let
          pkgs = nixpkgs.legacyPackages.x86_64-linux;

          # The release binary embeds the frontend — tauri-build reads
          # ../dist at compile time — so the npm half is a separate derivation
          # and the cargo half only has to stage its output. No cargo-tauri
          # involved: `cargo build --release` produces the whole app once
          # dist/ exists.
          frontend = pkgs.buildNpmPackage {
            pname = "hark-frontend";
            inherit version;
            src = self;

            npmDeps = pkgs.importNpmLock { npmRoot = self; };
            npmConfigHook = pkgs.importNpmLock.npmConfigHook;

            # `npm run build` is tsc --noEmit && vite build.
            installPhase = ''
              runHook preInstall
              cp -r dist $out
              runHook postInstall
            '';
          };
        in
        rec {
          default = hark;

          hark = pkgs.rustPlatform.buildRustPackage {
            pname = "hark";
            inherit version;
            src = self;

            # The cargo project is src-tauri/, not the repo root.
            cargoRoot = "src-tauri";
            buildAndTestSubdir = "src-tauri";
            cargoLock.lockFile = ./src-tauri/Cargo.lock;

            # Just the app. scan_report and play_probe are verification tools
            # for a dev shell, not things to put on a user's PATH.
            cargoBuildFlags = [ "--bin" "hark" ];

            # **Load-bearing.** tauri's build script does `dev = !custom_protocol`,
            # so without this feature the binary ignores its own embedded
            # frontend and tries to load build.devUrl — a window that says
            # "Could not connect to localhost: Connection refused". The Tauri
            # CLI passes this for you; a plain `cargo build --release`, which
            # is what this derivation runs, does not.
            buildFeatures = [ "tauri/custom-protocol" ];

            # The suite needs no GUI and no audio device, but it is the dev
            # loop's job and CI's — running it here would only slow the
            # install down.
            doCheck = false;

            nativeBuildInputs = with pkgs; [ pkg-config wrapGAppsHook3 ];

            buildInputs = with pkgs; [
              # Tauri's Linux webview stack: Tauri 2 targets the 4.1 ABI,
              # which is gtk3-based.
              webkitgtk_4_1
              libsoup_3
              gtk3
              glib
              cairo
              pango
              gdk-pixbuf
              atk
              librsvg

              # cpal talks ALSA; PipeWire's compat layer carries it into the
              # graph.
              alsa-lib

              # Both are here for wrapGAppsHook3 to find: without the
              # gsettings schemas, GTK's file chooser (the cover-art picker in
              # the edit modal) aborts on open.
              gsettings-desktop-schemas
              glib-networking
            ];

            # preBuild runs at the source root, before the build hook descends
            # into src-tauri — which is exactly where ../dist has to be.
            preBuild = ''
              cp -r ${frontend} dist
            '';

            preFixup = ''
              # **Load-bearing for the translucent window.** WebKitGTK reads
              # this variable's *value*, not just its presence, and `1` forces
              # the unaccelerated renderer — on which a transparent web view
              # never paints its background. That is the whole reason the Linux
              # build looked opaque: the window had alpha, the page refused to
              # use it. `0` keeps the accelerated path, which is what makes
              # --ink-translucent actually composite against the desktop.
              #
              # --set-default, not --set: a machine whose GPU needs the
              # unaccelerated path (the blank-white-window failure this used to
              # guard against) can still export 1 and get an opaque window back,
              # rather than no window at all.
              gappsWrapperArgs+=(--set-default WEBKIT_DISABLE_DMABUF_RENDERER 0)
            '';

            postInstall = ''
              # The .desktop entry says `Exec=hark`, resolved from PATH, which
              # is what makes this work from rofi with no rewriting:
              # nix profile puts the binary and this file in the same profile.
              install -Dm644 ${./packaging/hark.desktop} \
                $out/share/applications/hark.desktop

              for size in 32x32 128x128 256x256 512x512; do
                install -Dm644 ${./src-tauri/icons}/$size.png \
                  $out/share/icons/hicolor/$size/apps/hark.png
              done

              # The crate also builds as a cdylib/staticlib for Tauri's mobile
              # targets; nothing uses those here.
              rm -rf $out/lib
            '';

            meta = {
              description = "A music player that respects perfect albums";
              homepage = "https://github.com/hweikel/hark";
              mainProgram = "hark";
              platforms = [ "x86_64-linux" ];
            };
          };
        };

      devShells = forAllSystems (pkgs:
        let
          inherit (pkgs.stdenv) isLinux isDarwin;
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              pkg-config
              rustc
              cargo
              rustfmt
              clippy
              rust-analyzer
              cargo-tauri # 2.11.0 on this channel — no need for the npm @tauri-apps/cli
              nodejs
            ];

            buildInputs = with pkgs;
              [ openssl ]
              # Tauri's Linux webview stack. Tauri 2 targets the 4.1 ABI, which
              # is gtk3-based — not gtk4. None of this exists on macOS, where
              # Tauri uses the system WKWebView and cpal uses CoreAudio, both
              # of which come from the Xcode command line tools rather than
              # from nixpkgs.
              ++ lib.optionals isLinux [
                webkitgtk_4_1
                libsoup_3
                gtk3
                glib
                cairo
                pango
                gdk-pixbuf
                atk
                librsvg

                # cpal talks ALSA on Linux; PipeWire's alsa compat layer
                # (already enabled in configuration.nix) carries it into the
                # PipeWire graph.
                alsa-lib

                # The window is translucent and i3 does not composite, so
                # without a compositor running the 50% background renders
                # black. See packaging/picom.conf.
                picom
              ];

            shellHook =
              (if isLinux then ''
                # Keep WebKitGTK on its accelerated renderer: `1` here forces
                # the unaccelerated path, where a transparent web view never
                # paints its background, and `cargo tauri dev` comes up opaque
                # while the installed build is translucent. Overridable, so a
                # machine that needs the unaccelerated path can still say so.
                # See the preFixup in this file and packaging/picom.conf.
                export WEBKIT_DISABLE_DMABUF_RENDERER="''${WEBKIT_DISABLE_DMABUF_RENDERER:-0}"

                # This machine is X11 + i3, not Wayland.
                export GDK_BACKEND=x11

                # Without the gsettings schemas on XDG_DATA_DIRS, GTK's file
                # chooser (used by the cover-art picker in the edit modal)
                # aborts on open.
                export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:$XDG_DATA_DIRS"
                export GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules"
              '' else ''
                # macOS needs the Xcode command line tools for WKWebView and
                # CoreAudio; nixpkgs does not vendor Apple's frameworks.
                if ! xcode-select -p >/dev/null 2>&1; then
                  echo "warning: Xcode command line tools missing — run: xcode-select --install"
                fi
              '') + ''

                echo "hark! dev shell (${pkgs.stdenv.hostPlatform.system})"
                echo "  cargo tauri dev                      run the app (from repo root)"
                echo "  cd src-tauri && cargo test           run tests"
                echo "  cd src-tauri && cargo run --bin scan_report -- ~/music"
              '' + pkgs.lib.optionalString isLinux ''
                echo "  picom -b --config packaging/picom.conf   compositor, for the translucent background"
              '';
          };
        });
    };
}
