---
title: What is LWSpot?
description: What LWSpot offers, what you need to use it, and its current limits.
nav_order: 0
---

## Why LWSpot

LWSpot is a lightweight native Spotify app for Windows. It runs as one
process with no browser engine, and uses a fraction of the memory of
Spotify's desktop app ([measurements below](#how-light-is-it)).

LWSpot is a Windows-only fork of
[Spotifast](https://github.com/crmne/spotifast) by Carmine Paolino.

Your Spotify playlists stay on your account.

**Playback needs Spotify Premium.** Free accounts can browse and search, but
cannot play music through LWSpot on this computer or another device.

![LWSpot Home with the playlist library, recommendations, queue, and player visible](../screenshot.png)

## What it does

- **Plays music on this computer.** LWSpot appears as a Spotify Connect
  device. Select it from your phone or play music in the app. Playback has
  no added gaps between songs and supports up to 320 kbps. Optional settings
  keep songs at a similar loudness and reduce repeat downloads by keeping
  recently played audio.
- **Controls other devices.** Move playback to a speaker, a phone, or
  another computer from the device picker, and keep controlling it: play,
  pause, skip, seek, shuffle, repeat, volume. The picker expands upward to
  show several devices at once, fitting the window; longer lists scroll.
- **Library.** Browse playlists, Liked Songs, saved albums, followed artists,
  podcasts, and saved episodes. Create, edit, and reorder your playlists.
- **Search** across songs, artists, albums, playlists, podcasts, and
  episodes, with artist pages, discographies, and related artists. When Spotify
  provides an artist profile, select its name on a song, including the top
  result, to open that page.
- **Background playback.** Closing the window keeps the music playing from
  the system tray. Use your keyboard's media keys to play, pause, and skip.
- **Windows integration.** The Windows media overlay and lock screen show
  what is playing, the taskbar preview has Previous, Play/Pause and Next
  buttons, and the taskbar's right-click menu has jump list tasks. LWSpot can
  start with Windows, optionally in the tray, F11 fills the screen, and the
  title bar follows the theme's colours.
- **Themes.** Choose light, dark, or one of the themes in the gallery, or
  make your own in the theme editor and share it as a file. The accent can
  follow Windows or the playing song's cover, and Windows 11 can show Mica or
  acrylic through the window.
- **Extras.** A Winamp mini player with classic skins, synced lyrics, a
  ten-band equalizer, a player bar visualizer, and MilkDrop as an optional
  add-on.
- **Reliability.** Your sign-in survives restarts, playback follows your
  default audio output when you connect headphones or Bluetooth, and audio
  problems are explained in plain words.

![The theme gallery in Settings](../assets/images/themes.png)

## How light is it?

These numbers come from one laptop (an MSI GF65 with Windows 11), measured in
one session with release builds playing the same song, averaged over 60
seconds. LWSpot had the player bar spectrum visualizer on at 30 frames a
second.

| While playing | Official Spotify app | LWSpot 0.1.0 |
|---|---|---|
| Memory (working set) | 1,297 MB across 8 processes | 148 MB, 1 process |
| Private memory | 927 MB | 73 MB |
| CPU, window visible | 22% of one core | 18.4% of one core |
| CPU, window minimized | 2.1% of one core | 1.3% of one core |
| CPU, window covered by other windows | not measured | about 0.6% of one core |

Your numbers will vary with your hardware, library and settings. Most of the
visible-window figure is the visualizer: each of its frames redraws the
window, so a lower frame rate in **Settings > Appearance** uses less, and
with it off LWSpot redraws only when something on screen changes. MilkDrop,
when you turn it on, runs as a second process.

<a id="account-safety"></a>

## Will my Spotify account get banned?

**We're not aware of any confirmed account bans caused by normal Premium
listening through LWSpot or other players using the same playback
software, [librespot](https://github.com/librespot-org/librespot).**

LWSpot plays music using your Spotify Premium subscription. It does not
unlock Premium for Free accounts, remove ads, export songs, or bypass
Spotify's copy protection. You sign in on Spotify's own website, and
LWSpot never receives your Spotify password.

LWSpot is independent of Spotify, so we cannot guarantee Spotify's future
decisions. Changes at Spotify can also temporarily interrupt playback until
the app is updated.

## What it does not do

LWSpot has a limited scope:

- **Playing needs Spotify Premium**, both on this computer and when
  controlling another device. Free accounts can browse and search.
- Setup asks for two Spotify approvals: one to access your library, another
  to play music on this computer. [How it connects](../_reference/how-it-connects.md)
  explains why.
- **Spotify Lossless is not available.** Playback supports up to 320 kbps.
  The playback software LWSpot uses, librespot, cannot play Spotify's
  protected lossless audio. This may be reconsidered if
  [librespot adds lawful support](https://github.com/librespot-org/librespot/issues/1583).
- No video podcasts or social features.
- LWSpot is an **unofficial** app. Changes at Spotify can temporarily
  break features until LWSpot is updated.
- LWSpot runs on Windows only.

Bug reports should include `lwspot.log`, `panic.log` after a crash, and
steps to reproduce the problem. Both are in `%LOCALAPPDATA%\LWSpot\data`,
or in `data\state` beside `lwspot.exe` for the portable copy. See
the [issue tracker](https://github.com/BJDevelopes/lwspot/issues). The log
also records the app version, Windows version, and graphics details. If the
window fails to open, the log can help explain why, even when you started the
app from the Start menu.

## Prior art

LWSpot is a fork of [Spotifast](https://github.com/crmne/spotifast) by
Carmine Paolino, which is available under the MIT License. It is written in
Rust, with [egui](https://github.com/emilk/egui) for its interface and
[librespot](https://github.com/librespot-org/librespot) for Spotify playback.
Spotifast took inspiration from
[spotify-tui](https://github.com/Rigellute/spotify-tui),
[spotify-player](https://github.com/aome510/spotify-player),
[ncspot](https://github.com/hrkfdn/ncspot), and
[Omarchy Spotify](https://github.com/stappmus/Omarchy-Spotify).

LWSpot is an independent project, not affiliated with or endorsed by
Spotify AB. Spotify is a trademark of Spotify AB.
