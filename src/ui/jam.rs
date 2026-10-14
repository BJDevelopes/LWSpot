//! Listening with others: the Spotify Jam the listener is in (who hosts it,
//! who is listening, and leaving or ending it), and Discord's Listen Along.

use egui::{RichText, Ui};

use crate::app::App;
use crate::i18n::{Locale, gettext};
use crate::model::Action;
use crate::session_reads::Jam;
use crate::theme::{self, Icon};

use super::widgets;

/// "Jam with Alex", "Your Jam", or "Jam" when Spotify names no host.
pub fn title(locale: Locale, jam: &Jam) -> String {
    if jam.is_host {
        return gettext(locale, "Your Jam").into_owned();
    }
    match &jam.host_name {
        // Translators: {name} is the person hosting the Spotify Jam.
        Some(name) => gettext(locale, "Jam with {name}").replace("{name}", name),
        None => gettext(locale, "Jam").into_owned(),
    }
}

/// "End Jam" for its host, who ends it for everyone; "Leave Jam" otherwise.
pub fn leave_label(locale: Locale, jam: &Jam) -> String {
    if jam.is_host {
        gettext(locale, "End Jam").into_owned()
    } else {
        gettext(locale, "Leave Jam").into_owned()
    }
}

/// The player bar's Jam button, shown only while in a Jam, with a menu of
/// who is listening and Leave (or End) Jam.
pub fn player_bar_button(app: &mut App, ui: &mut Ui) {
    let Some(jam) = app.jam.clone() else {
        return;
    };
    let palette = app.palette;
    let locale = app.locale;
    let title = title(locale, &jam);
    let button = theme::icon_button(ui, Icon::Users, 18.0, palette.accent, palette.text, &title);
    egui::Popup::menu(&button)
        .frame(widgets::menu_frame(&palette))
        .show(|ui| {
            ui.set_min_width(200.0);
            ui.add_space(2.0);
            ui.label(RichText::new(&title).strong().color(palette.text));
            for member in &jam.members {
                let name = if member.is_you {
                    // Translators: {name} is the listener's own name in the Jam's member list.
                    gettext(locale, "{name} (you)").replace("{name}", &member.name)
                } else {
                    member.name.clone()
                };
                ui.label(RichText::new(name).color(palette.secondary));
            }
            widgets::menu_separator(ui, &palette);
            if widgets::menu_item(ui, &palette, Some(Icon::LogOut), &leave_label(locale, &jam)) {
                app.actions.push(Action::LeaveJam);
            }
        });
}

/// The player bar's Listen Along button, shown while Discord's Listen Along
/// is playing a friend's music here. Discord runs it, so the menu says how
/// to stop rather than offering to.
pub fn listen_along_button(app: &mut App, ui: &mut Ui) {
    if !app.listening_along {
        return;
    }
    let palette = app.palette;
    let locale = app.locale;
    let title = gettext(locale, "Listening along on Discord");
    let button = theme::icon_button(ui, Icon::Users, 18.0, palette.accent, palette.text, &title);
    egui::Popup::menu(&button)
        .frame(widgets::menu_frame(&palette))
        .show(|ui| {
            ui.set_max_width(260.0);
            ui.add_space(2.0);
            ui.label(RichText::new(title.as_ref()).strong().color(palette.text));
            ui.label(
                RichText::new(gettext(
                    locale,
                    "Discord is playing your friend's music here. To stop, end it in Discord or play something else.",
                ))
                .color(palette.secondary),
            );
        });
}
