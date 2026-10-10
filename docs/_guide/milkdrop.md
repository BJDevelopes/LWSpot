---
title: MilkDrop
description: Run the MilkDrop visualiser, install presets, and use its controls.
nav_order: 5
---

MilkDrop shows colourful animations that react to your music. Open it from
the top-bar visualiser button, Ctrl+Shift+K, Settings, or the mini player's
**V** menu. It runs in its own window.

<video autoplay loop muted playsinline preload="metadata" poster="../assets/images/milkdrop-poster.jpg" aria-label="MilkDrop running in LWSpot" style="width: 100%; height: auto;">
  <source src="../assets/images/milkdrop.mp4" type="video/mp4">
</video>

## The add-on

MilkDrop is an optional add-on, `lwspot-milkdrop.exe`, so LWSpot itself stays
small and nothing of it is on your computer until you use it. The first time
you turn MilkDrop on, LWSpot downloads the add-on for its own version from the
LWSpot release on GitHub (a few MB), checks it against the release's
`checksums.txt`, and opens the window as soon as it is in place. The button
and menus show MilkDrop as on while it downloads; turning it off again before
then keeps the window shut. Add-ons exist for both Intel and AMD PCs and ARM
PCs.

**Settings > MilkDrop > Add-on** shows whether it is installed. **Remove**
deletes it; turning MilkDrop on again downloads it again. After LWSpot
updates, the add-on of the old version is no longer used, and the next time you
turn MilkDrop on LWSpot downloads the one for the new version. A MilkDrop window
that was open when LWSpot closed opens again only if the right add-on is
already there.

The add-on lives in `%LOCALAPPDATA%\LWSpot\data\addons`; see
[file locations](../_reference/settings-and-files.md). A copy of
`lwspot-milkdrop.exe` next to `lwspot.exe` is used first, as long as it is
from the same version: that is where a build of your own goes (see
[Getting started](getting-started.md)).

## The window

Drag the image to move the window. Double-click or press **F** for fullscreen.
Press **Esc** to leave fullscreen or close the window. Drag the lower-right
corner to resize it.

Like the other visualisers, MilkDrop follows changes you make with the
equalizer. Turning down the volume does not change the picture, even at zero.
It reacts only to music playing on this computer.

## Presets

Each visual design is called a **preset**. They change every ten seconds by
default; choose a different interval in Settings.

On first use, LWSpot automatically downloads the 550 MilkDrop 2 presets
and the 9,800-preset Cream of the Crop pack. You can download either pack
again from Settings. A built-in animation appears while the download starts.

Presets are saved in the `milkdrop` folder alongside your settings; see
[file locations](../_reference/settings-and-files.md). MilkDrop is powered by
[projectM](https://github.com/projectM-visualizer/projectm) and supports its
`.milk` preset files.

## Controls

- **N** or right arrow: next preset.
- **P** or left arrow: previous preset.
- **H**: switch on the next beat.
- **L**: keep the current preset.
- **R**: switch between random and folder order.
- **T**: show or hide the preset name.
- **D**: show or hide the frame rate.
- **I**: cycle the song display between a change notification, always visible,
  and hidden.
- **?** or **F1**: show all shortcuts.

The normal playback shortcuts also work.
