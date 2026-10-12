# Packaging

LWSpot ships only for Windows, as GitHub releases built by
[`.github/workflows/release.yml`](.github/workflows/release.yml).

## Making a release

[RELEASING.md](RELEASING.md) is the step-by-step checklist. In short:

1. Set the version in `Cargo.toml` and add its section to
   [CHANGELOG.md](CHANGELOG.md), then commit both on `main`.
2. Wait for CI to pass on that commit.
3. Push a tag named after the version, such as `v0.1.0`.

Any tag matching `v*` starts the release workflow. A tag with a suffix, such
as `v0.1.0-rc1`, is published as a pre-release. Pre-releases stay off
"latest", so the app's update check never offers them, and they never go to
winget. The release notes are the tag's section of `CHANGELOG.md` (from
`## [X.Y.Z]` to the next `## [`); a tag without a section gets notes generated
by GitHub from the merged pull requests. Edit them on the release page if
needed. Release tags stay immutable: fix a bad release with a new version.

## What the workflow builds

The `build` job runs once per target, each on a native runner:

| Target | Runner | vcpkg triplet |
|---|---|---|
| `x86_64-pc-windows-msvc` | `windows-latest` | `x64-windows-static` |
| `aarch64-pc-windows-msvc` | `windows-11-arm` | `arm64-windows-static` |

For each target it:

1. Builds the app with `cargo build --release --locked --target <target>
   --no-default-features --bin lwspot`, which needs no vcpkg. Then installs
   GLEW through vcpkg for libprojectM, sets up the MSVC environment, and builds
   the MilkDrop add-on with `cargo build --release --locked --target <target>
   --features milkdrop --bin lwspot-milkdrop`. Release builds never use a
   compiler cache.
2. Checks that neither `lwspot.exe` nor `lwspot-milkdrop.exe` imports the MSVC
   runtime DLLs, that both are built for the right processor, and that their
   `--version` prints `lwspot X.Y.Z` and `lwspot-milkdrop X.Y.Z`.
3. When code signing is set up ([below](#code-signing)), has SignPath sign
   `lwspot.exe` and `lwspot-milkdrop.exe`, and uses the signed copies from
   here on.
4. Packs a portable ZIP containing `lwspot.exe`, `README.md`, `LICENSE`,
   `lwspot-portable.txt`, and a `data` folder holding only a `README.txt`
   ([portable copies](#portable-copies)). The marker file tells the updater
   that this copy is portable. MilkDrop is not in it.
5. Packs `lwspot-milkdrop-<tag>-<target>.zip` with `lwspot-milkdrop.exe` and
   `LICENSE`, deflated. The app downloads this the first time MilkDrop is
   turned on and checks it against `checksums.txt`, so its name must keep
   this form.
6. Uploads its `lwspot.exe` (the signed one when signing is on) for the
   installer.

The `installer` job then builds one installer for every PC with Inno Setup
from [`packaging/windows/lwspot.iss`](packaging/windows/lwspot.iss), on
`windows-latest`, from both targets' `lwspot.exe`. It first checks that each
program is built for its processor (the PE header's machine field) and that
the x64 one's `--version` prints `lwspot X.Y.Z`. The installer carries both
programs and installs the ARM64 one on an ARM processor and the x64 one
everywhere else, so an installed copy always runs natively and its updates,
the same installer, keep it that way. It is per-user: it installs to
`%LOCALAPPDATA%\Programs\LWSpot\lwspot.exe` with a Start menu entry and
needs no administrator rights. Its `AppId` must never change, because
Windows uses it to tell an update from a new program. When code signing is
set up, the job has SignPath sign the installer.

The `publish` job then collects the installer and both targets' zips, writes
`checksums.txt` with their SHA-256 hashes (so from the signed files when
signing is on), and creates the GitHub release. The `winget` job follows for
stable releases when it is turned on ([winget](#winget)).

## Release files

Asset names follow the updater's release contract, so do not rename them:

```
lwspot-vX.Y.Z-setup.exe
lwspot-vX.Y.Z-x86_64-pc-windows-msvc.zip
lwspot-vX.Y.Z-aarch64-pc-windows-msvc.zip
lwspot-milkdrop-vX.Y.Z-x86_64-pc-windows-msvc.zip
lwspot-milkdrop-vX.Y.Z-aarch64-pc-windows-msvc.zip
checksums.txt
```

The in-app updater picks the file for its install type (an installed copy
takes the installer, a portable copy the zip for its own architecture) and
verifies it against `checksums.txt` before installing. The installer has
had one name for every processor since v0.1.1; LWSpot carries a copy of
fastframe-update with that one change in `vendor/fastframe-update`
([vendor/README.md](vendor/README.md)). A copy installed from v0.1.0 looks
for the per-processor installers of that release, so it cannot update
itself to v0.1.1 or later: running the new installer once over it moves it
on. The app fetches the MilkDrop add-on of its own version and architecture
the same way, from `releases/download/vX.Y.Z/`, so a release without it
leaves MilkDrop unavailable to that version. Until code signing is set up, the releases are
not code-signed, so SmartScreen may warn on first run.

## Portable copies

A copy is portable when `lwspot-portable.txt` beside `lwspot.exe` reads
`lwspot-portable-v1`. The updater then updates it in place: it downloads the
portable ZIP and replaces only `lwspot.exe`, leaving everything else in the
folder alone.

Where a copy keeps its data is decided once at startup by `src/paths.rs`
(`placement`):

| Beside `lwspot.exe` | Data goes to |
|---|---|
| The portable marker and a writable `data` folder | `data\config`, `data\state`, `data\cache` |
| The portable marker and a `data` folder that cannot be written | The per-user folders, with a warning in the log |
| The portable marker, no `data` folder | The per-user folders |
| `lwspot-installer.txt` (an installed copy), with or without the above | The per-user folders |
| No marker (a build from source) | The per-user folders |

The per-user folders are `%APPDATA%\LWSpot\config`,
`%LOCALAPPDATA%\LWSpot\data` and `%LOCALAPPDATA%\LWSpot\cache`. Writability
is tested by creating and deleting a file in `data`, so a copy unpacked into
Program Files or onto read-only media falls back instead of failing.
eframe's window state (`app.ron`) and the downloaded MilkDrop add-on follow
the data. Two things stay per user on purpose:

- **Spotify sign-in.** Grants are in Windows Credential Manager, which
  belongs to the Windows user, and the fallback files in
  `credential-storage` are encrypted with DPAPI for that user. A portable
  copy records the name of its Credential Manager entries in
  `data\state\credential-profile`, so moving the folder to another path or
  drive letter keeps it signed in; on another PC or Windows account it signs
  in again.
- **The single-instance slot**, in `%LOCALAPPDATA%\LWSpot\data\instance`:
  one LWSpot runs per Windows user, so a portable and an installed copy never
  play at the same time.

## The installer and a running copy

`CloseApplications=force` in `lwspot.iss` has Windows' Restart Manager find a
running `lwspot.exe` in the install folder. An interactive install lists it on
the Preparing to Install page and closes it if the user agrees; a silent
install (an in-app update, or winget) closes it without asking. LWSpot saves
its state when Windows asks it to end its session, and is stopped if it does
not exit. If the installed copy still runs when files are about to be copied,
the script stops it, asking first in an interactive install. The uninstaller
stops it after the user confirms the uninstall. Both match the process by its
full path, so a portable copy elsewhere, or the update helper (a copy of
`lwspot.exe` in a folder of its own), is never touched. The uninstaller also
removes the Start with Windows entry and the `spotify:` link registration when
they name the installed copy, and the downloaded add-ons folder.

## Building the installer locally

With Inno Setup 6.3 or later installed, build a release binary for each
processor and run:

```powershell
rustup target add aarch64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc --no-default-features --bin lwspot
cargo build --release --target aarch64-pc-windows-msvc --no-default-features --bin lwspot
iscc /DVersion=0.1.1 `
  /DBinaryX64=$PWD\target\x86_64-pc-windows-msvc\release\lwspot.exe `
  /DBinaryArm64=$PWD\target\aarch64-pc-windows-msvc\release\lwspot.exe `
  /DOutputDir=dist packaging\windows\lwspot.iss
```

`BinaryX64` and `BinaryArm64` are the `lwspot.exe` the installer puts on an
x64 and an ARM PC. CI also compiles the installer script on every push and
pull request to catch syntax errors, with its debug build in both places.

## winget

The manifests for the `BJDevelopes.LWSpot` package are in
[`packaging/winget`](packaging/winget), in the three-file layout of manifest
schema 1.6.0: `BJDevelopes.LWSpot.yaml` (version),
`BJDevelopes.LWSpot.installer.yaml` (x64 and arm64 entries that both name
the one installer, type `inno`, scope `user`) and
`BJDevelopes.LWSpot.locale.en-US.yaml`. The version, URLs and hashes in them
are placeholders until filled from a release:

```powershell
cd packaging\winget
.\update-manifests.ps1 -Version 0.1.1 -OutDir ..\..\dist\winget
```

The script reads that release's `checksums.txt` from GitHub (or a local file
with `-Checksums`) and fails if the installer is missing from it.

### First submission (by hand)

winget-pkgs accepts a new package only through a reviewed pull request, and
the automation below can only update a package that already exists.

1. Publish the release and fill the manifests as above.
2. Check them with `winget validate --manifest ..\..\dist\winget`, and test
   with `winget install --manifest ..\..\dist\winget` (needs
   `winget settings --enable LocalManifestFiles` once, from an administrator
   prompt).
3. Fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs),
   copy the three files to `manifests/b/BJDevelopes/LWSpot/<version>/`, and
   open a pull request. Alternatively, `wingetcreate submit ..\..\dist\winget`
   opens it for you. Answer the moderators' questions on the pull request
   until it is merged.

### Later releases (automatic)

The `winget` job in the release workflow runs Microsoft's
[wingetcreate](https://github.com/microsoft/winget-create) to open the pull
request for each stable release. It is off by default. To turn it on:

1. Create a classic personal access token with the `public_repo` scope on
   the account that owns the winget-pkgs fork (fine-grained tokens cannot
   open pull requests on another owner's repository).
2. Add it as the repository secret `WINGET_TOKEN`.
3. Add the repository variable `WINGET_SUBMIT` with the value `true`
   (Settings > Secrets and variables > Actions > Variables).

Pre-releases are never submitted. If a run fails, the release is unaffected;
submit that version by hand.

## Code signing

Releases are signed through [SignPath](https://signpath.io) when the
repository variable `SIGNPATH_ORGANIZATION_ID` is set; without it the signing
steps are skipped and the release is unsigned, as now. The order is: sign
`lwspot.exe` and `lwspot-milkdrop.exe` in each `build` job, pack the zips
and, in the `installer` job, build the installer from the signed programs,
sign the installer, then write `checksums.txt`.
Inno Setup's uninstaller inside the installer stays unsigned, since Inno
Setup can only sign it while compiling.

SignPath signs GitHub artifacts that the workflow uploads, and returns them
signed. What it signs inside each is set by an artifact configuration in the
SignPath project; the two the workflow uses are in
[`packaging/signpath`](packaging/signpath):

| Slug | File | Signs |
|---|---|---|
| `programs` | `programs.xml` | `lwspot.exe` and `lwspot-milkdrop.exe` |
| `installer` | `installer.xml` | `lwspot-setup.exe` (the installer, under a fixed name) |

To turn signing on once the project is accepted:

1. In SignPath, create the project with the slug `lwspot` (or set the
   repository variable `SIGNPATH_PROJECT_SLUG`), link it to the GitHub
   repository as its trusted build system, and add the two artifact
   configurations above with those slugs.
2. Create a signing policy with the slug `release-signing` (or set
   `SIGNPATH_POLICY_SLUG`) that uses the SignPath Foundation certificate.
3. Create a CI user with the submitter role on that policy and an API token
   for it, and add the token as the repository secret `SIGNPATH_API_TOKEN`.
4. Add the repository variable `SIGNPATH_ORGANIZATION_ID` with the
   organization ID shown in SignPath.
5. Release a pre-release tag (`vX.Y.Z-rc1`) and check that SignPath received
   the requests (each target's programs, then the installer), that an
   approver approved them if the policy asks for it, and that the files on the release page show a valid signature
   (`Get-AuthenticodeSignature`).
6. After the first signed release, consider giving the updater a publisher
   key (`publisher_key` in `src/updates.rs`) so later versions refuse
   unsigned updates. That is a separate key, for fastframe-update's own
   release signature, not the code-signing certificate.

These steps follow SignPath's documentation as of this writing; check the
input names of `signpath/github-action-submit-signing-request` and the
artifact configuration format against its current documentation when setting
it up.

### Applying to the SignPath Foundation

The SignPath Foundation gives open source projects a free code-signing
certificate. The certificate is the Foundation's, so Windows shows "SignPath
Foundation" as the publisher, and SmartScreen reputation builds up for that
certificate rather than starting from nothing with each release. Its
conditions, as published on [signpath.org](https://signpath.org) (check the
current terms before applying):

- The project uses an OSI-approved open source license (LWSpot: MIT) and has
  no proprietary components of its own.
- The repository is public, the project is maintained, and it has already
  been released (make v0.1.0 public first).
- Every signed file is built from that repository's source by a trusted CI
  system (GitHub-hosted Actions runners here), with SignPath verifying where
  each signing request came from. Nothing built on a personal machine is
  signed.
- The project publishes a code signing policy on its website or README:
  "Free code signing provided by SignPath.io, certificate by SignPath
  Foundation", the team members and their roles (committers, reviewers,
  approvers), and a privacy statement saying what the program sends over the
  network (link [docs/_reference/privacy.md](docs/_reference/privacy.md)).
- Team members with SignPath roles use multi-factor authentication on GitHub
  and SignPath.
- Signed programs carry the project's name and version in their file
  metadata (`build.rs` already sets `ProductName` and the version) and do
  nothing harmful or deceptive.

Steps for the maintainer:

1. Make the repository public and publish v0.1.0 unsigned.
2. Add the code signing policy section to the README (wording above),
   listing yourself as committer, reviewer and approver.
3. Turn on two-factor authentication on GitHub if it is not on.
4. Apply through the form on [signpath.org](https://signpath.org) with the
   repository URL, the license, a short description, and the release
   workflow's URL.
5. When accepted, follow the setup steps under [Code signing](#code-signing).

### Microsoft Defender false positives

Unsigned programs, especially new ones, are sometimes flagged by Microsoft
Defender (spotifast#817). Until releases are signed, and whenever a release is
flagged:

1. Go to [microsoft.com/wdsi/filesubmission](https://www.microsoft.com/wdsi/filesubmission)
   and sign in with a Microsoft account.
2. Choose **Software developer**, product **Microsoft Defender Antivirus
   (Windows 10/11)**, and upload the flagged file: the installer, and
   `lwspot.exe` from each portable ZIP (zip several files into one upload if
   needed).
3. Select **Incorrectly detected as malware/malicious**, name the detection
   shown in Windows Security if there is one, and in the comments say it is
   the open source LWSpot release built by GitHub Actions, with the release
   URL and the file's SHA-256 from `checksums.txt`.
4. Microsoft answers by email, usually within a few days; a cleared file
   needs the Defender definitions to update (`Update-MpSignature`).

SmartScreen's "Windows protected your PC" warning is a separate reputation
check that submissions do not clear; it fades as a signed release is
downloaded more.
