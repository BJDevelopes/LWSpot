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
                    self.ivars()
                        .commands
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(ControlCommand::OpenLink(uri));
                    (self.ivars().wake)();
                }
                None => log::warn!("not a Spotify link: {text}"),
            }
        }
    }

    unsafe impl NSObjectProtocol for Handler {}
);

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
    // The manager does not retain its handler, and links may come until
    // the app quits.
    std::mem::forget(handler);
}
