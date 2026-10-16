---
title: Download
description: Download LWSpot for Windows or Mac and install it.
nav_order: 1
---

LWSpot runs on Windows and on Macs. Every release is on the
[releases page](https://github.com/BJDevelopes/lwspot/releases). Each release
lists `checksums.txt`, with a SHA-256 checksum for every download, so you can
check that a file has not been damaged or changed.

## Choose a download

| You want | Intel or AMD PCs (most PCs) | ARM PCs |
|---|---|---|
| Installer | `lwspot-vX.Y.Z-setup.exe` | the same file |
| Portable ZIP | `lwspot-vX.Y.Z-x86_64-pc-windows-msvc.zip` | `lwspot-vX.Y.Z-aarch64-pc-windows-msvc.zip` |

`X.Y.Z` is the version number. The installer is the same for every PC: it
installs the version for your processor. For the portable ZIP, pick the file
for your PC. Most PCs use an Intel or AMD processor (x64). If your PC uses an
ARM processor (Windows on ARM), pick the `aarch64` file. Not sure? Open
**Settings → System → About**: "x64-based processor" means Intel or AMD,
"ARM-based processor" means ARM.

You don't need the `lwspot-milkdrop-...zip` files: LWSpot downloads the
MilkDrop add-on itself the first time you turn it on.

## Installer

Download and run the installer. It installs LWSpot for your Windows user
only, adds it to the Start menu, and does not need an administrator password.
On an ARM PC it installs the ARM version, which runs natively; on any other
PC, the x64 version. If LWSpot is running, the installer offers to close it
first. LWSpot updates itself from then on; uninstall it from
**Settings → Apps → Installed apps**.

## Portable ZIP

To run LWSpot without an installer, download the ZIP file, extract it to a
folder you can write to (your Documents, a USB drive), and open `lwspot.exe`.

The ZIP has an empty `data` folder beside `lwspot.exe`. While it is there,
LWSpot keeps everything it saves in it (settings, play history, caches, logs
and the MilkDrop add-on) instead of in your Windows profile, so you can move
or copy the whole folder and take them along. Updates replace only
`lwspot.exe` and leave `data` alone.

- Your Spotify sign-in belongs to your Windows user, not to the folder. The
  copy stays signed in when you move it on the same PC, and asks you to sign
  in again on another PC or Windows account.
- Without the `data` folder, the copy uses the same folders as an installed
  one. If it cannot write to `data` (for example under Program Files), it
  uses those folders too.
- One LWSpot runs at a time for each Windows user, portable or installed.

[Settings and files](../_reference/settings-and-files.md#where-things-live)
lists what goes where.

## Mac

Download `lwspot-vX.Y.Z-macos-universal.dmg`. It runs on macOS 11 and
later, on Apple silicon and Intel Macs, and needs nothing else installed.

1. Open the disk image and drag **LWSpot** onto the **Applications** folder.
2. Open LWSpot from Applications, Launchpad or Spotlight.

LWSpot is not yet notarized by Apple, so the first time you open it macOS
stops it. You only do this once:

- **macOS 15 Sequoia or later:** click **Done**, open **System Settings →
  Privacy & Security**, scroll to **Security**, and next to "LWSpot was
  blocked to protect your Mac" click **Open Anyway**. Enter your Mac
  password, then click **Open Anyway** again.
- **macOS 14 Sonoma or earlier:** in Applications, Control-click (or
  right-click) LWSpot, choose **Open**, then **Open**.

LWSpot keeps your sign-in in the login Keychain and updates itself from then
on. To uninstall it, drag it from Applications to the Trash.

## Check a download

Each release lists `checksums.txt` with the SHA-256 checksum of every file.
In PowerShell, in the folder you downloaded to:

```powershell
Get-FileHash .\lwspot-vX.Y.Z-setup.exe -Algorithm SHA256
```

The `Hash` it prints must match the line for that file in `checksums.txt`
(PowerShell prints it in capitals). To check every file you downloaded at
once, save `checksums.txt` in the same folder and run:

```powershell
Get-Content .\checksums.txt | ForEach-Object {
  $hash, $name = $_ -split '\s+\*?', 2
  if (Test-Path $name) {
    $ok = (Get-FileHash $name -Algorithm SHA256).Hash -eq $hash
    if ($ok) { "OK        $name" } else { "MISMATCH  $name" }
  }
}
```

## SmartScreen warning

The installer and the app are not yet code-signed, so Windows SmartScreen may
warn about an unknown publisher on first run. Choose **More info**, then
**Run anyway**. Check the file's checksum first, as above. If Microsoft
Defender reports a new release as a threat, please
[open an issue](https://github.com/BJDevelopes/lwspot/issues) with the file
name and the detection name; it is reported to Microsoft as a false positive.

## winget

A winget package is coming soon. Until it is listed, use the installer
above. Once it is available:

```powershell
winget install BJDevelopes.LWSpot
```

## Requirements

LWSpot needs OpenGL 2.0 or newer, which every current Windows graphics
driver provides. In a virtual machine or remote session that only has the
Microsoft Basic Display Adapter, it may not start; install the machine's
graphics driver, or enable GPU acceleration for the virtual machine.

To choose which app opens Spotify links, use **Settings → Apps → Default apps**
in Windows.

To build LWSpot yourself, see [Build from source](getting-started.md#build-from-source).
