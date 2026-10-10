# LWSpot roadmap

LWSpot is a Windows-only hard fork of [Spotifast](https://github.com/crmne/spotifast)
(MIT). The goal: the lightest, most reliable Spotify client on Windows.

Decisions made at fork time (2026-10-10):

- **Hard fork, Windows only.** macOS and Linux code, packaging and CI are removed.
  The `upstream` remote stays so useful fixes can still be cherry-picked.
- **Keep:** Winamp mini player, lyrics, equalizer, MilkDrop.
  MilkDrop must cost nothing (memory, exe size) unless the user turns it on.
- **Sign-in:** normal "Sign in with Spotify" by default (public shared Web API app,
  as upstream), with as many reads as possible served by the librespot session.
  A guided personal-app setup is offered when the shared quota rate-limits.
- **Distribution:** ship unsigned at first with SHA-256 checksums and winget; apply
  to SignPath's free OSS code-signing program.
- **Repo:** `BJDevelopes/LWSpot`, private until v0.1, starting from a single
  squashed commit (upstream history is not carried over).

Issue numbers below refer to `crmne/spotifast`.

`[~]` = implemented and unit-tested, awaiting a real-hardware check.

## Phase 0: Fork setup

- [x] Rename to LWSpot everywhere: crate, exe, window title, Connect device name,
      AppData folders, credential-store keys, single-instance mutex, AUMID,
      registry keys. LWSpot must coexist with an installed Spotifast.
- [x] Remove macOS/Linux modules, `cfg` branches, packaging, Flatpak/Nix/Homebrew,
      and their CI. Windows-only CI (x64 + ARM64).
- [x] Repoint updater, docs links and issue forms at our repo.
- [x] README with credit to upstream; keep `LICENSE` (MIT) with both copyrights.
- [x] Record a baseline: exe size, idle/playing RAM, idle CPU, cold start time.
      Upstream 0.12.0 without MilkDrop: `spotifast.exe` 28.5 MB (release, MSVC).
      Measured 2026-10-10 (release build, MSI GF65 laptop, 12 threads), see Phase 2.
      The MilkDrop add-on build needs vcpkg + GLEW + libclang; the app does not.

Phase 0 also removed: the macOS notch, menu bar and Touch Bar code, Linux
desktop integration (MPRIS, Wayland/X11, Omarchy themes, PulseAudio),
Flatpak/Nix/Homebrew/AUR packaging, upstream's issue-triage bots and
funding links. The updater now reads `BJDevelopes/lwspot` releases only.

## Phase 1: Reliability

- [~] Audio output switching (#689, #810): rebuilding the stream on a Bluetooth
      device change fails with `RPC_E_CHANGED_MODE` (-2147417850). Open WASAPI on
      a dedicated MTA thread and follow default-device changes.
- [~] One-ear / channel-imbalance audio after a Connect handoff or volume/EQ
      change (#804).
- [~] Black-frame flicker on NVIDIA since 0.12 (#656) and fullscreen lyrics
      flicker on a portrait monitor (#783). Evaluate a D3D (wgpu/DXGI flip model)
      backend versus the current OpenGL (glow) one.
- [~] Sign-in not persisting across reboots / repeated reauthorization
      (#748, #815, #824, #803). Check the Credential Manager blob size limit
      (2560 bytes for generic credentials) as a hypothesis; add a DPAPI-protected
      fallback (encrypted per user, never plaintext).
- [~] Window restore after a monitor is removed: tiny or offscreen main window
      and mini player (#704, upstream #719 equivalent).
- [~] Seek latency on "This computer" (#343). Our side done (1.5 s read-ahead,
      8 ms post-seek fade, timing logs); the larger wins need a librespot fork.

## Phase 2: Lighter

Baseline (2026-10-10, release build, same song, 60 s averages):

| State | Official Spotify | LWSpot 0.1.0 |
|---|---|---|
| Memory (working set), playing | 1,297 MB, 8 processes | 203 MB, 1 process |
| CPU, playing, window visible | 22% of a core | 107% of a core |
| CPU, playing, minimized | 2.1% of a core | 1.5% of a core |
| CPU, paused, window visible | n/a | 5.7% of a core |
| CPU, paused, minimized | n/a | 0.2% of a core |

- [x] Stop continuous redraws while playing with the window visible. Player-bar
      spectrum capped at 30 fps (15 unfocused, the power saver's half rate now
      applies); the progress bar redraws only when the time or its pixel changes.
      After (same machine, release): playing visible with spectrum 17.8%, paused
      visible 0.4%, playing minimized 1.4%, paused minimized 0.1% of a core.
      A vsync-off experiment saved only ~20%, so the rest is egui's whole-window
      repaint per spectrum frame; redrawing only the visualizer would be the
      next step if it matters.
- [ ] Profile one frame and cut its cost (target 2-3x): ~6 ms of CPU per frame in
      release is high for egui; suspects are rounded artwork meshes, shadows,
      text layout, the sidebar list.
- [ ] Stop visualizer frames while the window is fully covered by other windows.
- [ ] Memory audit: decoded artwork, font atlases, caches (target 120-150 MB).
- [ ] Optional visualizer frame-rate setting (20 / 30 / 60 fps).
- [ ] Winamp mini player visualizer still runs at 60 fps (its analyser steps
      once per frame, so slowing it needs time-based falloff first).

- [x] MilkDrop as an optional component: not linked into the main exe, loaded
      only when enabled. Done as the `lwspot-milkdrop.exe` add-on, downloaded
      from the release the first time MilkDrop is turned on.
- [ ] Zero repaint and minimal timers when minimized or in the tray.
- [ ] Review cache budgets and image decode sizes. Targets set after the baseline.

## Phase 3: Native Windows

- [ ] Start with Windows, optionally to the tray (#122).
- [~] Real fullscreen for the main window (F11) (#675).
- [ ] Jump list entries (play/pause, next, mini player).
- [ ] Discord status: first test whether Discord's built-in Spotify connection
      already shows LWSpot playback; if not, add Discord Rich Presence (local IPC,
      opt-in, needs our own Discord application ID).
- [ ] Optional Last.fm scrobbling.
- [ ] Jam: joining a Jam has no public API (Web API or librespot). Test whether
      LWSpot works as the playback device for a Jam started on a phone.

## Phase 3b: Themes

Built on the existing palette system (Light, Dark, Follow Windows, and JSON
palettes in the themes folder with eight shared presets).

- [ ] Theme gallery: a grid of live previews in Settings replacing the theme
      dropdown, one click to switch.
- [ ] In-app theme editor: start from any theme, adjust colours with pickers
      and a live preview, save as a new theme file. Import and export a theme
      as a single `.json` file to share.
- [ ] Album-art colours (opt-in): derive the accent, and optionally the player
      bar tint, from the current cover; contrast-checked so text stays legible.
- [~] Windows title bar painted in the theme's colours (Windows 11 caption and
      text colours, dark mode on Windows 10), updated on every theme change.
- [ ] Follow the Windows accent colour (opt-in) and react to changes live.
- [ ] Mica / acrylic backdrop on Windows 11 (#336), with an opaque fallback on
      Windows 10 and when transparency effects are off.

## Phase 4: Install and distribution

- [ ] Installer closes or asks to close a running LWSpot before (un)installing (#753).
      Uninstall side written in `lwspot.iss` (stops only the installed copy); untested.
- [ ] True portable mode: data next to the exe (#703).
- [ ] winget manifest and release automation (#176).
- [ ] Apply to SignPath; submit releases to Microsoft for false-positive review (#817).

## Phase 5: Polish

- [ ] Scrollbars overlapping carousel cards; hide-until-hover option (#700).
- [ ] Sign-in card scrollbar (#782).
- [ ] New LWSpot icon and identity (#697).

## Not fixable by a client

Documented in the FAQ, not worked on:

- "Spotify refused the audio key" (#283, #690, #819): server-side refusal.
- Facebook login stuck (#755): Spotify's OAuth flow.
- Shared Web API quota rate limits (#698, #761): mitigated by session reads and
  the guided personal app, not eliminated.
