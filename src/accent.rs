//! Accent colours from outside the theme: the Windows accent colour and the
//! playing song's cover, made legible against the theme they sit in.
//!
//! Everything here but the registry read is a pure function of colours, so
//! the contrast rules are tested without a window.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::Color32;

use crate::theme::Palette;

/// The least contrast the accent keeps against the window and the panels,
/// WCAG's ratio for large text and interface components.
pub const ACCENT_CONTRAST: f32 = 3.0;
/// The least contrast text on the accent keeps, WCAG's ratio for body text.
pub const TEXT_CONTRAST: f32 = 4.5;
/// How long the accent takes to move to a new song's colour.
pub const FADE_SECONDS: f32 = 0.6;
/// Covers greyer than this keep the theme's accent: a grey accent reads as
/// a disabled control.
const LEAST_SATURATION: f32 = 0.15;
/// The dark text a light accent carries when the theme's own does not read.
const DARK_ON_ACCENT: Color32 = Color32::from_rgb(0x0a, 0x0c, 0x0e);

/// WCAG relative luminance of an sRGB colour, ignoring alpha.
pub fn relative_luminance(color: Color32) -> f32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    let linear = |channel: u8| {
        let c = f32::from(channel) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// The WCAG contrast ratio of two colours, from 1 (the same) to 21 (black
/// on white), whichever is lighter.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    let (light, dark) = if a > b { (a, b) } else { (b, a) };
    (light + 0.05) / (dark + 0.05)
}

/// Hue (0..1), saturation and lightness of an sRGB colour.
fn to_hsl(color: Color32) -> [f32; 3] {
    let [r, g, b, _] = color.to_srgba_unmultiplied().map(|c| f32::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta < f32::EPSILON {
        return [0.0, 0.0, lightness];
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    [hue / 6.0, saturation.clamp(0.0, 1.0), lightness]
}

fn from_hsl([hue, saturation, lightness]: [f32; 3]) -> Color32 {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue * 6.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let offset = lightness - chroma / 2.0;
    let channel = |value: f32| ((value + offset).clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(channel(r), channel(g), channel(b))
}

/// The colour with its lightness moved by `step` (negative darkens).
fn shift_lightness(color: Color32, step: f32) -> Color32 {
    let [hue, saturation, lightness] = to_hsl(color);
    from_hsl([hue, saturation, (lightness + step).clamp(0.0, 1.0)])
}

/// The text colour for an accent: the theme's own when it reads, otherwise
/// white or near-black, whichever reads better.
fn text_on(accent: Color32, theme_text: Color32) -> Color32 {
    if contrast_ratio(theme_text, accent) >= TEXT_CONTRAST {
        return theme_text;
    }
    if contrast_ratio(Color32::WHITE, accent) >= contrast_ratio(DARK_ON_ACCENT, accent) {
        Color32::WHITE
    } else {
        DARK_ON_ACCENT
    }
}

/// `base` with its accent taken from `color`: the hue kept, the lightness
/// moved away from the window until the accent stands out from the window
/// and the panels (3:1) and its text reads on it (4.5:1).
pub fn with_accent(base: &Palette, color: Color32) -> Palette {
    let color = Color32::from_rgb(color.r(), color.g(), color.b());
    let least_against_ground = |accent: Color32| {
        contrast_ratio(accent, base.window).min(contrast_ratio(accent, base.panel))
    };
    // Lighter on a dark window, darker on a light one. Below this
    // luminance, white contrasts more than black does.
    let step = if relative_luminance(base.window) < 0.18 {
        0.01
    } else {
        -0.01
    };
    let mut accent = color;
    for _ in 0..100 {
        let text = text_on(accent, base.on_accent);
        if least_against_ground(accent) >= ACCENT_CONTRAST
            && contrast_ratio(text, accent) >= TEXT_CONTRAST
        {
            break;
        }
        let next = shift_lightness(accent, step);
        if next == accent {
            break;
        }
        accent = next;
    }
    let on_accent = text_on(accent, base.on_accent);
    // Hover moves away from the text, so the label only gets clearer.
    let hover_step = if relative_luminance(on_accent) > relative_luminance(accent) {
        -0.06
    } else {
        0.06
    };
    Palette {
        accent,
        accent_hover: shift_lightness(accent, hover_step),
        on_accent,
        ..*base
    }
}

/// The accent a cover's colour gives, or `None` for a cover too grey to
/// give one (the theme's accent stays).
pub fn from_art(base: &Palette, rgb: [u8; 3]) -> Option<Palette> {
    let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    (to_hsl(color)[1] >= LEAST_SATURATION).then(|| with_accent(base, color))
}

/// Reads the `AccentColor` DWORD Windows stores as 0xAABBGGRR.
pub fn from_abgr(value: u32) -> Color32 {
    let [r, g, b, _] = value.to_le_bytes();
    Color32::from_rgb(r, g, b)
}

/// The accent colour chosen in System Settings > Appearance, as AppKit
/// resolves it (the multicolour setting gives the system blue).
#[cfg(target_os = "macos")]
pub fn windows_accent() -> Option<Color32> {
    use objc2_app_kit::{NSColor, NSColorSpace};
    let color =
        NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(Color32::from_rgb(
        channel(color.redComponent()),
        channel(color.greenComponent()),
        channel(color.blueComponent()),
    ))
}

/// The accent colour chosen in Windows Settings, from
/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor`.
#[cfg(windows)]
pub fn windows_accent() -> Option<Color32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let mut value: u32 = 0;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: the key and value names are static wide strings, and the
    // output points at a DWORD whose size is passed alongside.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows_sys::w!("Software\\Microsoft\\Windows\\DWM"),
            windows_sys::w!("AccentColor"),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut value).cast(),
            &mut bytes,
        )
    };
    (read == 0 && bytes == 4).then(|| from_abgr(value))
}

static WINDOWS_ACCENT_CHANGED: AtomicBool = AtomicBool::new(false);

/// Notes that Windows may have changed its accent colour; called from the
/// main window's procedure on a colour or setting change.
pub fn note_windows_change() {
    WINDOWS_ACCENT_CHANGED.store(true, Ordering::Relaxed);
}

/// Whether Windows announced a colour change since the last call.
pub fn take_windows_change() -> bool {
    WINDOWS_ACCENT_CHANGED.swap(false, Ordering::Relaxed)
}

/// A colour `t` of the way from `from` to `to`, channel by channel.
fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let [r0, g0, b0, a0] = from.to_srgba_unmultiplied();
    let [r1, g1, b1, a1] = to.to_srgba_unmultiplied();
    let channel = |a: u8, b: u8| egui::lerp(f32::from(a)..=f32::from(b), t).round() as u8;
    Color32::from_rgba_unmultiplied(
        channel(r0, r1),
        channel(g0, g1),
        channel(b0, b1),
        channel(a0, a1),
    )
}

/// Moves the shown accent to a new target over [`FADE_SECONDS`], so a new
/// song's colour arrives as a short blend rather than a jump.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fade {
    /// The accent, hover and text colours on screen.
    shown: Option<[Color32; 3]>,
    /// Where they are going, from where, and since when (egui's clock).
    moving: Option<([Color32; 3], [Color32; 3], f64)>,
}

impl Fade {
    /// The colours to show at `now` for `target`, and whether they are still
    /// moving. `snap` jumps straight there (a theme change, which brings its
    /// own transition).
    pub fn step(&mut self, target: &Palette, now: f64, snap: bool) -> (Palette, bool) {
        let goal = [target.accent, target.accent_hover, target.on_accent];
        let shown = match self.shown {
            Some(shown) if !snap => shown,
            _ => {
                self.shown = Some(goal);
                self.moving = None;
                return (*target, false);
            }
        };
        match self.moving {
            Some((_, to, _)) if to == goal => {}
            _ if shown == goal => return (*target, false),
            _ => self.moving = Some((shown, goal, now)),
        }
        let Some((from, to, start)) = self.moving else {
            return (*target, false);
        };
        let t = (((now - start) as f32) / FADE_SECONDS).clamp(0.0, 1.0);
        // Ease out: most of the change in the first moments.
        let eased = 1.0 - (1.0 - t).powi(3);
        let colors = [0, 1, 2].map(|index| mix(from[index], to[index], eased));
        self.shown = Some(colors);
        if t >= 1.0 {
            self.moving = None;
        }
        (
            Palette {
                accent: colors[0],
                accent_hover: colors[1],
                on_accent: colors[2],
                ..*target
            },
            t < 1.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_follows_the_wcag_formula() {
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
        // #767676 on white is the classic 4.54:1.
        let grey = Color32::from_rgb(0x76, 0x76, 0x76);
        assert!((contrast_ratio(grey, Color32::WHITE) - 4.54).abs() < 0.01);
        assert_eq!(
            contrast_ratio(grey, Color32::WHITE),
            contrast_ratio(Color32::WHITE, grey)
        );
        assert!((relative_luminance(Color32::WHITE) - 1.0).abs() < 0.001);
    }

    #[test]
    fn hsl_round_trips() {
        for color in [
            Color32::from_rgb(0x1e, 0xd7, 0x60),
            Color32::from_rgb(0x89, 0xb4, 0xfa),
            Color32::from_rgb(0xd2, 0x0f, 0x39),
            Color32::from_rgb(0x80, 0x80, 0x80),
        ] {
            let back = from_hsl(to_hsl(color));
            for (a, b) in back.to_array().into_iter().zip(color.to_array()) {
                assert!(a.abs_diff(b) <= 1, "{color:?} came back as {back:?}");
            }
        }
    }

    /// Whatever colour comes in, the accent stands out from the window and
    /// panels, and its text reads on it.
    #[test]
    fn derived_accents_are_always_legible() {
        let mut themes = vec![Palette::dark(), Palette::light()];
        themes.extend(fastframe_theme::presets::themes::<Palette>().map(|theme| theme.palette));
        for base in themes {
            for rgb in [
                [0, 0, 0],
                [255, 255, 255],
                [255, 255, 0],
                [0, 0, 255],
                [20, 30, 120],
                [230, 220, 90],
                [0, 120, 215],
                [128, 128, 128],
                [200, 40, 40],
            ] {
                let palette = with_accent(&base, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                assert!(
                    contrast_ratio(palette.accent, palette.window) >= ACCENT_CONTRAST,
                    "{rgb:?} on {:?}",
                    base.window
                );
                assert!(contrast_ratio(palette.accent, palette.panel) >= ACCENT_CONTRAST);
                assert!(
                    contrast_ratio(palette.on_accent, palette.accent) >= TEXT_CONTRAST,
                    "{rgb:?}: {:?} on {:?}",
                    palette.on_accent,
                    palette.accent
                );
                // Only the accent colours change.
                assert_eq!(
                    Palette {
                        accent: base.accent,
                        accent_hover: base.accent_hover,
                        on_accent: base.on_accent,
                        ..palette
                    },
                    base
                );
            }
        }
    }

    #[test]
    fn a_legible_colour_keeps_its_value_and_hue() {
        // Windows' default blue already reads on the light theme.
        let blue = Color32::from_rgb(0x00, 0x5a, 0x9e);
        let palette = with_accent(&Palette::light(), blue);
        assert_eq!(palette.accent, blue);
        // On the dark theme it is lightened, not recoloured.
        let palette = with_accent(&Palette::dark(), blue);
        assert_ne!(palette.accent, blue);
        let (hue, original) = (to_hsl(palette.accent)[0], to_hsl(blue)[0]);
        assert!((hue - original).abs() < 0.02, "{hue} vs {original}");
        assert!(relative_luminance(palette.accent) > relative_luminance(blue));
    }

    #[test]
    fn grey_covers_keep_the_theme_accent() {
        assert!(from_art(&Palette::dark(), [120, 120, 124]).is_none());
        let red = from_art(&Palette::dark(), [200, 30, 40]).unwrap();
        assert!(red.accent.r() > red.accent.g());
    }

    #[test]
    fn the_windows_accent_is_read_as_abgr() {
        // Windows' default accent, #0078D7, as the registry stores it.
        assert_eq!(from_abgr(0xffd7_7800), Color32::from_rgb(0x00, 0x78, 0xd7));
        assert_eq!(from_abgr(0x0011_2233), Color32::from_rgb(0x33, 0x22, 0x11));
    }

    #[test]
    fn the_accent_fades_to_a_new_song_and_snaps_on_a_theme_change() {
        let base = Palette::dark();
        let red = with_accent(&base, Color32::from_rgb(220, 40, 40));
        let blue = with_accent(&base, Color32::from_rgb(40, 80, 220));
        let mut fade = Fade::default();
        assert_eq!(fade.step(&red, 0.0, false), (red, false), "first at once");
        let (halfway, moving) = fade.step(&blue, 1.0, false);
        assert!(moving);
        assert_eq!(halfway.accent, red.accent, "starts where it was");
        let (later, moving) = fade.step(&blue, 1.0 + f64::from(FADE_SECONDS) / 2.0, false);
        assert!(moving);
        assert!(later.accent != red.accent && later.accent != blue.accent);
        let (done, moving) = fade.step(&blue, 1.0 + f64::from(FADE_SECONDS), false);
        assert!(!moving);
        assert_eq!(done, blue);
        assert_eq!(fade.step(&red, 5.0, true), (red, false), "snapped");
    }

    #[test]
    fn a_change_of_heart_midway_starts_from_what_is_shown() {
        let base = Palette::light();
        let red = with_accent(&base, Color32::from_rgb(220, 40, 40));
        let blue = with_accent(&base, Color32::from_rgb(40, 80, 220));
        let mut fade = Fade::default();
        fade.step(&red, 0.0, false);
        fade.step(&blue, 0.0, false);
        let (midway, _) = fade.step(&blue, 0.2, false);
        let (turned, moving) = fade.step(&red, 0.2, false);
        assert!(moving);
        assert_eq!(turned.accent, midway.accent, "no jump back");
    }
}
