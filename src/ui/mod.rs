//! Window layout: panels, overlays, keyboard shortcuts.

use std::sync::Arc;

pub mod artist;
pub mod collection;
pub(crate) mod devices;
mod dialogs;
pub mod home;
mod keys;
pub mod library;
pub mod login;
mod lyrics;
pub mod player_bar;
pub mod queue;
pub mod radio;
pub mod search;
pub mod settings;
pub mod show;
pub mod sidebar;
pub mod topbar;
mod update;
pub mod widgets;
pub mod winamp;

use egui::{Align2, Color32, Context, CornerRadius, Frame, Id, Margin, Rect, Stroke, vec2};

use crate::api::models::pick_image;
use crate::app::App;
use crate::backend::AuthStatus;
use crate::model::{Action, Loadable, Page, ToastKind};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let ctx = &ctx;
    keys::handle(app, ctx);
    for path in winamp::dropped_skins(ctx) {
        app.actions.push(Action::InstallSkin(path));
    }
    let signed_in = app.is_connected() && app.user.is_some();
    let connecting = matches!(app.auth, AuthStatus::Connecting | AuthStatus::Starting)
        || (app.is_connected() && app.user.is_none());
    if !signed_in {
        player_bar::end_tint_session(ctx);
        login::show(app, ui, connecting);
        update::show(app, ctx);
        toasts(app, ctx, 20.0);
        window_controls(ui, &app.palette, app.locale);
        window_resize(ui);
        return;
    }
    // Without a native title bar, the top of every panel drags the window, not
    // only the page's top bar.
    if crate::window::custom_titlebar() {
        titlebar_drag(
            ui,
            Rect::from_min_size(
                ui.max_rect().min,
                vec2(ui.max_rect().width(), theme::TOP_BAR_HEIGHT),
            ),
        );
    }
    player_bar::show(app, ui);
    if app.lyrics_fullscreen.is_some() {
        lyrics::fullscreen(app, ui);
    } else {
        if app.settings.sidebar_visible {
            sidebar::show(app, ui);
        }
        if app.show_queue_panel {
            queue::side_panel(app, ui);
        }
        if app.show_lyrics_panel {
            lyrics::side_panel(app, ui);
        }
        central(app, ui);
        keep_room_for_panels(app, ctx);
    }
    devices::popup(app, ctx);
    dialogs::show(app, ctx);
    update::show(app, ctx);
    widgets::drag_ghost(ctx, &app.palette, app.locale);
    toasts(app, ctx, theme::PLAYER_BAR_HEIGHT + 16.0);
    window_controls(ui, &app.palette, app.locale);
    window_resize(ui);
}

/// The main window's narrowest width with these panels open: their least
/// widths and the page's, but never wider than the monitor, which a
/// portrait one can be narrower than (#783).
fn main_min_width(page: f32, sidebar: bool, right_panel: bool, monitor: Option<f32>) -> f32 {
    let sidebar = if sidebar { SIDEBAR_MIN_WIDTH } else { 0.0 };
    let right = if right_panel {
        theme::SIDE_PANEL_MIN_WIDTH
    } else {
        0.0
    };
    let width = (sidebar + right + page).max(crate::window::MAIN_MIN_SIZE[0]);
    match monitor {
        Some(monitor) if monitor.is_finite() && monitor > 0.0 => width.min(monitor),
        _ => width,
    }
}

/// The sidebar's narrowest width.
pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 210.0;

/// Raise the window's minimum width while the Queue or Lyrics panel is open,
/// so even at its narrowest the page beside the panels keeps the room its
/// top bar needs (#624), and lower it again once they close. Window managers
/// that tile ignore the minimum; the panels still give way there.
fn keep_room_for_panels(app: &App, ctx: &Context) {
    // A full-screen window has no minimum; it comes back on leaving.
    if crate::window::fixed_size()
        || app.window_fullscreen()
        || ctx.input(|input| input.viewport().fullscreen.unwrap_or(false))
    {
        return;
    }
    let width = main_min_width(
        topbar::least_width(ctx),
        app.settings.sidebar_visible,
        app.show_queue_panel || app.show_lyrics_panel,
        ctx.input(|input| input.viewport().monitor_size.map(|size| size.x)),
    )
    .round();
    // The minimum goes to winit in pixels, so a new scale needs it again.
    let scale = ctx.pixels_per_point();
    let sent = crate::window::sent_min_width(ctx);
    // The window starts at MAIN_MIN_SIZE, which winit rescales itself, so
    // there is nothing to send until the panels need more.
    let stale = match sent {
        Some(sent) => sent != [width, scale],
        None => width != crate::window::MAIN_MIN_SIZE[0],
    };
    if stale {
        crate::window::record_min_width(ctx, width, scale);
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(
            width,
            crate::window::MAIN_MIN_SIZE[1],
        )));
    }
}

/// How a resizable side panel fits beside the page.
pub(crate) struct PanelFit {
    /// The widths the panel may take this frame.
    pub range: std::ops::RangeInclusive<f32>,
    /// Whether the room the page needs holds the panel under the width the
    /// person chose for it.
    pub yielding: bool,
}

/// Fit a side panel between `range` and the page beside it. The panel gives
/// up width, down to its minimum, so the page keeps `room_for_page`, the
/// least its top bar needs (#624). The width chosen for the panel stays in
/// the settings and comes back once the window widens again.
///
/// `room` is what the panel and everything after it share.
pub(crate) fn yielding_panel(
    ctx: &Context,
    id: &str,
    range: std::ops::RangeInclusive<f32>,
    chosen: f32,
    room: f32,
) -> PanelFit {
    let (min, max) = range.into_inner();
    let max = max.min(room).max(min);
    let width = chosen.clamp(min, max);
    // egui remembers the width the panel last drew at, a clamped one too.
    // Hand it back the chosen width whenever there is room for more.
    let id = Id::new(id);
    if let Some(mut state) = egui::containers::panel::PanelState::load(ctx, id)
        && state.outer_rect.width() + 0.5 < width
    {
        state.outer_rect.set_width(width);
        ctx.data_mut(|data| data.insert_persisted(id, state));
    }
    PanelFit {
        range: min..=max,
        yielding: max < chosen - 0.5,
    }
}

/// Whether a panel's new width is one the person chose: it moved while not
/// held back by the page, or by their own drag.
pub(crate) fn panel_width_chosen(ctx: &Context, id: &str, fit: &PanelFit) -> bool {
    !fit.yielding || ctx.is_being_dragged(Id::new(id).with("__resize"))
}

/// Spotify artwork width used by the library grid and its page preview.
const GRID_ART_TARGET_WIDTH: u32 = 300;

/// Keeps the most recent loading preview of each metadata type available to
/// the loaded hero as an artwork fallback. The fixed typed slot bounds this to
/// one playlist, album, artist, and show instead of scanning known metadata on
/// every loaded frame.
fn loading_preview<T>(
    ctx: &Context,
    key: &str,
    details: &Loadable<T>,
    known: impl FnOnce() -> Option<T>,
) -> Option<Arc<T>>
where
    T: Send + Sync + 'static,
{
    let memory_id = Id::new("collection-loading-preview");
    let key = Id::new(key);
    if details.get().is_some() {
        ctx.data(|data| data.get_temp::<(Id, Arc<T>)>(memory_id))
            .filter(|(stored_key, _)| *stored_key == key)
            .map(|(_, preview)| preview)
    } else {
        let preview = known().map(Arc::new);
        ctx.data_mut(|data| match &preview {
            Some(preview) => {
                data.insert_temp(memory_id, (key, Arc::clone(preview)));
            }
            None => data.remove::<(Id, Arc<T>)>(memory_id),
        });
        preview
    }
}

fn page_tint(app: &mut App) -> Option<Color32> {
    let page = app.page().clone();
    let image = match &page {
        Page::Playlist(id) => app
            .playlist_pages
            .get(id)
            .and_then(|page| page.playlist.get())
            .or_else(|| app.known_playlist(id))
            .and_then(|playlist| pick_image(&playlist.images, 64))
            .map(str::to_string),
        Page::Album(id) => app
            .album_pages
            .get(id)
            .and_then(|page| page.album.get())
            .or_else(|| app.known_album(id))
            .and_then(|album| pick_image(&album.images, 64))
            .map(str::to_string),
        Page::Artist(id) => app
            .artist_pages
            .get(id)
            .and_then(|page| page.artist.get())
            .or_else(|| app.known_artist(id))
            .and_then(|artist| pick_image(&artist.images, 64))
            .map(str::to_string),
        Page::Show(id) => app
            .show_pages
            .get(id)
            .and_then(|page| page.show.get())
            .or_else(|| app.known_show(id))
            .and_then(|show| pick_image(&show.images, 64))
            .map(str::to_string),
        Page::Radio(seed) => pick_image(&app.radio_images(seed), 64).map(str::to_string),
        Page::LikedSongs => return Some(Color32::from_rgb(0x50, 0x38, 0xc8)),
        _ => None,
    };
    if !app.settings.accent_from_art && image.is_some() {
        return None;
    }
    match image {
        Some(url) => app.tint_for(Some(&url)).or_else(|| app.now_playing_tint()),
        None => app.now_playing_tint(),
    }
}

fn central(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let tint = page_tint(app);
    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            if let Some(tint) = tint {
                let strength = if matches!(
                    app.page(),
                    Page::Home | Page::Search | Page::Settings | Page::Queue
                ) {
                    0.45
                } else {
                    0.85
                };
                let top = blend(palette.window, tint, strength);
                let header = Rect::from_min_size(rect.min, vec2(rect.width(), 340.0));
                widgets::paint_vertical_gradient(ui, header, top, palette.window);
            }
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            // egui fades a scrolled page's edge into the panel's plain
            // colour, which shows as a pale band over a cover's tint; the
            // page casts a shadow under the header instead.
            ui.spacing_mut().scroll.fade.strength = 0.0;
            topbar::show(app, ui);
            let page = app.page().clone();
            let scroll = crate::autoscroll::show(
                ui,
                egui::ScrollArea::vertical()
                    .id_salt(("page", page.encode()))
                    .auto_shrink([false, false]),
                egui::Vec2b::new(false, true),
                |ui| {
                    Frame::new()
                        .inner_margin(Margin {
                            left: widgets::PAGE_PADDING as i8,
                            right: widgets::PAGE_PADDING as i8,
                            top: 4,
                            bottom: 48,
                        })
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            match page {
                                Page::Home => home::show(app, ui),
                                Page::TopSongs => collection::top_songs(app, ui),
                                Page::Search => search::show(app, ui),
                                Page::LikedSongs => collection::liked(app, ui),
                                Page::Albums | Page::Artists | Page::Podcasts | Page::Episodes => {
                                    library::show(app, ui, page)
                                }
                                Page::Playlist(id) => collection::playlist(app, ui, &id),
                                Page::Album(id) => collection::album(app, ui, &id),
                                Page::Artist(id) => artist::show(app, ui, &id),
                                Page::Show(id) => show::show(app, ui, &id),
                                Page::Radio(seed) => radio::radio(app, ui, &seed),
                                Page::Queue => queue::page(app, ui),
                                Page::Settings => settings::show(app, ui),
                            }
                        });
                },
            );
            header_shadow(ui, scroll.inner_rect, scroll.state.offset.y, palette.dark);
        });
}

/// The shadow the header casts on a page scrolled under it, deepening over
/// the first few points of scrolling. Dark in both themes, lighter over a
/// light page.
fn header_shadow(ui: &egui::Ui, page: Rect, scrolled: f32, dark: bool) {
    let depth = (scrolled / 24.0).clamp(0.0, 1.0);
    if depth <= 0.0 {
        return;
    }
    let strength = if dark { 110.0 } else { 36.0 };
    let rect = Rect::from_min_size(page.min, vec2(page.width(), 14.0));
    widgets::paint_vertical_gradient(
        ui,
        rect,
        egui::Color32::from_black_alpha((strength * depth) as u8),
        egui::Color32::TRANSPARENT,
    );
}

/// Makes `rect` drag the borderless window. Register it before child widgets so
/// they keep their clicks.
pub fn titlebar_drag(ui: &mut egui::Ui, rect: egui::Rect) {
    let fullscreen = ui
        .ctx()
        .input(|input| input.viewport().fullscreen.unwrap_or(false));
    if fullscreen || !crate::window::custom_titlebar() {
        return;
    }
    let response = ui.interact(
        rect,
        ui.id().with("titlebar-drag"),
        egui::Sense::click_and_drag(),
    );
    if crate::window::custom_titlebar() && response.double_clicked() {
        let maximized = ui
            .ctx()
            .input(|input| input.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if crate::window::custom_titlebar() && response.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

const WINDOW_RESIZE_BORDER: f32 = 5.0;
const WINDOW_RESIZE_CORNER: f32 = 10.0;
const WINDOWS_WINDOW_CONTROLS_WIDTH: f32 = 3.0 * 36.0 + WINDOW_RESIZE_BORDER;
const WINDOWS_WINDOW_CONTROLS_HEIGHT: f32 = 36.0 + WINDOW_RESIZE_BORDER;
// The 760-point minimum with the default 250-point sidebar leaves 510 points.
const WINDOWS_MIN_INLINE_TOPBAR_WIDTH: f32 = 510.0 + WINDOWS_WINDOW_CONTROLS_WIDTH;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WindowControlsReservation {
    pub topbar_width: f32,
    pub topbar_top: f32,
    pub queue_top: f32,
    pub lyrics_top: f32,
}

const fn windows_chrome_visible(on_windows: bool, fullscreen: bool) -> bool {
    on_windows && !fullscreen
}

fn windows_chrome_visible_here(ctx: &egui::Context) -> bool {
    let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
    windows_chrome_visible(crate::window::custom_titlebar(), fullscreen)
}

const fn controls_reservation(
    on_windows: bool,
    fullscreen: bool,
    queue: bool,
    lyrics: bool,
    topbar_width: f32,
    controls_width: f32,
) -> WindowControlsReservation {
    let mut space = WindowControlsReservation {
        topbar_width: 0.0,
        topbar_top: 0.0,
        queue_top: 0.0,
        lyrics_top: 0.0,
    };
    if windows_chrome_visible(on_windows, fullscreen) && controls_width > 0.0 {
        if queue {
            space.queue_top = WINDOWS_WINDOW_CONTROLS_HEIGHT;
        } else if lyrics {
            space.lyrics_top = WINDOWS_WINDOW_CONTROLS_HEIGHT;
        } else if topbar_width
            < WINDOWS_MIN_INLINE_TOPBAR_WIDTH - WINDOWS_WINDOW_CONTROLS_WIDTH + controls_width
        {
            space.topbar_top = WINDOWS_WINDOW_CONTROLS_HEIGHT;
        } else {
            space.topbar_width = controls_width;
        }
    }
    space
}

pub(super) fn window_controls_reservation(
    ctx: &egui::Context,
    queue: bool,
    lyrics: bool,
    topbar_width: f32,
) -> WindowControlsReservation {
    let fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
    let width = WINDOWS_WINDOW_CONTROLS_WIDTH;
    controls_reservation(
        crate::window::custom_titlebar(),
        fullscreen,
        queue,
        lyrics,
        topbar_width,
        width,
    )
}

/// Draws caption controls over the outermost header.
pub fn window_controls(ui: &mut egui::Ui, palette: &theme::Palette, locale: crate::i18n::Locale) {
    use crate::i18n::gettext;
    use crate::window::WindowButton;
    if !windows_chrome_visible_here(ui.ctx()) {
        return;
    }
    let maximized = ui
        .ctx()
        .input(|input| input.viewport().maximized.unwrap_or(false));
    let groups = [(
        true,
        [
            crate::window::WindowButton::Minimize,
            crate::window::WindowButton::Maximize,
            crate::window::WindowButton::Close,
        ]
        .as_slice(),
    )];
    let (margin, spacing) = (WINDOW_RESIZE_BORDER, 0.0);
    for (right, buttons) in groups {
        if buttons.is_empty() {
            continue;
        }
        egui::Area::new(egui::Id::new(if right {
            "windows-window-controls"
        } else {
            "window-controls-left"
        }))
        .anchor(
            if right {
                Align2::RIGHT_TOP
            } else {
                Align2::LEFT_TOP
            },
            vec2(if right { -margin } else { margin }, margin),
        )
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            ui.spacing_mut().item_spacing.x = spacing;
            ui.horizontal(|ui| {
                for &button in buttons {
                    let (icon, tooltip, command) = match button {
                        WindowButton::Minimize => (
                            Icon::Minus,
                            gettext(locale, "Minimize"),
                            egui::ViewportCommand::Minimized(true),
                        ),
                        WindowButton::Maximize => (
                            if maximized { Icon::Copy } else { Icon::Square },
                            if maximized {
                                gettext(locale, "Restore")
                            } else {
                                gettext(locale, "Maximize")
                            },
                            egui::ViewportCommand::Maximized(!maximized),
                        ),
                        WindowButton::Close => (
                            Icon::X,
                            gettext(locale, "Close"),
                            egui::ViewportCommand::Close,
                        ),
                    };
                    let image = icon
                        .image(palette.secondary, 14.0)
                        .alt_text(tooltip.as_ref());
                    let button = egui::Button::image(image).frame_when_inactive(false);
                    let response = ui
                        .add_sized(egui::Vec2::splat(36.0), button)
                        .on_hover_text(tooltip.as_ref());
                    if response.clicked() {
                        ui.ctx().send_viewport_cmd(command);
                    }
                }
            });
        });
    }
}

fn window_resize(ui: &mut egui::Ui) {
    let (fullscreen, maximized) = ui.ctx().input(|input| {
        (
            input.viewport().fullscreen.unwrap_or(false),
            input.viewport().maximized.unwrap_or(false),
        )
    });
    if !window_resize_enabled(crate::window::custom_titlebar(), fullscreen, maximized) {
        return;
    }

    let Some(position) = ui.input(|input| input.pointer.hover_pos()) else {
        return;
    };
    let Some(direction) = resize_direction(ui.ctx().content_rect(), position) else {
        return;
    };
    ui.ctx().set_cursor_icon(direction.1);
    if ui.input(|input| input.pointer.primary_pressed()) {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction.0));
    }
}

const fn window_resize_enabled(on_windows: bool, fullscreen: bool, maximized: bool) -> bool {
    on_windows && !fullscreen && !maximized
}

fn resize_direction(
    window: Rect,
    position: egui::Pos2,
) -> Option<(egui::ResizeDirection, egui::CursorIcon)> {
    if !window.contains(position) {
        return None;
    }
    let [left, right, top, bottom] = [
        position.x - window.left(),
        window.right() - position.x,
        position.y - window.top(),
        window.bottom() - position.y,
    ];
    let mut x = i8::from(right <= WINDOW_RESIZE_BORDER) - i8::from(left <= WINDOW_RESIZE_BORDER);
    let mut y = i8::from(bottom <= WINDOW_RESIZE_BORDER) - i8::from(top <= WINDOW_RESIZE_BORDER);
    if y != 0 {
        x = i8::from(right <= WINDOW_RESIZE_CORNER) - i8::from(left <= WINDOW_RESIZE_CORNER);
    }
    if x != 0 {
        y = i8::from(bottom <= WINDOW_RESIZE_CORNER) - i8::from(top <= WINDOW_RESIZE_CORNER);
    }
    use egui::{CursorIcon as C, ResizeDirection as D};
    match (x, y) {
        (-1, -1) => Some((D::NorthWest, C::ResizeNwSe)),
        (1, -1) => Some((D::NorthEast, C::ResizeNeSw)),
        (-1, 1) => Some((D::SouthWest, C::ResizeNeSw)),
        (1, 1) => Some((D::SouthEast, C::ResizeNwSe)),
        (-1, 0) => Some((D::West, C::ResizeHorizontal)),
        (1, 0) => Some((D::East, C::ResizeHorizontal)),
        (0, -1) => Some((D::North, C::ResizeVertical)),
        (0, 1) => Some((D::South, C::ResizeVertical)),
        _ => None,
    }
}

pub fn blend(base: Color32, tint: Color32, amount: f32) -> Color32 {
    let a = egui::Rgba::from(base);
    let b = egui::Rgba::from(tint);
    let mixed = a * (1.0 - amount) + b * amount;
    let mut color = Color32::from(mixed);
    color[3] = 255;
    color
}

fn toasts(app: &mut App, ctx: &egui::Context, bottom_offset: f32) {
    if app.toasts.is_empty() {
        return;
    }
    let palette = app.palette;
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -bottom_offset))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            for toast in &app.toasts {
                let age = toast.created.elapsed().as_secs_f32();
                let alpha = if age < 0.15 {
                    age / 0.15
                } else if age > 2.8 {
                    ((3.2 - age) / 0.4).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                ui.set_opacity(alpha);
                Frame::new()
                    .fill(palette.overlay)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: palette.shadow,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (icon, color) = match toast.kind {
                                ToastKind::Info => (Icon::CircleCheck, palette.accent),
                                ToastKind::Error => (Icon::CircleAlert, palette.danger),
                            };
                            theme::icon(ui, icon, 16.0, color);
                            // Laid out at its own width. The area
                            // remembers its size, so after a short toast a
                            // label left to wrap at the area's width broke
                            // long messages on every word.
                            let galley = ui.painter().layout(
                                toast.message.clone(),
                                theme::medium(13.5),
                                palette.text,
                                280.0,
                            );
                            ui.add(egui::Label::new(galley));
                        });
                    });
            }
        });
}

#[cfg(test)]
mod window_chrome_tests {
    use super::*;

    #[test]
    fn chrome_visibility_matches_window_state() {
        assert!(windows_chrome_visible(true, false));
        assert!(!windows_chrome_visible(true, true));
        assert!(!windows_chrome_visible(false, false));
        assert!(window_resize_enabled(true, false, false));
        assert!(!window_resize_enabled(true, false, true));
        assert!(!window_resize_enabled(true, true, false));
        assert!(!window_resize_enabled(false, false, false));
    }

    #[test]
    fn caption_space_belongs_to_the_outermost_header() {
        let values = |queue, lyrics| {
            let space = controls_reservation(
                true,
                false,
                queue,
                lyrics,
                f32::INFINITY,
                WINDOWS_WINDOW_CONTROLS_WIDTH,
            );
            [
                space.topbar_width,
                space.topbar_top,
                space.queue_top,
                space.lyrics_top,
            ]
        };
        assert_eq!(
            values(false, false),
            [WINDOWS_WINDOW_CONTROLS_WIDTH, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            values(true, false),
            [0.0, 0.0, WINDOWS_WINDOW_CONTROLS_HEIGHT, 0.0]
        );
        assert_eq!(
            values(false, true),
            [0.0, 0.0, 0.0, WINDOWS_WINDOW_CONTROLS_HEIGHT]
        );
        assert_eq!(
            values(true, true),
            [0.0, 0.0, WINDOWS_WINDOW_CONTROLS_HEIGHT, 0.0]
        );
        assert_eq!(
            controls_reservation(
                true,
                true,
                true,
                true,
                f32::INFINITY,
                WINDOWS_WINDOW_CONTROLS_WIDTH
            ),
            WindowControlsReservation {
                topbar_width: 0.0,
                topbar_top: 0.0,
                queue_top: 0.0,
                lyrics_top: 0.0,
            }
        );
    }

    #[test]
    fn minimum_windows_window_stacks_caption_space_above_the_topbar() {
        let available = 760.0 - 250.0;
        let space = controls_reservation(
            true,
            false,
            false,
            false,
            available,
            WINDOWS_WINDOW_CONTROLS_WIDTH,
        );
        assert_eq!(space.topbar_width, 0.0);
        assert_eq!(space.topbar_top, WINDOWS_WINDOW_CONTROLS_HEIGHT);

        let inline = controls_reservation(
            true,
            false,
            false,
            false,
            WINDOWS_MIN_INLINE_TOPBAR_WIDTH,
            WINDOWS_WINDOW_CONTROLS_WIDTH,
        );
        assert_eq!(inline.topbar_width, WINDOWS_WINDOW_CONTROLS_WIDTH);
        assert_eq!(inline.topbar_top, 0.0);
    }

    #[test]
    fn resize_hit_test_covers_edges_and_corners() {
        use egui::ResizeDirection as D;

        let window = Rect::from_min_max(egui::pos2(20.0, 30.0), egui::pos2(120.0, 110.0));
        for (position, expected) in [
            (egui::pos2(21.0, 31.0), Some(D::NorthWest)),
            (egui::pos2(70.0, 31.0), Some(D::North)),
            (egui::pos2(119.0, 31.0), Some(D::NorthEast)),
            (egui::pos2(21.0, 70.0), Some(D::West)),
            (egui::pos2(70.0, 70.0), None),
            (egui::pos2(119.0, 70.0), Some(D::East)),
            (egui::pos2(21.0, 109.0), Some(D::SouthWest)),
            (egui::pos2(70.0, 109.0), Some(D::South)),
            (egui::pos2(119.0, 109.0), Some(D::SouthEast)),
        ] {
            assert_eq!(
                resize_direction(window, position).map(|hit| hit.0),
                expected
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the sidebar and the Lyrics panel open the minimum is wider than
    /// a portrait monitor; it stops at the monitor's width (#783).
    #[test]
    fn the_minimum_width_never_exceeds_the_monitor() {
        let page = 560.0;
        let wide = main_min_width(page, true, true, None);
        assert_eq!(wide, SIDEBAR_MIN_WIDTH + theme::SIDE_PANEL_MIN_WIDTH + page);
        assert!(wide > 800.0, "{wide}");
        assert_eq!(main_min_width(page, true, true, Some(800.0)), 800.0);
        assert_eq!(main_min_width(page, true, true, Some(2560.0)), wide);
        // The usual minimum holds on any monitor wide enough for it.
        assert_eq!(
            main_min_width(page, false, false, Some(1080.0)),
            crate::window::MAIN_MIN_SIZE[0]
        );
        assert_eq!(main_min_width(page, true, true, Some(0.0)), wide);
        assert_eq!(main_min_width(page, true, true, Some(f32::NAN)), wide);
    }

    /// The minimum goes to winit in pixels, so a new scale sends it again,
    /// and so does a window that left full screen with none.
    #[test]
    fn the_minimum_is_sent_again_for_a_new_scale_and_after_full_screen() {
        let root =
            std::env::temp_dir().join(format!("lwspot-min-width-test-{}", std::process::id()));
        let mut app = App::new(
            &crate::backend::Waker::default(),
            crate::paths::AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            crate::settings::Settings::default(),
            crate::app::AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
            },
        );
        app.show_lyrics_panel = true;
        let ctx = Context::default();
        let frame = |app: &App, scale: f32, monitor: f32| {
            let mut input = egui::RawInput::default();
            let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            viewport.native_pixels_per_point = Some(scale);
            viewport.monitor_size = Some(vec2(monitor, 1920.0));
            let mut output = ctx.run_ui(input, |ui| keep_room_for_panels(app, ui.ctx()));
            output.textures_delta.clear();
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .filter_map(|command| match command {
                    egui::ViewportCommand::MinInnerSize(size) => Some(size.x),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let first = frame(&app, 1.0, 1080.0);
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(frame(&app, 1.0, 1080.0).is_empty());
        assert_eq!(frame(&app, 1.5, 720.0), vec![720.0]);
        assert!(frame(&app, 1.5, 720.0).is_empty());
        crate::window::forget_min_size(&ctx);
        assert_eq!(frame(&app, 1.5, 720.0), vec![720.0]);
        app.backend.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }

    /// The header casts a shadow only on a page scrolled under it, and it
    /// is black in both themes, never the page's own colour.
    #[test]
    fn the_header_shadow_appears_once_the_page_scrolls() {
        let ctx = egui::Context::default();
        let page = Rect::from_min_size(egui::pos2(0.0, 80.0), vec2(800.0, 600.0));
        let shadows = |scrolled: f32, dark: bool| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                header_shadow(ui, page, scrolled, dark);
            });
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh.vertices.clone()),
                    _ => None,
                })
                .flatten()
                .collect::<Vec<_>>()
        };
        assert!(
            shadows(0.0, true).is_empty(),
            "nothing at the top of the page"
        );
        for dark in [true, false] {
            let vertices = shadows(40.0, dark);
            let top = vertices
                .iter()
                .find(|vertex| vertex.pos.y == page.top())
                .expect("a shadow along the page's top edge");
            assert!(top.color.a() > 0);
            assert_eq!((top.color.r(), top.color.g(), top.color.b()), (0, 0, 0));
        }
    }
}
