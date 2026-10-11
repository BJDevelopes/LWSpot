//! The user's own startup list, in `HKEY_CURRENT_USER`.
//!
//! Plain registry calls on whichever thread asks; each one opens the key it
//! needs and closes it again, so nothing is held between them.

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};

/// Where Windows looks for what to start at sign-in.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager and Settings record that an entry was turned off.
const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// `HKCU`'s Run key and its `StartupApproved` marker.
pub struct System;

impl super::Registry for System {
    fn run(&self, name: &str) -> Option<String> {
        let bytes = read(RUN_KEY, name, RRF_RT_REG_SZ)?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|unit| *unit != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    fn set_run(&mut self, name: &str, command: &str) -> std::io::Result<()> {
        let data = wide(command);
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                wide(name).as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        result(status)
    }

    fn delete_run(&mut self, name: &str) -> std::io::Result<()> {
        delete(RUN_KEY, name)
    }

    fn approval(&self, name: &str) -> Option<Vec<u8>> {
        read(APPROVED_KEY, name, RRF_RT_REG_BINARY)
    }

    fn delete_approval(&mut self, name: &str) -> std::io::Result<()> {
        delete(APPROVED_KEY, name)
    }
}

/// The bytes of the value `name` under `key`, if it is there with the type
/// `kind` asks for.
fn read(key: &str, name: &str, kind: u32) -> Option<Vec<u8>> {
    let key = wide(key);
    let name = wide(name);
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            kind,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            kind,
            std::ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    data.truncate(size as usize);
    Some(data)
}

/// Removes the value `name` under `key`; one that is not there is done.
fn delete(key: &str, name: &str) -> std::io::Result<()> {
    let status =
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(key).as_ptr(), wide(name).as_ptr()) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    result(status)
}

fn result(status: WIN32_ERROR) -> std::io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status as i32))
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Writes a `StartupApproved` marker, as Task Manager does, for the test
/// that exercises the real registry under a test name.
#[cfg(test)]
pub(super) fn set_approval(name: &str, marker: &[u8]) -> std::io::Result<()> {
    use windows_sys::Win32::System::Registry::REG_BINARY;
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            wide(APPROVED_KEY).as_ptr(),
            wide(name).as_ptr(),
            REG_BINARY,
            marker.as_ptr().cast(),
            marker.len() as u32,
        )
    };
    result(status)
}
