---
title: Download
description: Download LWSpot for Windows and install it.
nav_order: 1
---

LWSpot runs on Windows. Every release is on the
[releases page](https://github.com/BJDevelopes/lwspot/releases). Each release
lists `checksums.txt`, with a SHA-256 checksum for every download, so you can
check that a file has not been damaged or changed.

## Choose a download

Pick the file for your PC. Most PCs use an Intel or AMD processor (x64). If
your PC uses an ARM processor (Windows on ARM), pick the `aarch64` file.

| You want | Intel or AMD PCs (most PCs) | ARM PCs |
|---|---|---|
| Installer | `lwspot-vX.Y.Z-x86_64-pc-windows-msvc-setup.exe` | `lwspot-vX.Y.Z-aarch64-pc-windows-msvc-setup.exe` |
| Portable ZIP | `lwspot-vX.Y.Z-x86_64-pc-windows-msvc.zip` | `lwspot-vX.Y.Z-aarch64-pc-windows-msvc.zip` |

`X.Y.Z` is the version number.

## Installer

Download and run the installer. It installs LWSpot for your Windows user
only, adds it to the Start menu, and does not need an administrator password.

## Portable ZIP

To run LWSpot without an installer, download the ZIP file, extract it, and
open `lwspot.exe`.

## SmartScreen warning

The installer and the app are not yet code-signed, so Windows SmartScreen may
warn about an unknown publisher on first run. Choose **More info**, then
**Run anyway**. To check the file first, compare its SHA-256 checksum with
`checksums.txt`:

```powershell
Get-FileHash .\lwspot-vX.Y.Z-x86_64-pc-windows-msvc-setup.exe -Algorithm SHA256
```

## winget

A winget package is planned but not available yet.

## Requirements

LWSpot needs OpenGL 2.0 or newer, which every current Windows graphics
driver provides. In a virtual machine or remote session that only has the
Microsoft Basic Display Adapter, it may not start; install the machine's
graphics driver, or enable GPU acceleration for the virtual machine.

To choose which app opens Spotify links, use **Settings → Apps → Default apps**
in Windows.

To build LWSpot yourself, see [Build from source](getting-started.md#build-from-source).
