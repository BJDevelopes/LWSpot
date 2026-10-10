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

/// The main window's smallest size with no side panel open. The page and
/// its top bar fit beside the narrowest sidebar from here.
pub const MAIN_MIN_SIZE: [f32; 2] = [760.0, 520.0];

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

#[cfg(test)]
mod tests {
    use super::*;

    fn reachable(pos: [f32; 2], scale: f32, area: [f32; 4]) -> bool {
        let rect =
            egui::Rect::from_min_max(egui::pos2(area[0], area[1]), egui::pos2(area[2], area[3]));
        titlebar_anchor(pos, scale).is_some_and(|anchor| rect.contains(anchor))
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
}
