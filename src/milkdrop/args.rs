//! The command line between the app and the MilkDrop add-on.
//!
//! The app starts `lwspot-milkdrop.exe` with these arguments (see
//! [`super::host`]) and the add-on reads them back (see `super::child`). Both
//! sides live here so that what one writes is what the other reads, and the
//! protocol number keeps an add-on from another version from misreading them.

use std::ffi::OsString;
use std::path::PathBuf;

use super::{DEFAULT_FPS, DEFAULT_SECONDS, DEFAULT_SIZE, PROTOCOL};

/// The flag that marks a launch as the MilkDrop window.
pub const CHILD_FLAG: &str = "--milkdrop-child";

/// How the window is started.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    /// The shared-memory audio ring the app writes.
    pub shm: PathBuf,
    /// The folder of `.milk` presets.
    pub presets: PathBuf,
    pub size: [f32; 2],
    pub pos: Option<[f32; 2]>,
    pub fullscreen: bool,
    pub fps: u32,
    pub seconds: u32,
    pub scale: u32,
}

impl Args {
    /// Reads the window's arguments from this process's command line.
    /// `None` when this is not a MilkDrop launch; an error when it is one
    /// from an app that speaks another protocol.
    pub fn parse() -> Option<Result<Self, String>> {
        Self::parse_from(std::env::args_os().skip(1))
    }

    /// [`Args::parse`] for a given command line, without the program name.
    pub fn parse_from(args: impl IntoIterator<Item = OsString>) -> Option<Result<Self, String>> {
        let args: Vec<OsString> = args.into_iter().collect();
        if !args.iter().any(|arg| arg == CHILD_FLAG) {
            return None;
        }
        let mut all = args.into_iter();
        let mut protocol = None;
        let mut shm = None;
        let mut presets = None;
        let mut size = DEFAULT_SIZE;
        let mut pos = None;
        let mut fullscreen = false;
        let mut fps = DEFAULT_FPS;
        let mut seconds = DEFAULT_SECONDS;
        let mut scale = 1u32;
        let number = |value: Option<OsString>| value.and_then(|v| v.to_str()?.parse().ok());
        let pair = |value: Option<OsString>| value.and_then(|v| parse_pair(v.to_str()?));
        while let Some(arg) = all.next() {
            match arg.to_str().unwrap_or_default() {
                "--milkdrop-protocol" => protocol = number(all.next()),
                "--milkdrop-shm" => shm = all.next().map(PathBuf::from),
                "--milkdrop-presets" => presets = all.next().map(PathBuf::from),
                "--milkdrop-size" => size = pair(all.next()).unwrap_or(size),
                "--milkdrop-pos" => pos = pair(all.next()),
                "--milkdrop-fullscreen" => fullscreen = true,
                "--milkdrop-fps" => fps = number(all.next()).unwrap_or(fps),
                "--milkdrop-seconds" => seconds = number(all.next()).unwrap_or(seconds),
                "--milkdrop-scale" => scale = number(all.next()).unwrap_or(scale),
                _ => {}
            }
        }
        if let Err(error) = check_protocol(protocol) {
            return Some(Err(error));
        }
        let (Some(shm), Some(presets)) = (shm, presets) else {
            return Some(Err(
                "LWSpot did not say where the sound and the presets are".into(),
            ));
        };
        Some(Ok(Self {
            shm,
            presets,
            size,
            pos,
            fullscreen,
            fps,
            seconds,
            scale,
        }))
    }

    /// The command line that starts the window with these arguments.
    pub fn command_line(&self) -> Vec<OsString> {
        let mut line: Vec<OsString> = vec![
            CHILD_FLAG.into(),
            "--milkdrop-protocol".into(),
            PROTOCOL.to_string().into(),
            "--milkdrop-shm".into(),
            self.shm.clone().into(),
            "--milkdrop-presets".into(),
            self.presets.clone().into(),
            "--milkdrop-size".into(),
            format!("{}x{}", self.size[0], self.size[1]).into(),
            "--milkdrop-fps".into(),
            self.fps.to_string().into(),
            "--milkdrop-seconds".into(),
            self.seconds.to_string().into(),
            "--milkdrop-scale".into(),
            self.scale.to_string().into(),
        ];
        if let Some(pos) = self.pos {
            line.push("--milkdrop-pos".into());
            line.push(format!("{},{}", pos[0], pos[1]).into());
        }
        if self.fullscreen {
            line.push("--milkdrop-fullscreen".into());
        }
        line
    }
}

/// Whether the app that started the window speaks this add-on's protocol.
/// An app from before the add-on sends none.
pub fn check_protocol(given: Option<u32>) -> Result<(), String> {
    match given {
        Some(PROTOCOL) => Ok(()),
        Some(other) => Err(format!(
            "This MilkDrop add-on speaks protocol {PROTOCOL}, but LWSpot started it with \
             protocol {other}. Turn MilkDrop on again in LWSpot to get the add-on for its \
             version."
        )),
        None => Err(format!(
            "This MilkDrop add-on speaks protocol {PROTOCOL}, but LWSpot started it without \
             one. Turn MilkDrop on again in LWSpot to get the add-on for its version."
        )),
    }
}

/// Two numbers separated by a comma or an `x`: a position or a size.
fn parse_pair(value: &str) -> Option<[f32; 2]> {
    let (a, b) = value.split_once([',', 'x'])?;
    Some([a.trim().parse().ok()?, b.trim().parse().ok()?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn sample() -> Args {
        Args {
            shm: PathBuf::from(r"C:\Temp\lwspot-milkdrop-42.pcm"),
            presets: PathBuf::from(r"C:\Users\me\AppData\Roaming\LWSpot\config\milkdrop"),
            size: [800.0, 600.0],
            pos: Some([-1920.0, 40.5]),
            fullscreen: true,
            fps: 144,
            seconds: 20,
            scale: 2,
        }
    }

    /// What the app writes is what the window reads, every field of it.
    #[test]
    fn the_window_reads_back_what_the_app_wrote() {
        let args = sample();
        assert_eq!(Args::parse_from(args.command_line()), Some(Ok(args)));
        let plain = Args {
            pos: None,
            fullscreen: false,
            ..sample()
        };
        assert_eq!(Args::parse_from(plain.command_line()), Some(Ok(plain)));
    }

    #[test]
    fn a_launch_without_the_flag_is_not_the_window() {
        assert_eq!(Args::parse_from(line(&["--version"])), None);
        assert_eq!(Args::parse_from(line(&[])), None);
    }

    /// An app of another version starts the window with another protocol,
    /// or none at all; the window refuses rather than misread it.
    #[test]
    fn another_protocol_is_refused_with_a_reason() {
        assert_eq!(check_protocol(Some(PROTOCOL)), Ok(()));
        let newer = check_protocol(Some(PROTOCOL + 1)).unwrap_err();
        assert!(
            newer.contains(&format!("protocol {}", PROTOCOL + 1)),
            "{newer}"
        );
        assert!(check_protocol(None).unwrap_err().contains("without one"));

        let mut old = sample().command_line();
        let at = old
            .iter()
            .position(|arg| arg == "--milkdrop-protocol")
            .unwrap();
        old.drain(at..at + 2);
        assert!(matches!(Args::parse_from(old), Some(Err(_))));

        let mut other = sample().command_line();
        other[at + 1] = (PROTOCOL + 1).to_string().into();
        assert!(matches!(Args::parse_from(other), Some(Err(_))));
    }

    #[test]
    fn missing_paths_are_an_error_and_bad_numbers_keep_the_defaults() {
        let protocol = PROTOCOL.to_string();
        assert!(matches!(
            Args::parse_from(line(&[CHILD_FLAG, "--milkdrop-protocol", &protocol])),
            Some(Err(_))
        ));
        let parsed = Args::parse_from(line(&[
            CHILD_FLAG,
            "--milkdrop-protocol",
            &protocol,
            "--milkdrop-shm",
            "ring",
            "--milkdrop-presets",
            "presets",
            "--milkdrop-fps",
            "lots",
            "--milkdrop-size",
            "wide",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.fps, DEFAULT_FPS);
        assert_eq!(parsed.size, DEFAULT_SIZE);
        assert_eq!(parsed.seconds, DEFAULT_SECONDS);
        assert_eq!(parsed.scale, 1);
    }
}
