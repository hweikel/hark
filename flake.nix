{
  description = "hark! — a music player that respects perfect albums";

  # Pinned to the exact nixpkgs revision this machine's system channel is on
  # (nixos-26.05.6282.2f5a153c270b). This matters: an unpinned `nixpkgs` would
  # resolve to unstable and pull a *different* webkitgtk, which is a multi-hour
  # source build instead of a store path that is already here.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/2f5a153c270b70cb0f8c11f46d96d6d3bc39f4e3";

  outputs = { self, nixpkgs }:
    let
      # The ThinkPad is the Linux one; the mac mini is almost certainly
      # aarch64-darwin, with x86_64-darwin here for an Intel Mac.
      systems = [ "x86_64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
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
              ];

            shellHook =
              (if isLinux then ''
                # WebKitGTK's DMA-BUF renderer paints a blank white window on a
                # lot of Nix setups. Disabling it costs nothing for a grid of
                # static images.
                export WEBKIT_DISABLE_DMABUF_RENDERER=1

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
              '';
          };
        });
    };
}
