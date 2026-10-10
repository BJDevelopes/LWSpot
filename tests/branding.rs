use std::path::{Path, PathBuf};
use std::process::Command;

const COMMAND: &str = env!("CARGO_BIN_EXE_lwspot");

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("lwspot-branding-{:016x}", rand::random::<u64>()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_command_reports_its_name_and_passes_the_update_version_check() {
    let output = Command::new(COMMAND).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("lwspot {}", env!("CARGO_PKG_VERSION"))
    );
    let help = Command::new(COMMAND).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("Usage: lwspot")
    );
    assert_eq!(lwspot::updates::CONFIG.slug, "lwspot");
}

/// An earlier name of the upstream project must not creep back in.
/// Spelled in two halves so this file does not match itself.
#[test]
fn no_file_carries_the_old_name() {
    let old = ["fast", "potify"].concat();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // Build output, the docs site's generated files, and every hidden folder
    // (.git, editor and tool state) are not part of the project.
    let skipped = ["target", "_site", "vendor", "dist"];
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name.starts_with('.') || skipped.contains(&name.as_str()) {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).to_lowercase();
            if relative.to_lowercase().contains(&old) || text.contains(&old) {
                found.push(relative);
            }
        }
    }
    found.sort();
    assert!(found.is_empty(), "the old name is still in {found:?}");
}

#[test]
fn existing_preferences_and_custom_connect_names_survive_a_save() {
    use lwspot::settings::{Settings, ThemeChoice};

    let scratch = Scratch::new();
    let path = scratch.0.join("settings.json");
    for name in ["LWSpot", "Living room", "Carmine's laptop"] {
        let saved = Settings {
            device_name: name.into(),
            theme: ThemeChoice::Light,
            volume: 37,
            pinned_contexts: vec![
                "spotify:playlist:123".into(),
                lwspot::settings::LIKED_SONGS_KEY.into(),
            ],
            ..Settings::default()
        };
        saved.save(&path);
        assert_eq!(Settings::load(&path), saved);
    }
    assert_eq!(Settings::default().device_name, "LWSpot");
}
