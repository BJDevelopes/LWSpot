//! Spotify links handed over by macOS.
//!
//! On Windows a link starts `lwspot.exe <link>`, which passes it to the
//! running copy over the single-instance channel. On a Mac, Launch Services
//! sends the running app (starting it first if need be) a "get URL" Apple
//! Event instead, for the `spotify` scheme `Info.plist` claims. The handler
//! here parses the link like a command-line one and queues it with the
//! channel's other commands, so the app opens it the same way.
//!
//! The handler is registered before the event loop starts, so the event
//! that launched the app is handled too: AppKit holds Apple Events until its
//! run loop runs, and leaves this event class alone because winit's delegate
//! has no `application:openURLs:`.
//!
//! The same handler answers two events while the app has no window
//! ([`take_over_app_events`]):
//! - A Dock click, which macOS sends as "reopen" and which becomes the same
//!   Show a second launch sends. AppKit would pass it to the application
//!   delegate, which does nothing without a window.
//! - Quit (from the Dock, at log out, restart or shut down), which becomes
//!   the app's own Quit, so it saves and shuts down in order. AppKit's own
//!   handler would end the process at once, as no window is there to close.
//!   The reply says yes, so a log out is never held up or cancelled.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, PoisonError};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSObject, NSObjectProtocol};

use crate::single_instance::ControlCommand;

/// `kInternetEventClass` and `kAEGetURL`, both 'GURL'.
const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
/// `keyDirectObject`, '----': the event's URL.
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
/// `kCoreEventClass` and `kAEReopenApplication`: a Dock click or a launch
/// of the running app.
const CORE_EVENT: u32 = u32::from_be_bytes(*b"aevt");
const REOPEN: u32 = u32::from_be_bytes(*b"rapp");
/// `kAEQuitApplication`.
const QUIT: u32 = u32::from_be_bytes(*b"quit");

/// Set by a quit event while windowless, taken by [`take_quit_request`].
static QUIT_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// The one handler, kept for the life of the process: the Apple Event
    /// manager does not retain it.
    static HANDLER: RefCell<Option<Retained<Handler>>> = const { RefCell::new(None) };
}

struct Ivars {
    commands: Arc<Mutex<Vec<ControlCommand>>>,
    wake: Box<dyn Fn() + Send + Sync>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "LWSpotURLEventHandler"]
    #[ivars = Ivars]
    struct Handler;

    impl Handler {
        #[unsafe(method(handleGetURL:withReplyEvent:))]
        fn handle_get_url(&self, event: &NSAppleEventDescriptor, _reply: *mut AnyObject) {
            // SAFETY: `paramDescriptorForKeyword:` returns a descriptor or
            // nil, and the descriptor's `stringValue` an NSString or nil.
            let descriptor: Option<Retained<NSAppleEventDescriptor>> =
                unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
            let Some(text) = descriptor.and_then(|descriptor| descriptor.stringValue()) else {
                return;
            };
            let text = text.to_string();
            match crate::link::parse(&text) {
                Some(uri) => {
                    log::info!("opening a link from macOS");
                    self.queue(ControlCommand::OpenLink(uri));
                }
                None => log::warn!("not a Spotify link: {text}"),
            }
        }

        #[unsafe(method(handleReopen:withReplyEvent:))]
        fn handle_reopen(&self, _event: &NSAppleEventDescriptor, _reply: *mut AnyObject) {
            log::debug!("reopen from macOS");
            self.queue(ControlCommand::Show);
        }

        #[unsafe(method(handleQuit:withReplyEvent:))]
        fn handle_quit(&self, _event: &NSAppleEventDescriptor, _reply: *mut AnyObject) {
            log::info!("quit from macOS");
            QUIT_REQUESTED.store(true, std::sync::atomic::Ordering::Release);
            (self.ivars().wake)();
        }
    }

    unsafe impl NSObjectProtocol for Handler {}
);

impl Handler {
    fn queue(&self, command: ControlCommand) {
        self.ivars()
            .commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(command);
        (self.ivars().wake)();
    }
}

/// Starts taking links from macOS into `commands`, waking the app for each.
/// Call once, on the main thread, before the event loop runs.
pub fn install(commands: Arc<Mutex<Vec<ControlCommand>>>, wake: impl Fn() + Send + Sync + 'static) {
    let handler: Retained<Handler> = {
        let this = Handler::alloc().set_ivars(Ivars {
            commands,
            wake: Box::new(wake),
        });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    };
    let manager = NSAppleEventManager::sharedAppleEventManager();
    // SAFETY: the selector is the handler's, with the signature Apple Event
    // handlers take; the handler is kept for the life of the process below.
    unsafe {
        let _: () = msg_send![
            &manager,
            setEventHandler: &*handler,
            andSelector: sel!(handleGetURL:withReplyEvent:),
            forEventClass: GET_URL,
            andEventID: GET_URL,
        ];
    }
    HANDLER.with_borrow_mut(|slot| *slot = Some(handler));
}

/// Answers Dock clicks with Show, and quit events with the app's own Quit,
/// from now on. Call on the main thread each time the app goes without a
/// window: AppKit installs its own handlers as it starts each event loop, so
/// this takes the events back after it. Does nothing before [`install`].
pub fn take_over_app_events() {
    HANDLER.with_borrow(|handler| {
        let Some(handler) = handler else {
            return;
        };
        let manager = NSAppleEventManager::sharedAppleEventManager();
        for (selector, event) in [
            (sel!(handleReopen:withReplyEvent:), REOPEN),
            (sel!(handleQuit:withReplyEvent:), QUIT),
        ] {
            // SAFETY: as in `install`.
            unsafe {
                let _: () = msg_send![
                    &manager,
                    setEventHandler: &**handler,
                    andSelector: selector,
                    forEventClass: CORE_EVENT,
                    andEventID: event,
                ];
            }
        }
    });
}

/// Whether macOS asked the app to quit since the last call.
pub fn take_quit_request() -> bool {
    QUIT_REQUESTED.swap(false, std::sync::atomic::Ordering::AcqRel)
}

/// Whether LWSpot is running without a window (closed to the menu bar).
static WINDOWLESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tells [`install_terminate_handler`]'s handler whether a window is open.
pub fn set_windowless(windowless: bool) {
    WINDOWLESS.store(windowless, std::sync::atomic::Ordering::Release);
}

/// `NSTerminateCancel` and `NSTerminateNow`.
const TERMINATE_CANCEL: usize = 0;
const TERMINATE_NOW: usize = 1;

/// `applicationShouldTerminate:`, added to winit's application delegate.
/// Quit from the app menu or ⌘Q while windowless would end the process at
/// once; instead it becomes the app's own Quit, which saves and shuts down,
/// and AppKit's terminate is cancelled. Log out, restart and Dock Quit come
/// as quit events, which [`take_over_app_events`] handles, so this never
/// holds those up. With a window, AppKit goes ahead as usual.
unsafe extern "C-unwind" fn should_terminate(
    _this: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _sender: *mut AnyObject,
) -> usize {
    if !WINDOWLESS.load(std::sync::atomic::Ordering::Acquire) {
        return TERMINATE_NOW;
    }
    log::info!("quit from the app menu while windowless");
    QUIT_REQUESTED.store(true, std::sync::atomic::Ordering::Release);
    HANDLER.with_borrow(|handler| {
        if let Some(handler) = handler {
            (handler.ivars().wake)();
        }
    });
    TERMINATE_CANCEL
}

/// Adds [`should_terminate`] to winit's application delegate, once. Call on
/// the main thread with a window open (the delegate exists then).
pub fn install_terminate_handler() {
    static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if INSTALLED.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    // SAFETY: `delegate` returns the application delegate or nil.
    let delegate: *mut AnyObject = unsafe { msg_send![&app, delegate] };
    if delegate.is_null() {
        return;
    }
    // SAFETY: a live object's class; the method's types match
    // `should_terminate` (an NSUInteger returned, the receiver, the
    // selector and one object). class_addMethod leaves an existing method
    // alone and returns NO.
    let added = unsafe {
        let class: *const objc2::runtime::AnyClass = objc2::ffi::object_getClass(delegate);
        let imp: objc2::runtime::Imp = std::mem::transmute::<
            unsafe extern "C-unwind" fn(
                *mut AnyObject,
                objc2::runtime::Sel,
                *mut AnyObject,
            ) -> usize,
            objc2::runtime::Imp,
        >(should_terminate);
        objc2::ffi::class_addMethod(
            class.cast_mut(),
            sel!(applicationShouldTerminate:),
            imp,
            c"Q@:@".as_ptr(),
        )
        .as_bool()
    };
    if !added {
        log::warn!(
            "the application delegate already decides on quitting; ⌘Q without a window will not save"
        );
    }
    INSTALLED.store(true, std::sync::atomic::Ordering::Release);
}
