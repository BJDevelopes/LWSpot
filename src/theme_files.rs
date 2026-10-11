//! Saving, importing and exporting palette files for the theme editor.
//!
//! Every file operation runs on its own thread and reports back through
//! [`Jobs`], so the interface never waits on the disk or a file dialog.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::theme::{Palette, theme_filename};

/// The theme being edited, held by the editor dialog. The app draws with
/// its palette while the dialog is open, and the theme before it comes back
/// on Cancel because nothing else was changed.
#[derive(Clone, Debug)]
pub struct Draft {
    pub palette: Palette,
    /// What the theme is to be called; its file name comes from this.
    pub name: String,
    /// The file a save found already there, waiting for Replace.
    pub replace: Option<String>,
    /// Why the last save failed.
    pub error: Option<String>,
    /// A save is on its way to the disk.
    pub saving: bool,
    /// The colour the picker is showing, an index into
    /// [`fastframe_theme::BASE_COLORS`].
    pub selected: usize,
}

impl Draft {
    pub fn new(palette: Palette, name: String) -> Self {
        Self {
            palette,
            name,
            replace: None,
            error: None,
            saving: false,
            selected: fastframe_theme::BASE_COLORS
                .iter()
                .position(|name| *name == "accent")
                .unwrap_or(0),
        }
    }
}

/// What a finished file operation found.
#[derive(Debug, PartialEq)]
pub enum Done {
    /// The palette was written to the themes folder as `filename`.
    Saved { filename: String, palette: Palette },
    /// A file of that name exists; nothing was written.
    Exists { filename: String },
    /// A palette file was copied into the themes folder as `filename`.
    Imported { filename: String, palette: Palette },
    /// The palette was written where the person chose.
    Exported { path: PathBuf },
    /// The person closed the file dialog.
    Cancelled,
    /// Saving failed.
    Failed(String),
    /// The chosen file is not a palette, or could not be copied.
    ImportFailed(String),
    /// The file could not be written.
    ExportFailed(String),
}

/// File operations in flight.
pub struct Jobs {
    sender: Sender<Done>,
    receiver: Receiver<Done>,
}

impl Default for Jobs {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self { sender, receiver }
    }
}

impl Jobs {
    /// Runs `job` on its own thread; `wake` asks for the frame that reads
    /// its result.
    pub fn run(
        &self,
        wake: impl Fn() + Send + 'static,
        job: impl FnOnce() -> Done + Send + 'static,
    ) {
        let sender = self.sender.clone();
        let spawned = std::thread::Builder::new()
            .name("lwspot-theme-file".into())
            .spawn(move || {
                let _ = sender.send(job());
                wake();
            });
        if let Err(error) = spawned {
            let _ = self.sender.send(Done::Failed(error.to_string()));
        }
    }

    /// The operations that finished since the last call.
    pub fn finished(&self) -> Vec<Done> {
        self.receiver.try_iter().collect()
    }
}

/// Writes `contents` to `directory/filename` through a temporary file, so a
/// palette is never half written.
fn write_atomically(directory: &Path, filename: &str, contents: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(".{filename}.tmp"));
    let written = std::fs::write(&temporary, contents)
        .and_then(|()| std::fs::rename(&temporary, directory.join(filename)));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// Whether anything, even a broken link, already has this name.
fn taken(directory: &Path, filename: &str) -> bool {
    std::fs::symlink_metadata(directory.join(filename)).is_ok()
}

/// Saves `palette` as `filename` in the themes folder. An existing file is
/// replaced only with `replace`.
pub fn save(directory: &Path, filename: &str, palette: &Palette, replace: bool) -> Done {
    if !replace && taken(directory, filename) {
        return Done::Exists {
            filename: filename.to_owned(),
        };
    }
    match write_atomically(directory, filename, &palette.to_json()) {
        Ok(()) => Done::Saved {
            filename: filename.to_owned(),
            palette: *palette,
        },
        Err(error) => Done::Failed(error.to_string()),
    }
}

/// Reads a palette file from anywhere, as the themes folder would.
fn read_palette(path: &Path) -> Result<Palette, String> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take(fastframe_theme::MAX_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > fastframe_theme::MAX_FILE_BYTES {
        return Err("theme exceeds the 64 KiB file limit".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "expected UTF-8 JSON".to_owned())?;
    fastframe_theme::parse_palette(text)
}

/// Copies the palette file at `source` into the themes folder, under its
/// own name or, when that is taken, the first free "Name 2", "Name 3".
/// A file that is not a valid palette is refused and nothing is written.
pub fn import(directory: &Path, source: &Path) -> Done {
    let palette = match read_palette(source) {
        Ok(palette) => palette,
        Err(error) => return Done::ImportFailed(error),
    };
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(first) = theme_filename(&stem).or_else(|| theme_filename("Imported theme")) else {
        return Done::ImportFailed("no usable file name".into());
    };
    let base = first.trim_end_matches(".json").to_owned();
    let filename = (1..1000)
        .map(|number| match number {
            1 => first.clone(),
            _ => format!("{base} {number}.json"),
        })
        .find(|filename| !taken(directory, filename));
    let Some(filename) = filename else {
        return Done::ImportFailed("no free file name".into());
    };
    // The file as written by the editor, so it reads the same as any other.
    match write_atomically(directory, &filename, &palette.to_json()) {
        Ok(()) => Done::Imported { filename, palette },
        Err(error) => Done::ImportFailed(error.to_string()),
    }
}

/// Writes `palette` to `path`, chosen in a save dialog.
pub fn export(path: &Path, palette: &Palette) -> Done {
    match std::fs::write(path, palette.to_json()) {
        Ok(()) => Done::Exported {
            path: path.to_owned(),
        },
        Err(error) => Done::ExportFailed(error.to_string()),
    }
}

/// Asks for a palette file to import, then imports it.
pub fn pick_and_import(directory: PathBuf, title: String, filter: String) -> Done {
    match rfd::FileDialog::new()
        .set_title(title)
        .add_filter(filter, &["json"])
        .pick_file()
    {
        Some(source) => import(&directory, &source),
        None => Done::Cancelled,
    }
}

/// Asks where to save `palette`, suggesting `filename`, then writes it.
pub fn pick_and_export(palette: Palette, filename: String, title: String, filter: String) -> Done {
    match rfd::FileDialog::new()
        .set_title(title)
        .set_file_name(filename)
        .add_filter(filter, &["json"])
        .save_file()
    {
        Some(path) => export(&path, &palette),
        None => Done::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("lwspot-theme-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    fn odd_palette() -> Palette {
        let mut palette = Palette::light();
        palette.accent = egui::Color32::from_rgb(0x8c, 0x3f, 0xa5);
        palette.window = egui::Color32::from_rgb(1, 2, 3);
        palette.shadow = egui::Color32::from_rgba_unmultiplied(10, 20, 30, 128);
        palette
    }

    /// A saved palette reads back as the same palette through the parser
    /// the themes folder uses, and the file names every colour.
    #[test]
    fn a_saved_theme_reads_back_through_the_catalog_parser() {
        let palette = odd_palette();
        let text = palette.to_json();
        let read: Palette = fastframe_theme::parse_palette(&text).unwrap();
        assert_eq!(read, palette);
        for name in fastframe_theme::BASE_COLORS {
            assert!(text.contains(&format!("\"{name}\": \"#")), "{name}");
        }
        assert!(text.contains("\"base\": \"light\""));
        assert!(text.contains("\"shadow\": \"#0a141e80\""));
        let dark: Palette = fastframe_theme::parse_palette(&Palette::dark().to_json()).unwrap();
        assert_eq!(dark, Palette::dark());
        // Every shared palette survives the trip.
        for theme in fastframe_theme::presets::themes::<Palette>() {
            let back: Palette = fastframe_theme::parse_palette(&theme.palette.to_json()).unwrap();
            assert_eq!(back, theme.palette, "{}", theme.filename);
        }
    }

    #[test]
    fn saving_never_replaces_a_theme_without_asking() {
        let directory = folder("save");
        let palette = odd_palette();
        assert_eq!(
            save(&directory, "Mine.json", &palette, false),
            Done::Saved {
                filename: "Mine.json".into(),
                palette
            }
        );
        let other = Palette::dark();
        assert_eq!(
            save(&directory, "Mine.json", &other, false),
            Done::Exists {
                filename: "Mine.json".into()
            }
        );
        let kept: Palette = fastframe_theme::parse_palette(
            &std::fs::read_to_string(directory.join("Mine.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(kept, palette, "untouched until replace is confirmed");
        assert!(matches!(
            save(&directory, "Mine.json", &other, true),
            Done::Saved { .. }
        ));
        let replaced: Palette = fastframe_theme::parse_palette(
            &std::fs::read_to_string(directory.join("Mine.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(replaced, other);
        let leftovers = std::fs::read_dir(&directory).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file left behind");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn imports_are_validated_and_never_overwrite() {
        let directory = folder("import");
        let outside = folder("import-source");
        std::fs::create_dir_all(&outside).unwrap();
        let source = outside.join("Sunset.json");
        std::fs::write(
            &source,
            r##"{"base":"light","colors":{"accent":"#ff8800"}}"##,
        )
        .unwrap();
        let Done::Imported { filename, palette } = import(&directory, &source) else {
            panic!("imported");
        };
        assert_eq!(filename, "Sunset.json");
        assert_eq!(palette.accent, egui::Color32::from_rgb(0xff, 0x88, 0x00));
        let Done::Imported { filename, .. } = import(&directory, &source) else {
            panic!("imported again");
        };
        assert_eq!(filename, "Sunset 2.json", "the first copy is kept");
        let broken = outside.join("Broken.json");
        std::fs::write(&broken, r##"{"colors":{"typo":"#ffffff"}}"##).unwrap();
        assert_eq!(
            import(&directory, &broken),
            Done::ImportFailed("unknown color: typo".into())
        );
        assert!(!directory.join("Broken.json").exists());
        let _ = std::fs::remove_dir_all(&directory);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn an_export_is_a_palette_file() {
        let outside = folder("export");
        std::fs::create_dir_all(&outside).unwrap();
        let path = outside.join("Shared.json");
        assert_eq!(
            export(&path, &odd_palette()),
            Done::Exported { path: path.clone() }
        );
        let read: Palette =
            fastframe_theme::parse_palette(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, odd_palette());
        let _ = std::fs::remove_dir_all(&outside);
    }
}
