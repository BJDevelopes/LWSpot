# Releasing LWSpot

A checklist for each release. [PACKAGING.md](PACKAGING.md) explains what the
release workflow builds and how winget and code signing are set up.

## Once, before the first release

- [ ] Make the repository public. The in-app updater and the MilkDrop add-on
      download read the public GitHub releases of `BJDevelopes/lwspot`, and a
      private repository's releases cannot be downloaded without a token, so
      v0.1.0 must be published from a public repository.
- [ ] Check that GitHub Actions is allowed to create releases: Settings >
      Actions > General > Workflow permissions, "Read and write permissions".
- [ ] Publish the documentation site: Settings > Pages > Build and
      deployment > Source: "GitHub Actions". Then run the Docs workflow
      (Actions > Docs > Run workflow) and check
      https://bjdevelopes.github.io/LWSpot/ opens. Until the repository is
      public, the Docs workflow builds the site but skips publishing it.
- [ ] Optionally protect `v*` tags (Settings > Rules > Rulesets) so only
      maintainers can push them; release tags are never moved or deleted.

## Each release

1. [ ] On `main`, set `version` in `Cargo.toml` to the new version and run
       `cargo check` so `Cargo.lock` follows.
2. [ ] In `CHANGELOG.md`, move the entries under `[Unreleased]` into a new
       `## [X.Y.Z] - YYYY-MM-DD` section with today's date, written for
       listeners (what changed for them, not how). Add the comparison link at
       the bottom. The release workflow publishes this section as the release
       notes; a tag without a section gets notes generated from the pull
       requests instead. For v0.1.0 the section already exists: replace
       `Unreleased` in `## [0.1.0] - Unreleased` with the date.
3. [ ] Commit both ("Release vX.Y.Z") and push it to `main`. Wait for CI to
       pass on that commit.
4. [ ] Tag that commit and push the tag:

       ```powershell
       git tag vX.Y.Z
       git push origin vX.Y.Z
       ```

       A tag with a suffix, such as `v0.2.0-rc1`, is published as a
       pre-release, which the updater never offers and winget never receives.
5. [ ] Wait for the Release workflow to finish (about 30 to 45 minutes, most
       of it building the MilkDrop add-on).
6. [ ] Check the release page lists all of these, with the right version in
       every name:
       - `lwspot-vX.Y.Z-setup.exe` (one installer for every PC)
       - `lwspot-vX.Y.Z-x86_64-pc-windows-msvc.zip`
       - `lwspot-vX.Y.Z-aarch64-pc-windows-msvc.zip`
       - `lwspot-milkdrop-vX.Y.Z-x86_64-pc-windows-msvc.zip`
       - `lwspot-milkdrop-vX.Y.Z-aarch64-pc-windows-msvc.zip`
       - `checksums.txt`, with a line for each of the five files above
7. [ ] Check the release notes are the CHANGELOG section.

## Smoke test

On a Windows PC (and on an ARM PC when one is available):

- [ ] Download the installer, check its hash against `checksums.txt`
      (`Get-FileHash <file> -Algorithm SHA256`), install, sign in and play a
      song.
- [ ] The installer put the build for this processor in place: in Task
      Manager's Details tab (add the Architecture column), `lwspot.exe` shows
      ARM64 on an ARM PC and x64 on any other. On an ARM PC with a copy
      installed from an x64 installer, installing over it switches it to
      ARM64, keeping settings and sign-in.
- [ ] With LWSpot running, run the installer again: its Preparing to
      Install page offers to close LWSpot, and the install then succeeds.
- [ ] Update: install the previous release, start it, and accept the update
      it offers (Settings, or the update badge). It restarts as the new
      version with settings and sign-in intact. Skip for the first release.
      For v0.1.1, an installed v0.1.0 cannot update itself (it looks for the
      per-processor installers v0.1.0 had): test the update with a portable
      v0.1.0 copy, and check that running the v0.1.1 installer over an
      installed v0.1.0 keeps settings and sign-in.
- [ ] Turn MilkDrop on: the add-on downloads and its window opens.
- [ ] The new icon shows in the Start menu, the taskbar, the tray and the
      installer, and the jump list (right-click the taskbar button) lists its
      five tasks.
- [ ] Settings > Playback on this computer > Start with Windows > Start in
      the tray, then sign out of Windows and back in: LWSpot starts with only
      its tray icon.
- [ ] Portable: extract the portable ZIP to a folder you can write to, start
      `lwspot.exe`, change a setting, and check that `data\config`,
      `data\state` and `data\cache` appeared beside it and that the log
      (`data\state\lwspot.log`) has the "portable copy" line. Move the folder
      to another drive letter or path: it starts with the same settings and
      still signed in.
- [ ] Portable fallback: extract it under `C:\Program Files` (as an
      administrator) and start it as a normal user: it runs with the
      per-user folders and its log says the data folder cannot be written.
- [ ] Uninstall from Settings > Apps while LWSpot runs: it closes, and Start
      with Windows (if it was on) no longer lists it.

## After the release

- [ ] winget: for the first release, submit the manifests by hand (see
      [PACKAGING.md](PACKAGING.md#winget)). Once the package is accepted,
      set the repository variable `WINGET_SUBMIT` to `true` so later stable
      releases open the winget-pkgs pull request themselves.
- [ ] Code signing: until releases are signed, submit the installer and
      `lwspot.exe` to Microsoft for a false-positive review if Defender
      flags them (see [PACKAGING.md](PACKAGING.md#microsoft-defender-false-positives)).
      Apply to the SignPath Foundation once the repository is public and has
      a release (see [PACKAGING.md](PACKAGING.md#applying-to-the-signpath-foundation)).
- [ ] Update `docs/_guide/download.md` and the README when winget or signing
      becomes available.
- [ ] Add an empty `## [Unreleased]` section back to the top of
      `CHANGELOG.md` if it is missing.
