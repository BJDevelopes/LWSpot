# LWSpot for macOS

LWSpot builds and runs on macOS 11 (Big Sur) and later, on Apple silicon and
Intel Macs.

## Build the app

You need Xcode's command line tools (`xcode-select --install`) and
[rustup](https://rustup.rs). The pinned toolchain in `rust-toolchain.toml`
installs itself on first build.

```sh
packaging/macos/build.sh            # universal: Apple silicon and Intel
packaging/macos/build.sh --native   # this Mac's processor only, faster
```

Both write to `dist/`:

- `LWSpot.app`, the app.
- `lwspot-v<version>-macos-universal.dmg` (or `-arm64` / `-x86_64` with
  `--native`), the disk image to publish.

For development, `cargo run` works as on Windows.

## Publish a release

Attach `lwspot-v<version>-macos-universal.dmg` to the GitHub release and
list it in `checksums.txt`. The in-app updater looks for exactly that name,
so Macs update themselves like the Windows installer does. A release without
it simply offers no update on a Mac.

The app is signed ad hoc unless `CODESIGN_IDENTITY` names a Developer ID
certificate:

```sh
CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" packaging/macos/build.sh
xcrun notarytool submit dist/lwspot-v*-macos-universal.dmg --keychain-profile <profile> --wait
xcrun stapler staple dist/lwspot-v*-macos-universal.dmg
```

Without notarization, people open it the first time with right-click >
**Open**.

## What differs from Windows

| Windows | macOS |
|---|---|
| Credential Manager, with a DPAPI fallback file | The login Keychain (no fallback file) |
| Start with Windows (Run key) | Open at login (a launch agent in `~/Library/LaunchAgents`) |
| Windows accent colour | The accent colour from System Settings > Appearance |
| Title bar in the theme's colours, Mica and acrylic | The system title bar in the theme's light or dark appearance |
| Taskbar thumbnail buttons and jump list | Not applicable; media keys and Now Playing cover playback |
| Polls the default output device | CoreAudio's own default-device notifications |
| EcoQoS while hidden | App Nap, and the visualizer stops while the window is covered |
| MilkDrop add-on download | Only if a release carries `lwspot-milkdrop-v<version>-<arch>-apple-darwin.zip` |
