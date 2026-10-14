# Vendored crates

LWSpot carries its own copy of the few dependency crates it changes, so it
builds from this repository plus the public originals, with no forks. Each
folder keeps its upstream license. Everything else comes from the upstream git
revisions pinned in `Cargo.toml`.

| Crate | From | LWSpot's change |
|---|---|---|
| `fastframe-fonts` | [dennisgr7/fastframe](https://github.com/dennisgr7/fastframe) at `f7dc6fb` (a fork of [crmne/fastframe](https://github.com/crmne/fastframe)) | System fallback fonts are memory-mapped instead of read whole onto the heap (about 25 MB less private memory). |
| `souvlaki` | [crates.io](https://crates.io/crates/souvlaki) 0.8.3 | macOS memory: the Now Playing strings were never released, and the artwork's handler block was passed to `msg_send!` by value, so it and the cover image it holds leaked for every track (about 140 KB a track; `leaks` now finds none). Calls run in an autorelease pool. |
| `winit` | [crmne/winit](https://github.com/crmne/winit) at `ed7caa9` | When an event loop exits on macOS it closes only its own windows, not the app's others: the menu-bar item's window stays open, so the item keeps answering clicks after the window closes to the menu bar. |
| `fastframe-shell` | the same | `Shell::interruptible_idle`: on macOS the app runs AppKit for a whole headless wait and the `Waker` cuts it short, instead of stopping every 150 ms. |
| `fastframe-update` | the same | The Windows installer is one file for every processor, `<slug>-v<version>-setup.exe`, so the updater downloads it instead of `<slug>-v<version>-<target>-setup.exe`. Portable archives keep their per-target names. |
| `librespot-audio` | [crmne/librespot](https://github.com/crmne/librespot) at `23fc42c` (a fork of [librespot-org/librespot](https://github.com/librespot-org/librespot)) | Response bodies are usable as they arrive; random-access reads (seek probes) get their own download slot and ask only for what they read. |
| `librespot-playback` | the same | A seek switches to random-access mode while it probes, and waits for `read_ahead_before_playback` instead of the full read-ahead. |
| `librespot-connect` | the same | `remote_play::subscribe()` reports who sent each play command (`webapi-<client id>` for the Web API), so LWSpot can show when Discord's Listen Along is driving playback. |

Workspace-inherited manifest values are written out. Paths to sibling crates
that are not vendored were replaced by their version, which
`[patch.crates-io]` resolves to the pinned upstream revision.

To refresh one: copy the crate from the upstream revision, reapply the change
(see `git log -- vendor/<crate>`), and keep `Cargo.toml` and this table in step.
