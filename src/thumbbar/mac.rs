//! The Dock menu: Previous, Play or Pause, and Next when the Dock icon is
//! right-clicked, the Mac's counterpart of the taskbar thumbnail buttons.
//!
//! AppKit asks the application delegate for this menu with
//! `applicationDockMenu:`. winit owns the delegate, so the method is added
//! to its class once, and answers with the one menu kept here. The items
//! send their action to a small target object, which queues the command
//! and wakes the app, the way the Windows toolbar's `WM_COMMAND` does.
//! Everything runs on the main thread, where AppKit and the window shell
//! both live.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

use super::{ThumbCommand, ThumbState};
use crate::i18n::{Locale, gettext};

thread_local! {
    /// The menu `applicationDockMenu:` answers with, while one is attached.
    static MENU: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
    /// Whether the delegate class has the method already.
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// What the target's actions write to.
struct Ivars {
    commands: Rc<RefCell<Vec<ThumbCommand>>>,
    wake: Box<dyn Fn()>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LWSpotDockMenuTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        #[unsafe(method(previous:))]
        fn previous(&self, _sender: *mut AnyObject) {
            self.press(ThumbCommand::Previous);
        }

        #[unsafe(method(playPause:))]
        fn play_pause(&self, _sender: *mut AnyObject) {
            self.press(ThumbCommand::PlayPause);
        }

        #[unsafe(method(next:))]
        fn next(&self, _sender: *mut AnyObject) {
            self.press(ThumbCommand::Next);
        }
    }

    unsafe impl NSObjectProtocol for Target {}
);

impl Target {
    fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    fn press(&self, command: ThumbCommand) {
        self.ivars().commands.borrow_mut().push(command);
        (self.ivars().wake)();
    }
}

/// `applicationDockMenu:`, added to winit's application delegate.
unsafe extern "C-unwind" fn dock_menu(
    _this: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut AnyObject,
) -> *mut NSMenu {
    MENU.with_borrow(|menu| {
        menu.as_ref().map_or(std::ptr::null_mut(), |menu| {
            Retained::as_ptr(menu).cast_mut()
        })
    })
}

/// Teaches the application delegate's class to answer with [`MENU`].
fn install(mtm: MainThreadMarker) -> bool {
    if INSTALLED.get() {
        return true;
    }
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    // SAFETY: `delegate` returns the application delegate or nil.
    let delegate: *mut AnyObject = unsafe { msg_send![&app, delegate] };
    if delegate.is_null() {
        return false;
    }
    // SAFETY: a live object's class; the method's types match `dock_menu`
    // (an object returned, the receiver, the selector and one object).
    let added = unsafe {
        let class: *const AnyClass = objc2::ffi::object_getClass(delegate);
        let imp: Imp = std::mem::transmute::<
            unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> *mut NSMenu,
            Imp,
        >(dock_menu);
        objc2::ffi::class_addMethod(
            class.cast_mut(),
            sel!(applicationDockMenu:),
            imp,
            c"@@:@".as_ptr(),
        )
        .as_bool()
    };
    if !added {
        log::warn!("the application delegate already has a Dock menu; LWSpot's is not shown");
        return false;
    }
    INSTALLED.set(true);
    log::info!("Dock menu installed");
    true
}

/// The Dock menu. It belongs to the application, not a window: the Dock
/// icon stays while LWSpot runs without one (closed to the menu bar, or
/// started at log in), so the menu lives in [`STATE`] for the process's
/// life. Every `ThumbBar` is a handle on it; the window shell and the app's
/// background pass both sync it and take its presses.
#[derive(Default)]
pub struct ThumbBar;

struct Attached {
    /// The items' action target, kept alive as long as they are.
    _target: Retained<Target>,
    items: [Retained<NSMenuItem>; super::COUNT],
    commands: Rc<RefCell<Vec<ThumbCommand>>>,
    shown: Option<ThumbState>,
    locale: Locale,
}

thread_local! {
    /// The menu's items and presses, once attached; main thread only.
    static STATE: RefCell<Option<Attached>> = const { RefCell::new(None) };
}

impl ThumbBar {
    pub fn new() -> Self {
        Self
    }

    /// Builds the menu and offers it to the Dock, once per process; later
    /// windows find it there. `_view` is unused: the Dock menu belongs to
    /// the application, not a window.
    ///
    /// # Safety
    /// Call on the main thread.
    pub unsafe fn attach(&mut self, _view: isize, wake: impl Fn() + Send + Sync + 'static) {
        if STATE.with_borrow(Option::is_some) {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if !install(mtm) {
            return;
        }
        let commands = Rc::new(RefCell::new(Vec::new()));
        let target = Target::new(
            mtm,
            Ivars {
                commands: Rc::clone(&commands),
                wake: Box::new(wake),
            },
        );
        let menu = NSMenu::new(mtm);
        // The items are enabled by `sync`, not by AppKit's validation.
        menu.setAutoenablesItems(false);
        let item = |action: Sel| {
            // SAFETY: an empty title and key equivalent, and an action the
            // target implements.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(""),
                    Some(action),
                    &NSString::from_str(""),
                )
            };
            // SAFETY: the target lives in STATE for the rest of the process.
            unsafe { item.setTarget(Some(&target)) };
            menu.addItem(&item);
            item
        };
        let items = [
            item(sel!(previous:)),
            item(sel!(playPause:)),
            item(sel!(next:)),
        ];
        MENU.set(Some(menu));
        STATE.set(Some(Attached {
            _target: target,
            items,
            commands,
            shown: None,
            locale: Locale::default(),
        }));
        relabel();
    }

    /// Nothing to undo: the Dock menu outlives each window.
    pub fn detach(&mut self) {}

    /// The interface language for the items' titles.
    pub fn set_locale(&mut self, locale: Locale) {
        let changed = STATE.with_borrow_mut(|state| match state {
            Some(attached) if attached.locale != locale => {
                attached.locale = locale;
                true
            }
            _ => false,
        });
        if changed {
            relabel();
        }
    }

    /// Shows `state` in the menu; nothing is touched while it is unchanged.
    pub fn sync(&mut self, shown: ThumbState) {
        let changed = STATE.with_borrow_mut(|state| match state {
            Some(attached) if attached.shown != Some(shown) => {
                attached.shown = Some(shown);
                true
            }
            _ => false,
        });
        if changed {
            relabel();
        }
    }

    pub fn drain_commands(&self) -> Vec<ThumbCommand> {
        STATE.with_borrow(|state| {
            state
                .as_ref()
                .map(|attached| std::mem::take(&mut *attached.commands.borrow_mut()))
                .unwrap_or_default()
        })
    }
}

/// Writes the titles and enabled states for what is shown now.
fn relabel() {
    STATE.with_borrow(|state| {
        let Some(attached) = state else {
            return;
        };
        let shown = attached.shown.unwrap_or_default();
        for (item, button) in attached.items.iter().zip(super::buttons(&shown)) {
            item.setTitle(&NSString::from_str(&gettext(attached.locale, button.tip)));
            item.setEnabled(button.enabled);
        }
    });
}
