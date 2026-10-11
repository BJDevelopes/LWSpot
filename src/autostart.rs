//! Start with Windows (#122).
//!
//! Windows starts what the per-user Run key names when the user signs in:
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `LWSpot`,
//! whose data is the command line, `"<lwspot.exe>" --autostart`, or
//! `--autostart=tray` to start without a window. The key is the only record
//! of the choice, so Settings reads it rather than `settings.json`.
//!
//! Task Manager's Startup apps page and Settings > Apps > Startup leave the
//! Run value alone and turn it off with a marker of their own, a binary
//! value of the same name under `Explorer\StartupApproved\Run`. Its first
//! byte is even while the entry may run (2) and odd once it was turned off
//! (3), so an entry turned off there reads as Off here. Choosing a mode in
//! Settings removes the marker: the newer choice wins.
//!
//! A copy that moved (a portable folder carried elsewhere) would leave the
//! Run value naming a program that is gone, so each start points it at the
//! running program while the entry is on.
//!
//! The registry sits behind [`Registry`], so everything here is tested
//! against [`Memory`]. Only the running instance uses the real one: demo
//! mode and the tests never touch the user's startup list.

use std::path::{Path, PathBuf};

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::System;

/// The Run value's name, and the marker's.
pub const VALUE_NAME: &str = "LWSpot";

/// The flag the Run value starts LWSpot with.
pub const FLAG: &str = "--autostart";

/// What happens when the user signs in to Windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Nothing: LWSpot is not in the startup list, or was turned off there.
    #[default]
    #[value(skip)]
    Off,
    /// LWSpot opens its window.
    Open,
    /// LWSpot starts in the tray, with no window and nothing playing.
    Tray,
}

/// Where the startup list lives. The methods name the value, so the real
/// registry can be exercised under a test name without touching LWSpot's.
pub trait Registry: Send {
    /// The data of the Run value `name`, if there is one.
    fn run(&self, name: &str) -> Option<String>;
    /// Writes the Run value `name`, creating the key if needed.
    fn set_run(&mut self, name: &str, command: &str) -> std::io::Result<()>;
    /// Removes the Run value `name`. A value that is not there is no error.
    fn delete_run(&mut self, name: &str) -> std::io::Result<()>;
    /// The `StartupApproved\Run` marker for `name`, if there is one.
    fn approval(&self, name: &str) -> Option<Vec<u8>>;
    /// Removes that marker. One that is not there is no error.
    fn delete_approval(&mut self, name: &str) -> std::io::Result<()>;
}

/// A startup list in memory, for tests and demo mode.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub run: std::collections::HashMap<String, String>,
    pub approved: std::collections::HashMap<String, Vec<u8>>,
}

impl Registry for Memory {
    fn run(&self, name: &str) -> Option<String> {
        self.run.get(name).cloned()
    }

    fn set_run(&mut self, name: &str, command: &str) -> std::io::Result<()> {
        self.run.insert(name.to_owned(), command.to_owned());
        Ok(())
    }

    fn delete_run(&mut self, name: &str) -> std::io::Result<()> {
        self.run.remove(name);
        Ok(())
    }

    fn approval(&self, name: &str) -> Option<Vec<u8>> {
        self.approved.get(name).cloned()
    }

    fn delete_approval(&mut self, name: &str) -> std::io::Result<()> {
        self.approved.remove(name);
        Ok(())
    }
}

/// The Run value's data for `mode`, or `None` for Off, which has none.
pub fn command(exe: &Path, mode: Mode) -> Option<String> {
    let flag = match mode {
        Mode::Off => return None,
        Mode::Open => FLAG.to_owned(),
        Mode::Tray => format!("{FLAG}=tray"),
    };
    Some(format!("\"{}\" {flag}", exe.display()))
}

/// The program and mode a Run value's data names. A value without the flag
/// (edited by hand, say) still starts LWSpot, which opens its window.
pub fn parse(command: &str) -> Option<(PathBuf, Mode)> {
    let command = command.trim();
    let (exe, arguments) = match command.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"')?,
        None => command.split_once(' ').unwrap_or((command, "")),
    };
    if exe.is_empty() {
        return None;
    }
    let mode = arguments
        .split_whitespace()
        .find_map(|argument| match argument {
            "--autostart" | "--autostart=open" => Some(Mode::Open),
            "--autostart=tray" => Some(Mode::Tray),
            _ => None,
        })
        .unwrap_or(Mode::Open);
    Some((PathBuf::from(exe), mode))
}

/// Whether Task Manager or Settings turned the entry off: an odd first
/// byte in its `StartupApproved\Run` marker.
pub fn turned_off(approval: &[u8]) -> bool {
    approval.first().is_some_and(|first| first & 1 == 1)
}

/// What the startup list says happens at sign-in for the value `name`.
pub fn current(registry: &dyn Registry, name: &str) -> Mode {
    let Some(command) = registry.run(name) else {
        return Mode::Off;
    };
    if registry
        .approval(name)
        .is_some_and(|approval| turned_off(&approval))
    {
        return Mode::Off;
    }
    parse(&command).map_or(Mode::Off, |(_, mode)| mode)
}

/// The startup entry as the app sees it: the list it lives in, the program
/// it should start, and what it does now.
pub struct Autostart {
    registry: Box<dyn Registry>,
    name: String,
    exe: Option<PathBuf>,
    mode: Mode,
}

impl Autostart {
    /// Reads the entry `name` from `registry` and, while it is on, points
    /// it at `exe` if it names another program.
    pub fn new(registry: Box<dyn Registry>, name: &str, exe: Option<PathBuf>) -> Self {
        let mut autostart = Self {
            registry,
            name: name.to_owned(),
            exe,
            mode: Mode::Off,
        };
        autostart.refresh();
        autostart
    }

    /// An entry kept in memory, off: tests and demo mode.
    pub fn in_memory() -> Self {
        Self::new(
            Box::new(Memory::default()),
            VALUE_NAME,
            std::env::current_exe().ok(),
        )
    }

    /// The user's own startup list, for the running instance.
    #[cfg(windows)]
    pub fn system() -> Self {
        Self::new(Box::new(System), VALUE_NAME, std::env::current_exe().ok())
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Makes `mode` what happens at sign-in.
    pub fn set(&mut self, mode: Mode) -> std::io::Result<()> {
        match self.exe.as_deref().and_then(|exe| command(exe, mode)) {
            Some(command) => self.registry.set_run(&self.name, &command)?,
            None if mode == Mode::Off => self.registry.delete_run(&self.name)?,
            None => {
                return Err(std::io::Error::other(
                    "the location of lwspot.exe is not known",
                ));
            }
        }
        // A marker left by Task Manager would keep the new choice off, and
        // one left behind after Off would turn a later choice off too.
        self.registry.delete_approval(&self.name)?;
        self.mode = mode;
        Ok(())
    }

    /// Reads the entry again and points it at this program if it is on and
    /// names another one.
    fn refresh(&mut self) {
        self.mode = current(&*self.registry, &self.name);
        let Some(wanted) = self.exe.as_deref().and_then(|exe| command(exe, self.mode)) else {
            return;
        };
        if self.registry.run(&self.name).as_deref() == Some(wanted.as_str()) {
            return;
        }
        match self.registry.set_run(&self.name, &wanted) {
            Ok(()) => log::info!("Start with Windows now starts {wanted}"),
            Err(error) => log::warn!("could not update the Start with Windows entry: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\me\AppData\Local\Programs\LWSpot\lwspot.exe";

    fn autostart(memory: Memory, exe: &str) -> Autostart {
        Autostart::new(Box::new(memory), VALUE_NAME, Some(PathBuf::from(exe)))
    }

    fn run_value(autostart: &Autostart) -> Option<String> {
        autostart.registry.run(VALUE_NAME)
    }

    #[test]
    fn nothing_in_the_startup_list_is_off() {
        // #given / #when
        let autostart = autostart(Memory::default(), EXE);

        // #then
        assert_eq!(autostart.mode(), Mode::Off);
        assert_eq!(run_value(&autostart), None, "reading writes nothing");
    }

    #[test]
    fn each_mode_writes_its_command_and_off_removes_it() {
        // #given
        let mut autostart = autostart(Memory::default(), EXE);

        // #when / #then
        autostart.set(Mode::Open).unwrap();
        assert_eq!(autostart.mode(), Mode::Open);
        assert_eq!(
            run_value(&autostart).as_deref(),
            Some(r#""C:\Users\me\AppData\Local\Programs\LWSpot\lwspot.exe" --autostart"#)
        );
        autostart.set(Mode::Tray).unwrap();
        assert_eq!(autostart.mode(), Mode::Tray);
        assert_eq!(
            run_value(&autostart).as_deref(),
            Some(r#""C:\Users\me\AppData\Local\Programs\LWSpot\lwspot.exe" --autostart=tray"#)
        );
        autostart.set(Mode::Off).unwrap();
        assert_eq!(autostart.mode(), Mode::Off);
        assert_eq!(run_value(&autostart), None);
    }

    #[test]
    fn the_startup_list_is_read_back_on_the_next_start() {
        for mode in [Mode::Open, Mode::Tray] {
            // #given
            let mut memory = Memory::default();
            memory
                .set_run(VALUE_NAME, &command(Path::new(EXE), mode).unwrap())
                .unwrap();

            // #when
            let autostart = autostart(memory, EXE);

            // #then
            assert_eq!(autostart.mode(), mode);
        }
    }

    /// Task Manager's Startup apps page leaves the Run value and writes a
    /// marker whose first byte is 3; Settings shows that as Off.
    #[test]
    fn an_entry_turned_off_in_task_manager_reads_as_off() {
        // #given
        let mut memory = Memory::default();
        memory
            .set_run(
                VALUE_NAME,
                &command(Path::new(r"D:\Old\lwspot.exe"), Mode::Tray).unwrap(),
            )
            .unwrap();
        let mut disabled = vec![0x03, 0, 0, 0];
        disabled.extend([0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x01]);
        memory.approved.insert(VALUE_NAME.to_owned(), disabled);

        // #when
        let autostart = autostart(memory, EXE);

        // #then
        assert_eq!(autostart.mode(), Mode::Off);
        assert_eq!(
            run_value(&autostart).as_deref(),
            Some(r#""D:\Old\lwspot.exe" --autostart=tray"#),
            "an entry that is off is left as Task Manager left it"
        );
    }

    #[test]
    fn an_entry_turned_back_on_in_task_manager_runs() {
        // #given
        let mut memory = Memory::default();
        memory
            .set_run(VALUE_NAME, &command(Path::new(EXE), Mode::Open).unwrap())
            .unwrap();
        memory
            .approved
            .insert(VALUE_NAME.to_owned(), vec![0x02, 0, 0, 0, 0, 0, 0, 0]);

        // #when / #then
        assert_eq!(autostart(memory, EXE).mode(), Mode::Open);
    }

    #[test]
    fn choosing_a_mode_in_settings_clears_the_task_manager_marker() {
        // #given
        let mut memory = Memory::default();
        memory
            .set_run(VALUE_NAME, &command(Path::new(EXE), Mode::Open).unwrap())
            .unwrap();
        memory
            .approved
            .insert(VALUE_NAME.to_owned(), vec![0x03, 0, 0, 0]);
        let mut autostart = autostart(memory, EXE);
        assert_eq!(autostart.mode(), Mode::Off);

        // #when
        autostart.set(Mode::Tray).unwrap();

        // #then
        assert_eq!(autostart.registry.approval(VALUE_NAME), None);
        assert_eq!(current(&*autostart.registry, VALUE_NAME), Mode::Tray);
    }

    /// A portable copy carried to another folder points the entry at
    /// itself on its next start, keeping the mode.
    #[test]
    fn a_moved_program_points_the_entry_at_itself() {
        // #given
        let mut memory = Memory::default();
        memory
            .set_run(
                VALUE_NAME,
                &command(Path::new(r"E:\Portable\LWSpot\lwspot.exe"), Mode::Tray).unwrap(),
            )
            .unwrap();

        // #when
        let autostart = autostart(memory, EXE);

        // #then
        assert_eq!(autostart.mode(), Mode::Tray);
        assert_eq!(
            run_value(&autostart),
            command(Path::new(EXE), Mode::Tray),
            "the entry names the running program"
        );
    }

    #[test]
    fn without_a_known_location_the_entry_cannot_be_turned_on() {
        // #given
        let mut autostart = Autostart::new(Box::new(Memory::default()), VALUE_NAME, None);

        // #when / #then
        assert!(autostart.set(Mode::Open).is_err());
        assert_eq!(autostart.mode(), Mode::Off);
        autostart.set(Mode::Off).expect("off needs no location");
    }

    #[test]
    fn run_values_are_read_with_or_without_quotes() {
        assert_eq!(
            parse(r#""C:\Program Files\LWSpot\lwspot.exe" --autostart=tray"#),
            Some((
                PathBuf::from(r"C:\Program Files\LWSpot\lwspot.exe"),
                Mode::Tray
            ))
        );
        assert_eq!(
            parse(r"C:\LWSpot\lwspot.exe --autostart"),
            Some((PathBuf::from(r"C:\LWSpot\lwspot.exe"), Mode::Open))
        );
        assert_eq!(
            parse(r"C:\LWSpot\lwspot.exe"),
            Some((PathBuf::from(r"C:\LWSpot\lwspot.exe"), Mode::Open))
        );
        assert_eq!(parse(r#""C:\unterminated"#), None);
        assert_eq!(parse(""), None);
        assert!(!turned_off(&[]));
        assert!(!turned_off(&[0x02, 0]));
        assert!(turned_off(&[0x03, 0]));
    }

    /// Writes, reads back and removes a Run value under a test name in the
    /// real `HKCU`, and the marker beside it. LWSpot's own entry is never
    /// touched. Run with `cargo test --lib autostart -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "writes to the real HKCU Run key"]
    fn the_real_startup_list_round_trips_under_a_test_name() {
        let name = format!("LWSpotTest{}", std::process::id());
        let mut registry = System;
        let _ = registry.delete_run(&name);
        let _ = registry.delete_approval(&name);
        let exe = PathBuf::from(r"C:\LWSpot Test\lwspot.exe");

        let mut autostart = Autostart::new(Box::new(System), &name, Some(exe.clone()));
        assert_eq!(autostart.mode(), Mode::Off);
        autostart.set(Mode::Tray).unwrap();
        assert_eq!(registry.run(&name), command(&exe, Mode::Tray));
        assert_eq!(current(&registry, &name), Mode::Tray);

        win::set_approval(&name, &[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(current(&registry, &name), Mode::Off);

        let moved = PathBuf::from(r"C:\Moved\lwspot.exe");
        registry.delete_approval(&name).unwrap();
        let refreshed = Autostart::new(Box::new(System), &name, Some(moved.clone()));
        assert_eq!(refreshed.mode(), Mode::Tray);
        assert_eq!(registry.run(&name), command(&moved, Mode::Tray));

        autostart.set(Mode::Off).unwrap();
        assert_eq!(registry.run(&name), None);
        assert_eq!(registry.approval(&name), None);
    }
}
