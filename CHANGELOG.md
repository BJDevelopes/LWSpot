# Changelog

All notable changes to LWSpot are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and LWSpot uses
[Semantic Versioning](https://semver.org/). The release workflow publishes a
version's section below as its release notes, so write it for listeners.

## [Unreleased]

### Added

- LWSpot runs on macOS 11 and later, on Apple silicon and Intel Macs, as
  `LWSpot.app` in a disk image. Sign-in is kept in the login Keychain, audio
  follows the Mac's output device, the media keys and Now Playing work, and
  the menu bar icon keeps music playing with the window closed.
- On a Mac: **Open at login** in Settings, the accent colour from System
  Settings (followed live), a title bar that follows the theme's light or
  dark look, and shortcuts shown with Cmd and Option.
- On a Mac: Previous, Play/Pause and Next in the Dock icon's menu,
  `spotify:` links open in LWSpot, your session is saved before the Mac
  sleeps, and the MilkDrop add-on downloads for Apple silicon and Intel.
- On a Mac, the visualizer stops while the window is covered, minimized or
  on another Space, animations run at half rate in Low Power Mode, and
  nothing is drawn while the screen is locked or the display sleeps.

### Fixed

- Updates download again. GitHub names the release files with the
  repository's own capital letters, and the updater refused every one of
  them, on Windows too.
- The Mac disk image says how to get past macOS's first-launch check
  ("LWSpot" Not Opened, then Privacy & Security > Open Anyway) until LWSpot
  is notarized by Apple.
- On a Mac, an update is no longer offered to an account that cannot
  replace the app (a copy another account installed); it points to the
  download page instead. Finished updates no longer leave their files hidden
  in Applications.
- On a Mac, denying the Keychain prompt asks once rather than again for each
  saved sign-in, and a copy opened from the disk image no longer takes over
  **Open at login**.

## [0.1.2] - 2026-10-13

### Added

- Spotify Jam: when you're in a Jam, a button in the player bar shows whose
  Jam it is and who is listening, with **Leave Jam** (or **End Jam** for a Jam
  you host). The mini player shows the Jam in its scrolling title and has the
  same entry in its options menu.
- Discord Listen Along: while a friend's Listen Along plays in LWSpot, the
  player bar and the mini player say so. To stop it, end it in Discord or
  play something else.
- A mini player button at the right end of the player bar, beside the
  volume, as well as the one in the top bar and Ctrl+M.

### Changed

- Settings > About credits BJDevelopes, linking to the GitHub profile.

## [0.1.1] - 2026-10-12

### Changed

- One installer for every PC: it installs the x64 or ARM version
  automatically, whichever matches your processor.
- The release page leads with a "Which file do I need?" guide.

A copy installed with the 0.1.0 installer doesn't update to 0.1.1 by
itself: run the 0.1.1 installer once over it. Your settings and sign-in are
kept. Portable copies update as usual.

## [0.1.0] - 2026-10-11

The first release of LWSpot, a Windows-only fork of
[Spotifast](https://github.com/crmne/spotifast) 0.12.0 by Carmine Paolino. It
keeps Spotifast's library, search, Spotify Connect, lyrics, equalizer, themes
and Winamp mini player, in English and 15 other languages, and focuses on
being the lightest and most reliable Spotify client on Windows.

### Added

- Discord status and Listen Along work through Discord's Spotify connection:
  LWSpot shows as "Listening to Spotify", and joining a friend's Listen Along
  plays in LWSpot. Documented in the user guide.
- Installers for Intel/AMD (x64) and ARM PCs that install for your Windows
  user only, without administrator rights, and portable ZIPs. Every release
  lists SHA-256 checksums, and LWSpot updates itself from its GitHub releases.
- Portable mode: the portable ZIP keeps settings, history, caches and logs in
  a `data` folder beside `lwspot.exe`, so the whole folder can move between
  drives and PCs (on another PC, you sign in to Spotify again). Without that
  folder, or when it cannot be written, the copy uses your Windows profile.
- LWSpot runs side by side with an installed Spotifast: its own settings
  folders, sign-in, Spotify Connect name and taskbar identity.
- A theme gallery in Settings > Appearance: every theme as a small live
  preview, one click to switch.
- A theme editor: start from any theme, change its colours with a live
  preview, and save it as your own. It warns when text would be hard to read.
  Import and export a theme as a single `.json` file to share it.
- Accent colour from the theme, the Windows accent colour, or the playing
  song's cover, adjusted when needed so text stays readable.
- Mica or acrylic window backdrop on Windows 11 (Settings > Appearance >
  Window backdrop).
- The Windows title bar takes the theme's colours.
- Start with Windows, optionally straight to the tray (Settings > Playback on
  this computer).
- Full screen for the main window with F11.
- Taskbar jump list tasks: play/pause, next, previous, mini player and show
  the window, in the interface language.
- A frame-rate choice for the player bar visualizer: 20, 30 or 60 frames a
  second (Settings > Appearance).
- MilkDrop is an optional add-on, downloaded and checked against the
  release's checksums the first time you turn it on, so it costs nothing
  until then.

### Changed

- A new icon, a feather wearing headphones on a violet tile, and violet as
  the default accent colour of the Light and Dark themes.
- Faster seeking and track starts: a seek asks only for the audio it needs,
  no longer waits behind background downloads, uses audio as it arrives, and
  resumes after 1 second of audio instead of 5. On our test laptop new audio
  was ready about 35 ms after a seek.
- Faster start: the first frame is drawn in about 0.2 s instead of 0.4 s, with
  fonts, media controls and the tray set up in the background.
- Much lighter: no continuous redraws while a song plays, the player bar
  visualizer capped at 30 frames a second (15 when the window is not
  focused), and visualizers paused while the window is covered, minimized or
  on another virtual desktop. On the test machine, playing with the window
  visible went from about 107% of a CPU core to about 18%, and memory while
  playing from 203 MB to 148 MB.
- Artwork is decoded at the size it is shown and freed after upload, within a
  fixed texture budget.
- Audio errors explain what went wrong in plain words, such as no output
  device, a device in use by another app, or a disconnected device, and what
  to try next.
- Installing an update or uninstalling closes a running LWSpot first, and
  uninstalling removes its Start with Windows entry.

### Fixed

- Playback follows a change of the default audio device, such as connecting
  Bluetooth headphones, instead of failing.
- Audio on a mono output (such as a Bluetooth headset in hands-free mode)
  now mixes both channels instead of keeping only the left one. This may help
  with audio in one ear after a Spotify Connect handoff; the log now records
  each output's channel count so remaining cases can be found.
- Full-screen lyrics flickering on a portrait monitor. The main window now
  clears to an opaque colour, which may help with black-frame flicker on some
  NVIDIA graphics (not yet confirmed).
- Having to sign in again after a reboot. When Windows Credential Manager
  cannot keep the sign-in, LWSpot keeps it in a file encrypted for your
  Windows user instead.
- The main window or the mini player reopening tiny or off screen after a
  monitor was disconnected.
- Slow seeking while playing on this computer.

### Removed

- macOS and Linux support, and their packages. Use
  [Spotifast](https://github.com/crmne/spotifast) on those systems.

[Unreleased]: https://github.com/BJDevelopes/lwspot/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/BJDevelopes/lwspot/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/BJDevelopes/lwspot/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/BJDevelopes/lwspot/releases/tag/v0.1.0
