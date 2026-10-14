//! Opens links and folders with the desktop.
//!
//! Uses `ShellExecuteW` rather than the `open` crate because the crate's
//! PowerShell launcher can be unavailable (#107), while `cmd /c start`
//! expands percent-encoded URL segments as environment variables.

use std::ffi::OsStr;
use std::io;

/// Opens a URL or a path with Launch Services, through `/usr/bin/open`,
/// which takes the target as one argument with no shell in between. It
/// returns once `open` has started, so a click never waits on Launch
/// Services; a failure after that is logged.
#[cfg(target_os = "macos")]
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    let mut child = std::process::Command::new("/usr/bin/open")
        .arg(target.as_ref())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::Builder::new()
        .name("open".into())
        .spawn(move || match child.wait() {
            Ok(status) if !status.success() => log::warn!("open exited with {status}"),
            Err(error) => log::warn!("open did not finish: {error}"),
            Ok(_) => {}
        })?;
    Ok(())
}

/// Opens a URL or a path in whatever the desktop uses for it.
#[cfg(windows)]
pub fn open(target: impl AsRef<OsStr>) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    // Shell extensions may require COM. Keep an existing apartment mode if
    // the worker thread was already initialized.
    unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };

    let file = wide(target.as_ref());
    let verb = wide(OsStr::new("open"));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Anything above 32 opened. At or below it the value is one of the
    // old SE_ERR_ codes, which the thread's last error carries too.
    if result as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
