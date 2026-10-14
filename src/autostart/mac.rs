//! Open at login on macOS, as a per-user launch agent.
//!
//! launchd starts what `~/Library/LaunchAgents/<label>.plist` names when the
//! user logs in, so the file is the whole entry: it holds the program and
//! its `--autostart` flag, the same command a Windows Run value holds, and
//! removing it turns the entry off. Nothing is loaded now and nothing keeps
//! running; System Settings lists it under Login Items, where the user can
//! also turn it off.

use std::path::PathBuf;

use super::{Registry, parse};

/// The launch agent's label and file name, from the app's identifier.
const LABEL: &str = "io.github.BJDevelopes.LWSpot";

/// The user's launch agents.
pub struct System;

impl System {
    fn path(name: &str) -> Option<PathBuf> {
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
        // The entry's name tells a test entry from the app's own.
        let label = if name == super::VALUE_NAME {
            LABEL.to_owned()
        } else {
            format!("{LABEL}.{name}")
        };
        Some(
            PathBuf::from(home)
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist")),
        )
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The launch agent that runs `arguments` at login.
fn plist(label: &str, arguments: &[String]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );
    out.push_str(&format!(
        "\t<key>Label</key>\n\t<string>{}</string>\n",
        escape(label)
    ));
    out.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    for argument in arguments {
        out.push_str(&format!("\t\t<string>{}</string>\n", escape(argument)));
    }
    out.push_str(
        "\t</array>\n\
         \t<key>RunAtLoad</key>\n\t<true/>\n\
         \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
         \t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n\
         </dict>\n</plist>\n",
    );
    out
}

/// The `ProgramArguments` of a launch agent this module wrote.
fn arguments(plist: &str) -> Option<Vec<String>> {
    let (_, rest) = plist.split_once("<key>ProgramArguments</key>")?;
    let (array, _) = rest.split_once("</array>")?;
    Some(
        array
            .split("<string>")
            .skip(1)
            .filter_map(|part| part.split_once("</string>"))
            .map(|(value, _)| unescape(value))
            .collect(),
    )
}

impl Registry for System {
    fn run(&self, name: &str) -> Option<String> {
        let text = std::fs::read_to_string(Self::path(name)?).ok()?;
        let arguments = arguments(&text)?;
        let (exe, flags) = arguments.split_first()?;
        let mut command = format!("\"{exe}\"");
        for flag in flags {
            command.push(' ');
            command.push_str(flag);
        }
        Some(command)
    }

    fn set_run(&mut self, name: &str, command: &str) -> std::io::Result<()> {
        let path = Self::path(name).ok_or_else(|| std::io::Error::other("no home folder"))?;
        let (exe, mode) =
            parse(command).ok_or_else(|| std::io::Error::other("no program to start"))?;
        let mut arguments = vec![exe.to_string_lossy().into_owned()];
        arguments.extend(
            super::command(&exe, mode)
                .and_then(|command| command.rsplit_once(' ').map(|(_, flag)| flag.to_owned())),
        );
        let label = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| LABEL.to_owned());
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let temporary = path.with_extension("plist.tmp");
        std::fs::write(&temporary, plist(&label, &arguments))?;
        crate::util::replace_file(&temporary, &path)
    }

    fn delete_run(&mut self, name: &str) -> std::io::Result<()> {
        let Some(path) = Self::path(name) else {
            return Ok(());
        };
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Login Items turns a launch agent off by its own record, which is
    /// not readable here; the file is the entry.
    fn approval(&self, _name: &str) -> Option<Vec<u8>> {
        None
    }

    fn delete_approval(&mut self, _name: &str) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_agent_keeps_its_arguments() {
        let written = plist(
            LABEL,
            &[
                "/Applications/A & B.app/Contents/MacOS/lwspot".into(),
                "--autostart=tray".into(),
            ],
        );
        assert_eq!(
            arguments(&written).unwrap(),
            [
                "/Applications/A & B.app/Contents/MacOS/lwspot",
                "--autostart=tray"
            ]
        );
    }
}
