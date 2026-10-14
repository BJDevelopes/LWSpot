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

The Release workflow (`.github/workflows/release.yml`) does this on every
`v*` tag: its `macos` job builds `lwspot-<tag>-macos-universal.dmg` and the
MilkDrop add-on for each processor
(`lwspot-milkdrop-<tag>-aarch64-apple-darwin.zip`,
`lwspot-milkdrop-<tag>-x86_64-apple-darwin.zip`), and the publish job lists
them in `checksums.txt` with the Windows files. The in-app updater and the
MilkDrop download look for exactly those names.

### Signing and notarization

Without Apple credentials the app is signed ad hoc. macOS then stops its
first launch with **"LWSpot" Not Opened**, and people have to allow it once
in **System Settings > Privacy & Security > Open Anyway** (macOS 15 and
later; Control-click > **Open** on 14 and earlier). The DMG carries a short
note saying so. Only a Developer ID signature with notarization removes that
step, so a download opens with a double-click. To sign and notarize, set these
repository secrets (Settings > Secrets and variables > Actions); the job
uses them when they are there:

| Secret | What it is |
|---|---|
| `APPLE_CERTIFICATE_P12` | A "Developer ID Application" certificate and key, exported from Keychain Access as .p12, then `base64 -i cert.p12 \| pbcopy` |
| `APPLE_CERTIFICATE_PASSWORD` | The password the .p12 was exported with |
| `APPLE_SIGNING_IDENTITY` | The certificate's name, e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | The Apple ID of the developer account |
| `APPLE_TEAM_ID` | Its team ID (developer.apple.com > Membership) |
| `APPLE_APP_PASSWORD` | An app-specific password for that Apple ID (appleid.apple.com) |

A Developer ID needs a paid Apple Developer Program membership.

To do the same by hand:

```sh
CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" packaging/macos/build.sh
xcrun notarytool submit dist/lwspot-v*-macos-universal.dmg --apple-id <id> --team-id <team> --password <app-password> --wait
xcrun stapler staple dist/lwspot-v*-macos-universal.dmg
```

## What differs from Windows

| Windows | macOS |
|---|---|
| Credential Manager, with a DPAPI fallback file | The login Keychain (no fallback file) |
| Start with Windows (Run key) | Open at login (a launch agent in `~/Library/LaunchAgents`) |
| Windows accent colour | The accent colour from System Settings > Appearance |
| Title bar in the theme's colours, Mica and acrylic | The system title bar in the theme's light or dark appearance |
| Jump list | Not applicable |
| Polls the default output device | fastframe-audio follows the default output (a cheap check every 2 s) |
| Asks the device for 44.1 kHz | Keeps the device's own rate and resamples, so other apps are not disturbed |
| EcoQoS while hidden | App Nap, and the visualizer stops while the window is covered |
| Taskbar thumbnail buttons | The Dock icon's menu: Previous, Play/Pause, Next |
| `spotify:` links through the registry | `spotify:` links through Info.plist and Apple Events |
| Saves before sleep (power broadcast) | Saves before sleep (IOKit system power) |
| MilkDrop add-on download | The same, from `lwspot-milkdrop-<tag>-<arch>-apple-darwin.zip` |
