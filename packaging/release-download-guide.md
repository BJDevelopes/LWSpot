## Which file do I need?

| Your PC | Download |
|---|---|
| **Most PCs** (Intel or AMD) | [**lwspot-{TAG}-x86_64-pc-windows-msvc-setup.exe**](https://github.com/BJDevelopes/lwspot/releases/download/{TAG}/lwspot-{TAG}-x86_64-pc-windows-msvc-setup.exe), the installer |
| Windows on ARM (for example Snapdragon laptops) | [**lwspot-{TAG}-aarch64-pc-windows-msvc-setup.exe**](https://github.com/BJDevelopes/lwspot/releases/download/{TAG}/lwspot-{TAG}-aarch64-pc-windows-msvc-setup.exe) |
| No install, run from a folder (portable) | the `.zip` for your PC: `x86_64` for Intel or AMD, `aarch64` for ARM |

- Not sure which PC you have? Open **Settings > System > About**: "x64-based processor" means Intel or AMD, "ARM-based processor" means ARM.
- You don't need the `lwspot-milkdrop-...zip` files: LWSpot downloads the MilkDrop add-on itself the first time you turn it on.
- `checksums.txt` lists each file's SHA-256 for checking a download (`Get-FileHash <file>` in PowerShell). LWSpot's updater uses it too.
- The installer is not code-signed yet, so Windows SmartScreen may show "Windows protected your PC": choose **More info**, then **Run anyway**.
- Playback needs Spotify Premium. See the [download guide](https://github.com/BJDevelopes/lwspot/blob/main/docs/_guide/download.md) for more.

## What's new
