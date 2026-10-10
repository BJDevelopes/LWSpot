//! The system's default output endpoint, read from the Windows device
//! enumerator.
//!
//! The sink opens the default output by its name rather than as "the
//! default" (#689, #810). cpal opens "the default" through
//! `ActivateAudioInterfaceAsync` and registers a device notification client
//! whose callbacks initialise COM single-threaded on Windows' own pool
//! threads. Once a pool thread is left that way, a later activation that
//! lands on it fails with `RPC_E_CHANGED_MODE`, and the output cannot be
//! opened again until the app restarts. A device opened by name takes
//! neither path.
//!
//! `windows-sys` has no COM interface definitions, so the three interfaces
//! read here are declared as the vtables they are, as the thumbnail toolbar
//! does. Each lists its entries in order up to the last one called; the
//! entries after that are left out, since only the prefix's layout matters.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr;

use windows_sys::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows_sys::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows_sys::core::{GUID, HRESULT};

/// An output endpoint: its id, which never changes while it exists, and the
/// name fastframe-audio opens it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Endpoint {
    pub(super) id: String,
    pub(super) name: String,
}

const CLSID_MMDEVICE_ENUMERATOR: GUID = GUID::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const IID_IMMDEVICE_ENUMERATOR: GUID = GUID::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);

/// `eRender` and `eConsole`: the output Windows plays ordinary sound on.
const E_RENDER: i32 = 0;
const E_CONSOLE: i32 = 0;
/// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`: there is no default output.
const E_NOTFOUND: HRESULT = 0x8007_0490_u32 as HRESULT;
const STGM_READ: u32 = 0;
const VT_LPWSTR: u16 = 31;

#[repr(C)]
struct PropertyKey {
    fmtid: GUID,
    pid: u32,
}

/// The names cpal gives a device, in its order of preference.
const NAME_KEYS: [PropertyKey; 2] = [
    // PKEY_Device_FriendlyName
    PropertyKey {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 14,
    },
    // PKEY_Device_DeviceDesc
    PropertyKey {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 2,
    },
];

/// `PROPVARIANT`: a type tag, three reserved words, then a union two
/// pointers wide. A string value is the first pointer.
#[repr(C)]
struct PropVariant {
    vt: u16,
    reserved: [u16; 3],
    value: [usize; 2],
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn PropVariantClear(value: *mut PropVariant) -> HRESULT;
}

type QueryInterface =
    unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;

/// Layout, not data: the unread entries hold the called ones in place.
#[repr(C)]
#[allow(dead_code)]
struct IUnknownVtbl {
    query_interface: QueryInterface,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
#[allow(dead_code)]
struct IMMDeviceEnumeratorVtbl {
    base: IUnknownVtbl,
    enum_audio_endpoints:
        unsafe extern "system" fn(*mut c_void, i32, u32, *mut *mut c_void) -> HRESULT,
    get_default_audio_endpoint:
        unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> HRESULT,
}

#[repr(C)]
#[allow(dead_code)]
struct IMMDeviceVtbl {
    base: IUnknownVtbl,
    activate: unsafe extern "system" fn(
        *mut c_void,
        *const GUID,
        u32,
        *const c_void,
        *mut *mut c_void,
    ) -> HRESULT,
    open_property_store: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT,
    get_id: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> HRESULT,
}

#[repr(C)]
#[allow(dead_code)]
struct IPropertyStoreVtbl {
    base: IUnknownVtbl,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_at: unsafe extern "system" fn(*mut c_void, u32, *mut PropertyKey) -> HRESULT,
    get_value:
        unsafe extern "system" fn(*mut c_void, *const PropertyKey, *mut PropVariant) -> HRESULT,
}

/// An interface pointer this module holds one reference to, released on
/// drop.
struct Interface<V> {
    this: *mut c_void,
    vtable: PhantomData<*const V>,
}

impl<V> Interface<V> {
    /// Takes over the reference a successful call returned in `this`.
    fn take(this: *mut c_void) -> Option<Self> {
        (!this.is_null()).then_some(Self {
            this,
            vtable: PhantomData,
        })
    }

    fn vtable(&self) -> &V {
        // SAFETY: `this` is a live interface of type `V`, whose first field
        // points at its vtable.
        unsafe { &**(self.this as *const *const V) }
    }
}

impl<V> Drop for Interface<V> {
    fn drop(&mut self) {
        // SAFETY: every vtable starts with `IUnknown`'s, and this holds one
        // reference to give back.
        unsafe {
            let unknown = &**(self.this as *const *const IUnknownVtbl);
            (unknown.release)(self.this);
        }
    }
}

/// COM on the calling thread for as long as this lives, left as it was found.
///
/// A thread without COM gets the multithreaded apartment, which device
/// enumeration is made for, and loses it again on drop, so a thread this
/// borrows (librespot's player thread) is not changed. A thread that already
/// has an apartment keeps it: the enumerator works from either kind.
pub(super) struct Apartment {
    joined: bool,
    thread_bound: PhantomData<*const ()>,
}

impl Apartment {
    pub(super) fn enter() -> Result<Self, String> {
        // SAFETY: no reserved pointer; the matching uninitialise is on drop,
        // on this thread, because `Apartment` cannot leave it.
        let result = unsafe { CoInitializeEx(ptr::null(), COINIT_MULTITHREADED as u32) };
        if result >= 0 {
            Ok(Self {
                joined: true,
                thread_bound: PhantomData,
            })
        } else if result == RPC_E_CHANGED_MODE {
            Ok(Self {
                joined: false,
                thread_bound: PhantomData,
            })
        } else {
            Err(format!("COM is unavailable ({})", hresult(result)))
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.joined {
            // SAFETY: balances the successful initialise in `enter`.
            unsafe { CoUninitialize() };
        }
    }
}

fn hresult(result: HRESULT) -> String {
    format!("0x{:08X}", result as u32)
}

/// The current default output, `None` when the system has none.
pub(super) fn default_output() -> Result<Option<Endpoint>, String> {
    let _apartment = Apartment::enter()?;
    let mut enumerator = ptr::null_mut();
    // SAFETY: the identifiers are the enumerator's, and `enumerator` receives
    // a reference that `Interface` releases.
    let result = unsafe {
        CoCreateInstance(
            &CLSID_MMDEVICE_ENUMERATOR,
            ptr::null_mut(),
            CLSCTX_ALL,
            &IID_IMMDEVICE_ENUMERATOR,
            &mut enumerator,
        )
    };
    if result < 0 {
        return Err(format!("cannot list audio outputs ({})", hresult(result)));
    }
    let enumerator = Interface::<IMMDeviceEnumeratorVtbl>::take(enumerator)
        .ok_or("cannot list audio outputs")?;

    let mut device = ptr::null_mut();
    // SAFETY: a live enumerator, and `device` receives a reference that
    // `Interface` releases.
    let result = unsafe {
        (enumerator.vtable().get_default_audio_endpoint)(
            enumerator.this,
            E_RENDER,
            E_CONSOLE,
            &mut device,
        )
    };
    if result == E_NOTFOUND {
        return Ok(None);
    }
    if result < 0 {
        return Err(format!(
            "cannot find the default audio output ({})",
            hresult(result)
        ));
    }
    let Some(device) = Interface::<IMMDeviceVtbl>::take(device) else {
        return Ok(None);
    };
    let id = id(&device)?;
    let name = name(&device)?;
    Ok(Some(Endpoint { id, name }))
}

fn id(device: &Interface<IMMDeviceVtbl>) -> Result<String, String> {
    let mut id = ptr::null_mut();
    // SAFETY: a live device; the string it returns is freed below.
    let result = unsafe { (device.vtable().get_id)(device.this, &mut id) };
    if result < 0 || id.is_null() {
        return Err(format!(
            "cannot identify the default audio output ({})",
            hresult(result)
        ));
    }
    // SAFETY: a NUL-terminated string from COM's allocator, read once and
    // given back to it.
    unsafe {
        let text = wide(id);
        CoTaskMemFree(id as *const c_void);
        Ok(text)
    }
}

fn name(device: &Interface<IMMDeviceVtbl>) -> Result<String, String> {
    let mut store = ptr::null_mut();
    // SAFETY: a live device, and `store` receives a reference that
    // `Interface` releases.
    let result =
        unsafe { (device.vtable().open_property_store)(device.this, STGM_READ, &mut store) };
    let store = (result >= 0)
        .then(|| Interface::<IPropertyStoreVtbl>::take(store))
        .flatten()
        .ok_or_else(|| {
            format!(
                "cannot read the default audio output's name ({})",
                hresult(result)
            )
        })?;
    for key in &NAME_KEYS {
        let mut value = PropVariant {
            vt: 0,
            reserved: [0; 3],
            value: [0; 2],
        };
        // SAFETY: a live store, a valid key, and a value cleared below.
        let result = unsafe { (store.vtable().get_value)(store.this, key, &mut value) };
        if result < 0 {
            continue;
        }
        let text = (value.vt == VT_LPWSTR && value.value[0] != 0)
            // SAFETY: a string value is a NUL-terminated wide string.
            .then(|| unsafe { wide(value.value[0] as *const u16) });
        // SAFETY: frees what `get_value` allocated, whatever its type.
        unsafe { PropVariantClear(&mut value) };
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            return Ok(text);
        }
    }
    Err("the default audio output has no name".into())
}

/// Reads a NUL-terminated UTF-16 string.
///
/// # Safety
///
/// `text` must point at a NUL-terminated wide string.
unsafe fn wide(text: *const u16) -> String {
    let mut len = 0;
    // SAFETY: the caller promises a terminator, which ends the scan.
    unsafe {
        while *text.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine without audio has no default output, which is an answer,
    /// not an error. One with audio names it, and the name is one
    /// fastframe-audio lists, so opening by it finds the same device.
    #[test]
    fn the_default_output_is_named_as_fastframe_lists_it() {
        let Ok(Some(endpoint)) = default_output() else {
            return;
        };
        assert!(!endpoint.id.is_empty());
        let names = fastframe_audio::output_device_names().unwrap_or_default();
        assert!(names.contains(&endpoint.name), "{endpoint:?} in {names:?}");
    }

    /// Reading the default leaves a thread's COM apartment as it was, so
    /// cpal's own single-threaded initialisation still succeeds afterwards.
    #[test]
    fn reading_the_default_leaves_the_thread_as_it_was() {
        std::thread::spawn(|| {
            let _ = default_output();
            // SAFETY: balanced below on success, on this thread.
            let result = unsafe {
                CoInitializeEx(
                    ptr::null(),
                    windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32,
                )
            };
            assert!(result >= 0, "{}", hresult(result));
            unsafe { CoUninitialize() };
        })
        .join()
        .unwrap();
    }
}
