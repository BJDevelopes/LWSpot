//! Checks saved window positions before restoring them.
//!
//! eframe already places the window on-screen. The session can still refer
//! to a monitor that was unplugged, so check before moving the window there.

/// Says why no window opened on Windows on ARM and offers the fix.
///
/// Snapdragon graphics drivers have no OpenGL of their own. Windows provides
/// it on ARM through Microsoft's compatibility pack, a Store app that
/// translates it to Direct3D 12, so a missing or broken pack leaves eframe
/// without a context and the app would otherwise close without a word.
#[cfg(target_arch = "aarch64")]
pub fn report_missing_opengl(locale: crate::i18n::Locale) {
    use crate::i18n::gettext;
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONERROR, MB_YESNO, MessageBoxW};
    /// The Store page of the OpenCL, OpenGL & Vulkan Compatibility Pack.
    const STORE_PAGE: &str = "ms-windows-store://pdp/?ProductId=9NQPSL29BFFF";

    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let title = wide(&gettext(locale, "LWSpot cannot open its window"));
    let text = wide(&gettext(
        locale,
        "On Windows on ARM, the OpenGL that LWSpot draws with comes from Microsoft's OpenCL, OpenGL & Vulkan Compatibility Pack. Open its Microsoft Store page to install or repair it?",
    ));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call,
    // and the box has no owner window.
    let answer = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONERROR,
        )
    };
    if answer == IDYES
        && let Err(error) = open::that(STORE_PAGE)
    {
        log::warn!("cannot open the Microsoft Store: {error}");
    }
}

/// Minimize the active window.
pub fn minimize_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
}

/// Whether the window already covers the screen, maximized or full screen.
///
/// eframe restores that state as it creates the window, and sizing or moving
/// the window afterwards restores it down again.
pub fn fills_the_screen(viewport: &egui::ViewportInfo) -> bool {
    viewport.maximized.unwrap_or(false) || viewport.fullscreen.unwrap_or(false)
}

fn titlebar_anchor(pos: [f32; 2], pixels_per_point: f32) -> Option<egui::Pos2> {
    // A maximized window can start at (-8, -8), so check a point inside
    // its title bar. Convert from egui points to pixels for Win32.
    let anchor = (egui::pos2(pos[0], pos[1]) + egui::vec2(32.0, 16.0)) * pixels_per_point;
    (pixels_per_point.is_finite() && pixels_per_point > 0.0 && anchor.is_finite()).then_some(anchor)
}

/// Checks that the saved position leaves the title bar in a monitor's work area.
pub fn can_restore(pos: [f32; 2], pixels_per_point: f32) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromPoint,
    };

    let Some(anchor) = titlebar_anchor(pos, pixels_per_point) else {
        return false;
    };
    let point = POINT {
        x: anchor.x as i32,
        y: anchor.y as i32,
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        // Asking for the nearest monitor would also accept off-screen positions.
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONULL);
        if monitor.is_null() || GetMonitorInfoW(monitor, &mut info) == 0 {
            return false;
        }
    }
    // Use the work area so the title bar cannot end up behind the taskbar.
    let area = info.rcWork;
    egui::Rect::from_min_max(
        egui::pos2(area.left as f32, area.top as f32),
        egui::pos2(area.right as f32, area.bottom as f32),
    )
    .contains(anchor)
}

/// The work area of `monitor`, in physical pixels.
fn work_area(monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR) -> Option<egui::Rect> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};

    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let area = info.rcWork;
    Some(egui::Rect::from_min_max(
        egui::pos2(area.left as f32, area.top as f32),
        egui::pos2(area.right as f32, area.bottom as f32),
    ))
}

/// The work area nearest `rect` and every monitor's, in physical pixels.
fn work_areas(rect: egui::Rect) -> Option<(egui::Rect, Vec<egui::Rect>)> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromRect,
    };

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        areas: LPARAM,
    ) -> windows_sys::core::BOOL {
        // SAFETY: `areas` is the vector passed below, borrowed for the call.
        let areas = unsafe { &mut *(areas as *mut Vec<egui::Rect>) };
        areas.extend(work_area(monitor));
        1
    }

    if !(rect.is_finite() && rect.is_positive()) {
        return None;
    }
    let mut areas = Vec::new();
    let bounds = RECT {
        left: rect.min.x.round() as i32,
        top: rect.min.y.round() as i32,
        right: rect.max.x.round() as i32,
        bottom: rect.max.y.round() as i32,
    };
    let nearest = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut areas as *mut Vec<egui::Rect> as LPARAM,
        );
        work_area(MonitorFromRect(&bounds, MONITOR_DEFAULTTONEAREST))?
    };
    (!areas.is_empty()).then_some((nearest, areas))
}

/// What [`keep_usable`] changes about a window, in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Correction {
    pub inner_size: Option<egui::Vec2>,
    pub outer_pos: Option<egui::Pos2>,
}

/// Why a window is being looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// It was just created, or the displays or its scale changed. A window
    /// mostly outside every work area, or whose title bar is, comes back.
    Display,
    /// It only moved or resized. The listener may have parked it half off
    /// the screen, so it comes back only once none of it is on a work area.
    Moved,
}

/// How far a size or position may be off before it counts, in points: the
/// window's pixels never convert to points exactly.
const SLACK: f32 = 0.5;

/// Keeps a window usable on the work areas there are now.
///
/// winit lets Windows size and place the window without its minimum outside
/// a drag, so removing a display or changing the scale can leave it tiny or
/// off every screen (#704). `outer` is the window with its frame, `inner`
/// the size of its client area, `least` the client area's minimum (`None`
/// to only place the window), and `nearest` the work area nearest it, all
/// in points.
pub fn keep_usable(
    outer: egui::Rect,
    inner: egui::Vec2,
    least: Option<egui::Vec2>,
    nearest: egui::Rect,
    work_areas: &[egui::Rect],
    cause: Cause,
) -> Correction {
    let mut correction = Correction::default();
    if work_areas.is_empty() || !nearest.is_positive() || !outer.is_finite() {
        return correction;
    }
    let frame = (outer.size() - inner).max(egui::Vec2::ZERO);
    let mut size = inner;
    if let Some(least) = least {
        // A work area smaller than the minimum gets all of itself.
        let least = least.min((nearest.size() - frame).max(egui::Vec2::splat(1.0)));
        let grown = egui::vec2(
            if size.x + SLACK < least.x {
                least.x
            } else {
                size.x
            },
            if size.y + SLACK < least.y {
                least.y
            } else {
                size.y
            },
        );
        if grown != size {
            correction.inner_size = Some(grown);
            size = grown;
        }
    }
    let rect = egui::Rect::from_min_size(outer.min, size + frame);
    let visible: f32 = work_areas
        .iter()
        .map(|area| area.intersect(rect))
        .filter(|part| part.is_positive())
        .map(|part| part.area())
        .sum();
    let title_bar = outer.min + egui::vec2(32.0, 16.0);
    let lost = match cause {
        Cause::Display => {
            visible * 2.0 < rect.area() || !work_areas.iter().any(|area| area.contains(title_bar))
        }
        Cause::Moved => visible <= 0.0,
    };
    // A window that grew past the edge of the screen is pulled back in too.
    let overhangs = correction.inner_size.is_some() && visible + SLACK < rect.area();
    if lost || overhangs {
        let axis = |start: f32, length: f32, low: f32, high: f32| {
            if length >= high - low {
                low
            } else {
                start.clamp(low, high - length)
            }
        };
        let pos = egui::pos2(
            axis(rect.min.x, rect.width(), nearest.min.x, nearest.max.x),
            axis(rect.min.y, rect.height(), nearest.min.y, nearest.max.y),
        );
        if (pos - rect.min).abs().max_elem() > SLACK {
            correction.outer_pos = Some(pos);
        }
    }
    correction
}

static DISPLAY_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Notes that a display was added, removed or changed, for the next frame
/// to check the window against. The main window's procedure calls it on
/// `WM_DISPLAYCHANGE`.
pub fn note_display_change() {
    DISPLAY_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Frames checked as if the displays had changed, after a window opens or
/// they do: Windows can place the window again a moment later.
const SETTLE_FRAMES: u8 = 3;

/// Watches a window for the displays changing under it and keeps it
/// usable with [`keep_usable`].
#[derive(Debug, Default)]
pub struct Watch {
    /// Frames still to check as if the displays had changed.
    settle: u8,
    /// The scale, monitor size and frame the window last had.
    seen: Option<(Option<f32>, Option<egui::Vec2>, Option<egui::Rect>)>,
}

impl Watch {
    /// A new window. Its first frame is left alone: eframe and the session
    /// place it then, and the frames after that are checked.
    pub fn attached() -> Self {
        Self {
            settle: SETTLE_FRAMES + 1,
            seen: None,
        }
    }

    /// Looks at the window once a frame. `least` is its least client area,
    /// or `None` to only keep it on a screen.
    pub fn check(&mut self, ctx: &egui::Context, least: Option<egui::Vec2>) {
        if DISPLAY_CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.settle = SETTLE_FRAMES;
        }
        let (seen, inner, skip) = ctx.input(|input| {
            let viewport = input.viewport();
            (
                (
                    viewport.native_pixels_per_point,
                    viewport.monitor_size,
                    viewport.outer_rect,
                ),
                viewport.inner_rect,
                viewport.minimized.unwrap_or(false) || fills_the_screen(viewport),
            )
        });
        let settling = (1..=SETTLE_FRAMES).contains(&self.settle);
        if self.settle > 0 {
            self.settle -= 1;
            // An idle window draws no frames of its own.
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        let previous = self.seen.replace(seen);
        let cause = match previous {
            _ if settling => Cause::Display,
            Some(previous) if (previous.0, previous.1) != (seen.0, seen.1) => Cause::Display,
            Some(previous) if previous.2 != seen.2 => Cause::Moved,
            _ => return,
        };
        // A maximized, full-screen or minimized window is placed by Windows.
        if skip || fixed_size() {
            return;
        }
        let (Some(outer), Some(inner)) = (seen.2, inner) else {
            return;
        };
        let scale = ctx.pixels_per_point();
        if !(scale.is_finite() && scale > 0.0) {
            return;
        }
        let Some((nearest, areas)) = work_areas(outer * scale) else {
            return;
        };
        let areas: Vec<egui::Rect> = areas.into_iter().map(|area| area / scale).collect();
        let correction = keep_usable(outer, inner.size(), least, nearest / scale, &areas, cause);
        if let Some(size) = correction.inner_size {
            log::info!(
                "window at {:?} is smaller than {least:?}; resizing to {size:?}",
                inner.size()
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        if let Some(pos) = correction.outer_pos {
            log::info!("window at {outer:?} is off the screens; moving to {pos:?}");
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        }
    }
}

/// The main window's smallest size with no side panel open. The page and
/// its top bar fit beside the narrowest sidebar from here.
pub const MAIN_MIN_SIZE: [f32; 2] = [760.0, 520.0];

/// Where the main window's interface remembers the minimum width it last
/// sent, and the scale it sent it at.
fn min_width_id() -> egui::Id {
    egui::Id::new("main-min-width")
}

/// The minimum width last sent to the main window and the scale it was
/// sent at, or `None` while the window keeps the one it was created with.
pub(crate) fn sent_min_width(ctx: &egui::Context) -> Option<[f32; 2]> {
    ctx.data(|data| data.get_temp::<[f32; 2]>(min_width_id()))
}

/// Records the minimum width just sent to the main window.
pub(crate) fn record_min_width(ctx: &egui::Context, width: f32, scale: f32) {
    ctx.data_mut(|data| data.insert_temp(min_width_id(), [width, scale]));
}

/// Makes the interface send the main window's minimum again on its next
/// frame.
pub(crate) fn forget_min_size(ctx: &egui::Context) {
    record_min_width(ctx, 0.0, 0.0);
}

/// Lets the main window take any size, for full screen: winit holds even a
/// full-screen window to its minimum, which can be wider than the monitor
/// (#783). The interface sends its minimum again once the window is back.
pub(crate) fn lift_min_size(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::Vec2::ZERO));
    forget_min_size(ctx);
}

/// The main window's least client area now: the panels' minimum once the
/// interface has sent one.
pub(crate) fn main_min_size(ctx: &egui::Context) -> egui::Vec2 {
    let width = sent_min_width(ctx)
        .map(|[width, _]| width)
        .filter(|width| *width > 0.0)
        .unwrap_or(MAIN_MIN_SIZE[0]);
    egui::vec2(width, MAIN_MIN_SIZE[1])
}

static FIXED_SIZE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the main window keeps one size it was given, as demo shots ask,
/// so the interface must not raise its minimum.
pub fn fixed_size() -> bool {
    FIXED_SIZE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Pins the main window to the size it is created with.
pub fn set_fixed_size(on: bool) {
    FIXED_SIZE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// A title-bar action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowButton {
    Minimize,
    Maximize,
    Close,
}

static CUSTOM_TITLEBAR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the main window draws its own title bar and window buttons
/// instead of the platform's frame. It is off unless the listener turns it
/// on in Settings.
pub fn custom_titlebar() -> bool {
    CUSTOM_TITLEBAR.load(std::sync::atomic::Ordering::Relaxed)
}

/// Sets the title bar choice the next main window is created with.
pub fn set_custom_titlebar(on: bool) {
    CUSTOM_TITLEBAR.store(on, std::sync::atomic::Ordering::Relaxed);
}

static MAIN_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Remembers the main window's handle, so its Windows title bar can follow
/// the theme.
pub fn set_main_window(hwnd: isize) {
    MAIN_WINDOW.store(hwnd, std::sync::atomic::Ordering::Relaxed);
}

/// The backdrop the main window was created with, as a [`Backdrop::ALL`]
/// index.
static BACKDROP: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// Whether transparency effects are on in Windows, read again after a
/// setting change.
static TRANSPARENCY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// Set by the main window's procedure on a setting change.
static TRANSPARENCY_STALE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

use crate::settings::Backdrop;

/// The first Windows build with `DWMWA_SYSTEMBACKDROP_TYPE`: Windows 11
/// 2022 Update (22H2).
const BACKDROP_BUILD: u32 = 22621;

/// The Windows build number, from the registry (the version functions
/// answer for the version the program says it supports).
fn windows_build() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let mut buffer = [0u16; 16];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: static wide names, and the output is a buffer of `bytes` bytes.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows_sys::w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            windows_sys::w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if read != 0 {
        return None;
    }
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
        .trim()
        .parse()
        .ok()
}

/// Whether this Windows can put Mica or acrylic behind a window.
pub fn backdrop_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| windows_build().is_some_and(|build| build >= BACKDROP_BUILD))
}

/// Sets the backdrop the next main window is created with: none on a
/// Windows without one.
pub fn set_backdrop(backdrop: Backdrop) {
    let backdrop = if backdrop_supported() {
        backdrop
    } else {
        Backdrop::Off
    };
    let index = Backdrop::ALL
        .iter()
        .position(|choice| *choice == backdrop)
        .unwrap_or(0);
    BACKDROP.store(index as u8, std::sync::atomic::Ordering::Relaxed);
}

/// The backdrop the main window was created with. A change of the setting
/// takes effect when the window is next created.
pub fn backdrop() -> Backdrop {
    Backdrop::ALL
        .get(usize::from(
            BACKDROP.load(std::sync::atomic::Ordering::Relaxed),
        ))
        .copied()
        .unwrap_or(Backdrop::Off)
}

/// Notes a setting change, after which transparency effects are read again.
pub fn note_setting_change() {
    TRANSPARENCY_STALE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether "Transparency effects" is on in Windows Settings.
fn transparency_effects() -> bool {
    use std::sync::atomic::Ordering;
    if TRANSPARENCY_STALE.swap(false, Ordering::Relaxed) {
        use windows_sys::Win32::System::Registry::{
            HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW,
        };
        let mut value: u32 = 1;
        let mut bytes = size_of::<u32>() as u32;
        // SAFETY: static wide names, and the output is one DWORD.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                windows_sys::w!(
                    "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
                ),
                windows_sys::w!("EnableTransparency"),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut value).cast(),
                &mut bytes,
            )
        };
        TRANSPARENCY.store(read != 0 || value != 0, Ordering::Relaxed);
    }
    TRANSPARENCY.load(Ordering::Relaxed)
}

/// Whether the backdrop shows through the main window now: the window was
/// made for one and transparency effects are on. Otherwise everything is
/// drawn opaque, as without a backdrop.
pub fn backdrop_shows() -> bool {
    backdrop() != Backdrop::Off && !custom_titlebar() && transparency_effects()
}

/// How much of the theme's window and panel colours covers the backdrop.
const WINDOW_COVER: f32 = 0.55;
const PANEL_COVER: f32 = 0.7;

/// The window's ground in `color`: as it is, or letting a Mica or acrylic
/// backdrop through. Only the large grounds (the page, the side panels and
/// the player bar) take this; text, controls, menus and dialogs, and the
/// places that use the window colour for text, stay opaque.
pub fn window_ground(color: egui::Color32) -> egui::Color32 {
    ground(color, WINDOW_COVER)
}

/// [`window_ground`] for the panels: the library, the queue, the lyrics
/// and the player bar, which cover a little more of the backdrop.
pub fn panel_ground(color: egui::Color32) -> egui::Color32 {
    ground(color, PANEL_COVER)
}

fn ground(color: egui::Color32, cover: f32) -> egui::Color32 {
    if !backdrop_shows() {
        return color;
    }
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let alpha = (f32::from(a) * cover).round() as u8;
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// Asks Windows for the chosen backdrop behind the main window, under a
/// frame stretched over the whole client area so it shows wherever the app
/// draws nothing opaque. Not for the mini player, whose skin is its shape.
pub fn apply_backdrop() {
    let hwnd = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd == 0 || custom_titlebar() {
        return;
    }
    backdrop_behind(hwnd as windows_sys::Win32::Foundation::HWND);
}

fn backdrop_behind(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMSBT_MAINWINDOW, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DwmSetWindowAttribute,
    };
    /// `MARGINS`, which windows-sys keeps behind a feature this crate does
    /// not otherwise need.
    #[repr(C)]
    struct Margins {
        left: i32,
        right: i32,
        top: i32,
        bottom: i32,
    }
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmExtendFrameIntoClientArea(
            hwnd: windows_sys::Win32::Foundation::HWND,
            margins: *const Margins,
        ) -> i32;
    }
    let kind = match backdrop() {
        Backdrop::Off => return,
        Backdrop::Mica => DWMSBT_MAINWINDOW,
        Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
    };
    let sheet = Margins {
        left: -1,
        right: -1,
        top: -1,
        bottom: -1,
    };
    // SAFETY: hwnd is the live main window, on the thread that owns it, and
    // both values point at readable memory of the size passed.
    unsafe {
        DwmExtendFrameIntoClientArea(hwnd, &raw const sheet);
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&raw const kind).cast(),
            size_of::<i32>() as u32,
        );
    }
}

/// The Windows title-bar colours for a palette, as COLORREF (0x00BBGGRR):
/// the caption in the window colour, so the bar reads as the top of the
/// app, and the caption text in the text colour.
fn title_bar_colors(palette: &crate::theme::Palette) -> (u32, u32) {
    let colorref =
        |c: egui::Color32| u32::from(c.r()) | u32::from(c.g()) << 8 | u32::from(c.b()) << 16;
    (colorref(palette.window), colorref(palette.text))
}

/// Paints the main window's Windows title bar in the palette's colours.
/// Dark mode works on Windows 10 and 11; the exact caption and text colours
/// need Windows 11, and older versions keep their own (the call is refused
/// harmlessly). Nothing happens before the window exists or when LWSpot
/// draws its own title bar.
pub fn tint_title_bar(palette: &crate::theme::Palette) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    };
    let hwnd = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd == 0 || custom_titlebar() {
        return;
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    let dark: i32 = palette.dark.into();
    // Over a backdrop the caption is left to Windows, so the backdrop shows
    // through it too; dark mode still picks its dark or light tint.
    let (caption, text) = if backdrop() == Backdrop::Off {
        title_bar_colors(palette)
    } else {
        use windows_sys::Win32::Graphics::Dwm::DWMWA_COLOR_DEFAULT;
        (DWMWA_COLOR_DEFAULT, DWMWA_COLOR_DEFAULT)
    };
    let set = |attribute: i32, value: *const std::ffi::c_void, size: usize| {
        // SAFETY: hwnd is the live main window (set once it exists, on the
        // thread that owns it), and value points at `size` readable bytes.
        unsafe { DwmSetWindowAttribute(hwnd, attribute as u32, value, size as u32) }
    };
    set(
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        (&raw const dark).cast(),
        size_of::<i32>(),
    );
    set(
        DWMWA_CAPTION_COLOR,
        (&raw const caption).cast(),
        size_of::<u32>(),
    );
    set(DWMWA_TEXT_COLOR, (&raw const text).cast(), size_of::<u32>());
}

/// How often a moving visualizer asks Windows whether anyone can see the
/// main window, and how often a hidden one wakes to ask again.
pub const OCCLUSION_CHECK: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether the main window can be seen, asked of Windows at most once per
/// [`OCCLUSION_CHECK`] while a visualizer moves.
#[derive(Debug, Default)]
pub struct Occlusion {
    checked: Option<(std::time::Instant, bool)>,
    hidden: bool,
}

impl Occlusion {
    /// Whether nobody can see the main window, so animation can stop.
    /// `focused` is whether it has the keyboard focus: gaining it asks
    /// again at once, as clicking a covered window brings it forward.
    pub fn hidden(&mut self, now: std::time::Instant, focused: bool) -> bool {
        self.hidden_by(now, focused, main_window_hidden)
    }

    /// The last answer, without asking again.
    pub fn last(&self) -> bool {
        self.hidden
    }

    fn hidden_by(
        &mut self,
        now: std::time::Instant,
        focused: bool,
        hidden: impl FnOnce() -> bool,
    ) -> bool {
        let due = match self.checked {
            None => true,
            Some((at, was_focused)) => {
                now.duration_since(at) >= OCCLUSION_CHECK || (focused && !was_focused)
            }
        };
        if due {
            self.hidden = hidden();
            self.checked = Some((now, focused));
        }
        self.hidden
    }
}

/// A rectangle of screen pixels; the right and bottom edges are outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScreenRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl ScreenRect {
    fn is_empty(self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }

    fn intersect(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }
}

/// The most pieces the uncovered part of a window is tracked in. Past it
/// the window is counted as seen, which only keeps a visualizer moving.
const MOST_PIECES: usize = 256;

/// Whether `covers` together hide every pixel of `target`. Each cover cuts
/// what is left of the target into at most four pieces around it.
fn fully_covered(target: ScreenRect, covers: &[ScreenRect]) -> bool {
    let mut uncovered = vec![target];
    uncovered.retain(|piece| !piece.is_empty());
    for &cover in covers {
        if uncovered.is_empty() {
            break;
        }
        let mut left = Vec::with_capacity(uncovered.len());
        for piece in uncovered {
            let overlap = piece.intersect(cover);
            if overlap.is_empty() {
                left.push(piece);
                continue;
            }
            let around = [
                ScreenRect {
                    bottom: overlap.top,
                    ..piece
                },
                ScreenRect {
                    top: overlap.bottom,
                    ..piece
                },
                ScreenRect {
                    top: overlap.top,
                    bottom: overlap.bottom,
                    right: overlap.left,
                    ..piece
                },
                ScreenRect {
                    top: overlap.top,
                    bottom: overlap.bottom,
                    left: overlap.right,
                    ..piece
                },
            ];
            left.extend(around.into_iter().filter(|piece| !piece.is_empty()));
        }
        if left.len() > MOST_PIECES {
            return false;
        }
        uncovered = left;
    }
    uncovered.is_empty()
}

/// Whether nobody can see any of the main window: it is hidden or
/// minimized, on another virtual desktop, or the windows above it cover all
/// of it, as a full-screen window on its monitor does. Windows that can be
/// seen through (click-through overlays, translucent or colour-keyed ones)
/// cover nothing. Before the window exists it counts as seen.
fn main_window_hidden() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_HWNDPREV, GetSystemMetrics, GetWindow, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };
    let hwnd = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd == 0 {
        return false;
    }
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    if !shown(hwnd) {
        return true;
    }
    let Some(own) = frame_bounds(hwnd) else {
        return false;
    };
    // SAFETY: GetSystemMetrics reads system values and takes no pointers.
    let screens = unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        ScreenRect {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    };
    let mut covers = Vec::new();
    // SAFETY: GetWindow takes any window handle and returns null at the
    // top of the z-order or for a window that no longer exists.
    let mut above = unsafe { GetWindow(hwnd, GW_HWNDPREV) };
    // A z-order that changes while it is walked could in principle cycle.
    for _ in 0..4096 {
        if above.is_null() {
            return fully_covered(own.intersect(screens), &covers);
        }
        if let Some(cover) = opaque_cover(above) {
            covers.push(cover);
        }
        // SAFETY: as above.
        above = unsafe { GetWindow(above, GW_HWNDPREV) };
    }
    false
}

/// Whether a window is on screen: shown, not minimized, and not cloaked by
/// Windows (on another virtual desktop, or a suspended app's window).
fn shown(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    let mut cloaked: u32 = 0;
    // SAFETY: these take any window handle; the attribute is written to a
    // u32 of the size given.
    unsafe {
        IsWindowVisible(hwnd) != 0
            && IsIconic(hwnd) == 0
            && (DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                (&raw mut cloaked).cast(),
                size_of::<u32>() as u32,
            ) != 0
                || cloaked == 0)
    }
}

/// The screen area a window hides of the windows below it, if it is shown
/// and opaque.
fn opaque_cover(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<ScreenRect> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetLayeredWindowAttributes, GetWindowLongPtrW, LWA_ALPHA, LWA_COLORKEY,
        WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };
    if !shown(hwnd) {
        return None;
    }
    // SAFETY: GetWindowLongPtrW takes any window handle.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    if style & WS_EX_TRANSPARENT != 0 {
        return None;
    }
    if style & WS_EX_LAYERED != 0 {
        let (mut key, mut alpha, mut flags) = (0, 0u8, 0);
        // SAFETY: the three outputs are writable locals. It fails for a
        // window drawn with per-pixel alpha, which may be see-through.
        let read = unsafe { GetLayeredWindowAttributes(hwnd, &mut key, &mut alpha, &mut flags) };
        if read == 0 || flags & LWA_COLORKEY != 0 || (flags & LWA_ALPHA != 0 && alpha < 255) {
            return None;
        }
    }
    frame_bounds(hwnd).filter(|rect| !rect.is_empty())
}

/// A window's visible frame on screen, without the invisible resize
/// borders Windows 10 and 11 count in its window rectangle.
fn frame_bounds(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<ScreenRect> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: both write one RECT to the local given, of the size given.
    let read = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&raw mut rect).cast(),
            size_of::<RECT>() as u32,
        ) == 0
            || GetWindowRect(hwnd, &mut rect) != 0
    };
    read.then_some(ScreenRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn a_window_is_covered_only_when_no_pixel_shows() {
        let window = screen_rect(100, 100, 900, 700);
        // Nothing above it.
        assert!(!fully_covered(window, &[]));
        // A full-screen window on its monitor.
        assert!(fully_covered(window, &[screen_rect(0, 0, 1920, 1080)]));
        // Exactly its own size.
        assert!(fully_covered(window, &[window]));
        // One pixel column left uncovered on the right.
        assert!(!fully_covered(window, &[screen_rect(0, 0, 899, 1080)]));
        // Two windows meeting edge to edge cover it together.
        assert!(fully_covered(
            window,
            &[
                screen_rect(0, 0, 500, 1080),
                screen_rect(500, 0, 1920, 1080)
            ]
        ));
        // Four windows around a hole in the middle leave the hole.
        let around_hole = [
            screen_rect(0, 0, 1920, 300),
            screen_rect(0, 400, 1920, 1080),
            screen_rect(0, 0, 400, 1080),
            screen_rect(500, 0, 1920, 1080),
        ];
        assert!(!fully_covered(window, &around_hole));
        // A fifth fills it.
        let mut filled = around_hole.to_vec();
        filled.push(screen_rect(390, 290, 510, 410));
        assert!(fully_covered(window, &filled));
        // Overlapping covers, in any order.
        assert!(fully_covered(
            window,
            &[
                screen_rect(300, 50, 950, 400),
                screen_rect(50, 350, 600, 750),
                screen_rect(50, 50, 320, 380),
                screen_rect(580, 380, 950, 750),
            ]
        ));
    }

    #[test]
    fn a_window_off_every_screen_counts_as_covered() {
        // What is left of a window wholly off the screens is empty.
        let off = screen_rect(-900, 100, -100, 700).intersect(screen_rect(0, 0, 1920, 1080));
        assert!(off.is_empty());
        assert!(fully_covered(off, &[]));
    }

    #[test]
    fn too_many_gaps_count_as_seen() {
        // A comb of thin covers leaves more gaps than are tracked.
        let window = screen_rect(0, 0, 2000, 2000);
        let comb: Vec<_> = (0..400)
            .map(|i| screen_rect(i * 5, 0, i * 5 + 4, 2000))
            .chain((0..400).map(|i| screen_rect(0, i * 5, 2000, i * 5 + 4)))
            .collect();
        assert!(!fully_covered(window, &comb));
    }

    #[test]
    fn occlusion_is_asked_once_a_second_and_again_on_focus() {
        let start = std::time::Instant::now();
        let mut occlusion = Occlusion::default();
        let asked = std::cell::Cell::new(0);
        let ask = |hidden: bool| {
            let asked = &asked;
            move || {
                asked.set(asked.get() + 1);
                hidden
            }
        };
        // #given a covered window, asked on its first frame
        assert!(occlusion.hidden_by(start, false, ask(true)));
        // #when frames come sooner than a second #then the answer holds
        let soon = start + std::time::Duration::from_millis(500);
        assert!(occlusion.hidden_by(soon, false, ask(false)));
        // #when it gains the focus #then it asks at once
        assert!(!occlusion.hidden_by(soon, true, ask(false)));
        // #when a second has passed #then it asks again
        let later = soon + OCCLUSION_CHECK;
        assert!(occlusion.hidden_by(later, true, ask(true)));
        assert_eq!(asked.get(), 3);
    }

    fn reachable(pos: [f32; 2], scale: f32, area: [f32; 4]) -> bool {
        let rect =
            egui::Rect::from_min_max(egui::pos2(area[0], area[1]), egui::pos2(area[2], area[3]));
        titlebar_anchor(pos, scale).is_some_and(|anchor| rect.contains(anchor))
    }

    #[test]
    fn the_title_bar_takes_the_window_and_text_colours() {
        let palette = crate::theme::Palette::dark();
        let (caption, text) = title_bar_colors(&palette);
        // COLORREF is 0x00BBGGRR.
        assert_eq!(caption, 0x0014_110f);
        assert_eq!(text & 0xff, u32::from(palette.text.r()));
        assert_eq!(text >> 16, u32::from(palette.text.b()));
    }

    #[test]
    fn disconnected_monitor_position_is_not_restored() {
        assert!(!reachable([1912.0, -8.0], 1.0, [0.0, 0.0, 1920.0, 1032.0]));
        assert!(reachable(
            [1912.0, -8.0],
            1.0,
            [1920.0, 0.0, 3840.0, 1080.0]
        ));
    }

    #[test]
    fn visible_positions_include_maximized_and_negative_coordinates() {
        assert!(reachable([-8.0, -8.0], 1.0, [0.0, 0.0, 1920.0, 1032.0]));
        assert!(reachable(
            [-1920.0, 100.0],
            1.0,
            [-1920.0, 0.0, 0.0, 1080.0]
        ));
    }

    #[test]
    fn logical_coordinates_are_scaled_to_monitor_pixels() {
        assert!(reachable([900.0, 100.0], 2.0, [0.0, 0.0, 1920.0, 1080.0]));
        assert!(!reachable([1000.0, 100.0], 2.0, [0.0, 0.0, 1920.0, 1080.0]));
    }

    #[test]
    fn only_a_maximized_or_full_screen_window_fills_the_screen() {
        let state = |maximized, fullscreen| {
            fills_the_screen(&egui::ViewportInfo {
                maximized,
                fullscreen,
                ..Default::default()
            })
        };
        assert!(state(Some(true), Some(false)));
        assert!(state(Some(false), Some(true)));
        assert!(!state(Some(false), Some(false)));
        // A backend that does not report the state leaves the window ordinary.
        assert!(!state(None, None));
    }

    #[test]
    fn invalid_coordinates_and_scale_are_rejected() {
        assert!(titlebar_anchor([f32::NAN, 0.0], 1.0).is_none());
        assert!(titlebar_anchor([0.0, f32::INFINITY], 1.0).is_none());
        assert!(titlebar_anchor([0.0, 0.0], 0.0).is_none());
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height))
    }

    /// A 1920x1080 monitor with its taskbar at the bottom.
    const SCREEN: [f32; 4] = [0.0, 0.0, 1920.0, 1032.0];

    fn screen() -> egui::Rect {
        rect(SCREEN[0], SCREEN[1], SCREEN[2], SCREEN[3])
    }

    /// The main window's frame: invisible borders and a title bar.
    const FRAME: egui::Vec2 = egui::vec2(16.0, 39.0);

    fn main_window(x: f32, y: f32, width: f32, height: f32) -> (egui::Rect, egui::Vec2) {
        let inner = egui::vec2(width, height);
        (rect(x, y, width + FRAME.x, height + FRAME.y), inner)
    }

    fn least() -> Option<egui::Vec2> {
        Some(egui::Vec2::from(MAIN_MIN_SIZE))
    }

    #[test]
    fn a_tiny_window_grows_to_its_minimum_where_it_is() {
        // The size reported in #704.
        let (outer, inner) = main_window(200.0, 150.0, 247.0, 261.0);
        let correction = keep_usable(outer, inner, least(), screen(), &[screen()], Cause::Display);
        assert_eq!(
            correction,
            Correction {
                inner_size: Some(egui::Vec2::from(MAIN_MIN_SIZE)),
                outer_pos: None,
            }
        );
    }

    #[test]
    fn a_window_that_grows_past_the_edge_is_pulled_back() {
        let (outer, inner) = main_window(1700.0, 900.0, 247.0, 261.0);
        let correction = keep_usable(outer, inner, least(), screen(), &[screen()], Cause::Display);
        assert_eq!(correction.inner_size, Some(egui::Vec2::from(MAIN_MIN_SIZE)));
        let pos = correction.outer_pos.expect("moved back onto the screen");
        let moved = egui::Rect::from_min_size(pos, egui::Vec2::from(MAIN_MIN_SIZE) + FRAME);
        assert!(screen().contains_rect(moved), "{moved:?}");
    }

    #[test]
    fn a_window_on_a_removed_monitor_comes_back_to_the_nearest() {
        // The monitor to the right is gone; the window still points there.
        let (outer, inner) = main_window(2200.0, 100.0, 1000.0, 700.0);
        let correction = keep_usable(outer, inner, least(), screen(), &[screen()], Cause::Display);
        assert_eq!(correction.inner_size, None);
        let pos = correction.outer_pos.expect("moved back onto the screen");
        assert_eq!(pos, egui::pos2(1920.0 - 1016.0, 100.0));
        // Even a window only moved there is brought back.
        assert_eq!(
            keep_usable(outer, inner, least(), screen(), &[screen()], Cause::Moved).outer_pos,
            Some(pos)
        );
    }

    #[test]
    fn a_window_straddling_two_monitors_stays_put() {
        let right = rect(1920.0, 0.0, 2560.0, 1400.0);
        let (outer, inner) = main_window(1500.0, 100.0, 1000.0, 700.0);
        let correction = keep_usable(
            outer,
            inner,
            least(),
            screen(),
            &[screen(), right],
            Cause::Display,
        );
        assert_eq!(correction, Correction::default());
    }

    #[test]
    fn a_window_parked_mostly_off_the_screen_stays_until_the_displays_change() {
        // Three quarters below the bottom edge, title bar still reachable.
        let (outer, inner) = main_window(400.0, 850.0, 1000.0, 700.0);
        assert_eq!(
            keep_usable(outer, inner, least(), screen(), &[screen()], Cause::Moved),
            Correction::default()
        );
        let pos = keep_usable(outer, inner, least(), screen(), &[screen()], Cause::Display)
            .outer_pos
            .expect("moved back once the displays changed");
        assert_eq!(pos, egui::pos2(400.0, 1032.0 - 739.0));
    }

    #[test]
    fn a_window_whose_title_bar_is_above_the_screen_comes_back() {
        let (outer, inner) = main_window(400.0, -30.0, 1000.0, 700.0);
        let pos = keep_usable(outer, inner, least(), screen(), &[screen()], Cause::Display)
            .outer_pos
            .expect("the title bar is reachable again");
        assert_eq!(pos, egui::pos2(400.0, 0.0));
    }

    #[test]
    fn a_scale_change_measures_the_minimum_in_the_new_points() {
        // 800 pixels wide was 800 points at 100% and is 533 at 150%.
        let scale = 1.5;
        let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(2560.0, 1392.0)) / scale;
        let outer_px = rect(100.0, 100.0, 800.0 + FRAME.x, 600.0 + FRAME.y);
        let outer = outer_px / scale;
        let inner = egui::vec2(800.0, 600.0) / scale;
        let correction = keep_usable(outer, inner, least(), area, &[area], Cause::Display);
        assert_eq!(
            correction,
            Correction {
                inner_size: Some(egui::Vec2::from(MAIN_MIN_SIZE)),
                outer_pos: None,
            }
        );
    }

    #[test]
    fn a_work_area_smaller_than_the_minimum_is_filled() {
        let small = rect(0.0, 0.0, 700.0, 480.0);
        let (outer, inner) = main_window(50.0, 50.0, 300.0, 200.0);
        let correction = keep_usable(outer, inner, least(), small, &[small], Cause::Display);
        assert_eq!(correction.inner_size, Some(small.size() - FRAME));
        assert_eq!(correction.outer_pos, Some(egui::Pos2::ZERO));
    }

    #[test]
    fn a_window_of_the_right_size_on_the_screen_is_left_alone() {
        let (outer, inner) = main_window(100.0, 100.0, 1240.0, 800.0);
        for cause in [Cause::Display, Cause::Moved] {
            assert_eq!(
                keep_usable(outer, inner, least(), screen(), &[screen()], cause),
                Correction::default()
            );
        }
        // A pixel short of the minimum after converting to points is not
        // a reason to resize.
        let (outer, inner) = main_window(100.0, 100.0, 759.7, 519.8);
        assert_eq!(
            keep_usable(outer, inner, least(), screen(), &[screen()], Cause::Display),
            Correction::default()
        );
    }

    #[test]
    fn the_mini_player_is_only_moved() {
        let outer = rect(2500.0, 300.0, 275.0, 116.0);
        let correction = keep_usable(
            outer,
            outer.size(),
            None,
            screen(),
            &[screen()],
            Cause::Display,
        );
        assert_eq!(
            correction,
            Correction {
                inner_size: None,
                outer_pos: Some(egui::pos2(1920.0 - 275.0, 300.0)),
            }
        );
    }
}
