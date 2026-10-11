---
title: Privacy
description: What LWSpot stores on your computer, what it sends and to whom, and what it never collects.
nav_order: 4
---

LWSpot is a desktop app that runs entirely on your computer. It has no
account of its own, no server, no telemetry, no analytics, and no advertising.
Its developers receive nothing about you or how you use it.

## What stays on your computer

- **Spotify sign-ins.** The grants Spotify issues when you sign in, and the
  reusable playback credential, are kept in Windows Credential Manager. Your
  Spotify password never passes through LWSpot; you sign in on Spotify's own
  pages. A proxy password, if you set one, uses the same store.
- **Settings and history.** Settings, skins, themes and MilkDrop presets live
  in the config folder. Recent plays and the last session live in the data
  folder.
- **Caches.** Downloaded audio, artwork, lyrics and library metadata live in
  the cache folder and can be deleted at any time.
- **Log.** `lwspot.log` records errors and diagnostics. It stays on your
  computer and never contains credentials; share it only if you choose to
  attach it to a bug report.

[Settings and files](settings-and-files.md) lists every location and what is safe
to delete. **Sign out** in Settings removes the stored credentials.

## What is sent, and to whom

LWSpot connects only to the services below.

- **Spotify.** Sign-in, your library, search, playlists, playback and Spotify
  Connect all go to Spotify, under your account. Spotify's own
  [privacy policy](https://www.spotify.com/legal/privacy-policy/) applies to
  that data.
- **LRCLIB.** When the lyrics panel is open and Spotify has no lyrics for the
  song, LWSpot sends its artist, title, album and length to
  [lrclib.net](https://lrclib.net). Nothing identifying you is included.
- **GitHub.** Once a day, LWSpot asks GitHub for the latest release. You
  can turn automatic checks off in Settings. Downloading an update also
  fetches files from GitHub, and so does turning MilkDrop on: the first time,
  and after an update, it downloads the MilkDrop add-on and the release's
  checksums, and with an empty preset folder the preset packs. Nothing is
  fetched for MilkDrop until you turn it on. No Spotify data is sent.
- **Your local network.** LWSpot looks for Spotify Connect speakers over
  mDNS and talks to the ones you choose.

Links you open from the app, such as the Winamp Skin Museum or this
documentation, open in your browser.

## Questions

Ask on [GitHub](https://github.com/BJDevelopes/lwspot/issues).
