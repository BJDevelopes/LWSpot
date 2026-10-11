//! The theme gallery in Settings and the theme editor.
//!
//! Every theme is shown as a small picture of the app drawn in its own
//! colours. The editor changes the app's colours as its pickers move, and
//! saves the result as a palette file in the themes folder.

use egui::{
    Align, Align2, Color32, CornerRadius, Frame, Id, Layout, Margin, Rect, Sense, Stroke,
    StrokeKind, Vec2, pos2, vec2,
};

use crate::app::App;
use crate::i18n::{Locale, gettext, pgettext};
use crate::model::{Action, Dialog};
use crate::settings::ThemeChoice;
use crate::theme::{self, Icon, Palette};

/// The narrowest a gallery tile gets before a column is dropped.
const TILE_MIN_WIDTH: f32 = 132.0;
const TILE_GAP: f32 = 14.0;
/// A preview's height for its width: about a window's proportions.
const PREVIEW_ASPECT: f32 = 0.64;
const LABEL_HEIGHT: f32 = 24.0;
const PREVIEW_RADIUS: u8 = 6;

/// What a tile shows: one palette, or Follow system's light and dark side
/// by side.
#[derive(Clone, Copy)]
enum Preview {
    One(Palette),
    Split(Palette, Palette),
}

struct Tile {
    name: String,
    preview: Preview,
    selected: bool,
    action: Action,
}

fn tiles(app: &App) -> Vec<Tile> {
    let locale = app.locale;
    let custom = app.settings.custom_theme.as_deref();
    let builtin = |choice: ThemeChoice| Tile {
        name: choice.label(locale).into_owned(),
        preview: match choice {
            ThemeChoice::System => Preview::Split(Palette::light(), Palette::dark()),
            ThemeChoice::Light => Preview::One(Palette::light()),
            ThemeChoice::Dark => Preview::One(Palette::dark()),
        },
        selected: custom.is_none() && app.settings.theme == choice,
        action: Action::SetTheme(choice),
    };
    let mut tiles: Vec<Tile> = ThemeChoice::ALL.into_iter().map(builtin).collect();
    tiles.extend(app.custom_themes.picker_themes().map(|theme| Tile {
        name: fastframe_theme::display_name(&theme.filename).to_owned(),
        preview: Preview::One(theme.palette),
        selected: custom == Some(theme.filename.as_str()),
        action: Action::SetCustomTheme(theme.filename.clone()),
    }));
    // A selected file the folder no longer lists still shows what is in use.
    if let Some(filename) = custom
        && !tiles.iter().any(|tile| tile.selected)
        && let Some(cached) = &app.settings.custom_theme_cache
    {
        tiles.push(Tile {
            name: fastframe_theme::display_name(filename).to_owned(),
            preview: Preview::One(cached.palette),
            selected: true,
            action: Action::SetCustomTheme(filename.to_owned()),
        });
    }
    tiles
}

/// The grid of themes, one click to switch, and the buttons to customize,
/// import and export a theme.
pub(super) fn gallery(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let locale = app.locale;
    let tiles = tiles(app);
    let width = ui.available_width();
    let columns = (((width + TILE_GAP) / (TILE_MIN_WIDTH + TILE_GAP)).floor() as usize).max(1);
    let tile_width = (width - TILE_GAP * (columns - 1) as f32) / columns as f32;
    let preview_height = (tile_width * PREVIEW_ASPECT).round();
    ui.add_space(10.0);
    for row in tiles.chunks(columns) {
        let (row_rect, _) = ui.allocate_exact_size(
            vec2(width, preview_height + 6.0 + LABEL_HEIGHT),
            Sense::hover(),
        );
        for (index, tile) in row.iter().enumerate() {
            let left = row_rect.left() + index as f32 * (tile_width + TILE_GAP);
            let rect = Rect::from_min_size(
                pos2(left, row_rect.top()),
                vec2(tile_width, preview_height + 6.0 + LABEL_HEIGHT),
            );
            if tile_ui(ui, &palette, rect, preview_height, tile).clicked() && !tile.selected {
                app.actions.push(tile.action.clone());
            }
        }
        ui.add_space(TILE_GAP - ui.spacing().item_spacing.y);
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Palette),
            &gettext(locale, "Customize"),
            false,
        )
        .on_hover_text(gettext(
            locale,
            "Change this theme's colours and save it as a new theme.",
        ))
        .clicked()
        {
            app.actions.push(Action::OpenThemeEditor);
        }
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Download),
            &gettext(locale, "Import theme"),
            false,
        )
        .clicked()
        {
            app.actions.push(Action::ImportTheme);
        }
        if theme::soft_button(
            ui,
            &palette,
            Some(Icon::Upload),
            &gettext(locale, "Export theme"),
            false,
        )
        .clicked()
        {
            app.actions.push(Action::ExportTheme);
        }
    });
}

/// One tile: the picture, a ring when it is the theme in use, and its name.
fn tile_ui(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    preview_height: f32,
    tile: &Tile,
) -> egui::Response {
    let response = ui.interact(
        rect,
        ui.id().with(("theme-tile", &tile.name)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::RadioButton,
            ui.is_enabled(),
            tile.selected,
            &tile.name,
        )
    });
    if ui.is_rect_visible(rect) {
        let preview = Rect::from_min_size(rect.min, vec2(rect.width(), preview_height));
        let radius = f32::from(PREVIEW_RADIUS);
        let painter = ui.painter();
        match tile.preview {
            Preview::One(colors) => paint_preview(painter, preview, &colors),
            Preview::Split(left, right) => {
                let middle = preview.center().x;
                paint_preview(
                    &painter.with_clip_rect(preview.with_max_x(middle)),
                    preview,
                    &left,
                );
                paint_preview(
                    &painter.with_clip_rect(preview.with_min_x(middle)),
                    preview,
                    &right,
                );
            }
        }
        if tile.selected {
            painter.rect_stroke(
                preview.expand(3.0),
                radius + 3.0,
                Stroke::new(2.0, palette.accent),
                StrokeKind::Outside,
            );
            // A tick in the corner, so the choice does not rest on colour.
            let center = pos2(preview.right() - 12.0, preview.top() + 12.0);
            painter.circle_filled(center, 9.0, palette.accent);
            painter.circle_stroke(center, 9.0, Stroke::new(1.5, palette.window));
            theme::paint_icon(
                ui,
                Icon::Check,
                Rect::from_center_size(center, Vec2::splat(12.0)),
                12.0,
                palette.on_accent,
            );
        } else if response.hovered() {
            painter.rect_stroke(
                preview.expand(3.0),
                radius + 3.0,
                Stroke::new(1.5, palette.dim),
                StrokeKind::Outside,
            );
        }
        let font = if tile.selected {
            theme::semibold(13.0)
        } else {
            theme::medium(13.0)
        };
        let color = if tile.selected || response.hovered() {
            palette.text
        } else {
            palette.secondary
        };
        let galley = crate::bidi::layout(
            painter,
            &tile.name,
            font,
            color,
            rect.width(),
            1,
            Some(crate::bidi::ELLIPSIS),
        );
        let at = pos2(
            rect.center().x - galley.size().x / 2.0,
            preview.bottom() + 6.0 + (LABEL_HEIGHT - galley.size().y) / 2.0,
        );
        painter.galley(at, galley, color);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(6.0),
            f32::from(PREVIEW_RADIUS) + 6.0,
            ui.visuals().selection.stroke,
            StrokeKind::Outside,
        );
    }
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    response
}

/// A small picture of the app in `colors`: the window, the library panel
/// with its rows, a page heading and two cards, and the player bar with
/// its accent play button and progress.
fn paint_preview(painter: &egui::Painter, rect: Rect, colors: &Palette) {
    let r = PREVIEW_RADIUS;
    let (w, h) = (rect.width(), rect.height());
    let line = |x: f32, y: f32, width: f32, height: f32, color: Color32| {
        painter.rect_filled(
            Rect::from_min_size(pos2(x, y), vec2(width.max(0.0), height)),
            height / 2.0,
            color,
        );
    };
    painter.rect_filled(rect, r, colors.window);
    let bar_height = (h * 0.22).round();
    let side_width = (w * 0.27).round();
    let bar_top = rect.bottom() - bar_height;
    // The library panel.
    let side = Rect::from_min_max(rect.min, pos2(rect.left() + side_width, bar_top));
    painter.rect_filled(
        side,
        CornerRadius {
            nw: r,
            ..CornerRadius::ZERO
        },
        colors.panel,
    );
    let unit = (h / 14.0).clamp(4.0, 8.0);
    for index in 0..3 {
        let y = side.top() + unit * 1.4 + index as f32 * unit * 2.2;
        painter.rect_filled(
            Rect::from_min_size(pos2(side.left() + unit, y), Vec2::splat(unit * 1.4)),
            2.0,
            colors.surface,
        );
        let text = if index == 0 {
            colors.text
        } else {
            colors.secondary
        };
        line(
            side.left() + unit * 3.0,
            y + unit * 0.45,
            side_width - unit * 4.0,
            unit * 0.5,
            text,
        );
    }
    // The page: a heading and two cards, the first with its play button.
    let left = side.right() + unit * 1.4;
    let right = rect.right() - unit * 1.4;
    let main_width = right - left;
    line(
        left,
        rect.top() + unit * 1.4,
        main_width * 0.5,
        unit * 0.8,
        colors.text,
    );
    line(
        left,
        rect.top() + unit * 2.9,
        main_width * 0.32,
        unit * 0.5,
        colors.secondary,
    );
    let cards_top = rect.top() + unit * 4.4;
    let cards_bottom = bar_top - unit * 1.2;
    let card_width = (main_width - unit) / 2.0;
    for index in 0..2 {
        let card = Rect::from_min_max(
            pos2(left + index as f32 * (card_width + unit), cards_top),
            pos2(
                left + index as f32 * (card_width + unit) + card_width,
                cards_bottom,
            ),
        );
        if card.height() < unit {
            break;
        }
        painter.rect_filled(card, 3.0, colors.surface);
        line(
            card.left() + unit * 0.7,
            card.bottom() - unit * 1.3,
            card.width() * 0.5,
            unit * 0.45,
            if index == 0 { colors.text } else { colors.dim },
        );
        if index == 0 {
            painter.circle_filled(
                pos2(card.right() - unit * 1.3, card.top() + unit * 1.3),
                unit * 0.75,
                colors.accent,
            );
        }
    }
    // The player bar.
    let bar = Rect::from_min_max(pos2(rect.left(), bar_top), rect.max);
    painter.rect_filled(
        bar,
        CornerRadius {
            sw: r,
            se: r,
            ..CornerRadius::ZERO
        },
        colors.panel,
    );
    painter.hline(bar.x_range(), bar.top(), Stroke::new(1.0, colors.outline));
    let middle = bar.center().y;
    let cover = bar_height * 0.5;
    painter.rect_filled(
        Rect::from_center_size(
            pos2(bar.left() + unit + cover / 2.0, middle),
            Vec2::splat(cover),
        ),
        2.0,
        colors.surface,
    );
    line(
        bar.left() + unit * 1.6 + cover,
        middle - unit * 0.25,
        w * 0.14,
        unit * 0.5,
        colors.text,
    );
    let play = pos2(bar.center().x + w * 0.06, middle - bar_height * 0.08);
    painter.circle_filled(play, bar_height * 0.22, colors.accent);
    painter.circle_filled(play, bar_height * 0.07, colors.on_accent);
    let track_left = play.x - w * 0.18;
    let track_width = w * 0.36;
    let track_y = bar.bottom() - bar_height * 0.2;
    line(track_left, track_y, track_width, 2.0, colors.surface_active);
    line(track_left, track_y, track_width * 0.4, 2.0, colors.accent);
    painter.rect_stroke(
        rect,
        r,
        Stroke::new(1.0, colors.outline),
        StrokeKind::Inside,
    );
}

/// The name each palette colour goes by in the editor.
fn color_label(locale: Locale, name: &str) -> String {
    match name {
        "window" => gettext(locale, "Window"),
        "panel" => gettext(locale, "Panels"),
        "surface" => gettext(locale, "Surfaces"),
        "surface_hover" => gettext(locale, "Surfaces on hover"),
        "surface_active" => gettext(locale, "Surfaces when pressed"),
        "outline" => gettext(locale, "Outlines"),
        "text" => gettext(locale, "Text"),
        "secondary" => gettext(locale, "Secondary text"),
        "dim" => gettext(locale, "Dim text"),
        "accent" => gettext(locale, "Accent"),
        "accent_hover" => gettext(locale, "Accent on hover"),
        "on_accent" => gettext(locale, "Text on accent"),
        "danger" => gettext(locale, "Danger"),
        "warning" => gettext(locale, "Warning"),
        "overlay" => gettext(locale, "Menus and dialogs"),
        "shadow" => gettext(locale, "Shadow"),
        other => other.to_owned().into(),
    }
    .into_owned()
}

/// `#rrggbb`, with the alpha after it when the colour is see-through.
fn hex(color: Color32) -> String {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// The contrast problems worth a warning in the editor, as sentences.
pub(crate) fn contrast_warnings(locale: Locale, colors: &Palette) -> Vec<String> {
    use crate::accent::{ACCENT_CONTRAST, TEXT_CONTRAST, contrast_ratio};
    let mut warnings = Vec::new();
    let text =
        contrast_ratio(colors.text, colors.window).min(contrast_ratio(colors.text, colors.panel));
    if text < TEXT_CONTRAST {
        warnings.push(
            // Translators: {ratio} is a contrast ratio such as 3.2 (read as 3.2:1).
            gettext(
                locale,
                "Text is hard to read on the window or panels ({ratio}:1). Aim for at least 4.5:1.",
            )
            .replace("{ratio}", &format!("{text:.1}")),
        );
    }
    // Labels on the accent are short and bold, as on buttons, so they are
    // held to the ratio for large text.
    let on_accent = contrast_ratio(colors.on_accent, colors.accent);
    if on_accent < ACCENT_CONTRAST {
        warnings.push(
            // Translators: {ratio} is a contrast ratio such as 2.1 (read as 2.1:1).
            gettext(
                locale,
                "Text on the accent is hard to read ({ratio}:1). Aim for at least 3:1.",
            )
            .replace("{ratio}", &format!("{on_accent:.1}")),
        );
    }
    warnings
}

const EDITOR_WIDTH: f32 = 600.0;
const LIST_WIDTH: f32 = 272.0;
const ROW_HEIGHT: f32 = 30.0;
const ROW_GAP: f32 = 2.0;

/// The theme editor: the palette's colours on the left, a picker for the
/// one chosen on the right, and the whole app drawn in the result while it
/// is open. It sits to the right so the app shows beside it.
pub(super) fn editor(app: &mut App, ctx: &egui::Context) {
    let palette = app.palette;
    let locale = app.locale;
    let Some(Dialog::ThemeEditor(draft)) = &app.dialog else {
        return;
    };
    let mut draft = (**draft).clone();
    let before = draft.clone();
    let mut actions = Vec::new();
    let frame = Frame::new()
        .fill(palette.overlay)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(theme::RADIUS + 4))
        .inner_margin(Margin::same(24))
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 40,
            spread: 0,
            color: palette.shadow,
        });
    let id = Id::new("theme-editor");
    let response = egui::Modal::new(id)
        .area(egui::Modal::default_area(id).anchor(Align2::RIGHT_CENTER, vec2(-24.0, 0.0)))
        .frame(frame)
        // Light enough that the app behind shows the colours being chosen.
        .backdrop_color(Color32::from_black_alpha(if palette.dark {
            50
        } else {
            20
        }))
        .show(ctx, |ui| {
            ui.set_width(EDITOR_WIDTH.min(ctx.content_rect().width() - 96.0));
            theme::text(
                ui,
                gettext(locale, "Customize theme"),
                theme::bold(20.0),
                palette.text,
            );
            ui.add_space(14.0);
            name_and_base(ui, &palette, locale, &mut draft);
            ui.add_space(14.0);
            ui.horizontal_top(|ui| {
                let room = ctx.content_rect().height() - 330.0;
                ui.vertical(|ui| {
                    ui.set_width(LIST_WIDTH);
                    crate::autoscroll::show(
                        ui,
                        egui::ScrollArea::vertical()
                            .id_salt("theme-editor-colors")
                            .max_height(room.clamp(150.0, (ROW_HEIGHT + ROW_GAP) * 16.0))
                            .auto_shrink([false, true]),
                        egui::Vec2b::new(false, true),
                        |ui| color_list(ui, &palette, locale, &mut draft),
                    );
                });
                ui.add_space(16.0);
                ui.vertical(|ui| picker(ui, &palette, locale, &mut draft));
            });
            let warnings = contrast_warnings(locale, &draft.palette);
            if !warnings.is_empty() {
                ui.add_space(12.0);
            }
            for warning in &warnings {
                note(
                    ui,
                    Icon::CircleAlert,
                    palette.warning,
                    palette.secondary,
                    warning,
                );
            }
            if let Some(error) = &draft.error {
                ui.add_space(8.0);
                note(ui, Icon::CircleX, palette.danger, palette.danger, error);
            }
            if let Some(filename) = &draft.replace {
                ui.add_space(8.0);
                note(
                    ui,
                    Icon::Info,
                    palette.secondary,
                    palette.secondary,
                    // Translators: {name} is the name of a theme.
                    &gettext(
                        locale,
                        "A theme called “{name}” already exists. Replace it?",
                    )
                    .replace("{name}", fastframe_theme::display_name(filename)),
                );
            }
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!draft.saving, |ui| {
                    let (label, replace) = if draft.replace.is_some() {
                        (gettext(locale, "Replace"), true)
                    } else {
                        (gettext(locale, "Save"), false)
                    };
                    if theme::pill_button(ui, &palette, &label, true).clicked() {
                        actions.push(Action::SaveTheme { replace });
                    }
                });
                if theme::pill_button(ui, &palette, &gettext(locale, "Cancel"), false).clicked() {
                    actions.push(Action::CloseDialog);
                }
            });
        });
    app.dialog_rect = Some(response.response.rect);
    if draft.name != before.name {
        draft.replace = None;
        draft.error = None;
    }
    if let Some(Dialog::ThemeEditor(current)) = &mut app.dialog {
        **current = draft;
    }
    app.actions.extend(actions);
}

/// An icon and a wrapped sentence, for warnings and errors.
fn note(ui: &mut egui::Ui, icon: Icon, icon_color: Color32, color: Color32, text: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        theme::icon(ui, icon, 16.0, icon_color);
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .font(theme::regular(13.0))
                    .color(color),
            )
            .wrap(),
        );
    });
}

/// The name field and the base (light or dark controls).
fn name_and_base(
    ui: &mut egui::Ui,
    palette: &Palette,
    locale: Locale,
    draft: &mut crate::theme_files::Draft,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let dark = pgettext(locale, "theme", "Dark");
        let light = pgettext(locale, "theme", "Light");
        let base = gettext(locale, "Base");
        let measure = |ui: &egui::Ui, text: &str| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), theme::medium(13.0), palette.secondary)
                .size()
                .x
        };
        let base_width = measure(ui, &base)
            + theme::soft_button_width(ui, &dark)
            + theme::soft_button_width(ui, &light)
            + 30.0;
        let label = theme::text(
            ui,
            pgettext(locale, "theme", "Name"),
            theme::medium(13.0),
            palette.secondary,
        );
        let field_width = (ui.available_width() - base_width).max(120.0);
        ui.allocate_ui(vec2(field_width, 36.0), |ui| {
            super::dialogs::text_field(
                ui,
                palette,
                locale,
                "theme-editor-name",
                &mut draft.name,
                &gettext(locale, "Theme name"),
                false,
            )
            .labelled_by(label.id);
        });
        ui.add_space(12.0);
        theme::text(ui, base, theme::medium(13.0), palette.secondary);
        for (is_dark, label) in [(true, &dark), (false, &light)] {
            if theme::soft_button(ui, palette, None, label, draft.palette.dark == is_dark).clicked()
            {
                draft.palette.dark = is_dark;
            }
        }
    });
}

/// The sixteen colours, each a row with its swatch and value; the chosen
/// one is shown in the picker.
fn color_list(
    ui: &mut egui::Ui,
    palette: &Palette,
    locale: Locale,
    draft: &mut crate::theme_files::Draft,
) {
    ui.spacing_mut().item_spacing.y = ROW_GAP;
    for (index, name) in fastframe_theme::BASE_COLORS.iter().enumerate() {
        let label = color_label(locale, name);
        let color = draft.palette.get(name).unwrap_or_default();
        let selected = draft.selected == index;
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                ui.is_enabled(),
                selected,
                &label,
            )
        });
        if response.clicked() {
            draft.selected = index;
        }
        if ui.is_rect_visible(rect) {
            let fill = if selected {
                palette.surface_active
            } else if response.hovered() {
                palette.surface_hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter()
                .rect_filled(rect, theme::RADIUS_SMALL + 2, fill);
            let swatch = Rect::from_center_size(
                pos2(rect.left() + 8.0 + 9.0, rect.center().y),
                Vec2::splat(18.0),
            );
            paint_swatch(ui.painter(), swatch, color, palette);
            let value = ui
                .painter()
                .layout_no_wrap(hex(color), theme::regular(12.0), palette.dim);
            let value_at = pos2(
                rect.right() - 8.0 - value.size().x,
                rect.center().y - value.size().y / 2.0,
            );
            let text_left = swatch.right() + 10.0;
            let galley = crate::bidi::layout(
                ui.painter(),
                &label,
                theme::medium(13.0),
                palette.text,
                (value_at.x - text_left - 6.0).max(0.0),
                1,
                Some(crate::bidi::ELLIPSIS),
            );
            ui.painter().galley(
                pos2(text_left, rect.center().y - galley.size().y / 2.0),
                galley,
                palette.text,
            );
            ui.painter().galley(value_at, value, palette.dim);
        }
        theme::focus_ring(ui, &response);
    }
}

/// A rounded colour sample on a checkerboard, so see-through colours show
/// as such.
fn paint_swatch(painter: &egui::Painter, rect: Rect, color: Color32, palette: &Palette) {
    let radius = 4.0;
    if color.a() < 255 {
        painter.rect_filled(rect, radius, Color32::WHITE);
        let half = rect.width() / 2.0;
        for (x, y) in [(0.0, 0.0), (half, half)] {
            painter.rect_filled(
                Rect::from_min_size(rect.min + vec2(x, y), Vec2::splat(half)),
                0.0,
                Color32::from_gray(200),
            );
        }
    }
    painter.rect_filled(rect, radius, color);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, palette.outline),
        StrokeKind::Inside,
    );
}

/// The picker for the chosen colour, with its value as text to type or
/// paste.
fn picker(
    ui: &mut egui::Ui,
    palette: &Palette,
    locale: Locale,
    draft: &mut crate::theme_files::Draft,
) {
    let names = fastframe_theme::BASE_COLORS;
    let index = draft.selected.min(names.len() - 1);
    let name = names[index];
    let Some(mut color) = draft.palette.get(name) else {
        return;
    };
    let title = theme::text(
        ui,
        color_label(locale, name),
        theme::semibold(15.0),
        palette.text,
    );
    ui.add_space(8.0);
    // Only the shadow is see-through by design.
    let alpha = if name == "shadow" {
        egui::color_picker::Alpha::OnlyBlend
    } else {
        egui::color_picker::Alpha::Opaque
    };
    ui.spacing_mut().slider_width = (ui.available_width() - 24.0).clamp(160.0, 300.0);
    let mut changed = egui::color_picker::color_picker_color32(ui, &mut color, alpha);
    ui.add_space(8.0);
    let text_id = Id::new(("theme-editor-hex", index));
    let mut text = ui
        .data(|data| data.get_temp::<String>(text_id))
        .unwrap_or_else(|| hex(color));
    let response = Frame::new()
        .fill(palette.surface)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            super::widgets::text_edit(
                ui,
                locale,
                egui::TextEdit::singleline(&mut text)
                    .id(Id::new(("theme-editor-hex-field", index)))
                    .font(theme::regular(13.0))
                    .frame(Frame::NONE)
                    .char_limit(9)
                    .desired_width(110.0),
            )
        })
        .inner;
    let response = response.labelled_by(title.id);
    if response.changed() {
        let typed = text.trim();
        let typed = if typed.starts_with('#') {
            typed.to_owned()
        } else {
            format!("#{typed}")
        };
        if let Some(parsed) = fastframe_theme::parse_color(&typed) {
            color = if alpha == egui::color_picker::Alpha::Opaque {
                Color32::from_rgb(parsed.r(), parsed.g(), parsed.b())
            } else {
                parsed
            };
            changed = true;
        }
    }
    if !response.has_focus() {
        text = hex(color);
    }
    ui.data_mut(|data| data.insert_temp(text_id, text));
    if changed {
        use fastframe_theme::Palette as _;
        draft.palette.set(name, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The editor warns about faint text, and about text on the accent,
    /// but not about the built-in themes.
    #[test]
    fn the_editor_warns_only_about_hard_to_read_text() {
        for palette in [Palette::dark(), Palette::light()] {
            assert!(
                contrast_warnings(Locale::English, &palette).is_empty(),
                "{palette:?}"
            );
        }
        let mut faded = Palette::light();
        faded.text = Color32::from_rgb(0xb0, 0xb4, 0xba);
        let warnings = contrast_warnings(Locale::English, &faded);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("(2.0:1)"), "{}", warnings[0]);
        faded.on_accent = faded.accent;
        assert_eq!(contrast_warnings(Locale::English, &faded).len(), 2);
    }

    #[test]
    fn colour_values_read_as_hex() {
        assert_eq!(hex(Color32::from_rgb(0x1e, 0xd7, 0x60)), "#1ed760");
        assert_eq!(hex(Color32::from_black_alpha(140)), "#0000008c");
    }
}
