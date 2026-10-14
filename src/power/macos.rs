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
//! Sleep and wake come from IOKit's system power notifications on the same
//! thread's run loop: before the Mac sleeps the app saves, and the Mac waits
//! for that (up to `SUSPEND_ACK`), as Windows does. The end of the session
//! is left to AppKit, which asks the app to quit at log out.

use std::ffi::{c_char, c_void};
use std::sync::{Arc, Mutex, PoisonError};
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
    static kCFRunLoopDefaultMode: CFStringRef;
    fn CFRetain(value: CFTypeRef) -> CFTypeRef;
    fn CFRunLoopGetCurrent() -> CFTypeRef;
    fn CFRunLoopAddSource(run_loop: CFTypeRef, source: CFTypeRef, mode: CFStringRef);
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source: u8) -> i32;
    fn CFRunLoopStop(run_loop: CFTypeRef);
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
    fn IORegisterForSystemPower(
        context: *mut c_void,
        port: *mut *mut c_void,
        callback: unsafe extern "C" fn(*mut c_void, u32, u32, *mut c_void),
        notifier: *mut u32,
    ) -> u32;
    fn IOAllowPowerChange(root: u32, notification: isize) -> i32;
    fn IODeregisterForSystemPower(notifier: *mut u32) -> i32;
    fn IOServiceClose(connection: u32) -> i32;
    fn IONotificationPortGetRunLoopSource(port: *mut c_void) -> CFTypeRef;
    fn IONotificationPortDestroy(port: *mut c_void);
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

/// The IOKit system-power registration, read by its callback on the power
/// thread's run loop.
struct Sleep {
    shared: Arc<Shared>,
    /// The root power domain's port, which sleep is allowed through.
    root: u32,
}

/// `kCFRunLoopRunFinished`: the run loop had no sources to run.
const RUN_FINISHED: i32 = 1;

/// `iokit_common_msg` values from IOKit's IOMessage.h.
const CAN_SYSTEM_SLEEP: u32 = 0xE000_0270;
const SYSTEM_WILL_SLEEP: u32 = 0xE000_0280;
const SYSTEM_HAS_POWERED_ON: u32 = 0xE000_0300;

/// Called on the power thread for each system power message.
unsafe extern "C" fn on_power(
    context: *mut c_void,
    _service: u32,
    message: u32,
    argument: *mut c_void,
) {
    // SAFETY: `context` is the `Sleep` the thread owns for as long as the
    // registration exists, and callbacks only run on that thread.
    let sleep = unsafe { &*context.cast::<Sleep>() };
    match message {
        // Idle sleep may go ahead: nothing here needs to hold it off.
        CAN_SYSTEM_SLEEP => unsafe {
            IOAllowPowerChange(sleep.root, argument as isize);
        },
        // Save first, for as long as SUSPEND_ACK, then let the Mac sleep.
        SYSTEM_WILL_SLEEP => {
            sleep
                .shared
                .push_and_wait(super::Event::Suspending, super::SUSPEND_ACK);
            // SAFETY: the port and the message's own identifier.
            unsafe { IOAllowPowerChange(sleep.root, argument as isize) };
        }
        SYSTEM_HAS_POWERED_ON => sleep.shared.push(super::Event::Resumed),
        _ => {}
    }
}

/// The power thread's run loop, so `Drop` can stop it from another thread.
struct RunLoop(CFTypeRef);

// SAFETY: CFRunLoopStop and CFRunLoopWakeUp may be called from any thread,
// and the reference is retained until it is released once.
unsafe impl Send for RunLoop {}

pub(super) struct Reader {
    stop: Arc<std::sync::atomic::AtomicBool>,
    run_loop: Arc<Mutex<Option<RunLoop>>>,
    thread: Option<JoinHandle<()>>,
}

impl Reader {
    pub(super) fn start(shared: Arc<Shared>) -> Result<Self, String> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let run_loop = Arc::new(Mutex::new(None));
        let (stopping, published) = (Arc::clone(&stop), Arc::clone(&run_loop));
        let thread = std::thread::Builder::new()
            .name("power".into())
            .spawn(move || run(shared, &stopping, &published))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            run_loop,
            thread: Some(thread),
        })
    }
}

/// The power thread: system power messages as they come, and the cheap
/// reads every [`POLL`] between them.
fn run(
    shared: Arc<Shared>,
    stop: &std::sync::atomic::AtomicBool,
    published: &Mutex<Option<RunLoop>>,
) {
    use std::sync::atomic::Ordering;
    // SAFETY: the current thread's run loop, retained for `Drop` to stop.
    unsafe {
        let current = CFRunLoopGetCurrent();
        CFRetain(current);
        *published.lock().unwrap_or_else(PoisonError::into_inner) = Some(RunLoop(current));
    }
    let mut sleep = Box::new(Sleep {
        shared: Arc::clone(&shared),
        root: 0,
    });
    let mut port: *mut c_void = std::ptr::null_mut();
    let mut notifier: u32 = 0;
    // SAFETY: the box outlives the registration, which is undone below on
    // this thread before the box is dropped.
    let root = unsafe {
        IORegisterForSystemPower((&raw mut *sleep).cast(), &mut port, on_power, &mut notifier)
    };
    if root == 0 {
        log::warn!("sleep and wake are not followed");
    } else {
        sleep.root = root;
        // SAFETY: the port's source joins this thread's run loop.
        unsafe {
            CFRunLoopAddSource(
                CFRunLoopGetCurrent(),
                IONotificationPortGetRunLoopSource(port),
                kCFRunLoopDefaultMode,
            );
        }
    }

    let lock = Key::new(c"CGSSessionScreenIsLocked");
    let accent_key = Key::new(c"AppleAccentColor");
    let mut accent = accent_key.as_ref().map(Key::accent);
    while !stop.load(Ordering::Acquire) {
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
        // Sleeps until POLL passes or `Drop` stops it, handling power
        // messages meanwhile.
        // SAFETY: runs this thread's own run loop.
        let finished = unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, POLL.as_secs_f64(), 0) };
        // Without the power source the run loop has nothing to wait on and
        // returns at once (`kCFRunLoopRunFinished`): wait here instead, so
        // the thread never spins. `Drop` unparks it.
        if finished == RUN_FINISHED {
            std::thread::park_timeout(POLL);
        }
    }

    if root != 0 {
        // SAFETY: undoes the registration made above, once, on this thread.
        unsafe {
            IODeregisterForSystemPower(&mut notifier);
            IOServiceClose(root);
            IONotificationPortDestroy(port);
        }
    }
    drop(sleep);
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(RunLoop(run_loop)) = self
            .run_loop
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            // SAFETY: retained by the thread; stopped, then released once.
            unsafe {
                CFRunLoopStop(run_loop);
                CFRelease(run_loop);
            }
        }
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
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
