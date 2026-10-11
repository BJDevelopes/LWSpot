//! The Win32 half of the jump list.
//!
//! As in `thumbbar/win.rs`, `windows-sys` carries the class identifiers and
//! the COM functions but no interface definitions, so the four interfaces
//! used here are declared below as the vtables they are: the entries have
//! to be present and in order for the few that are called to sit where
//! Windows looks for them.
//!
//! The list is built on a thread this module starts and owns, which joins a
//! single-threaded apartment for the shell objects and leaves it before it
//! ends. Builds are taken one at a time, so a language change made while
//! the startup list is still being written cannot interleave with it.
//!
//! No AppUserModelID is set, on the list or on the process. Nothing else in
//! LWSpot sets one (the windows, the tray, the media controls and the Start
//! menu shortcut all go by the one Windows derives from the program's
//! path), and the list attaches to the taskbar button with the same
//! identity only when both agree.

use std::ffi::c_void;
use std::sync::Mutex;

use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::UI::Shell::{DestinationList, EnumerableObjectCollection, ShellLink};
use windows_sys::core::{GUID, HRESULT};

use super::Task;

const IID_ICUSTOMDESTINATIONLIST: GUID = GUID::from_u128(0x6332debf_87b5_4670_90c0_5e57b408a49e);
const IID_IOBJECTARRAY: GUID = GUID::from_u128(0x92ca9dcd_5622_4bba_a805_5e9f541bd8c9);
const IID_IOBJECTCOLLECTION: GUID = GUID::from_u128(0x5632b1a4_e38a_400a_928a_d4cd63230295);
const IID_ISHELLLINKW: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPROPERTYSTORE: GUID = GUID::from_u128(0x886d8eeb_8cf2_4446_8d02_cdba1dbdcf99);

/// `PROPERTYKEY`, which `windows-sys` keeps behind a feature LWSpot does
/// not otherwise need.
#[repr(C)]
struct PropertyKey {
    fmtid: GUID,
    pid: u32,
}

/// `PKEY_Title`: what a task in a jump list is called.
const PKEY_TITLE: PropertyKey = PropertyKey {
    fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
    pid: 2,
};

/// `VT_LPWSTR`.
const VT_LPWSTR: u16 = 31;

/// A `PROPVARIANT` holding a string: the type, three reserved words, and
/// the pointer, padded to the union's size (16 bytes on 32-bit Windows, 24
/// on 64-bit).
#[repr(C)]
struct PropVariant {
    vt: u16,
    reserved: [u16; 3],
    text: *const u16,
    padding: usize,
}

const _: () = assert!(
    size_of::<PropVariant>()
        == if cfg!(target_pointer_width = "64") {
            24
        } else {
            16
        }
);

#[repr(C)]
struct IUnknownVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

/// Layout, not data, like the taskbar's vtable in `thumbbar/win.rs`.
#[repr(C)]
#[allow(dead_code)]
struct ICustomDestinationListVtbl {
    unknown: IUnknownVtbl,
    set_app_id: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    begin_list:
        unsafe extern "system" fn(*mut c_void, *mut u32, *const GUID, *mut *mut c_void) -> HRESULT,
    append_category: unsafe extern "system" fn(*mut c_void, *const u16, *mut c_void) -> HRESULT,
    append_known_category: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
    add_user_tasks: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
    commit_list: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    get_removed_destinations:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    delete_list: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    abort_list: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

/// `IObjectCollection`, after the two entries of `IObjectArray`.
#[repr(C)]
#[allow(dead_code)]
struct IObjectCollectionVtbl {
    unknown: IUnknownVtbl,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_at: unsafe extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void) -> HRESULT,
    add_object: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
    add_from_array: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT,
    remove_object_at: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    clear: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

#[repr(C)]
#[allow(dead_code)]
struct IShellLinkWVtbl {
    unknown: IUnknownVtbl,
    get_path: unsafe extern "system" fn(*mut c_void, *mut u16, i32, *mut c_void, u32) -> HRESULT,
    get_id_list: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
    set_id_list: unsafe extern "system" fn(*mut c_void, *const c_void) -> HRESULT,
    get_description: unsafe extern "system" fn(*mut c_void, *mut u16, i32) -> HRESULT,
    set_description: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_working_directory: unsafe extern "system" fn(*mut c_void, *mut u16, i32) -> HRESULT,
    set_working_directory: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_arguments: unsafe extern "system" fn(*mut c_void, *mut u16, i32) -> HRESULT,
    set_arguments: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_hotkey: unsafe extern "system" fn(*mut c_void, *mut u16) -> HRESULT,
    set_hotkey: unsafe extern "system" fn(*mut c_void, u16) -> HRESULT,
    get_show_cmd: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
    set_show_cmd: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
    get_icon_location: unsafe extern "system" fn(*mut c_void, *mut u16, i32, *mut i32) -> HRESULT,
    set_icon_location: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
    set_relative_path: unsafe extern "system" fn(*mut c_void, *const u16, u32) -> HRESULT,
    resolve: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> HRESULT,
    set_path: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
}

#[repr(C)]
#[allow(dead_code)]
struct IPropertyStoreVtbl {
    unknown: IUnknownVtbl,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_at: unsafe extern "system" fn(*mut c_void, u32, *mut PropertyKey) -> HRESULT,
    get_value:
        unsafe extern "system" fn(*mut c_void, *const PropertyKey, *mut PropVariant) -> HRESULT,
    set_value:
        unsafe extern "system" fn(*mut c_void, *const PropertyKey, *const PropVariant) -> HRESULT,
    commit: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

/// One build at a time.
static BUILDING: Mutex<()> = Mutex::new(());

/// Builds the list on a thread of its own.
pub(super) fn publish(tasks: Vec<Task>) {
    let spawned = std::thread::Builder::new()
        .name("jump-list".into())
        .spawn(move || {
            let _one_at_a_time = BUILDING.lock().unwrap_or_else(|p| p.into_inner());
            match build(&tasks) {
                Ok(slots) => log::info!(
                    "jump list: {} tasks committed (S_OK, {slots} slots)",
                    tasks.len()
                ),
                Err(failure) => log::warn!("jump list not written: {failure}"),
            }
        });
    if let Err(error) = spawned {
        log::warn!("jump list not written: no thread for it ({error})");
    }
}

/// A COM call that failed, and with what.
struct Failure {
    step: &'static str,
    code: HRESULT,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed ({:#010x})", self.step, self.code)
    }
}

fn check(step: &'static str, code: HRESULT) -> Result<(), Failure> {
    if code < 0 {
        Err(Failure { step, code })
    } else {
        Ok(())
    }
}

/// This thread's place in a single-threaded apartment, left on drop.
struct Apartment;

impl Apartment {
    fn join() -> Result<Self, Failure> {
        let joined = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        check("CoInitializeEx", joined)?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// One interface pointer, released on drop. Declared after the
/// [`Apartment`] in every scope, so it goes first.
struct Object(*mut c_void);

impl Object {
    fn create(class: &GUID, interface: &GUID, step: &'static str) -> Result<Self, Failure> {
        let mut object = std::ptr::null_mut();
        let created = unsafe {
            CoCreateInstance(
                class,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                interface,
                &mut object,
            )
        };
        Self::taken(step, created, object)
    }

    fn query(&self, interface: &GUID, step: &'static str) -> Result<Self, Failure> {
        let mut object = std::ptr::null_mut();
        let found = unsafe {
            (self.vtable::<IUnknownVtbl>().query_interface)(self.0, interface, &mut object)
        };
        Self::taken(step, found, object)
    }

    fn taken(step: &'static str, code: HRESULT, object: *mut c_void) -> Result<Self, Failure> {
        check(step, code)?;
        if object.is_null() {
            return Err(Failure { step, code });
        }
        Ok(Self(object))
    }

    /// The object's vtable, read as `V`.
    ///
    /// # Safety
    /// The object must implement the interface `V` describes.
    unsafe fn vtable<V>(&self) -> &V {
        unsafe { &**self.0.cast::<*const V>() }
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        unsafe { (self.vtable::<IUnknownVtbl>().release)(self.0) };
    }
}

/// Writes the list, and says how many entries Windows will show.
fn build(tasks: &[Task]) -> Result<u32, Failure> {
    let exe = std::env::current_exe().map_err(|error| Failure {
        step: "current_exe",
        code: error.raw_os_error().unwrap_or(-1),
    })?;
    let exe = wide(&exe.to_string_lossy());
    let _apartment = Apartment::join()?;
    let list = Object::create(
        &DestinationList,
        &IID_ICUSTOMDESTINATIONLIST,
        "CoCreateInstance(DestinationList)",
    )?;
    let list_vtable = unsafe { list.vtable::<ICustomDestinationListVtbl>() };
    let mut slots = 0u32;
    let mut removed = std::ptr::null_mut();
    check("BeginList", unsafe {
        (list_vtable.begin_list)(list.0, &mut slots, &IID_IOBJECTARRAY, &mut removed)
    })?;
    // Only destinations can be removed by the user; tasks cannot.
    if !removed.is_null() {
        drop(Object(removed));
    }
    let written = (|| {
        let collection = Object::create(
            &EnumerableObjectCollection,
            &IID_IOBJECTCOLLECTION,
            "CoCreateInstance(EnumerableObjectCollection)",
        )?;
        for task in tasks {
            let link = link(&exe, task)?;
            check("AddObject", unsafe {
                (collection.vtable::<IObjectCollectionVtbl>().add_object)(collection.0, link.0)
            })?;
        }
        let array = collection.query(&IID_IOBJECTARRAY, "QueryInterface(IObjectArray)")?;
        check("AddUserTasks", unsafe {
            (list_vtable.add_user_tasks)(list.0, array.0)
        })?;
        check("CommitList", unsafe { (list_vtable.commit_list)(list.0) })
    })();
    if written.is_err() {
        unsafe { (list_vtable.abort_list)(list.0) };
    }
    written.map(|()| slots)
}

/// A shell link that starts this program with the task's arguments, named
/// by its title.
fn link(exe: &[u16], task: &Task) -> Result<Object, Failure> {
    let link = Object::create(&ShellLink, &IID_ISHELLLINKW, "CoCreateInstance(ShellLink)")?;
    let vtable = unsafe { link.vtable::<IShellLinkWVtbl>() };
    let title = wide(&task.title);
    check("SetPath", unsafe {
        (vtable.set_path)(link.0, exe.as_ptr())
    })?;
    check("SetArguments", unsafe {
        (vtable.set_arguments)(link.0, wide(task.arguments).as_ptr())
    })?;
    check("SetIconLocation", unsafe {
        (vtable.set_icon_location)(link.0, exe.as_ptr(), 0)
    })?;
    check("SetDescription", unsafe {
        (vtable.set_description)(link.0, title.as_ptr())
    })?;
    let store = link.query(&IID_IPROPERTYSTORE, "QueryInterface(IPropertyStore)")?;
    let store_vtable = unsafe { store.vtable::<IPropertyStoreVtbl>() };
    // The store copies the string, so `title` only has to outlive the call.
    let value = PropVariant {
        vt: VT_LPWSTR,
        reserved: [0; 3],
        text: title.as_ptr(),
        padding: 0,
    };
    check("SetValue(PKEY_Title)", unsafe {
        (store_vtable.set_value)(store.0, &PKEY_TITLE, &value)
    })?;
    check("Commit", unsafe { (store_vtable.commit)(store.0) })?;
    drop(store);
    Ok(link)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    /// Writes a jump list for the test binary itself (its own taskbar
    /// identity, never lwspot.exe's) and checks every COM call succeeds.
    /// Run with `cargo test --lib jumplist -- --ignored`.
    #[test]
    #[ignore = "writes a jump list through the Windows shell"]
    fn the_shell_takes_the_list() {
        let tasks = super::super::tasks(crate::i18n::Locale::German);
        match super::build(&tasks) {
            Ok(slots) => assert!(slots > 0, "Windows shows no entries"),
            Err(failure) => panic!("{failure}"),
        }
    }
}
