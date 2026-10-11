# Contributing to LWSpot

LWSpot is a lightweight native Spotify client for Windows, forked from
[Spotifast](https://github.com/crmne/spotifast). Changes should improve the
desktop app without adding a browser, fallback services, or another backend.

## Before opening an issue

Search open and closed [issues](https://github.com/BJDevelopes/lwspot/issues)
first. For a bug, use the bug form and include the requested log and exact
steps to reproduce it. Reports without enough information to investigate may
be closed.

For a feature, explain the user problem. Discuss large changes in an issue
before writing code. Existing code does not guarantee that a feature fits the
project.

Some boundaries come from Spotify or from upstream libraries:

- Local playback requires Spotify Premium because librespot requires it.
- Spotify Lossless is not available through librespot. It may be reconsidered
  if librespot gains lawful upstream support; proposals that depend on
  bypassing Spotify's DRM are out of scope.
- Spotify tracks must come from Spotify. Substituting audio from YouTube,
  Piped, `yt-dlp`, or another catalogue is out of scope.
- LWSpot will not embed a browser engine, add telemetry, or introduce an
  LWSpot-operated service.
- LWSpot runs on Windows only. macOS and Linux support was removed in the
  fork; use [Spotifast](https://github.com/crmne/spotifast) on those systems.

[What Spotify lets a client do](docs/_reference/what-spotify-allows.md)
lists what each of the three surfaces offers and the requests none of them
can serve, with the reason for each; a request in its last section is
answered from there and closed.

Duplicate, out-of-scope, or incomplete issues may be closed with a short
explanation.

A bug can be closed once its fix is on `main` and the relevant checks pass,
with the commit and release status stated. Reporter confirmation is welcome
but is not required for closure. Reopen the issue if it persists after updating.

## Design principles

1. **Native and fast.** Startup time, idle work, memory use, and binary size
   are product features. Keep the UI thread free of network and disk waits.
2. **Focused.** Prefer a complete, coherent workflow over a collection of
   settings, modes, and speculative features.
3. **Honest integrations.** Use Spotify's Web API and librespot for what they
   support. Do not scrape, impersonate capabilities, bypass technical
   protections, or silently replace one service with another.
4. **Windows first.** Use native Windows integration where it helps (media
   controls, taskbar, tray, Credential Manager) and keep it isolated in its
   own modules. Both x64 and ARM64 builds must keep working.
5. **Small dependency surface.** Reuse the standard library and existing
   crates where practical. A new dependency needs a concrete benefit worth
   its build time, binary size, maintenance, and security cost.
6. **Visible failure, private data.** Errors should be actionable, rate limits
   should be respected, and credentials must never appear in logs. Network
   behaviour belongs in the documentation.

## Architecture

- `src/ui/` draws views and emits `Action`s. Actions are applied after drawing
  in `src/app.rs`; never mutate application state from inside a borrowed view.
- Network and playback work runs on the runtime in `src/backend.rs` or in the
  player engine in `src/player.rs`, never as blocking work on the UI thread.
- Settings and state files stay readable, backward compatible, and atomically
  written. Never log credentials or authorization responses.
- egui, winit and librespot come from pinned forks (see `[patch.crates-io]`
  in `Cargo.toml`). Move all crates of one fork to a new revision together.
- The egui fork shapes right-to-left runs in their own direction but leaves
  them in logical order. Pass logical text to `crate::bidi`, which reorders
  the laid-out runs; never reorder the string before layout.
- Read `docs/_reference/how-it-connects.md` before changing authentication,
  Spotify requests, Connect, credential storage, or network behaviour. Read
  `docs/_reference/queue.md` before touching the queue: its rules are the
  contract, and the queue tests in `src/app.rs` enforce them.
- Check `docs/_reference/what-spotify-allows.md` before building or promising
  a Spotify-facing feature. Requests listed there as impossible are answered
  with that reason, not worked on.
- The interface is optimistic. A control shows its result the moment it is
  used (a double-clicked song is the playing song, Next pops the queue's
  head); the backend then makes it true. A late answer that still describes
  the state from before the user's action is stale: hold the shown state and
  ask again. Nothing the user did may flicker away and come back.
- Every visualiser (spectrum, oscilloscope, MilkDrop) shows the signal
  after the equalizer and before the volume: zero volume still dances.

## Pull requests

Keep each pull request to one change. Explain why it belongs in LWSpot,
what changed, and how you tested it. Avoid unrelated formatting, refactors,
generated prose, and large mechanical rewrites.

`main` has a linear history. Pull requests are squash-merged into one focused
commit with contributor credit; merge commits are not accepted. Do not rewrite
published history.

The same rules apply to every change, however it was written. The author must
understand every line and answer review comments with specific reasoning.

Code changes should include tests for behaviour that can regress. UI changes
should include before/after screenshots or a short recording and should use
demo mode where possible. User-visible behaviour, settings, files, or network
access must be documented in the same pull request.

## Building

You need Rust with the MSVC toolchain (`rust-toolchain.toml` pins the
version; rustup installs it) and Visual Studio 2022 or its Build Tools with
the C++ workload.

```powershell
# lwspot.exe: needs nothing else (no vcpkg, no CMake)
cargo build --release

# The MilkDrop add-on, lwspot-milkdrop.exe
vcpkg install glew:x64-windows-static     # glew:arm64-windows-static on ARM
cargo build --release --features milkdrop --bin lwspot-milkdrop
```

`lwspot.exe` never contains projectM. MilkDrop is the add-on, which the app
downloads from the release of its own version the first time MilkDrop is
turned on, and uses from next to `lwspot.exe` first when one of the same
version is there. Building it compiles libprojectM from source, so it needs
CMake, libclang (from LLVM), and vcpkg with `glew:x64-windows-static`
installed and `VCPKG_INSTALLATION_ROOT` pointing at the vcpkg folder. The app
and the add-on talk over the protocol in `src/milkdrop/args.rs`; raise
`milkdrop::PROTOCOL` with any change to it.

### Demo mode and screenshots

To look at the interface without a Spotify account:

```powershell
cargo run --features demo -- --demo
```

Demo mode uses sample data, never signs in, and never writes your settings.
`--demo-page` opens a page, `--demo-show` adds surfaces such as `queue`,
`light`, `dark`, `themes` or `theme-editor`, and `--demo-language` picks an
interface language. `--demo-shot <file>.png` saves the window and exits, which
is how the screenshots in `docs/` are made:

```powershell
cargo run --features demo -- --demo-shot docs/screenshot.png --demo-size 1440x900 --demo-show dark,queue
```

`lwspot --help` lists the options, and
[Settings and files](docs/_reference/settings-and-files.md#demo-mode) lists
the surfaces and the commands for each screenshot. Use demo shots for the
before/after images in a pull request, and check that nothing personal shows.
Keep committed images under 1 MB.

### Icons

The icon is a white feather wearing dark headphones on a violet (`#6D5BF0`)
rounded tile. Its source is `packaging/icons/lwspot.svg`, with a simplified
`lwspot-small.svg` for sizes of 24 pixels and below. After changing either,
regenerate `packaging/windows/lwspot.ico` and `docs/assets/images/logo.svg`:

```powershell
cargo run --example icon_assets
# with PNG previews of every size:
cargo run --example icon_assets -- --previews target\icon-previews
```

The default accent colour is the same violet in the Dark theme (`#5A47E0` in
Light); keep the two in step.

## Checks

Run the same checks CI runs before submitting:

```powershell
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --no-default-features
cargo test --locked --all-targets --no-default-features --features demo
cargo test --locked --all-targets --all-features
cargo test --locked --all-features --doc
```

The `--all-features` commands build the MilkDrop add-on too, so they need its
toolchain (CMake, libclang and vcpkg's GLEW). Without it, run `cargo fmt` and
the two `--no-default-features` test commands, and let CI run the rest.
CI runs these on Windows x64 and ARM64 runners, checks that the app and the
MilkDrop add-on do not depend on the MSVC runtime DLLs, and compiles the Inno Setup
installer script. Passing CI is required, but does not replace review for
correctness, product fit, maintainability, or security.

Credential-storage changes also need a native store round trip:
`cargo test --locked --lib credentials::tests::native_store_round_trip -- --ignored --exact`.
It writes temporary dummy grants to Windows Credential Manager and deletes
them afterward. The ordinary test suite uses an isolated fake store and never
reads a real Spotify grant. Demo mode also skips credential restoration.

All 15 translations are complete, and `tests/localization.rs` keeps them
that way: a new or changed interface string needs a translation in every
catalogue in `assets/i18n/` before the tests pass. Run
`.github/scripts/update-translations.sh` (GNU gettext tools with Rust support
and a bash shell) when translatable source strings change, fill in and review
every new, fuzzy or missing entry, then run the script with `--check` and
`cargo test --locked --test localization`. Normal Cargo builds compile the
catalogues without gettext tools. See
[Translating LWSpot](docs/_reference/translating.md) for the contributor
workflow.

The user guide in `docs/` is a Jekyll site. Link between its pages with
relative links to the `.md` files so they also work when read on GitHub. To
preview it, run `bundle exec jekyll serve` in `docs/`.

By contributing, you agree that your contribution is licensed under the
project's MIT License.
