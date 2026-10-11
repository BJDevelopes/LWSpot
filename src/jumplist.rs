//! The taskbar button's jump list: the Tasks a right-click on LWSpot's
//! taskbar button (or its pinned icon) offers.
//!
//! Each task starts `lwspot.exe` again with one of its command-line verbs,
//! and that launch hands the verb to the running copy over the
//! single-instance channel, as `lwspot next` from a terminal would. "Show
//! LWSpot" starts the program with no verb at all: that brings a running
//! copy forward, and starts LWSpot when none is running, which a pinned
//! icon can be asked to do.
//!
//! Windows keeps the list between runs, so it is written once at startup
//! and again when the interface language changes.

use crate::i18n::{Locale, gettext};

#[cfg(windows)]
mod win;

/// One entry under Tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// What the entry says, in the interface language.
    pub title: String,
    /// The command line after `lwspot.exe`.
    pub arguments: &'static str,
}

/// The tasks, in the order the list shows them.
pub fn tasks(locale: Locale) -> Vec<Task> {
    [
        (gettext(locale, "Play / Pause"), "play-pause"),
        (gettext(locale, "Next"), "next"),
        (gettext(locale, "Previous"), "previous"),
        (gettext(locale, "Mini player"), "mini-player"),
        (gettext(locale, "Show LWSpot"), ""),
    ]
    .into_iter()
    .map(|(title, arguments)| Task {
        title: title.into_owned(),
        arguments,
    })
    .collect()
}

/// Replaces the jump list with the tasks in `locale`, on a thread of its
/// own. A failure is logged and leaves the taskbar's default list.
pub fn publish(locale: Locale) {
    #[cfg(windows)]
    win::publish(tasks(locale));
    #[cfg(not(windows))]
    let _ = locale;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tasks_are_the_transport_the_mini_player_and_show() {
        // #given / #when
        let tasks = tasks(Locale::English);

        // #then
        let shown: Vec<(&str, &str)> = tasks
            .iter()
            .map(|task| (task.title.as_str(), task.arguments))
            .collect();
        assert_eq!(
            shown,
            [
                ("Play / Pause", "play-pause"),
                ("Next", "next"),
                ("Previous", "previous"),
                ("Mini player", "mini-player"),
                ("Show LWSpot", ""),
            ]
        );
    }

    #[test]
    fn the_task_titles_follow_the_interface_language() {
        // #given / #when
        let german = tasks(Locale::German);

        // #then
        assert_eq!(german[1].title, "Weiter");
        assert_eq!(german[3].title, "Mini-Player");
        assert!(
            german
                .iter()
                .zip(tasks(Locale::English))
                .all(|(german, english)| german.arguments == english.arguments),
            "the verbs are the same in every language"
        );
    }
}
