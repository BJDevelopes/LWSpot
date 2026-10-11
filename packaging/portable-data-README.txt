LWSpot portable data

While this folder sits beside lwspot.exe and lwspot-portable.txt, this copy
of LWSpot keeps its settings, play history, caches, logs and downloaded
add-ons here (in config, state and cache) instead of in your Windows
profile. Move or copy the whole LWSpot folder to take them along. Updates
replace only lwspot.exe and leave this folder as it is.

Your Spotify sign-in belongs to your Windows user: it is kept in Windows
Credential Manager, or here encrypted so that only that user can read it.
On another PC or Windows account, sign in again.

Without this folder, the copy uses %APPDATA%\LWSpot and %LOCALAPPDATA%\LWSpot
like an installed one. If LWSpot cannot write here (for example in Program
Files or on read-only media), it uses those folders too and says so in its
log.
