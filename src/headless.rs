//! AppKit's event loop while LWSpot runs without a window (macOS).
//!
//! The menu-bar item, the Dock icon and its menu, and Apple Events (links,
//! reopen, quit) are only answered while AppKit's loop runs on the main
//! thread, so the shell runs it between headless ticks instead of sleeping.
//! It runs for the whole wait, until a deadline or until the shell's waker
//! interrupts it with work for the app, so an idle app is not woken several
//! times a second (fastframe-tray's pump stops every 150 ms).
//!
//! Both stops go through the main dispatch queue and carry the number of the
//! pump they are for. One that arrives late, after its pump returned, finds
//! another number (or none) and does nothing, so it can never end a later
//! pump or a window's event loop.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::NSPoint;

/// The pump running now, or 0. Written on the main thread only.
static RUNNING: AtomicU64 = AtomicU64::new(0);
/// The next pump's number; never 0.
static NEXT: AtomicU64 = AtomicU64::new(1);
/// An interrupt is already on its way to the main thread.
static PENDING: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    /// The main queue, `dispatch_get_main_queue()`.
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
    fn dispatch_after_f(
        when: u64,
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
    fn dispatch_time(when: u64, delta: i64) -> u64;
}

/// `DISPATCH_TIME_NOW`.
const NOW: u64 = 0;

fn main_queue() -> *const c_void {
    // Only the address of libdispatch's main queue object, never read here.
    (&raw const _dispatch_main_q).cast()
}

/// Runs AppKit's event loop for up to `duration`, or until [`interrupt`].
/// Call on the main thread, with no window's event loop running.
pub fn idle(duration: Duration) {
    let Some(mtm) = MainThreadMarker::new() else {
        std::thread::sleep(duration);
        return;
    };
    let pump = NEXT.fetch_add(1, Ordering::Relaxed);
    RUNNING.store(pump, Ordering::SeqCst);
    let delta = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);
    // SAFETY: the main queue outlives the process; the context is the pump's
    // number carried as a pointer-sized integer, never dereferenced.
    unsafe {
        dispatch_after_f(
            dispatch_time(NOW, delta),
            main_queue(),
            pump as usize as *mut c_void,
            deadline,
        );
    }
    // `run` reports an Objective-C exception raised while an event is
    // handled, as it does while a window is open.
    NSApplication::sharedApplication(mtm).run();
    RUNNING.store(0, Ordering::SeqCst);
}

/// Ends the running [`idle`] early, from any thread; does nothing when
/// none runs. Several calls before the main thread gets to it count once.
pub fn interrupt() {
    if !PENDING.swap(true, Ordering::AcqRel) {
        // SAFETY: as in `idle`; this work takes no context.
        unsafe { dispatch_async_f(main_queue(), std::ptr::null_mut(), interrupted) };
    }
}

extern "C" fn deadline(pump: *mut c_void) {
    stop(pump as usize as u64);
}

extern "C" fn interrupted(_context: *mut c_void) {
    PENDING.store(false, Ordering::Release);
    stop(RUNNING.load(Ordering::SeqCst));
}

/// On the main thread: stops pump `pump` if it is still the one running.
fn stop(pump: u64) {
    if pump == 0 || RUNNING.load(Ordering::SeqCst) != pump {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    // `stop:` takes effect after the next event, and dispatched work is not
    // one, so post a no-op event for `run` to return on.
    if let Some(event) =
        NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        )
    {
        app.postEvent_atStart(&event, true);
    }
}
