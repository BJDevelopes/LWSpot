<p align="center">
  <img src="docs/assets/images/logo.svg" alt="LWSpot logo" width="88" height="88">
</p>

<h1 align="center">LWSpot</h1>

<p align="center"><strong>A lightweight native Spotify client for Windows.</strong></p>

<p align="center">
  <a href="https://github.com/BJDevelopes/lwspot/releases"><strong>Download</strong></a> ·
  <a href="docs/_guide/getting-started.md">Getting started</a> ·
  <a href="docs/_guide/using-lwspot.md">User guide</a> ·
  <a href="CONTRIBUTING.md">Contribute</a>
</p>

<p align="center">
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Built_with-Rust-1ed760" alt="Built with Rust"></a>
  <a href="https://github.com/BJDevelopes/lwspot/releases"><img src="https://img.shields.io/badge/Platform-Windows-24292f" alt="Windows"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-1ed760" alt="MIT license"></a>
</p>

LWSpot is a Spotify client for Windows, written in Rust with
[egui](https://github.com/emilk/egui). It plays music through
[librespot](https://github.com/librespot-org/librespot), typically uses
100–250 MB of RAM, starts in well under a second, and has no browser engine.
It is a Windows-only fork of [Spotifast](https://github.com/crmne/spotifast).

The interface includes complete Spanish, Turkish, and Ukrainian translations.

**Playback needs Spotify Premium.** Free accounts can browse and search, but
cannot play music through LWSpot.

![LWSpot Home with the playlist library, recommendations, queue, and player](docs/screenshot.png)

## Features

| Feature | What you can do |
|---|---|
| **Library and search** | Browse playlists, Liked Songs, albums, artists, and podcasts. Search the catalogue and edit playlists you own. |
| **Spotify Connect** | Play on this computer or control playback on your other devices. |
| **Themes** | Choose light, dark, follow Windows, or use your own colours. |
| **Desktop controls** | Use keyboard shortcuts, media keys, the Windows media overlay, and Previous/Play/Next buttons on the taskbar. Keep music playing from the tray after closing the window. |
| **Winamp mini player** | Use classic `.wsz` skins with an equalizer, playlist, and animated sound displays. Switch with **Ctrl+M**. |
| **Lyrics** | Follow synced lyrics in a side panel or full screen. |
| **Equalizer** | Shape local playback with a ten-band equalizer, preamp, and balance. |
| **MilkDrop** (optional) | Watch music-reactive visuals in a separate window or full screen, with more than 10,000 presets downloaded on first use. |

## Install

Download the latest release from
[GitHub Releases](https://github.com/BJDevelopes/lwspot/releases):

| File | Use it for |
|---|---|
| `lwspot-vX.Y.Z-x86_64-pc-windows-msvc-setup.exe` | Installer for most PCs (Intel and AMD). Installs for your user only; no administrator rights needed. |
| `lwspot-vX.Y.Z-x86_64-pc-windows-msvc.zip` | Portable copy for most PCs. Extract and run `lwspot.exe`. |
| `lwspot-vX.Y.Z-aarch64-pc-windows-msvc-setup.exe` / `.zip` | The same, for Windows on ARM. |
| `checksums.txt` | SHA-256 checksums for every download. |

The installer is not code-signed yet, so SmartScreen may warn about an unknown
publisher. Choose **More info**, then **Run anyway**. A winget package is
planned. To build it yourself, see
[Build from source](docs/_guide/getting-started.md#build-from-source).

## Start listening

1. Open LWSpot and choose **Sign in with Spotify**. Approve access in your
   browser, then return to the app to see your library.
2. To listen on this computer, open the device menu in the bottom player bar
   and choose **Set up playback here**, also available in Settings.
3. Complete the separate playback approval in your browser. Your computer
   appears as a Spotify Connect device named **LWSpot**.

Library access and local playback have separate approvals. LWSpot remembers
both in Windows Credential Manager. See
[Getting started](docs/_guide/getting-started.md) for the full walkthrough
and [How it connects](docs/_reference/how-it-connects.md) for the details.

## Guides and help

| You want to… | Read |
|---|---|
| Find out what LWSpot does and does not do | [What is LWSpot?](docs/_guide/what-is-lwspot.md) |
| Install it and check a download | [Download](docs/_guide/download.md) |
| Sign in, set up playback, or configure themes and proxies | [Getting started](docs/_guide/getting-started.md) |
| Learn shortcuts, command-line controls, and updates | [Everyday use](docs/_guide/using-lwspot.md) |
| Use the Winamp mini player or MilkDrop | [Winamp mini player](docs/_guide/winamp.md) · [MilkDrop](docs/_guide/milkdrop.md) |
| Reduce loading delays | [Make it even faster](docs/_guide/make-it-even-faster.md) |
| Find configuration and stored files | [Settings and files](docs/_reference/settings-and-files.md) |
| Understand what is stored and sent | [Privacy](docs/_reference/privacy.md) · [How it connects](docs/_reference/how-it-connects.md) |
| Check Spotify and librespot limitations | [What Spotify allows](docs/_reference/what-spotify-allows.md) |
| Understand account risk | [Will my account get banned?](docs/_guide/what-is-lwspot.md#will-my-spotify-account-get-banned) |
| Report a bug or propose a feature | Read [Contributing](CONTRIBUTING.md), then open an [issue](https://github.com/BJDevelopes/lwspot/issues/new/choose). |

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening an issue or pull
request. To look at the interface without a Spotify account, run:

```powershell
cargo run --features demo -- --demo
```

Translations live in `assets/i18n/`; see
[Translating LWSpot](docs/_reference/translating.md). Release packaging
is described in [PACKAGING.md](PACKAGING.md).

## Acknowledgements

LWSpot is a fork of [Spotifast](https://github.com/crmne/spotifast) by
[Carmine Paolino](https://github.com/crmne), released under the MIT License.
LWSpot builds on that work and narrows it to Windows.

LWSpot uses [librespot](https://github.com/librespot-org/librespot),
[egui](https://github.com/emilk/egui), the [Inter](https://rsms.me/inter/)
typeface (OFL), and [Lucide](https://lucide.dev) icons (ISC).

LWSpot is an independent project and is not affiliated with Spotify.
Spotify is a trademark of Spotify AB.

Licensed under the [MIT License](LICENSE).
