//! macOS: Low Power Mode, battery, display sleep and the screen lock.
//!
//! One thread reads them every few seconds. Each read is a handful of cheap
//! calls (no allocation beyond one small session dictionary), so the thread
//! costs nothing measurable, and it needs no run loop, notification
//! observers or blocks. The window itself is already throttled by AppKit
//! when it is covered, so the lock and display reads only catch the cases
//! occlusion does not report quickly.
//!
//! The same thread notices a new accent colour in System Settings, as the
//! Windows window procedure does, so the accent follows it while running.
//!
//! Sleep, wake and the end of the session are left to the system: AppKit
//! asks the app to quit at log out, which saves through the normal exit.

use std::ffi::{c_char, c_void};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool};

use super::Shared;

/// How often the state is read.
const POLL: Duration = Duration::from_secs(3);

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: CFTypeRef);
    fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        text: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFBooleanGetValue(value: CFTypeRef) -> u8;
    fn CFGetTypeID(value: CFTypeRef) -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: CFTypeRef, kind: isize, value: *mut c_void) -> u8;
    fn CFPreferencesCopyAppValue(key: CFStringRef, application: CFStringRef) -> CFTypeRef;
    fn CFPreferencesAppSynchronize(application: CFStringRef) -> u8;
    static kCFPreferencesAnyApplication: CFStringRef;
}

/// `kCFNumberSInt64Type`.
const SINT64: isize = 4;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    /// `kIOPSTimeRemainingUnlimited` (-2) while on external power.
    fn IOPSGetTimeRemainingEstimate() -> f64;
}

const UTF8: u32 = 0x0800_0100;

/// A CFString made once and released on drop.
struct Key(CFStringRef);

// SAFETY: an immutable CFString may be used from any thread.
unsafe impl Send for Key {}

impl Key {
    fn new(text: &std::ffi::CStr) -> Option<Self> {
        // SAFETY: a NUL-terminated literal in UTF-8, as the encoding says.
        let key = unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
        (!key.is_null()).then_some(Self(key))
    }

    /// The global `AppleAccentColor` setting: `None` for the multicolour
    /// default, otherwise the chosen colour's number.
    fn accent(&self) -> Option<i64> {
        // SAFETY: the copied value is ours to release, and is checked to be
        // a CFNumber before it is read into an i64.
        unsafe {
            CFPreferencesAppSynchronize(kCFPreferencesAnyApplication);
            let value = CFPreferencesCopyAppValue(self.0, kCFPreferencesAnyApplication);
            if value.is_null() {
                return None;
            }
            let mut number: i64 = 0;
            let read = CFGetTypeID(value) == CFNumberGetTypeID()
                && CFNumberGetValue(value, SINT64, (&raw mut number).cast()) != 0;
            CFRelease(value);
            read.then_some(number)
        }
    }

    /// Whether the screen is locked or the session switched away from.
    fn locked(&self) -> bool {
        // SAFETY: the dictionary is ours to release; the value it holds is
        // borrowed from it and checked to be a CFBoolean before it is read.
        unsafe {
            let session = CGSessionCopyCurrentDictionary();
            if session.is_null() {
                // No window server session to ask: never stop drawing on
                // a guess.
                return false;
            }
            let value = CFDictionaryGetValue(session, self.0);
            let locked = !value.is_null()
                && CFGetTypeID(value) == CFBooleanGetTypeID()
                && CFBooleanGetValue(value) != 0;
            CFRelease(session);
            locked
        }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: created in `new` and released once.
        unsafe { CFRelease(self.0) };
    }
}

fn low_power_mode() -> bool {
    let Some(class) = AnyClass::get(c"NSProcessInfo") else {
        return false;
    };
    // SAFETY: `processInfo` returns the shared instance, and
    // `isLowPowerModeEnabled` (macOS 12) is checked before it is called.
    unsafe {
        let info: *mut AnyObject = msg_send![class, processInfo];
        if info.is_null() {
            return false;
        }
        let known: Bool = msg_send![info, respondsToSelector: objc2::sel!(isLowPowerModeEnabled)];
        if !known.as_bool() {
            return false;
        }
        let enabled: Bool = msg_send![info, isLowPowerModeEnabled];
        enabled.as_bool()
    }
}

fn on_battery() -> bool {
    // SAFETY: takes nothing and returns a number.
    let remaining = unsafe { IOPSGetTimeRemainingEstimate() };
    remaining != -2.0
}

fn display_off() -> bool {
    // SAFETY: takes nothing beyond the display id it was given.
    unsafe { CGDisplayIsAsleep(CGMainDisplayID()) != 0 }
}

pub(super) struct Reader {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Reader {
    pub(super) fn start(shared: Arc<Shared>) -> Result<Self, String> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("power".into())
            .spawn(move || {
                let lock = Key::new(c"CGSSessionScreenIsLocked");
                let accent_key = Key::new(c"AppleAccentColor");
                let mut accent = accent_key.as_ref().map(Key::accent);
                let (stopped, ring) = &*signal;
                let mut stopped = stopped.lock().unwrap_or_else(PoisonError::into_inner);
                while !*stopped {
                    let saver = low_power_mode();
                    let battery = on_battery();
                    let display = display_off();
                    let locked = lock.as_ref().is_some_and(Key::locked);
                    let now = accent_key.as_ref().map(Key::accent);
                    if now != accent {
                        accent = now;
                        crate::accent::note_windows_change();
                        (shared.wake)();
                    }
                    shared.update(|conditions| {
                        conditions.saver = saver;
                        conditions.on_battery = battery;
                        conditions.display_off = display;
                        conditions.locked = locked;
                    });
                    stopped = ring
                        .wait_timeout(stopped, POLL)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let (stopped, ring) = &*self.stop;
        *stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        ring.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The audio and decoding threads carry their own QoS, and AppKit already
/// naps an app nobody can see unless it is playing sound, so there is
/// nothing to lower by hand.
#[derive(Default)]
pub(super) struct EcoQos {
    _nothing: (),
}

impl EcoQos {
    pub(super) fn set(&mut self, _eco: bool) {}
}
