//! The guided setup of a personal Spotify app: from Spotify's developer
//! dashboard to an authorized Client ID, step by step in one dialog.
//!
//! Spotify shares one request allowance among everyone who uses LWSpot's
//! default app. A personal app gets an allowance of its own, so the setup is
//! offered where that matters: from the one-time introduction, from
//! Settings, and once a session from a rate-limit toast while no personal
//! app is ready.

use egui::{Align, CornerRadius, Frame, Layout, Margin, Sense, Stroke, Vec2};

use crate::app::App;
use crate::i18n::{Locale, gettext};
use crate::model::{Action, Dialog, ToastAction};
use crate::theme::{self, Icon, Palette};

/// Where a Spotify developer creates and manages apps.
pub const DASHBOARD_URL: &str = "https://developer.spotify.com/dashboard";

const CLIENT_ID_LEN: usize = 32;
const FIELD_ID: &str = "personal-setup-client-id";

/// The Redirect URI a personal app must list. It is built from the same
/// port and path the sign-in listens on, so the two never drift apart.
pub fn redirect_uri() -> String {
    format!(
        "http://127.0.0.1:{}{}",
        crate::auth::WEB_REDIRECT_PORT,
        crate::auth::REDIRECT_PATH
    )
}

/// Why a pasted Client ID cannot be one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientIdError {
    Empty,
    /// Not 32 characters; how many there are.
    Length(usize),
    NotHex,
}

impl ClientIdError {
    fn message(self, locale: Locale) -> Option<String> {
        match self {
            Self::Empty => None,
            Self::Length(_) => {
                Some(gettext(locale, "A Client ID is exactly 32 characters long.").into_owned())
            }
            Self::NotHex => Some(
                gettext(
                    locale,
                    "A Client ID uses only the digits 0 to 9 and the letters a to f.",
                )
                .into_owned(),
            ),
        }
    }
}

/// The Client ID in `text` without the spaces a copy picks up, when it
/// can be one: 32 hexadecimal digits, as Spotify's dashboard shows it.
pub fn validate_client_id(text: &str) -> Result<&str, ClientIdError> {
    let id = text.trim();
    if id.is_empty() {
        return Err(ClientIdError::Empty);
    }
    let length = id.chars().count();
    if length != CLIENT_ID_LEN {
        return Err(ClientIdError::Length(length));
    }
    if !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ClientIdError::NotHex);
    }
    Ok(id)
}

/// Where authorization stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing asked yet, or the Client ID changed since.
    Idle,
    /// The browser was sent to Spotify's approval page.
    Waiting,
    /// The browser handed the approval back; LWSpot is checking it with
    /// Spotify.
    Verifying,
    /// Spotify verified the app for this account.
    Ready,
    /// The flow failed with the error the app reports elsewhere.
    Failed(String),
}

/// The guided setup's state, kept in its dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The Client ID field as typed.
    pub client_id: String,
    pub phase: Phase,
    pub dashboard_opened: bool,
    pub uri_copied: bool,
}

/// The five steps, for their ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Dashboard,
    CreateApp,
    RedirectUri,
    ClientId,
    Authorize,
}

impl Setup {
    /// Starts from the saved Client ID; ready when that is the app in use.
    pub fn new(saved: Option<&str>, verified: Option<&str>) -> Self {
        let client_id = saved.map(str::trim).unwrap_or_default().to_string();
        let ready = !client_id.is_empty() && verified.map(str::trim) == Some(client_id.as_str());
        Self {
            client_id,
            phase: if ready { Phase::Ready } else { Phase::Idle },
            dashboard_opened: false,
            uri_copied: false,
        }
    }

    /// The Client ID to authorize, when the field holds one.
    pub fn valid_id(&self) -> Option<&str> {
        validate_client_id(&self.client_id).ok()
    }

    /// Whether the browser flow is under way.
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Waiting | Phase::Verifying)
    }

    /// Starts authorizing the typed Client ID and returns it to configure,
    /// or `None` when the field does not hold one.
    pub fn authorize(&mut self) -> Option<String> {
        let id = self.valid_id()?.to_string();
        self.client_id.clone_from(&id);
        self.phase = Phase::Waiting;
        Some(id)
    }

    /// The field changed: an earlier answer no longer describes it.
    pub fn edited(&mut self) {
        if matches!(self.phase, Phase::Ready | Phase::Failed(_)) {
            self.phase = Phase::Idle;
        }
    }

    /// The browser handed back the approval for this app: what is left is
    /// LWSpot checking it with Spotify.
    pub fn approval_received(&mut self) {
        if self.phase == Phase::Waiting {
            self.phase = Phase::Verifying;
        }
    }

    /// The backend reported the verified personal app. Returns whether that
    /// finished this setup.
    pub fn web_app(&mut self, verified: Option<&str>) -> bool {
        let answered = self.busy() || matches!(self.phase, Phase::Failed(_));
        if answered && verified.is_some_and(|id| id == self.client_id.trim()) {
            self.phase = Phase::Ready;
            return true;
        }
        false
    }

    /// An error arrived while authorizing. Returns whether the setup shows
    /// it, so the caller need not.
    pub fn error(&mut self, message: &str) -> bool {
        if self.busy() {
            self.phase = Phase::Failed(message.to_string());
            return true;
        }
        false
    }

    /// Whether `step` shows its tick. A later step done implies the earlier
    /// ones: nobody has a Client ID without having created the app.
    pub fn done(&self, step: Step) -> bool {
        let ready = self.phase == Phase::Ready;
        let id = self.valid_id().is_some() || ready;
        match step {
            Step::Dashboard => self.dashboard_opened || self.uri_copied || id,
            Step::CreateApp => self.uri_copied || id,
            Step::RedirectUri => self.uri_copied || ready,
            Step::ClientId => id,
            Step::Authorize => ready,
        }
    }
}

/// Whether a toast with `message` should offer the guided setup: it says
/// Spotify is rate limiting, no personal app is ready to take requests off
/// the shared app, the account can have one, and no toast offered it yet
/// this session.
pub fn offers_fix(message: &str, ready: bool, product: Option<&str>, offered: bool) -> bool {
    !offered
        && !ready
        && !matches!(product, Some("free" | "open"))
        && message.contains(&crate::api::ApiError::RateLimited.to_string())
}

/// The button a new error toast carries, if any. Offering it uses up the
/// session's one offer.
pub(crate) fn toast_action(app: &mut App, message: &str) -> Option<ToastAction> {
    let product = app.user.as_ref().and_then(|user| user.product.as_deref());
    let offer = offers_fix(
        message,
        app.web_app.is_some(),
        product,
        app.personal_setup_offered,
    ) && !matches!(app.dialog, Some(Dialog::PersonalAppSetup(_)));
    if !offer {
        return None;
    }
    app.personal_setup_offered = true;
    Some(ToastAction::PersonalAppSetup)
}

/// Opens the guided setup, which also counts as having seen the intro.
pub(crate) fn open(app: &mut App) {
    if !app.settings.personal_app_intro_seen {
        app.settings.personal_app_intro_seen = true;
        app.mark_settings_dirty();
    }
    app.toasts
        .retain(|toast| toast.action != Some(ToastAction::PersonalAppSetup));
    app.dialog = Some(Dialog::PersonalAppSetup(Setup::new(
        app.settings.web_client_id.as_deref(),
        app.web_app.as_deref(),
    )));
}

/// After `app.web_app` changed: a setup waiting for it closes, confirmed.
pub(crate) fn web_app_changed(app: &mut App) {
    let finished = match &mut app.dialog {
        Some(Dialog::PersonalAppSetup(setup)) => setup.web_app(app.web_app.as_deref()),
        _ => false,
    };
    if finished {
        app.dialog = None;
        app.toasts
            .retain(|toast| toast.action != Some(ToastAction::PersonalAppSetup));
        app.toast(gettext(app.locale, "Personal app ready"));
    }
}

/// A backend error while the setup authorizes is shown in the setup.
/// Returns whether it was, so the caller does not toast it too.
pub(crate) fn take_error(app: &mut App, message: &str) -> bool {
    match &mut app.dialog {
        Some(Dialog::PersonalAppSetup(setup)) => setup.error(message),
        _ => false,
    }
}

/// Draws the guided setup inside the dialog frame.
pub(super) fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let Some(Dialog::PersonalAppSetup(setup)) = &app.dialog else {
        return;
    };
    let mut setup = setup.clone();
    let before = setup.clone();

    ui.set_width(520.0);
    theme::text(
        ui,
        gettext(locale, "Use your own Spotify app"),
        theme::bold(20.0),
        palette.text,
    );
    ui.add_space(6.0);
    paragraph(
        ui,
        &gettext(
            locale,
            "Why? Spotify shares one request allowance among everyone who uses LWSpot's default app. Your own app gets an allowance of its own.",
        ),
        13.5,
        palette.secondary,
    );
    ui.add_space(18.0);

    let room = ui.ctx().content_rect().height() - 210.0;
    let mut badges = Vec::with_capacity(5);
    crate::autoscroll::show(
        ui,
        egui::ScrollArea::vertical()
            .id_salt("personal-setup-steps")
            .max_height(room.max(160.0))
            .auto_shrink([false, true]),
        egui::Vec2b::new(false, true),
        |ui| {
            badges.push(step(
                ui,
                &palette,
                1,
                setup.done(Step::Dashboard),
                &gettext(locale, "Open the Spotify developer dashboard"),
                |ui| {
                    paragraph(
                        ui,
                        &gettext(
                            locale,
                            "Sign in with your Spotify account. Spotify asks that it be a Premium account.",
                        ),
                        13.0,
                        palette.secondary,
                    );
                    ui.add_space(8.0);
                    if theme::soft_button(
                        ui,
                        &palette,
                        Some(Icon::ExternalLink),
                        &gettext(locale, "Open dashboard"),
                        false,
                    )
                    .clicked()
                    {
                        setup.dashboard_opened = true;
                        app.actions.push(Action::OpenUrl(DASHBOARD_URL.into()));
                    }
                },
            ));
            badges.push(step(
                ui,
                &palette,
                2,
                setup.done(Step::CreateApp),
                &gettext(locale, "Create an app"),
                |ui| {
                    paragraph(
                        ui,
                        &gettext(
                            locale,
                            "Click Create app and give it any name and description; nobody else sees them. Tick Web API and accept the terms.",
                        ),
                        13.0,
                        palette.secondary,
                    );
                },
            ));
            badges.push(step(
                ui,
                &palette,
                3,
                setup.done(Step::RedirectUri),
                &gettext(locale, "Add the Redirect URI"),
                |ui| {
                    paragraph(
                        ui,
                        &gettext(
                            locale,
                            "Paste this under Redirect URIs exactly as shown, then save the app.",
                        ),
                        13.0,
                        palette.secondary,
                    );
                    ui.add_space(8.0);
                    let uri = redirect_uri();
                    ui.horizontal(|ui| {
                        let copy_label = if setup.uri_copied {
                            gettext(locale, "Copied")
                        } else {
                            gettext(locale, "Copy")
                        };
                        let copy_width = theme::soft_button_width(ui, &copy_label) + 21.0;
                        let width = (ui.available_width() - copy_width - 8.0).max(160.0);
                        let label = gettext(locale, "Redirect URI");
                        field_frame(ui, &palette, width, |ui| {
                            let response = super::widgets::text_edit(
                                ui,
                                locale,
                                egui::TextEdit::singleline(&mut uri.as_str())
                                    .id(egui::Id::new("personal-setup-redirect-uri"))
                                    .font(theme::regular(13.5))
                                    .text_color(palette.text)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY),
                            );
                            ui.ctx().accesskit_node_builder(response.id, |node| {
                                node.set_label(label.as_ref());
                            });
                        });
                        let icon = if setup.uri_copied {
                            Icon::Check
                        } else {
                            Icon::Copy
                        };
                        if theme::soft_button(ui, &palette, Some(icon), &copy_label, false)
                            .clicked()
                        {
                            ui.ctx().copy_text(uri.clone());
                            setup.uri_copied = true;
                        }
                    });
                },
            ));
            badges.push(step(
                ui,
                &palette,
                4,
                setup.done(Step::ClientId),
                &gettext(locale, "Paste your Client ID"),
                |ui| {
                    paragraph(
                        ui,
                        &gettext(
                            locale,
                            "Your app's page shows it. Use the Client ID, not the Client secret.",
                        ),
                        13.0,
                        palette.secondary,
                    );
                    ui.add_space(8.0);
                    let label = gettext(locale, "Client ID");
                    let width = ui.available_width();
                    let response = field_frame(ui, &palette, width, |ui| {
                        let response = super::widgets::text_edit(
                            ui,
                            locale,
                            egui::TextEdit::singleline(&mut setup.client_id)
                                .id(egui::Id::new(FIELD_ID))
                                .hint_text(egui::RichText::new(label.as_ref()).color(palette.dim))
                                .font(theme::regular(13.5))
                                .text_color(palette.text)
                                .frame(egui::Frame::NONE)
                                .char_limit(64)
                                .desired_width(f32::INFINITY),
                        );
                        ui.ctx().accesskit_node_builder(response.id, |node| {
                            node.set_label(label.as_ref());
                        });
                        response
                    });
                    if response.changed() {
                        setup.edited();
                    }
                    let error = validate_client_id(&setup.client_id).err();
                    // A length is judged once typing stops; a wrong
                    // character, or one too many, is clear at once.
                    let shown = error.filter(|error| match error {
                        ClientIdError::Length(length) => {
                            !response.has_focus() || *length > CLIENT_ID_LEN
                        }
                        _ => true,
                    });
                    if let Some(message) = shown.and_then(|error| error.message(locale)) {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            theme::icon(ui, Icon::CircleAlert, 14.0, palette.danger);
                            paragraph(ui, &message, 12.5, palette.danger);
                        });
                    }
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                        && !setup.busy()
                    {
                        authorize(app, &mut setup);
                    }
                },
            ));
            badges.push(step(
                ui,
                &palette,
                5,
                setup.done(Step::Authorize),
                &gettext(locale, "Authorize"),
                |ui| {
                    paragraph(
                        ui,
                        &gettext(
                            locale,
                            "Spotify opens in your browser to approve the app. LWSpot then checks that it belongs to the account you use here.",
                        ),
                        13.0,
                        palette.secondary,
                    );
                    ui.add_space(10.0);
                    authorize_status(app, ui, &palette, &mut setup);
                },
            ));
            connect_badges(ui, &palette, &badges);
        },
    );

    ui.add_space(16.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let ready = setup.phase == Phase::Ready;
        let label = if ready {
            gettext(locale, "Done")
        } else {
            gettext(locale, "Close")
        };
        if theme::pill_button(ui, &palette, &label, ready).clicked() {
            app.actions.push(Action::CloseDialog);
        }
    });

    if setup != before
        && let Some(Dialog::PersonalAppSetup(current)) = &mut app.dialog
    {
        *current = setup;
    }
}

/// Saves the typed Client ID and starts the personal app's authorization,
/// the same one Settings starts.
fn authorize(app: &mut App, setup: &mut Setup) {
    if let Some(id) = setup.authorize() {
        app.settings.web_client_id = Some(id);
        app.mark_settings_dirty();
        app.actions.push(Action::ConfigurePersonalWebApp);
    }
}

fn authorize_status(app: &mut App, ui: &mut egui::Ui, palette: &Palette, setup: &mut Setup) {
    let locale = app.locale;
    match setup.phase.clone() {
        Phase::Waiting | Phase::Verifying => {
            let (status, hint) = if setup.phase == Phase::Verifying {
                (gettext(locale, "Checking with Spotify…"), None)
            } else {
                (
                    gettext(locale, "Waiting for your browser…"),
                    Some(gettext(
                        locale,
                        "If your browser still shows Spotify, finish there first.",
                    )),
                )
            };
            ui.horizontal(|ui| {
                theme::spinner(ui, 16.0, palette.accent);
                ui.add_space(4.0);
                theme::text(ui, status, theme::medium(13.5), palette.text);
            });
            if let Some(hint) = hint {
                ui.add_space(4.0);
                paragraph(ui, &hint, 12.5, palette.secondary);
            }
            ui.add_space(6.0);
            // A wrong Client ID can stop on Spotify's page without the app
            // ever hearing back, so the way to start over stays in reach.
            if theme::link(
                ui,
                gettext(locale, "Try again"),
                theme::regular(13.0),
                palette.secondary,
            )
            .clicked()
            {
                authorize(app, setup);
            }
        }
        Phase::Ready => {
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::CircleCheck, 16.0, palette.accent);
                ui.add_space(2.0);
                theme::text(
                    ui,
                    gettext(locale, "Personal app ready"),
                    theme::medium(13.5),
                    palette.text,
                );
            });
        }
        Phase::Idle | Phase::Failed(_) => {
            if let Phase::Failed(message) = &setup.phase {
                ui.horizontal_top(|ui| {
                    theme::icon(ui, Icon::CircleAlert, 16.0, palette.danger);
                    paragraph(ui, message, 12.5, palette.danger);
                });
                ui.add_space(8.0);
            }
            let valid = setup.valid_id().is_some();
            let clicked = ui
                .add_enabled_ui(valid, |ui| {
                    theme::pill_button(ui, palette, &gettext(locale, "Authorize"), true).clicked()
                })
                .inner;
            if clicked {
                authorize(app, setup);
            }
        }
    }
}

/// One numbered step: its badge, title, and whatever `body` adds.
/// Returns the badge's rectangle, for the line joining the badges.
fn step(
    ui: &mut egui::Ui,
    palette: &Palette,
    number: usize,
    done: bool,
    title: &str,
    body: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    let badge = ui
        .horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let badge = step_badge(ui, palette, number, done, title);
            ui.vertical(|ui| {
                ui.add_space(2.0);
                theme::text(ui, title, theme::semibold(14.5), palette.text);
                ui.add_space(2.0);
                body(ui);
            });
            badge
        })
        .inner;
    ui.add_space(16.0);
    badge
}

/// The step's number in a ring, or a filled tick once it is done. Screen
/// readers hear it as the step's checked or unchecked box.
fn step_badge(
    ui: &mut egui::Ui,
    palette: &Palette,
    number: usize,
    done: bool,
    title: &str,
) -> egui::Rect {
    let size = 26.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), done, title)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = size / 2.0;
        if done {
            painter.circle_filled(rect.center(), radius, palette.accent);
            theme::paint_icon(ui, Icon::Check, rect, 15.0, palette.on_accent);
        } else {
            painter.circle_filled(rect.center(), radius, palette.surface);
            painter.circle_stroke(
                rect.center(),
                radius - 0.5,
                Stroke::new(1.0, palette.outline),
            );
            let galley = painter.layout_no_wrap(
                number.to_string(),
                theme::semibold(12.5),
                palette.secondary,
            );
            painter.galley(
                rect.center() - galley.size() / 2.0,
                galley,
                palette.secondary,
            );
        }
    }
    rect
}

/// A thin line from each badge down to the next, so the steps read as one
/// path.
fn connect_badges(ui: &egui::Ui, palette: &Palette, badges: &[egui::Rect]) {
    for pair in badges.windows(2) {
        let (top, bottom) = (pair[0].bottom() + 4.0, pair[1].top() - 4.0);
        if bottom > top {
            ui.painter().vline(
                pair[0].center().x,
                top..=bottom,
                Stroke::new(1.0, palette.outline),
            );
        }
    }
}

fn field_frame<R>(
    ui: &mut egui::Ui,
    palette: &Palette,
    width: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(width - 20.0);
            add(ui)
        })
        .inner
}

fn paragraph(ui: &mut egui::Ui, text: &str, size: f32, color: egui::Color32) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::regular(size))
                .color(color),
        )
        .wrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::User;
    use crate::app::AppOptions;
    use crate::backend::{AuthStatus, Waker};
    use crate::paths::AppDirs;
    use crate::settings::Settings;

    const ID: &str = "8f2c1d0e4a6b4c3d9e7f5a1b2c3d4e5f";
    const RATE_LIMITED: &str =
        "Couldn't load your profile: Spotify is rate limiting requests; try again in a moment";

    fn signed_in_app(name: &str) -> App {
        let root = crate::test_support::temp_dir().join(format!(
            "lwspot-personal-setup-{name}-{}",
            std::process::id()
        ));
        let mut app = App::new(
            &Waker::default(),
            AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            Settings::default(),
            AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
            },
        );
        app.backend.set_offline(true);
        app.auth = AuthStatus::Connected {
            username: "listener".into(),
        };
        app.user = Some(User {
            product: Some("premium".into()),
            ..User::default()
        });
        app
    }

    #[test]
    fn client_ids_are_32_hex_digits_once_trimmed() {
        assert_eq!(validate_client_id(&format!("  {ID}\n")), Ok(ID));
        assert_eq!(
            validate_client_id(&ID.to_uppercase()),
            Ok(ID.to_uppercase().as_str())
        );
        assert_eq!(validate_client_id("   "), Err(ClientIdError::Empty));
        assert_eq!(
            validate_client_id(&ID[..31]),
            Err(ClientIdError::Length(31))
        );
        assert_eq!(
            validate_client_id(&format!("{ID}0")),
            Err(ClientIdError::Length(33))
        );
        let not_hex = format!("{}g", &ID[..31]);
        assert_eq!(validate_client_id(&not_hex), Err(ClientIdError::NotHex));
        assert_eq!(
            validate_client_id(&format!("{} {}", &ID[..15], &ID[16..])),
            Err(ClientIdError::NotHex),
            "a space inside is not trimmed away"
        );
        assert_eq!(
            ClientIdError::Empty.message(Locale::default()),
            None,
            "an empty field is not an error yet"
        );
    }

    #[test]
    fn the_redirect_uri_is_the_one_sign_in_listens_on() {
        let grant = crate::auth::Grant::personal_web_api(ID).unwrap();
        assert_eq!(redirect_uri(), grant.redirect_uri());
        assert_eq!(redirect_uri(), "http://127.0.0.1:8989/login");
    }

    #[test]
    fn authorizing_waits_then_verifies_then_is_ready() {
        let mut setup = Setup::new(None, None);
        assert_eq!(setup.phase, Phase::Idle);
        assert_eq!(setup.authorize(), None, "nothing to authorize yet");
        setup.client_id = format!(" {ID} ");
        assert_eq!(setup.authorize().as_deref(), Some(ID));
        assert_eq!(setup.client_id, ID);
        assert_eq!(setup.phase, Phase::Waiting);
        // Only the browser handing back the approval moves it on.
        setup.approval_received();
        assert_eq!(setup.phase, Phase::Verifying);
        setup.approval_received();
        assert_eq!(setup.phase, Phase::Verifying);
        // The backend clears the old app before it authorizes the new one.
        assert!(!setup.web_app(None));
        assert!(!setup.web_app(Some("another-app")));
        assert_eq!(setup.phase, Phase::Verifying);
        assert!(setup.web_app(Some(ID)));
        assert_eq!(setup.phase, Phase::Ready);
        assert!(setup.done(Step::Authorize));
        setup.client_id.push('0');
        setup.edited();
        assert_eq!(setup.phase, Phase::Idle, "a new ID needs a new answer");
    }

    #[test]
    fn a_failure_shows_in_the_setup_and_a_later_success_still_lands() {
        let mut setup = Setup::new(Some(ID), None);
        assert!(
            !setup.error("unrelated"),
            "errors before Authorize are not ours"
        );
        setup.authorize();
        assert!(setup.error("Personal app authorization failed: denied"));
        assert_eq!(
            setup.phase,
            Phase::Failed("Personal app authorization failed: denied".into())
        );
        assert!(
            !setup.error("a second one"),
            "only the flow's first error is kept"
        );
        assert!(setup.web_app(Some(ID)));
        assert_eq!(setup.phase, Phase::Ready);
    }

    #[test]
    fn steps_tick_as_the_user_gets_there() {
        let mut setup = Setup::new(None, None);
        assert!(!setup.done(Step::Dashboard));
        setup.dashboard_opened = true;
        assert!(setup.done(Step::Dashboard));
        assert!(!setup.done(Step::CreateApp));
        setup.uri_copied = true;
        assert!(setup.done(Step::CreateApp) && setup.done(Step::RedirectUri));
        assert!(!setup.done(Step::ClientId));
        setup.client_id = ID.into();
        assert!(setup.done(Step::ClientId));
        assert!(!setup.done(Step::Authorize));

        let ready = Setup::new(Some(ID), Some(ID));
        assert_eq!(ready.phase, Phase::Ready);
        for step in [
            Step::Dashboard,
            Step::CreateApp,
            Step::RedirectUri,
            Step::ClientId,
            Step::Authorize,
        ] {
            assert!(ready.done(step), "{step:?}");
        }
        assert_eq!(
            Setup::new(Some(ID), Some("other")).phase,
            Phase::Idle,
            "a different app in use is not this one"
        );
    }

    #[test]
    fn only_rate_limits_offer_the_fix_and_only_without_a_ready_app() {
        assert!(offers_fix(RATE_LIMITED, false, Some("premium"), false));
        assert!(offers_fix(RATE_LIMITED, false, None, false));
        assert!(!offers_fix(
            "Couldn't load your profile: offline",
            false,
            None,
            false
        ));
        assert!(
            !offers_fix(RATE_LIMITED, true, Some("premium"), false),
            "requests that stay shared cannot be helped by another app"
        );
        assert!(!offers_fix(RATE_LIMITED, false, Some("free"), false));
        assert!(!offers_fix(RATE_LIMITED, false, Some("premium"), true));
    }

    #[test]
    fn a_rate_limit_toast_offers_setup_once_a_session() {
        let mut app = signed_in_app("toast-once");
        app.toasts.clear();
        app.toast_error(RATE_LIMITED);
        app.toast_error(RATE_LIMITED);
        let actions: Vec<_> = app.toasts.iter().map(|toast| toast.action).collect();
        assert_eq!(actions, [Some(ToastAction::PersonalAppSetup), None]);
        assert!(app.toasts[0].lifetime() > app.toasts[1].lifetime());

        let mut ready = signed_in_app("toast-ready");
        ready.web_app = Some(ID.into());
        ready.toasts.clear();
        ready.toast_error(RATE_LIMITED);
        assert_eq!(ready.toasts[0].action, None);
        assert!(!ready.personal_setup_offered);

        let mut plain = signed_in_app("toast-plain");
        plain.toasts.clear();
        plain.toast_error("Couldn't list devices: network error");
        assert_eq!(plain.toasts[0].action, None);
        assert!(
            !plain.personal_setup_offered,
            "the offer is kept for a rate limit"
        );
        app.backend.shutdown();
        ready.backend.shutdown();
        plain.backend.shutdown();
    }

    #[test]
    fn fix_this_opens_the_setup_and_success_closes_it_with_a_toast() {
        let mut app = signed_in_app("fix-this");
        let ctx = egui::Context::default();
        app.toast_error(RATE_LIMITED);
        app.apply(Action::OpenPersonalAppWalkthrough, &ctx);
        assert!(app.settings.personal_app_intro_seen);
        assert!(
            app.toasts.iter().all(|toast| toast.action.is_none()),
            "the offer goes once it is taken"
        );
        let Some(Dialog::PersonalAppSetup(setup)) = &mut app.dialog else {
            panic!("the setup opens");
        };
        setup.client_id = ID.into();
        let mut setup = setup.clone();
        authorize(&mut app, &mut setup);
        assert_eq!(app.settings.web_client_id.as_deref(), Some(ID));
        assert!(matches!(
            app.actions.last(),
            Some(Action::ConfigurePersonalWebApp)
        ));
        app.dialog = Some(Dialog::PersonalAppSetup(setup));

        // An error while waiting stays in the setup instead of a toast.
        let toasts = app.toasts.len();
        assert!(take_error(&mut app, "Sign-in failed: timed out"));
        assert_eq!(app.toasts.len(), toasts);
        if let Some(Dialog::PersonalAppSetup(setup)) = &mut app.dialog {
            setup.authorize();
        }

        app.web_app = Some(ID.into());
        web_app_changed(&mut app);
        assert!(app.dialog.is_none(), "success closes the setup");
        assert_eq!(
            app.toasts.last().map(|toast| toast.message.as_str()),
            Some("Personal app ready")
        );
        assert!(
            !take_error(&mut app, "later"),
            "nothing is waiting any more"
        );
        app.backend.shutdown();
    }
}
