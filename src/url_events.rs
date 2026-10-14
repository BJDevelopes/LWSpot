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
//! The same handler answers a Dock click while the app has no window
//! ([`take_over_reopen`]): macOS sends the running app a "reopen" event,
//! which becomes the same Show a second launch sends. AppKit would pass it
//! to the application delegate, and winit's delegate only exists while a
//! window does, so with the window closed (or never opened, after starting
//! in the menu bar) a Dock click would otherwise do nothing.

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

/// Answers Dock clicks with Show from now on. Call on the main thread each
/// time the app goes without a window: AppKit installs its own reopen
/// handler as it starts each event loop, so this takes the event back after
/// it. Does nothing before [`install`].
pub fn take_over_reopen() {
    HANDLER.with_borrow(|handler| {
        let Some(handler) = handler else {
            return;
        };
        let manager = NSAppleEventManager::sharedAppleEventManager();
        // SAFETY: as in `install`.
        unsafe {
            let _: () = msg_send![
                &manager,
                setEventHandler: &**handler,
                andSelector: sel!(handleReopen:withReplyEvent:),
                forEventClass: CORE_EVENT,
                andEventID: REOPEN,
            ];
        }
    });
}
