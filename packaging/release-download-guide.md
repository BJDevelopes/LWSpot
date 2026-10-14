## Which file do I need?

**[lwspot-{TAG}-setup.exe](https://github.com/BJDevelopes/lwspot/releases/download/{TAG}/lwspot-{TAG}-setup.exe)**, the installer, for every PC: it installs the right version for your processor (Intel, AMD or ARM).

- **Mac** (macOS 11 or later, Apple silicon or Intel): **[lwspot-{TAG}-macos-universal.dmg](https://github.com/BJDevelopes/lwspot/releases/download/{TAG}/lwspot-{TAG}-macos-universal.dmg)**. Open it, drag LWSpot to **Applications**, and open it from there. If macOS says **"LWSpot" Not Opened**, click **Done**, then **System Settings > Privacy & Security > Open Anyway** (once; on macOS 14 or earlier, Control-click LWSpot and choose **Open**). Nothing else needs installing.
- No install, run from a folder (portable): the `.zip` for your PC, `x86_64` for Intel or AMD, `aarch64` for ARM (for example Snapdragon laptops). Not sure? Open **Settings > System > About**: "x64-based processor" means Intel or AMD, "ARM-based processor" means ARM.
- You don't need the `lwspot-milkdrop-...zip` files: LWSpot downloads the MilkDrop add-on itself the first time you turn it on.
- `checksums.txt` lists each file's SHA-256 for checking a download (`Get-FileHash <file>` in PowerShell, `shasum -a 256 <file>` on a Mac). LWSpot's updater uses it too.
- The installer is not code-signed yet, so Windows SmartScreen may show "Windows protected your PC": choose **More info**, then **Run anyway**.
- Playback needs Spotify Premium. See the [download guide](https://github.com/BJDevelopes/lwspot/blob/main/docs/_guide/download.md) for more.

## What's new
