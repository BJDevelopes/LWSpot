---
title: Settings and files
description: Configuration, credential and cache locations, and what is safe to delete.
nav_order: 0
---

## Where things live

LWSpot keeps its files in three folders:

- **Config:** `%APPDATA%\LWSpot\config`
- **Data (state and logs):** `%LOCALAPPDATA%\LWSpot\data`
- **Cache:** `%LOCALAPPDATA%\LWSpot\cache`

A portable copy with a `data` folder beside `lwspot.exe` (as the portable
ZIP comes) uses that folder instead, with the same three inside:

```
LWSpot  lwspot.exe
  lwspot-portable.txt   marks the copy as portable, for updates too
  data    config\             settings.json, skins, MilkDrop presets, themes
    state\              session, history, logs, add-ons, app.ron
    cache\              audio, artwork, lyrics, playlist pages
```

In the table below, read `%APPDATA%\LWSpot\config` as `data\config`,
`%LOCALAPPDATA%\LWSpot\data` as `data\state`, and
`%LOCALAPPDATA%\LWSpot\cache` as `data\cache` for such a copy. The window
state eframe keeps (`app.ron`, in `%APPDATA%\lwspot\data` otherwise) moves
there too. The copy needs both `lwspot-portable.txt` reading
`lwspot-portable-v1` and the `data` folder; a copy without the folder, an
installed copy, and a copy that cannot write to `data` (such as one under
Program Files, which then logs a warning) use the usual folders. Credential
Manager entries stay with the Windows user, and the encrypted fallback files
in `data\state\credential-storage` open only for the user who wrote them:
moving the folder keeps the copy signed in, while another PC or Windows
account signs in again. `data\state\credential-profile` records which
Credential Manager entries the copy uses, so a new drive letter does not
lose them.

| What | Where | Safe to delete? |
| --- | --- | --- |
| Settings | `%APPDATA%\LWSpot\config\settings.json` | Yes, you lose preferences |
| Winamp skins | `%APPDATA%\LWSpot\config\skins\` | Yes, you add them again |
| MilkDrop presets | `%APPDATA%\LWSpot\config\milkdrop\` | Yes, you fetch them again |
| MilkDrop add-on | `%LOCALAPPDATA%\LWSpot\data\addons\lwspot-milkdrop.exe` | Yes, it downloads again the next time you turn MilkDrop on |
| Custom themes | `%APPDATA%\LWSpot\config\themes\` | Yes, you lose your palettes |
| Spotify grants | Windows Credential Manager | Use Sign out in Settings |
| Credential revocation markers | `%LOCALAPPDATA%\LWSpot\data\credential-storage\` | Keep after a failed sign-out deletion |
| Encrypted grant fallback | `%LOCALAPPDATA%\LWSpot\data\credential-storage\<slot>.bin` | Use Sign out in Settings |
| Proxy password | Windows Credential Manager | Clear the password and apply the manual proxy settings |
| Last session | `%LOCALAPPDATA%\LWSpot\data\session.json` | Yes |
| Play history | `%LOCALAPPDATA%\LWSpot\data\history.json` | Yes |
| Audio cache | `%LOCALAPPDATA%\LWSpot\cache\audio\` | Always |
| Artwork cache | `%LOCALAPPDATA%\LWSpot\cache\art\` | Always |
| Lyrics cache | `%LOCALAPPDATA%\LWSpot\cache\lyrics\` | Always |
| Account-scoped playlist page cache | `%LOCALAPPDATA%\LWSpot\cache\playlists\<account-id>\` | Always |
| Last run's log | `%LOCALAPPDATA%\LWSpot\data\lwspot.log` | Always |
| Crash log | `%LOCALAPPDATA%\LWSpot\data\panic.log` | Always |

The add-ons folder holds only what LWSpot downloads on first use: today the
MilkDrop add-on. **Remove** under Settings > MilkDrop deletes it, and
uninstalling LWSpot deletes the folder.

Clearing caches never signs you out. Sign-out from Settings covers the shared
and personal Web API grants and the independent playback credential.

## Credentials

Durable grants are kept in **Windows Credential Manager** with Local
persistence: they stay on this computer and never roam with a domain profile.
Entries saved by earlier versions with roaming persistence are rewritten as
Local the next time they are read. Entries are
separated by application state location and grant type; Web grants carry their
Client ID and must verify as the same account. Playback and receiver
activation require that account too. Non-secret settings and session data
remain readable JSON. Native credential protection reduces exposure from
copying ordinary application files. It does not protect a usable session from
arbitrary code running as the same user.

Some computers cannot keep a Credential Manager entry: the store refuses the
write, a policy makes every entry last only until Windows sign-out, or the
record is too large for it. LWSpot then keeps that grant in
`credential-storage\<slot>.bin` (`shared-web`, `personal-web`, `playback`, or
`proxy-password`), encrypted with Windows DPAPI for the signed-in Windows user,
and reads it after Credential Manager. A file left by a save the store
refused holds the newer grant, so it wins over an older entry and moves back
into Credential Manager; a successful save there removes the file. Sign-out and forgetting a grant delete both copies.
Nothing is ever written in plaintext. A failure is logged by kind and Windows
error code only, never with the grant.

An unavailable store produces an error without blocking the interface. A new
sign-in can still be used for this session. At startup, a store that refuses
to answer is asked again a few times within about two seconds before a saved
grant is treated as missing.
If Spotify rejects a saved refresh grant, only that grant is forgotten so the
next launch cannot keep restoring it. Librespot's reusable grant stays in
memory until LWSpot saves it through this same store. Volume and disposable
audio caches are independent.

Sign-out invalidates pending authorization, refresh, and playback connections,
and cancels pending Spotify requests so their results cannot undo a new sign-in.
It records non-secret revocation markers before deleting the protected entries,
including temporary copies. A store or file system failure is reported.
Revocation markers prevent a failed protected entry deletion from restoring
the session on restart; keep these markers when a deletion failed. Removing or
changing a personal Client ID clears that app's old grant.

Keep credential-store exports out of issue attachments and diagnostic uploads.

Proxy passwords use a separate protected entry in Credential Manager. They
belong to the configured host, port, and username. Editing any of these
fields clears the old password; changing only HTTP/SOCKS5 mode keeps it. Off,
System, and Spotify sign-out retain the saved manual proxy password. Clear the
password and apply the manual settings to forget it. A revocation marker keeps
a failed protected deletion from restoring that password. Newly entered
passwords have no plaintext fallback. Settings store the confirmed proxy mode,
host, port, and username, never an unapplied draft or password.

## Caches

Progress through a playlist is periodically cached as a contiguous prefix.
When the playlist has not changed on Spotify, reopening it resumes from that
prefix instead of requesting the same pages again. LWSpot validates the
cache against Spotify's playlist snapshot and reported song count before
showing it. A cache with a mismatched count is replaced by live rows even if
its snapshot matches, so stale cached songs cannot choose the playback order.
Successful playlist edits keep that loaded prefix and save it under Spotify's
new snapshot after all pending writes have succeeded. Pending edits remain
visible immediately, but are not saved as confirmed playlist rows. A failed
write reloads the playlist to reconcile the edit.

Playlist checkpoints are written and read on a background file worker, so a
large playlist does not need a second complete copy in memory. A failed write
leaves the previous cache in place, and missing or invalid caches are ignored
and fetched again.

Playlist checkpoints keep new rows in a separate append-only data file.
An atomically replaced manifest records the committed byte length, row count,
snapshot, and next Spotify offset. A failed append leaves the previous
checkpoint readable; a later write discards the unfinished tail. Older JSON
caches remain readable and are retained when a new checkpoint is written.
Reloads, edits, and changes to previously loaded rows write a fresh data file
instead of appending to the old prefix. A small lock file in the account cache
directory coordinates reads and writes; reopening the cache removes row files
left without a manifest reference by an interrupted replacement.

The artwork loader shares downloaded image bytes with the background cache
writer instead of making a separate copy. Visible library cards and collection
covers request 640-pixel artwork for sharper output on high-DPI displays;
compact rows and softened placeholders keep using 64-pixel thumbnails. Up to 64
softened 256-pixel covers may be retained in addition to the existing artwork
budget, and their downloaded JPEG bytes are released after decoding. A failed
cache write does not prevent the downloaded image from being displayed.
Softened previews reuse egui's decoded artwork after its encoded bytes have
been released, without repeatedly reading the disk cache or downloading again.

Liked Songs metadata is stored separately under `liked-songs\` in the cache
folder, one JSON file per account. Only the verified account's rows are
shown. Fresh cached pages are reused for 15 minutes; older pages refresh in
the background. Refreshing keeps the last usable rows until their replacement
is ready, and a failed refresh leaves those rows visible. The refresh control
requests current data immediately. Partial caches resume from their next page.
Like and Unlike change the rows immediately, and confirmed edits survive a
restart even if Spotify's next read still reports the old state. This cache
contains metadata, not offline audio, and can be deleted without signing out.

The last good playlist folder tree is kept in `session.json`, scoped to the
account that supplied it. This keeps folders visible when local playback is
temporarily unavailable. Live session data is still required for edit grants.

Memory caches retain the open page, the playing context, and a limited set of
recently used playlist, album, artist, and show pages. Older pages reload when
revisited, using the saved playlist prefix when its snapshot still matches.
Pending playlist edits and their rows stay in memory until the write and its
snapshot are confirmed, even if this temporarily exceeds the usual page limit.
Track metadata is limited to 800 cached tracks; navigation and periodic
cleanup trim old entries.

Album and playlist scrollbars reserve the full track count as soon as Spotify
reports it. Dragging to an unloaded section shows placeholders and requests
that section directly. Loaded windows stay in memory while the page is retained;
returning to one does not download it again. Unavailable entries keep their row
positions. Playlist edits and refreshes invalidate other cached windows because
their server positions may have changed. Only contiguous playlist prefixes are
saved on disk.

Large playlist pages have no **Go to song** control; drag the scrollbar to
reach a distant song instead. Filtering or sorting returns to the beginning
and loads remaining pages as needed, since local search and ordering require
the track metadata. A failed window stops automatic requests and shows a Retry
button in the reserved row space.

## Windows and the running copy

The session remembers separate positions for the main window and the Winamp
mini player. The shade modes are kept in `settings.json`. A position whose
title bar is no longer on an available monitor's work area is discarded when
reopening the window, keeping its initial on-screen placement instead.

A main window left maximized or full screen reopens that way, and comes back
that way from the mini player. The remembered size and position describe an
ordinary window and are not applied to one that already fills the screen,
because sizing or moving such a window restores it down. A closing main window
keeps its own geometry until the native window closes, so it cannot be resized
to the mini player and overwrite its saved size or maximized state.

**Start with Windows** is not kept in `settings.json`. It is the string
value `LWSpot` under
`HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`, whose
data is `"<path to lwspot.exe>" --autostart` to open the window, or
`--autostart=tray` to start in the tray. Off removes the value. Task Manager
and Settings > Apps > Startup turn an entry off without removing it, through
a binary value of the same name under
`...\CurrentVersion\Explorer\StartupApproved\Run` whose first byte is 3;
LWSpot reads that as Off, and removes the marker when you choose a mode in
its Settings. While the entry is on, each start rewrites the path if
`lwspot.exe` has moved. The uninstaller removes both values when they name
the installed copy. Demo mode never reads or writes them.

The taskbar's jump list is written by each start of LWSpot, and again when
the interface language changes. Windows keeps it with its own taskbar data.

The running copy keeps its single-instance files in an `instance` folder in
the data folder, `%LOCALAPPDATA%\LWSpot\data`, for a portable copy too: one
LWSpot runs per Windows user. `instance.lock` marks the running copy; Windows releases it
when LWSpot quits or crashes. `instance.key` holds the loopback port and a
random token that every request from a second launch or the `lwspot` command
must carry. LWSpot writes them on each start; nothing in them needs keeping.

## settings.json

Settings are stored in one readable JSON file and written atomically. Its
main fields are:

| Field | Default | Meaning |
| --- | --- | --- |
| `device_name` | `LWSpot` | Name on Spotify Connect |
| `bitrate` | `320` | 96, 160, or 320 kbps |
| `normalisation` | `false` | Volume normalisation |
| `autoplay` | `true` | Keep playing similar music at the end |
| `gapless` | `true` | Gapless playback |
| `audio_cache_mb` | `1024` | On-disk audio cache budget |
| `theme` | `system` | Follow the system appearance by default; explicit `dark` and `light` choices remain available |
| `language` | `system` | The interface language. `system` follows Windows' preferred languages and falls back to English; a tag such as `es`, `de-DE`, `pt-BR` or `zh-Hant` selects that language. An unknown tag follows the system |
| `custom_theme` | `null` | Selected JSON filename from the `themes` folder |
| `custom_theme_cache` | absent | Last accepted custom palette; preserves appearance if its file is missing or invalid |
| `accent_from_art` | `true` | Tint pages and the player bar with album art |
| `accent_source` | `theme` | Where the accent colour comes from: `theme`, `windows` (the Windows accent colour) or `album_art` (the playing song's cover) |
| `backdrop` | `off` | Mica or acrylic behind the main window on Windows 11 2022 Update (22H2) or later: `off`, `mica` or `acrylic`. Read when the window opens |
| `player_bar_vis` | `off` | What moves behind the player bar while a song plays on this computer: `off`, `spectrum` or `waveform` |
| `player_bar_vis_fps` | `30` | How many frames a second the player bar's visualizer draws: `20`, `30` or `60`; another number draws 30 |
| `library_sort` | `{}` | Per-section Library order overrides: `library`, `recently_played`, `name`, `recently_added`, `local`, or `spotify`, where supported |
| `sidebar_order` | `[]` | Saved local playlist arrangement, including an unpinned Liked Songs, retained when another sort is selected |
| `pinned_contexts` | `[]` | Local Library pin order; Liked Songs uses a local key that is never sent to Spotify |
| `liked_songs_pinned` | `true` | Keep Liked Songs in the pin block |
| `sidebar_compact` | `false` | Names only in the library sidebar, no covers |
| `sidebar_grid` | `false` | Library entries as responsive cover cards instead of rows |
| `tracklist_compact` | `false` | One-line track rows without covers |
| `winamp_window` | `false` | The window is the Winamp mini player |
| `winamp_show_taskbar` | `true` | Show the Winamp window's taskbar button; the main window always keeps its button |
| `custom_titlebar` | `false` | Draw LWSpot's own title bar and window buttons instead of the Windows frame |
| `skin` | none | File or folder name in the skins folder; blank uses the built-in skin |
| `random_skin` | `false` | Pick a different skin (built-in or installed) each time the mini player opens; `skin` holds the one picked |
| `skin_scale` | by display | Screen pixels per skin pixel, 1 to 4 |
| `winamp_on_top` | `false` | Keep the mini player above other windows |
| `vis` | `bars` | The mini player's visualiser: `bars`, `scope`, or `off` |
| `playlist_open` | `false` | The playlist window is open under the mini player |
| `playlist_height` | `174` | The playlist window's height in skin pixels |
| `eq_open` | `false` | The equalizer window is open under the mini player |
| `eq_on` | `false` | The equalizer shapes local playback |
| `eq_preamp_db` | `0` | The preamp, in decibels, -12 to 12 |
| `eq_bands_db` | ten zeros | The bands from 60 Hz to 16 kHz, in decibels, -12 to 12 |
| `balance` | `0` | Left to right, -1 to 1, for local playback |
| `mono` | `false` | Play both channels the same |
| `playlist_shaded` | `false` | The playlist window is rolled up to its title bar |
| `winamp_shaded` | `false` | The main window is rolled up to its title bar |
| `milkdrop_open` | `false` | The MilkDrop window is open |
| `milkdrop_seconds` | `30` | How long each MilkDrop preset plays |
| `milkdrop_fps` | `60` | MilkDrop frame rate; `0` is uncapped |
| `milkdrop_screen_hz` | `0` | Last reported display refresh rate |
| `milkdrop_fullscreen` | `false` | The MilkDrop window fills the screen |
| `milkdrop_size` | `640, 480` | The MilkDrop window's size in points |
| `keep_playing_in_background` | `true` | Close to tray |
| `check_for_updates` | `true` | Ask GitHub once a day for a newer release |
| `web_client_id` | none | Optional personal Spotify app id used alongside shared coverage |
| `personal_app_intro_seen` | `false` | Whether the Premium personal-app introduction was dismissed or followed |
| `proxy_mode` | `system` | `off`, `system`, `http`, or `socks`. Files without this field stay on `system` |
| `proxy_host` | none | Host of a manual HTTP or SOCKS5 proxy. Ignored when the mode is `off` or `system` |
| `proxy_port` | none | Port of a manual HTTP or SOCKS5 proxy |
| `proxy_username` | none | Optional proxy login for Web requests; authenticated local playback proxying is not supported |

## Command line

```
lwspot [OPTIONS] [LINK]

  LINK                  A Spotify link to open: spotify:track:…, or an
                        open.spotify.com address
  --device-name <NAME>  Spotify Connect name for this session
  -v, --verbose         More logs from librespot and the API client
```

Two more arguments exist for Windows rather than for typing, so `--help`
leaves them out: `--autostart[=tray]`, which the Start with Windows entry
starts LWSpot with, and the `mini-player` verb, which the jump list's
**Mini player** task sends to the running copy.

A link goes to the running LWSpot when there is one, which then opens
the page and brings its window forward; otherwise the app starts on it.
Windows runs exactly this when you open a `spotify:` link. The playback
commands are listed in [Everyday use](../_guide/using-lwspot.md#controlling-it-from-the-command-line).

Attach `lwspot.log` from the data folder to bug reports. It contains
the last run's output, including extra lines from `lwspot -v`. After a
crash, attach `panic.log` too.

## Demo mode

Builds made with `--features demo` accept `--demo`, which loads sample data
for screenshots and interface work. Demo mode never writes settings:

```powershell
cargo run --features demo -- --demo
```

`--demo-page` opens a page, such as `home`, `playlist:pl1`, or `artist:art0`,
and `--demo-show` adds surfaces on top of it: a comma separated list of
`queue`, `playing-next`, `devices`, `shortcuts`, `premium`, `create`, `duplicate`, `light`, `dark`,
`focus`, `winamp`, `playlist`, `eq`, `eq-shade`, `compact`, `update`, `personal-app`,
`collection-loading`, `shuffle-selected`, `shuffle-started`, `library-list`,
`library-list-narrow`, `library-list-wide`, `library-grid`, `library-grid-narrow`,
`library-grid-wide`, `rtl`, `player-bar-spectrum`, `player-bar-waveform`,
`lyrics-fullscreen-view`, `lyrics-fullscreen-instrumental`, `signed-out`, and
`connecting`. The Library variants show the list or cover grid with
a normal, narrow, or wide sidebar and collapsed artwork for matching captures.
`shuffle-selected` and `shuffle-started` capture the selected-mode and
playback-started outcomes of a collection Shuffle click. `update` shows a sample
update badge for checking its layout. `personal-app` shows the personal Spotify
app introduction.
`signed-out` and `connecting` show the sign-in card before and while the
session connects.
`player-bar-spectrum` and `player-bar-waveform` play a fixed, music-like
sound on this computer with that player bar visualizer on.
`lyrics-fullscreen-view` and `lyrics-fullscreen-instrumental` draw full-screen
lyrics, with words or without, at the window's own size.
`rtl` gives the first songs of `playlist:pl1` invented Hebrew and Arabic
titles, some mixed with English, numbers, and brackets.
`scripts` gives them Japanese, Chinese and Korean titles instead.
`collection-loading` keeps known collection metadata and placeholder artwork
visible while replacing the page content, with unfinished controls disabled.
`--demo-language <TAG>` shows the interface in one of the bundled languages,
such as `es` or `ja`, in place of the saved setting and the system's language.

`--demo-shot <PATH>` writes the window to a PNG and exits, which is useful for
making deterministic screenshots for these pages:

```powershell
cargo run --release --features demo -- --demo-shot docs/screenshot.png --demo-size 1440x900 --demo-show dark,queue
```

The screenshots in these pages were made this way: `docs/screenshot.png` as
above, `assets/images/home-light.png` with `light,queue`,
`assets/images/themes.png` with `--demo-page settings --demo-show dark,themes`,
`assets/images/scripts.png` with
`--demo-page playlist:pl1 --demo-show dark,scripts,player-bar-spectrum`, and
the full-screen lyrics with `dark,lyrics-fullscreen,player-bar-spectrum` (or
`light,...`), scaled down to 1240 pixels wide. Keep each image under 1 MB.

The image uses the current window size. `--demo-size WIDTHxHEIGHT` sets that
size in logical pixels for a shot (for example `760x800` or `1240x800`).
`--demo-shot-delay <MS>` sets how long to wait for cover art before taking it.
`--demo-drag X,Y:X,Y` shows a drag in progress: the pointer presses at the
first point, in logical pixels, and is still held down at the second when the
shot is taken.
Demo windows ignore saved window geometry and do not read or save the normal
window's framework state. Built-in appearance settings still apply.
`--demo-data <DIRECTORY>` keeps demo caches and logs under that directory's
`cache` and `state` folders, with settings read from `config`.
Custom palettes and their cache are used only with an explicit `--demo-data`
directory; the ordinary demo does not scan your real themes folder, and the
theme editor and Import do not write to it.
`--demo-show themes` lists the shared palettes in the theme gallery and
opens Settings at Appearance; `theme-editor` and `theme-editor-contrast` open
the theme editor; `accent-windows` and `accent-art` pick an accent source; and
`filter:<text>` types `<text>` into the Settings search (`+` for a space).

## Home shelves

You can hide **Made for you** and **Recommended for you** from Home
independently. Quit LWSpot before editing `settings.json`, then restart it.
Add this field to hide both:

```json
"home": {
  "made_for_you": { "visible": false },
  "recommendations": { "visible": false }
}
```

Set either `visible` value to `true` to show that shelf again. Omitted
preferences keep both shelves visible. Other Home sections keep their normal
order and contents. This changes what is displayed; hidden shelves still
refresh in the background.

## Custom themes

LWSpot puts eight palettes in the `themes` folder beside `settings.json` the
first time it starts: Catppuccin, Catppuccin Latte, Nord, Ristretto, Rose
Pine, Rose Pine Dawn, Rose Pine Moon and Tokyo Night. They are ordinary
palette files: read them to see how a theme is written, change them, or
delete the ones you do not want. LWSpot never rewrites them, and a deleted one
stays deleted; `.installed-palettes` in the folder records which it has
already put there.

**Settings → Appearance → Theme** shows every theme as a small picture of
the app in its own colours: **Follow system** (light and dark side by side),
**Light**, **Dark**, then each palette file by its filename without `.json`.
Click a picture, or move to it with Tab and press Enter or Space, to switch;
the theme in use has an accent ring and a tick. Screen readers hear each
picture as a radio button with the theme's name.

The default is **Follow system**, which uses the light or dark mode set in
Windows. Saved Dark, Light and custom choices are preserved when updating.
Themes change colors and keep the app's existing fonts. **How to make a
theme** opens this section. **Open themes folder** creates the folder if
needed and opens it in File Explorer, using the same button style as the
Winamp skins folder. After adding or editing a JSON file by hand, run
`lwspot reload-themes` to refresh the gallery and the selected palette without
restarting playback. Choosing a built-in theme clears the custom selection.
Whenever the colours change (a theme picked here, or the system going light or
dark), the new colours open from the middle of the window outwards.
The Windows title bar follows the theme too: on Windows 11 it takes the
theme's window and text colours, and on Windows 10 it switches between light
and dark with the theme.

### Theme files

For example, `themes\gruvbox.json`:

```json
{
  "base": "dark",
  "colors": {
    "window": "#282828",
    "panel": "#1d2021",
    "surface": "#32302f",
    "text": "#ebdbb2",
    "accent": "#b8bb26"
  }
}
```

`base` is `dark` (the default) or `light`. Omitted colors inherit that palette.
Supported colors are `window`, `panel`, `surface`, `surface_hover`,
`surface_active`, `outline`, `text`, `secondary`, `dim`, `accent`,
`accent_hover`, `on_accent`, `danger`, `warning`, `overlay`, and `shadow`.
Values must be `#RRGGBB` or `#RRGGBBAA`.

A custom palette's `base` controls both its inherited colors and the light or
dark styling of standard controls. Album-art tinting and the accent colour
source remain independent settings; turn tinting off and keep the theme's
accent for fixed colors throughout. Palettes apply to the main
window; Winamp skins remain separate. This first format controls colors only.

Files are read in the background at launch and when `lwspot reload-themes`
is called. The command updates the selected palette without interrupting
playback, changing your selection or showing the window. It does not start a
stopped app. Repeated requests are combined while a scan is running. Use
regular UTF-8 `.json` files, not symbolic links or subfolders. Each file is
limited to 64 KiB. Keep at most 128 JSON files and 512 total entries in the
themes folder; the saved selection is still checked when a folder exceeds
these limits.

Invalid files are skipped with a warning in the log. LWSpot remembers the
last accepted custom palette in `settings.json`. If the selected file is
removed or becomes invalid, that appearance stays in place, including after
a restart, and the Theme row explains the problem. Other preferences are
preserved. A custom selection without any usable cached colors uses the
built-in choice. Choosing Dark, Light or Follow system clears the custom
selection and its cache. Editing or deleting the optional cache does not
reset unrelated settings.

### Theme editor

**Customize**, under the gallery, opens the theme editor on the theme in use.
The editor lists the sixteen colours a palette file sets; choose one to change
it with the colour picker, or type its `#RRGGBB` value. **Base** chooses
whether standard controls are drawn light or dark. The whole app is drawn in
the colours being chosen while the editor is open, and **Cancel** (or Esc)
brings back the theme from before. The editor warns when text has less than
4.5:1 contrast with the window or panels, or text on the accent less than
3:1 (the WCAG ratios for text and for large text).

**Save** writes the theme into the `themes` folder as `<name>.json` and
selects it. Characters Windows does not allow in a file name are left out, and
a name Windows reserves (such as `CON`) is refused. When a theme of that name
already exists, the editor asks before replacing it. A saved theme is an
ordinary palette file naming its base and all sixteen colours.

**Import theme** copies a palette file chosen in a file dialog into the
`themes` folder and selects it. The file is checked first and refused, with
the reason, if it is not a valid palette. An import never replaces a theme:
a second `Sunset.json` comes in as `Sunset 2.json`. **Export theme** saves
the theme in use as a palette file wherever you choose, to share it.

### Accent colour

**Settings → Appearance → Accent colour** chooses where buttons, switches,
the progress bar and other highlights take their colour from, one source at a
time:

- **Theme**: the theme's own accent (the default). Light and Dark use violet
  (`#5A47E0` and `#6D5BF0`).
- **Windows**: the accent colour set in Windows Settings → Personalization →
  Colors. A change there reaches LWSpot at once.
- **Album art**: the colour of the playing song's cover. The accent blends to
  the next song's colour over about half a second, and the last colour stays
  until the next one is known, so it never flashes back in between. Without a
  cover, or with a nearly grey one, the theme's accent stays.

A colour from Windows or a cover keeps its hue, with its lightness adjusted
when needed so it stands out from the window and panels by at least 3:1 and
the text on it reads at 4.5:1 or better. **Colour from album art** is a
separate switch: it tints page headers and the player bar with the cover.

### Window backdrop

On Windows 11 2022 Update (22H2) or later, **Settings → Appearance → Window
backdrop** can put Windows' **Mica** (the wallpaper, blurred and tinted) or
**Acrylic** (what is behind the window, frosted) behind LWSpot. The window and
panels let it through; text, controls, menus and dialogs stay opaque, and the
title bar takes the backdrop's own colour. The window is made see-through
when it opens, so a change takes effect after restarting LWSpot, as the row
says. With transparency effects turned off in Windows, the window stays
opaque. Older Windows versions, and the custom title bar, keep the row off.
