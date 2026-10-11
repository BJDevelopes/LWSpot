//! Where LWSpot keeps its files.
//!
//! Configuration, durable non-secret state, and disposable caches live in the
//! conventional Windows directories: configuration under `%APPDATA%\LWSpot`,
//! state and caches under `%LOCALAPPDATA%\LWSpot`. Spotify grants use the
//! Windows credential store; the token paths below are retained only for
//! migration and sign-out cleanup.
//!
//! A portable copy keeps them beside itself instead, in `data\config`,
//! `data\state` and `data\cache` next to `lwspot.exe`, when the portable
//! marker and a writable `data` folder are both there (see [`placement`]).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::ProjectDirs;

/// The marker the portable ZIP ships beside `lwspot.exe`. The updater reads
/// the same file to update a portable copy in place.
pub const PORTABLE_MARKER: &str = "lwspot-portable.txt";
const PORTABLE_MARKER_TEXT: &str = "lwspot-portable-v1";
/// The installer's marker. An installed copy is never portable, whatever
/// else sits in its folder; the updater gives it the same precedence.
const INSTALLER_MARKER: &str = "lwspot-installer.txt";
const INSTALLER_MARKER_TEXT: &str = "lwspot-installer-v1";
/// The folder beside a portable `lwspot.exe` that holds its data. The
/// portable ZIP ships it empty; deleting it sends that copy's data back to
/// the per-user folders.
pub const PORTABLE_DATA: &str = "data";

/// What the folder of `lwspot.exe` holds, as far as the data location is
/// concerned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortableFacts {
    /// `lwspot-portable.txt` reads `lwspot-portable-v1`.
    pub portable_marker: bool,
    /// `lwspot-installer.txt` reads `lwspot-installer-v1`.
    pub installer_marker: bool,
    /// `data` is a folder.
    pub data_folder: bool,
    /// A file could be created in `data`.
    pub data_writable: bool,
}

impl PortableFacts {
    /// Looks at `exe_dir`. Writing is the only reliable test of a folder
    /// (permissions, read-only media, policies), so it is tried only when
    /// its answer matters.
    pub fn read(exe_dir: &Path) -> Self {
        let marker = |name: &str, text: &str| {
            std::fs::read_to_string(exe_dir.join(name)).is_ok_and(|value| value.trim() == text)
        };
        let data = exe_dir.join(PORTABLE_DATA);
        let portable_marker = marker(PORTABLE_MARKER, PORTABLE_MARKER_TEXT);
        let installer_marker = marker(INSTALLER_MARKER, INSTALLER_MARKER_TEXT);
        let data_folder = data.is_dir();
        Self {
            portable_marker,
            installer_marker,
            data_folder,
            data_writable: portable_marker && !installer_marker && data_folder && writable(&data),
        }
    }
}

/// Where this copy keeps its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// The per-user folders: an installed copy, a copy built from source,
    /// or a portable copy without a `data` folder.
    Profile,
    /// `data` beside `lwspot.exe`.
    Portable,
    /// A portable copy whose `data` folder cannot be written, such as one
    /// unpacked into Program Files or onto read-only media. It uses the
    /// per-user folders, and says so in the log.
    PortableReadOnly,
}

/// The rule: the portable marker, no installer marker, and a `data` folder
/// make a portable copy, which keeps its data there when it can write to it.
pub fn placement(facts: PortableFacts) -> Placement {
    if !facts.portable_marker || facts.installer_marker || !facts.data_folder {
        Placement::Profile
    } else if facts.data_writable {
        Placement::Portable
    } else {
        Placement::PortableReadOnly
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".lwspot-write-test-{}", std::process::id()));
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
        .is_ok();
    if created {
        let _ = std::fs::remove_file(&probe);
    }
    created
}

struct Resolved {
    placement: Placement,
    /// `data` beside the executable, when the executable's folder is known.
    data: Option<PathBuf>,
}

/// Decided once per process, so every part of the app agrees.
fn resolved() -> &'static Resolved {
    static RESOLVED: OnceLock<Resolved> = OnceLock::new();
    RESOLVED.get_or_init(|| {
        let dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        match dir {
            Some(dir) => Resolved {
                placement: placement(PortableFacts::read(&dir)),
                data: Some(dir.join(PORTABLE_DATA)),
            },
            None => Resolved {
                placement: Placement::Profile,
                data: None,
            },
        }
    })
}

/// The `data` folder of a portable copy that keeps its data there.
pub fn portable_data() -> Option<&'static Path> {
    let resolved = resolved();
    if resolved.placement == Placement::Portable {
        resolved.data.as_deref()
    } else {
        None
    }
}

/// Records where the data went, once logging is up: the location is
/// decided before the log file can be opened.
pub fn log_placement() {
    let resolved = resolved();
    let data = resolved
        .data
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    match resolved.placement {
        Placement::Profile => {}
        Placement::Portable => log::info!("portable copy: keeping settings and data in {data}"),
        Placement::PortableReadOnly => log::warn!(
            "portable copy, but {data} cannot be written; keeping settings and data in the user's profile instead"
        ),
    }
}

/// eframe's own file (the main window's geometry and egui's memory):
/// beside the rest of a portable copy's state, or where eframe keeps it.
pub fn window_state_file() -> Option<PathBuf> {
    match portable_data() {
        Some(data) => Some(AppDirs::portable(data).state.join("app.ron")),
        None => eframe::storage_dir("lwspot").map(|dir| dir.join("app.ron")),
    }
}

#[derive(Clone, Debug)]
pub struct AppDirs {
    pub config: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
}

impl AppDirs {
    /// This copy's folders: a portable copy's own, or the per-user ones.
    pub fn discover() -> Self {
        match portable_data() {
            Some(data) => Self::portable(data),
            None => Self::profile(),
        }
    }

    /// The per-user folders, whichever folders this copy uses.
    pub fn profile() -> Self {
        Self::for_name("LWSpot")
    }

    /// The layout inside a portable copy's `data` folder.
    pub fn portable(data: &Path) -> Self {
        Self {
            config: data.join("config"),
            state: data.join("state"),
            cache: data.join("cache"),
        }
    }

    /// The single-instance slot stays in the user's profile even for a
    /// portable copy: one LWSpot runs per Windows user, as before, so a
    /// portable and an installed copy never play side by side.
    pub fn instance_dir() -> PathBuf {
        Self::profile().state.join("instance")
    }

    fn for_name(name: &str) -> Self {
        let project = ProjectDirs::from("", "", name);
        match project {
            Some(project) => Self {
                config: project.config_dir().to_path_buf(),
                state: project
                    .state_dir()
                    .map(|path| path.to_path_buf())
                    .unwrap_or_else(|| project.data_local_dir().to_path_buf()),
                cache: project.cache_dir().to_path_buf(),
            },
            None => {
                let fallback = std::env::current_dir().unwrap_or_default();
                Self {
                    config: fallback.join(format!("{name}-config")),
                    state: fallback.join(format!("{name}-state")),
                    cache: fallback.join(format!("{name}-cache")),
                }
            }
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// Winamp skins the listener has added, as `.wsz` files or folders.
    pub fn skins_dir(&self) -> PathBuf {
        self.config.join("skins")
    }

    /// MilkDrop presets, as `.milk` files, in folders or not, with any
    /// textures they use in a `textures` folder inside.
    pub fn milkdrop_dir(&self) -> PathBuf {
        self.config.join("milkdrop")
    }

    /// Optional parts LWSpot downloads when they are first used, such as
    /// the MilkDrop add-on. Nothing is here until then.
    pub fn addons_dir(&self) -> PathBuf {
        self.state.join("addons")
    }

    pub fn session_file(&self) -> PathBuf {
        self.state.join("session.json")
    }

    /// What was played here, which Spotify never hears about and so
    /// cannot tell us later. See [`crate::history`].
    pub fn history_file(&self) -> PathBuf {
        self.state.join("history.json")
    }

    pub fn shared_web_token_file(&self) -> PathBuf {
        self.state.join("shared_web_api_token.json")
    }

    pub fn personal_web_token_file(&self) -> PathBuf {
        self.state.join("personal_web_api_token.json")
    }

    pub fn legacy_web_token_file(&self) -> PathBuf {
        self.state.join("web_api_token.json")
    }

    /// The log of the current run, replaced at every start.
    pub fn log_file(&self) -> PathBuf {
        self.state.join("lwspot.log")
    }

    /// Where a panic is recorded before the process dies of it.
    pub fn panic_log(&self) -> PathBuf {
        self.state.join("panic.log")
    }

    pub fn credentials_dir(&self) -> PathBuf {
        self.state.join("credentials")
    }

    /// Optional proxy password, owner-only, never written to settings.json.
    pub fn proxy_secret_file(&self) -> PathBuf {
        self.state.join("proxy_password")
    }

    pub fn volume_dir(&self) -> PathBuf {
        self.state.join("volume")
    }

    pub fn audio_cache_dir(&self) -> PathBuf {
        self.cache.join("audio")
    }

    pub fn art_cache_dir(&self) -> PathBuf {
        self.cache.join("art")
    }

    pub fn lyrics_cache_dir(&self) -> PathBuf {
        self.cache.join("lyrics")
    }

    pub fn playlist_cache_dir(&self) -> PathBuf {
        self.cache.join("playlists")
    }

    pub fn account_playlist_cache_dir(&self, account_id: &str) -> PathBuf {
        self.playlist_cache_dir().join(account_id)
    }

    pub fn liked_songs_cache_file(&self, account_id: &str) -> PathBuf {
        // Hex encoding also keeps unusual account IDs within the cache root.
        let account: String = account_id
            .bytes()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.cache
            .join("liked-songs")
            .join(format!("{account}.json"))
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [&self.config, &self.state, &self.cache] {
            std::fs::create_dir_all(dir)?;
        }
        if portable_data().is_some_and(|data| Self::portable(data).state == self.state) {
            pin_credential_profile(&self.state);
        }
        Ok(())
    }
}

/// Credential Manager entries are named after a hash of the state folder's
/// path unless `credential-profile` there names them (see
/// [`crate::credentials`]). A portable copy records the name it uses now, so
/// it stays signed in after its drive letter or folder changes. The entries
/// themselves stay with the Windows user: on another PC or account the copy
/// signs in again. Best effort: without the file the hash is used, as before.
fn pin_credential_profile(state: &Path) {
    use sha2::Digest;
    let file = state.join("credential-profile");
    if file.exists() {
        return;
    }
    let profile = format!(
        "{:x}",
        sha2::Sha256::digest(state.to_string_lossy().as_bytes())
    );
    let _ = std::fs::write(file, profile);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORTABLE: PortableFacts = PortableFacts {
        portable_marker: true,
        installer_marker: false,
        data_folder: true,
        data_writable: true,
    };

    #[test]
    fn a_portable_copy_with_a_writable_data_folder_keeps_its_data_there() {
        assert_eq!(placement(PORTABLE), Placement::Portable);
    }

    #[test]
    fn anything_less_than_marker_and_data_folder_uses_the_profile() {
        assert_eq!(placement(PortableFacts::default()), Placement::Profile);
        for facts in [
            // A data folder beside a build from source is not a portable copy.
            PortableFacts {
                portable_marker: false,
                ..PORTABLE
            },
            // A portable copy without a data folder: the per-user folders,
            // as every portable copy did before.
            PortableFacts {
                data_folder: false,
                data_writable: false,
                ..PORTABLE
            },
        ] {
            assert_eq!(placement(facts), Placement::Profile, "{facts:?}");
        }
    }

    #[test]
    fn the_installer_marker_wins_over_the_portable_one() {
        let installed = PortableFacts {
            installer_marker: true,
            ..PORTABLE
        };
        assert_eq!(placement(installed), Placement::Profile);
    }

    #[test]
    fn an_unwritable_data_folder_falls_back_to_the_profile() {
        let read_only = PortableFacts {
            data_writable: false,
            ..PORTABLE
        };
        assert_eq!(placement(read_only), Placement::PortableReadOnly);
    }

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("lwspot-portable-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_folder_is_read_for_markers_and_a_writable_data_folder() {
        let folder = Folder::new("read");
        let dir = &folder.0;
        assert_eq!(PortableFacts::read(dir), PortableFacts::default());

        std::fs::write(dir.join(PORTABLE_MARKER), "lwspot-portable-v2\n").unwrap();
        assert!(!PortableFacts::read(dir).portable_marker);
        std::fs::write(dir.join(PORTABLE_MARKER), "lwspot-portable-v1\r\n").unwrap();
        assert_eq!(
            PortableFacts::read(dir),
            PortableFacts {
                portable_marker: true,
                ..PortableFacts::default()
            }
        );

        // A file called data is not the folder.
        std::fs::write(dir.join(PORTABLE_DATA), "").unwrap();
        assert_eq!(placement(PortableFacts::read(dir)), Placement::Profile);
        std::fs::remove_file(dir.join(PORTABLE_DATA)).unwrap();

        std::fs::create_dir(dir.join(PORTABLE_DATA)).unwrap();
        assert_eq!(PortableFacts::read(dir), PORTABLE);
        // The write test leaves nothing behind.
        assert_eq!(
            std::fs::read_dir(dir.join(PORTABLE_DATA)).unwrap().count(),
            0
        );

        std::fs::write(dir.join(INSTALLER_MARKER), "lwspot-installer-v1").unwrap();
        let installed = PortableFacts::read(dir);
        assert!(installed.installer_marker);
        assert!(!installed.data_writable);
        assert_eq!(placement(installed), Placement::Profile);
    }

    #[test]
    fn a_missing_folder_is_not_writable() {
        let folder = Folder::new("missing");
        assert!(!writable(&folder.0.join("absent")));
        assert!(writable(&folder.0));
    }

    #[test]
    fn the_portable_layout_lives_inside_the_data_folder() {
        let data = Path::new(r"E:\LWSpot\data");
        let dirs = AppDirs::portable(data);
        assert_eq!(dirs.config, data.join("config"));
        assert_eq!(dirs.state, data.join("state"));
        assert_eq!(dirs.cache, data.join("cache"));
        assert_eq!(
            dirs.settings_file(),
            Path::new(r"E:\LWSpot\data\config\settings.json")
        );
        assert_eq!(dirs.addons_dir(), Path::new(r"E:\LWSpot\data\state\addons"));
        assert_eq!(
            dirs.audio_cache_dir(),
            Path::new(r"E:\LWSpot\data\cache\audio")
        );
        assert_eq!(
            dirs.log_file(),
            Path::new(r"E:\LWSpot\data\state\lwspot.log")
        );
    }

    #[test]
    fn a_build_from_source_uses_the_profile() {
        // Tests run from target\debug\deps, which has no marker.
        assert_eq!(resolved().placement, Placement::Profile);
        assert_eq!(portable_data(), None);
        let profile = AppDirs::profile();
        assert_eq!(AppDirs::discover().state, profile.state);
        assert_eq!(AppDirs::instance_dir(), profile.state.join("instance"));
        assert_eq!(
            window_state_file(),
            eframe::storage_dir("lwspot").map(|dir| dir.join("app.ron"))
        );
    }

    #[test]
    fn a_portable_copy_pins_its_credential_profile_once() {
        let folder = Folder::new("pin");
        pin_credential_profile(&folder.0);
        let file = folder.0.join("credential-profile");
        // What crate::credentials derives from this folder anyway.
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            format!(
                "{:x}",
                <sha2::Sha256 as sha2::Digest>::digest(folder.0.to_string_lossy().as_bytes())
            )
        );
        std::fs::write(&file, "a".repeat(64)).unwrap();
        pin_credential_profile(&folder.0);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a".repeat(64));
    }
}
