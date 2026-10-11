---
layout: home
title: LWSpot
description: A lightweight native Spotify client for Windows.
permalink: /
hero:
  name: LWSpot
  text: A lightweight Spotify client for Windows
  tagline: Listen on your computer, browse your music, and control your other devices. Native, with no browser engine.
  actions:
    - theme: brand
      text: Download
      link: /download/
    - theme: alt
      text: What is LWSpot?
      link: /what-is-lwspot/
    - theme: alt
      text: GitHub
      link: https://github.com/BJDevelopes/lwspot
  image:
    src: /screenshot.png
    alt: "LWSpot Home with the playlist library, recommendations, queue, and player visible"
    width: 1800
    height: 1080

features:
  - icon: ⚡
    title: Lightweight
    details: One process with no browser engine. While playing with the window open, about 4x less CPU and 10x less memory than Spotify's app, and 40% less memory than Spotifast.
    link: /what-is-lwspot/#how-light-is-it
    link_text: See the measurements
  - icon: 🔊
    title: Spotify Connect
    details: Listen on your computer with smooth transitions between songs, or control music on a speaker, phone, or TV. Playback needs Spotify Premium.
  - icon: 📚
    title: Library and search
    details: Browse playlists, Liked Songs, albums, artists, and podcasts. Search the catalogue and edit playlists you own.
  - icon: 🎨
    title: Themes
    details: Pick from a gallery or make your own in the theme editor. Take the accent from Windows or the playing cover, with Mica or acrylic on Windows 11.
  - icon: 📻
    title: Winamp mini player
    details: Bring back the classic Winamp look, with skins, a playlist, sound controls, and animations that move to your music.
    link: /winamp/
    link_text: See it in action
  - icon: 🌀
    title: MilkDrop
    details: Watch colourful animations react to your music in their own window or full screen. Choose from more than 10,000 designs.
    link: /milkdrop/
    link_text: Open the guide
  - icon: ⌨️
    title: Made for Windows
    details: Media keys, the Windows media overlay, taskbar buttons, and a jump list. Keep the music playing from the tray, start with Windows, or press F11 for full screen.
  - icon: 🔓
    title: Open source
    details: Free to use, study, and improve. A fork of Spotifast by Carmine Paolino, under the MIT License.
    link: https://github.com/BJDevelopes/lwspot
    link_text: Read the source
---

## It turns into Winamp

Load a classic `.wsz` skin from the
[Winamp Skin Museum](https://skins.webamp.org). The mini player includes
animated sound displays, an equalizer to adjust your sound, and a playlist.
Roll it up into a thin bar or enlarge it while keeping the classic pixels
sharp. [See the mini player in detail](_guide/winamp.md).

<div class="winamp-showcase">
  <img src="assets/images/winamp.png" alt="The mini player wearing the built-in skin" width="550" height="812">
</div>

## MilkDrop with more than 10,000 presets

MilkDrop is a small add-on LWSpot downloads the first time you turn it on,
along with the original MilkDrop 2 presets and projectM's Cream of the Crop
collection. These visual designs
react to music playing on your computer, in a resizable window or full screen.
[See the controls and preset details](_guide/milkdrop.md).

<video class="milkdrop-showcase" autoplay loop muted playsinline preload="metadata" poster="assets/images/milkdrop-poster.jpg" aria-label="MilkDrop presets reacting to music in LWSpot">
  <source src="assets/images/milkdrop.mp4" type="video/mp4">
</video>

## Credits

LWSpot is a Windows-only fork of
[Spotifast](https://github.com/crmne/spotifast) by Carmine Paolino. It is an
independent project, not affiliated with or endorsed by Spotify AB. Spotify
is a trademark of Spotify AB.

<style>
  /* The hero image slot is sized for a square logo; the screenshot needs the
     room. Page-scoped overrides, so the theme stays untouched. */
  .VPHero .image-container {
    width: 100% !important;
    height: auto !important;
    transform: none !important;
  }
  .VPHero .image-src {
    position: relative !important;
    top: auto !important;
    left: auto !important;
    transform: none !important;
    width: 100% !important;
    height: auto !important;
    max-width: 100% !important;
    max-height: none !important;
    padding: 0 !important;
    border-radius: 12px;
    box-shadow: 0 12px 48px rgba(0, 0, 0, 0.45);
  }
  .winamp-showcase {
    text-align: center;
  }
  .winamp-showcase img,
  .milkdrop-showcase {
    max-width: 100%;
    height: auto;
    border-radius: 12px;
    box-shadow: 0 12px 48px rgba(0, 0, 0, 0.35);
  }
  .milkdrop-showcase {
    display: block;
    width: 100%;
  }
  @media (max-width: 959px) {
    .VPHero .image {
      margin: 0 0 24px !important;
    }
  }
</style>
