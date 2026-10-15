# Releasing hark!

One machine builds: the NixOS ThinkPad. Everything else installs a tagged
release — the mac mini from GitHub Releases, the laptop from this flake. The mac
does not hold a clone and never compiles anything.

```
git tag v0.1.1  ──▶  GitHub Actions ──▶  Release
                       macos-14           hark_aarch64.app.tar.gz + .sig
                       ubuntu-24.04       hark_0.1.1_amd64.deb
                       tauri-action       latest.json
                                            │
                       mac mini ◀───────────┘  checks latest.json at launch,
                                               replaces itself, restarts
                       ThinkPad   nix profile install …/hark/v0.1.1#hark
```

## Cutting a release

**The first one is `v0.1.0` and needs no bump** — the tree already says 0.1.0,
so it is `git tag -a v0.1.0 -m v0.1.0 && git push --follow-tags` and nothing
else. Every release after that starts here.

The version lives in three files and all three must agree with the tag, or the
`verify` job fails the run before either build starts:

```sh
# 0.1.0 -> 0.1.1 in:
#   src-tauri/tauri.conf.json    "version"   <- the one the updater compares
#   src-tauri/Cargo.toml         version
#   package.json                 version
cd src-tauri && cargo check && cd ..   # refreshes hark's own entry in Cargo.lock

git commit -am "release: v0.1.1"
git tag -a v0.1.1 -m "v0.1.1"
git push --follow-tags
gh run watch
```

The release appears with both builds' artifacts attached and `latest.json`
written by the macOS job. Nothing is a draft — the mac starts offering the
update as soon as the run is green.

If the run fails after the release was created, delete the release *and* the
tag before retrying, or `tauri-action` will upload into the half-finished one:

```sh
gh release delete v0.1.1 --yes
git push --delete origin v0.1.1 && git tag -d v0.1.1
```

## Installing on the mac — once

No Apple Developer account is involved, so macOS treats hark! as an app from an
unidentified developer. That costs exactly one dialog, on the first launch ever.

```sh
cd ~/Downloads
# The macOS asset carries no version in its name — the .deb does, the .app.tar.gz
# does not. Don't reach for a `hark_<version>_aarch64` glob; it matches nothing.
gh release download v0.1.1 --pattern 'hark_aarch64.app.tar.gz'   # or curl -L -O <url>
tar xzf hark_aarch64.app.tar.gz
rm -rf /Applications/hark.app && mv hark.app /Applications/
xattr -dr com.apple.quarantine /Applications/hark.app
open /Applications/hark.app
```

- `gh release download` and `curl` do **not** set `com.apple.quarantine`. A
  browser download does, which is what the `xattr` line clears; it is harmless
  to run either way.
- If Gatekeeper still refuses: **System Settings → Privacy & Security**, scroll
  down, **Open Anyway**. macOS 15 removed the old Control-click → Open
  shortcut, so this is the only route. It is a one-time exception, remembered
  per app.
- Spotlight, Launchpad and the Dock pick it up from `/Applications` with no
  further work. If the Dock shows a stale icon, `killall Dock`.

After that, updates install themselves and this procedure is never needed
again.

## Do auto-updates work with no Apple Developer account?

**Yes.** The two signatures are unrelated, and only one of them matters here:

- **Tauri's updater signature** is minisign. CI signs `app.tar.gz` with the
  private key from the repo secrets; the installed app verifies it against the
  `pubkey` in `tauri.conf.json` before unpacking anything. This is the check
  that makes updating safe, and it works with no Apple involvement.
- **Apple codesigning** is what Gatekeeper looks at, and it is consulted only
  for a *quarantined* bundle. The updater writes the new bundle itself and does
  not set the quarantine attribute, so Gatekeeper is not re-run on an update.
  `signingIdentity: "-"` in `tauri.macos.conf.json` ad-hoc signs the bundle on
  the runner, which satisfies the separate Apple Silicon rule that an
  executable carry *some* signature.

Two things to know:

- `/Applications/hark.app` must be writable by the user running hark!, which on
  a single-admin mac it is. If it is not, the update fails and hark! logs
  `update check failed:` to stderr and keeps running the old version — it never
  blocks playback on an update.
- The app checks once, at launch. Quit and reopen to pick up a release;
  there is no in-app "check now".

Distributing to a Mac that is not this one would need a Developer ID and
notarization. Nothing here does that.

## Installing on NixOS

The tag in the flake ref *is* the pin:

```sh
nix profile install github:hweikel/hark/v0.1.1#hark
```

That puts `hark` on PATH and the `.desktop` entry plus icons in the profile, so
rofi finds it like any other program. `hark sync` works from the same binary.

`nix profile upgrade` cannot move this: a tag is immutable, so upgrading
resolves to the same source. Moving to a new release is deliberate:

```sh
nix profile remove hark
nix profile install github:hweikel/hark/v0.1.2#hark
```

To test an unreleased working copy, `nix build .#hark && ./result/bin/hark` —
or just `cargo tauri dev`, which is the actual dev loop.

## The signing key

| where | what |
|---|---|
| `~/.tauri/hark.key` | private key. Not in the repo, not in Nix, not backed up anywhere else. |
| `~/.tauri/hark.key.pub` | public key — the same string that is committed as `plugins.updater.pubkey`. |
| repo secret `TAURI_SIGNING_PRIVATE_KEY` | the contents of `hark.key`. |
| repo secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | empty — the key has no passphrase. |

It has no passphrase on purpose: the passphrase would have to live in the same
GitHub secret store as the key it protects, next to it, which protects nothing.
To change that:

```sh
npx tauri signer generate -w ~/.tauri/hark.key -f   # prompts for a password
```

**Losing `~/.tauri/hark.key` means installed copies can no longer be updated.**
A new keypair produces signatures the installed app rejects, so every machine
has to be reinstalled by hand once. That is the whole blast radius — it cannot
corrupt a collection, and it is the reason this file says where the key is.
