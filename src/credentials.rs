//! Durable Spotify grants and proxy passwords in the platform credential store.
//!
//! All native calls run on one dedicated thread. A locked store cannot hold
//! the UI, command loop, or runtime shutdown hostage. Generation checks reject
//! work from before sign-out, including a write that returns after sign-out.
//! Non-secret revocation markers prevent restoration after a failed deletion.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::time::Duration;

use librespot_core::authentication::Credentials;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{auth::StoredToken, paths::AppDirs};

const SERVICE: &str = "io.github.BJDevelopes.LWSpot";
/// How long a credential operation may take. On a Mac it can be a person:
/// after an ad-hoc-signed update the Keychain asks for the login password
/// once per saved item, and those prompts queue up behind each other.
const TIMEOUT: Duration = if cfg!(target_os = "macos") {
    Duration::from_secs(180)
} else {
    Duration::from_secs(20)
};
/// Startup reads retry a refusing store after 250 ms, 500 ms and 1 s.
const STARTUP_RETRIES: u32 = 3;
const STARTUP_RETRY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Slot {
    Shared,
    Personal,
    Playback,
    Proxy,
}

impl Slot {
    pub const SPOTIFY: [Self; 3] = [Self::Shared, Self::Personal, Self::Playback];
    pub const ALL: [Self; 4] = [Self::Shared, Self::Personal, Self::Playback, Self::Proxy];
    pub(crate) fn index(self) -> usize {
        self as usize
    }
    fn name(self) -> &'static str {
        match self {
            Self::Shared => "shared-web",
            Self::Personal => "personal-web",
            Self::Playback => "playback",
            Self::Proxy => "proxy-password",
        }
    }
}

/// Deliberately has no Debug implementation: it contains usable secrets.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum Grant {
    Web(StoredToken),
    Playback(Credentials),
    Proxy(ProxyPassword),
}

/// No Debug implementation: the password is usable, and the username is private.
/// A saved password belongs to one network endpoint and username.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyPassword {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

impl ProxyPassword {
    fn valid(&self) -> bool {
        let settings = crate::settings::Settings {
            proxy_host: self.host.clone(),
            proxy_port: self.port.to_string(),
            proxy_username: self.username.clone(),
            proxy_password: self.password.clone(),
            ..Default::default()
        };
        settings.proxy_password_record().ok().flatten().as_ref() == Some(self)
    }
}

impl Grant {
    fn valid_for(&self, slot: Slot) -> bool {
        match (slot, self) {
            (Slot::Shared, Self::Web(token)) => {
                token.client_id == crate::auth::DEFAULT_WEB_CLIENT_ID
                    && !token.refresh_token.is_empty()
            }
            (Slot::Personal, Self::Web(token)) => {
                !token.client_id.is_empty() && !token.refresh_token.is_empty()
            }
            (Slot::Playback, Self::Playback(credentials)) => {
                credentials
                    .username
                    .as_deref()
                    .is_some_and(|name| !name.is_empty())
                    && !credentials.auth_data.is_empty()
            }
            (Slot::Proxy, Self::Proxy(password)) => password.valid(),
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    slot: Slot,
    grant: Grant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error(
        "The system credential store is unavailable. Unlock or enable it to remember this sign-in."
    )]
    Unavailable,
    #[error(
        "The system credential store is locked or access was denied. Unlock it to remember this sign-in."
    )]
    Locked,
    #[error("The system credential store did not respond. This sign-in cannot be remembered yet.")]
    Timeout,
    /// Someone answered the Keychain's access prompt with Deny, or cancelled
    /// it. Not retried: asking again only shows the same dialog.
    #[error("Access to the saved sign-in was denied. Sign in again to remember it.")]
    Denied,
    #[error("The stored Spotify grant is invalid. Sign in again.")]
    Invalid,
    #[error(
        "Unable to update credential-storage state. Check the application state directory's permissions."
    )]
    Filesystem,
    #[error(
        "The system credential store did not retain the grant. This sign-in cannot be remembered."
    )]
    Verification,
    #[error("The sign-in changed while credential storage was in progress.")]
    Stale,
}

impl Error {
    pub(crate) fn proxy_message(self) -> &'static str {
        match self {
            Self::Unavailable | Self::Locked => {
                "Unlock or enable the system credential store to remember the proxy password."
            }
            Self::Timeout => {
                "The credential store did not respond. The proxy password is available only for this session."
            }
            Self::Invalid => {
                "The stored proxy password or its endpoint is invalid. Enter the proxy settings again."
            }
            Self::Filesystem => {
                "Unable to update proxy-password storage. Check the application state directory permissions."
            }
            Self::Verification => {
                "The credential store did not retain the proxy password. It is available only for this session."
            }
            Self::Stale => "The proxy settings changed while the password was being stored.",
            Self::Denied => {
                "Access to the saved proxy password was denied. It is available only for this session."
            }
        }
    }
}

/// Credential Manager refuses blobs over 2560 bytes. Larger records go
/// straight to the protected fallback file with a clear log line instead.
/// The macOS Keychain has no such limit for a generic password.
const NATIVE_LIMIT: Option<usize> = if cfg!(windows) { Some(2400) } else { None };

/// What a native store failure was, without anything it carried. Provider
/// errors can hold the secret (`BadEncoding`, `BadDataFormat`) or arbitrary
/// text, so only the variant, a platform code, or a length is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cause {
    variant: &'static str,
    code: Option<PlatformCode>,
    length: Option<u32>,
}

/// The code inside a platform error, named for the system that gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlatformCode {
    Win32(u32),
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    OsStatus(i32),
}

impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.variant)?;
        match self.code {
            Some(PlatformCode::Win32(code)) => write!(f, " (Win32 error {code})")?,
            Some(PlatformCode::OsStatus(code)) => write!(f, " (OSStatus {code})")?,
            None => {}
        }
        if let Some(length) = self.length {
            write!(f, " (limit {length})")?;
        }
        Ok(())
    }
}

/// The Win32 code inside a windows-native-keyring-store platform error (its
/// type is private, so the code is recovered from its fixed Display forms),
/// or the OSStatus inside a macOS Keychain error.
fn platform_code(error: &(dyn std::error::Error + Send + Sync + 'static)) -> Option<PlatformCode> {
    #[cfg(target_os = "macos")]
    if let Some(error) = error.downcast_ref::<security_framework::base::Error>() {
        return Some(PlatformCode::OsStatus(error.code()));
    }
    let text = error.to_string();
    if let Some(code) = text.strip_prefix("Windows error code ") {
        return code.trim().parse().ok().map(PlatformCode::Win32);
    }
    Some(PlatformCode::Win32(match text.as_str() {
        "Windows ERROR_NO_SUCH_LOGON_SESSION" => 1312,
        "Windows ERROR_NOT_FOUND" => 1168,
        "Windows ERROR_BAD_USERNAME" => 2202,
        "Windows ERROR_INVALID_FLAGS" => 1004,
        "Windows ERROR_INVALID_PARAMETER" => 87,
        _ => return None,
    }))
}

fn diagnose(error: &keyring_core::Error) -> Cause {
    use keyring_core::Error as E;
    let (variant, code, length) = match error {
        E::PlatformFailure(inner) => ("PlatformFailure", platform_code(inner.as_ref()), None),
        E::NoStorageAccess(inner) => ("NoStorageAccess", platform_code(inner.as_ref()), None),
        E::NoEntry => ("NoEntry", None, None),
        E::BadEncoding(_) => ("BadEncoding", None, None),
        E::BadDataFormat(..) => ("BadDataFormat", None, None),
        E::BadStoreFormat(_) => ("BadStoreFormat", None, None),
        E::TooLong(_, limit) => ("TooLong", None, Some(*limit)),
        E::Invalid(..) => ("Invalid", None, None),
        E::Ambiguous(_) => ("Ambiguous", None, None),
        E::NoDefaultStore => ("NoDefaultStore", None, None),
        E::NotSupportedByStore(_) => ("NotSupportedByStore", None, None),
        _ => ("Other", None, None),
    };
    Cause {
        variant,
        code,
        length,
    }
}

fn native_error(operation: &str, error: keyring_core::Error) -> Error {
    // Some provider errors contain the secret or arbitrary platform data.
    // Never propagate their Debug/Display text into application diagnostics.
    log::warn!("credential store {operation} failed: {}", diagnose(&error));
    if denied(&error) {
        return Error::Denied;
    }
    match error {
        keyring_core::Error::NoStorageAccess(_) => Error::Locked,
        _ => Error::Unavailable,
    }
}

/// Whether the Keychain refused because of the person at the Mac: Deny on
/// its access prompt (`errSecAuthFailed`) or a cancelled prompt
/// (`errSecUserCanceled`). After an ad-hoc-signed update the prompt can come
/// up for each saved grant, and retrying would show it again each time.
#[cfg(target_os = "macos")]
fn denied(error: &keyring_core::Error) -> bool {
    let keyring_core::Error::PlatformFailure(inner) = error else {
        return false;
    };
    inner
        .downcast_ref::<security_framework::base::Error>()
        .is_some_and(|error| matches!(error.code(), -128 | -25293))
}

#[cfg(not(target_os = "macos"))]
fn denied(_error: &keyring_core::Error) -> bool {
    false
}

trait ProtectedStore: Send {
    fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error>;
    fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), Error>;
    fn delete(&mut self, key: &str) -> Result<(), Error>;
}

/// How long Credential Manager keeps an entry. `Session` entries end at
/// sign-out of Windows, which is what a policy forbidding saved credentials
/// turns every write into.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Persistence {
    Session,
    Local,
    Enterprise,
}

/// The raw Credential Manager operations below the fallback layer.
trait Native: Send {
    fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error>;
    fn persistence(&mut self, key: &str) -> Result<Option<Persistence>, Error>;
    /// Writes with Local persistence: kept on this computer, never roamed.
    fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), Error>;
    fn delete(&mut self, key: &str) -> Result<(), Error>;
}

#[derive(Default)]
struct NativeStore {
    store: Option<Arc<keyring_core::api::CredentialStore>>,
}

impl NativeStore {
    fn entry(&mut self, key: &str) -> Result<keyring_core::Entry, Error> {
        if self.store.is_none() {
            #[cfg(windows)]
            let store = windows_native_keyring_store::Store::new();
            #[cfg(target_os = "macos")]
            let store = apple_native_keyring_store::keychain::Store::new();
            self.store = Some(store.map_err(|error| native_error("open", error))?);
        }
        // Credential Manager takes a persistence; the login Keychain is
        // always local to this Mac and takes no modifiers.
        #[cfg(windows)]
        let modifiers = Some(std::collections::HashMap::from([("persistence", "Local")]));
        #[cfg(not(windows))]
        let modifiers: Option<std::collections::HashMap<&str, &str>> = None;
        self.store
            .as_ref()
            .ok_or(Error::Unavailable)?
            .build(SERVICE, key, modifiers.as_ref())
            .map_err(|error| native_error("entry", error))
    }
}

impl Native for NativeStore {
    fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        match self.entry(key)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(native_error("read", error)),
        }
    }
    /// A Keychain item outlives the login session, so it is Local. This is
    /// only asked right after a read found the item or a write made it, and
    /// reading it again could bring up a second Keychain prompt.
    #[cfg(not(windows))]
    fn persistence(&mut self, _key: &str) -> Result<Option<Persistence>, Error> {
        Ok(Some(Persistence::Local))
    }
    #[cfg(windows)]
    fn persistence(&mut self, key: &str) -> Result<Option<Persistence>, Error> {
        match self.entry(key)?.get_attributes() {
            Ok(attributes) => Ok(Some(
                match attributes.get("persistence").map(String::as_str) {
                    Some("Session") => Persistence::Session,
                    Some("Local") => Persistence::Local,
                    _ => Persistence::Enterprise,
                },
            )),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(native_error("attribute read", error)),
        }
    }
    fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), Error> {
        self.entry(key)?
            .set_secret(secret)
            .map_err(|error| native_error("write", error))
    }
    fn delete(&mut self, key: &str) -> Result<(), Error> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(error) => Err(native_error("delete", error)),
        }
    }
}

/// Encrypts the fallback file for the signed-in Windows user with DPAPI.
#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr().cast_mut(),
        }
    }

    /// Copies DPAPI's output and wipes and frees its allocation.
    fn take(output: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if output.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: on success DPAPI returns `cbData` bytes allocated with
        // LocalAlloc; they are copied, wiped, and freed exactly once.
        unsafe {
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
            LocalFree(output.pbData.cast());
            bytes
        }
    }

    pub(super) fn protect(secret: &[u8], entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        let input = blob(secret);
        let entropy = blob(entropy);
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: every pointer refers to a live blob for the whole call.
        let done = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if done == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(take(output))
    }

    pub(super) fn unprotect(protected: &[u8], entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        let input = blob(protected);
        let entropy = blob(entropy);
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: every pointer refers to a live blob for the whole call.
        let done = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if done == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(take(output))
    }
}

/// Credential Manager first, then a DPAPI-encrypted file for the computers
/// where it cannot keep an entry across Windows sign-outs.
/// macOS has no DPAPI and the Keychain is the protected store, so there is
/// no fallback file: a Keychain failure is reported instead of leaving the
/// secret in a file a key for which would have to live in the same Keychain.
#[cfg(not(windows))]
mod dpapi {
    fn unsupported() -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no protected fallback file on this platform",
        )
    }
    pub(super) fn protect(_secret: &[u8], _entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        Err(unsupported())
    }
    pub(super) fn unprotect(_protected: &[u8], _entropy: &[u8]) -> std::io::Result<Vec<u8>> {
        Err(unsupported())
    }
}

struct Durable<N> {
    native: N,
    /// `credential-storage` in the state directory, beside the markers.
    folder: PathBuf,
}

impl<N: Native> Durable<N> {
    fn slot_name(key: &str) -> Option<&'static str> {
        Slot::ALL.into_iter().map(Slot::name).find(|name| {
            key.strip_suffix(name)
                .is_some_and(|rest| rest.ends_with(':'))
        })
    }

    fn fallback_path(&self, name: &str) -> PathBuf {
        self.folder.join(format!("{name}.bin"))
    }

    fn entropy(name: &str) -> Vec<u8> {
        format!("{SERVICE}:{name}").into_bytes()
    }

    fn read_fallback(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        let Some(name) = Self::slot_name(key) else {
            return Ok(None);
        };
        let protected = match std::fs::read(self.fallback_path(name)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Filesystem),
        };
        dpapi::unprotect(&protected, &Self::entropy(name))
            .map(Some)
            .map_err(|error| {
                log::warn!(
                    "credential fallback for {name} could not be decrypted: Win32 error {}",
                    error.raw_os_error().unwrap_or_default()
                );
                Error::Invalid
            })
    }

    fn write_fallback(&self, key: &str, secret: &[u8]) -> Result<(), Error> {
        use std::io::Write;
        let name = Self::slot_name(key).ok_or(Error::Unavailable)?;
        let protected = dpapi::protect(secret, &Self::entropy(name)).map_err(|error| {
            log::warn!(
                "credential fallback for {name} could not be encrypted: Win32 error {}",
                error.raw_os_error().unwrap_or_default()
            );
            Error::Unavailable
        })?;
        std::fs::create_dir_all(&self.folder).map_err(|_| Error::Filesystem)?;
        let path = self.fallback_path(name);
        let temporary = path.with_extension("bin.tmp");
        let result = (|| {
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&protected)?;
            file.sync_all()?;
            drop(file);
            crate::util::replace_file(&temporary, &path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
            log::warn!("credential fallback for {name} could not be written");
            return Err(Error::Filesystem);
        }
        log::info!("credential {name} is kept in the protected fallback file");
        Ok(())
    }

    fn delete_fallback(&self, key: &str) -> Result<(), Error> {
        let Some(name) = Self::slot_name(key) else {
            return Ok(());
        };
        let path = self.fallback_path(name);
        let temporary = remove_file(&path.with_extension("bin.tmp"));
        remove_file(&path).and(temporary)
    }

    /// Keep a secret that Credential Manager will not hold durably. A stale
    /// native entry would shadow the file, so it must be gone first.
    fn fall_back(&mut self, key: &str, secret: &[u8], cause: Error) -> Result<(), Error> {
        let _ = self.native.delete(key);
        if matches!(self.native.read(key), Ok(Some(_))) {
            log::warn!("credential store kept an older entry; the fallback cannot replace it");
            return Err(cause);
        }
        self.write_fallback(key, secret).map_err(|_| cause)
    }

    /// Rewrite an entry Credential Manager would roam or drop at sign-out.
    fn settle(&mut self, key: &str, secret: &[u8], persistence: Persistence) {
        match persistence {
            Persistence::Local => {}
            Persistence::Enterprise => {
                log::info!("moving a credential entry to local persistence");
                let moved = self
                    .native
                    .delete(key)
                    .and_then(|()| self.native.write(key, secret))
                    .and_then(|()| self.native.persistence(key));
                if !matches!(
                    moved,
                    Ok(Some(Persistence::Local | Persistence::Enterprise))
                ) {
                    log::warn!("credential entry could not be rewritten; using the fallback file");
                    let _ = self.fall_back(key, secret, Error::Verification);
                }
            }
            Persistence::Session => {
                if self.read_fallback(key).ok().flatten().as_deref() != Some(secret) {
                    log::warn!("credential entry is session-only; keeping the fallback file");
                    let _ = self.write_fallback(key, secret);
                }
            }
        }
    }
}

impl<N: Native> ProtectedStore for Durable<N> {
    fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        match self.native.read(key) {
            Ok(Some(secret)) => {
                // A durable native save removes the file or makes it match,
                // so a different file holds a newer secret the store refused.
                if let Ok(Some(newer)) = self.read_fallback(key)
                    && newer != secret
                {
                    log::warn!("credential fallback file is newer than the native entry");
                    let _ = self.write(key, &newer);
                    return Ok(Some(newer));
                }
                if let Ok(Some(persistence)) = self.native.persistence(key) {
                    self.settle(key, &secret, persistence);
                }
                Ok(Some(secret))
            }
            Ok(None) => self.read_fallback(key),
            Err(error) => match self.read_fallback(key) {
                Ok(Some(secret)) => Ok(Some(secret)),
                _ => Err(error),
            },
        }
    }

    fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), Error> {
        if let Some(limit) = NATIVE_LIMIT
            && secret.len() > limit
        {
            log::warn!(
                "credential record of {} bytes exceeds the {limit}-byte Credential Manager limit; using the fallback file",
                secret.len()
            );
            return self.fall_back(key, secret, Error::Unavailable);
        }
        if let Err(error) = self.native.write(key, secret) {
            // There is no fallback file off Windows, and the fallback path
            // would first delete the item the store still holds.
            if cfg!(not(windows)) {
                return Err(error);
            }
            return self.fall_back(key, secret, error);
        }
        match self.native.persistence(key) {
            Ok(Some(Persistence::Local | Persistence::Enterprise)) => {
                if self.delete_fallback(key).is_err() {
                    // A file that outlives a save must at least match it.
                    log::warn!("an outdated credential fallback file could not be removed");
                    return self.write_fallback(key, secret);
                }
                Ok(())
            }
            Ok(Some(Persistence::Session)) => {
                log::warn!("credential store saved a session-only entry; using the fallback file");
                self.fall_back(key, secret, Error::Verification)
            }
            Ok(None) => self.fall_back(key, secret, Error::Verification),
            Err(error) => self.fall_back(key, secret, error),
        }
    }

    fn delete(&mut self, key: &str) -> Result<(), Error> {
        let fallback = self.delete_fallback(key);
        self.native.delete(key).and(fallback)
    }
}

/// macOS: what the Keychain holds, as this process last read or wrote it.
///
/// An ad-hoc-signed app is a new app to the Keychain after each update, so
/// every read of an item asks for the login password until the person
/// chooses Always Allow, and so does every write (the Keychain looks the
/// item up first). Here each item is read once per run, a write that would
/// change nothing is skipped, and the check after a save is answered from
/// memory. One LWSpot runs per profile, so nothing else changes the items
/// meanwhile.
#[cfg(target_os = "macos")]
struct Remembering<S> {
    inner: S,
    known: std::collections::HashMap<String, Option<Vec<u8>>>,
}

#[cfg(target_os = "macos")]
impl<S> Remembering<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            known: std::collections::HashMap::new(),
        }
    }
}

#[cfg(target_os = "macos")]
impl<S: ProtectedStore> ProtectedStore for Remembering<S> {
    fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        if let Some(known) = self.known.get(key) {
            return Ok(known.clone());
        }
        let secret = self.inner.read(key)?;
        self.known.insert(key.to_owned(), secret.clone());
        Ok(secret)
    }

    fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), Error> {
        if self
            .known
            .get(key)
            .is_some_and(|known| known.as_deref() == Some(secret))
        {
            return Ok(());
        }
        let written = self.inner.write(key, secret);
        match &written {
            Ok(()) => {
                self.known.insert(key.to_owned(), Some(secret.to_vec()));
            }
            Err(_) => {
                self.known.remove(key);
            }
        }
        written
    }

    fn delete(&mut self, key: &str) -> Result<(), Error> {
        self.known.remove(key);
        let deleted = self.inner.delete(key);
        if deleted.is_ok() {
            self.known.insert(key.to_owned(), None);
        }
        deleted
    }
}

type Job = Box<dyn FnOnce(&mut dyn ProtectedStore) + Send>;

struct Inner {
    dirs: AppDirs,
    profile: String,
    generations: [AtomicU64; 4],
    // Held only around the tiny local marker files, never a native store call.
    markers: Mutex<()>,
    jobs: mpsc::SyncSender<Job>,
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

/// Permission to read/write one grant for one authorization generation.
#[derive(Clone)]
pub struct Lease {
    store: Store,
    slot: Slot,
    generation: u64,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    version: u32,
    revoked: bool,
}

pub struct Loaded {
    pub grant: Option<Grant>,
    pub warning: Option<Error>,
}

impl Store {
    pub fn new(dirs: AppDirs) -> Self {
        let backend = Durable {
            native: NativeStore::default(),
            folder: dirs.state.join("credential-storage"),
        };
        #[cfg(target_os = "macos")]
        let backend = Remembering::new(backend);
        Self::with_backend(dirs, Box::new(backend))
    }

    #[cfg(test)]
    pub(crate) fn in_memory(dirs: AppDirs) -> Self {
        tests::memory_store(dirs)
    }

    fn with_backend(dirs: AppDirs, mut backend: Box<dyn ProtectedStore>) -> Self {
        let (jobs, receiver) = mpsc::sync_channel::<Job>(16);
        let runtime = tokio::runtime::Handle::try_current().ok();
        let _ = std::thread::Builder::new()
            .name("spotify-credentials".into())
            .spawn(move || {
                let _entered = runtime.as_ref().map(tokio::runtime::Handle::enter);
                while let Ok(job) = receiver.recv() {
                    job(backend.as_mut());
                }
            });
        let profile = std::fs::read_to_string(dirs.state.join("credential-profile"))
            .ok()
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .unwrap_or_else(|| {
                format!(
                    "{:x}",
                    Sha256::digest(dirs.state.to_string_lossy().as_bytes())
                )
            });
        Self {
            inner: Arc::new(Inner {
                dirs,
                profile,
                generations: std::array::from_fn(|_| AtomicU64::new(0)),
                markers: Mutex::new(()),
                jobs,
            }),
        }
    }

    pub fn lease(&self, slot: Slot) -> Lease {
        Lease {
            store: self.clone(),
            slot,
            generation: self.inner.generations[slot.index()].load(Ordering::SeqCst),
        }
    }

    /// Invalidate in-flight work before canceling workers or deleting grants.
    pub fn invalidate(&self, slot: Slot) {
        self.inner.generations[slot.index()].fetch_add(1, Ordering::SeqCst);
    }

    /// Persist revocation before contacting the native store. Even a locked
    /// keychain or an interrupted deletion must not sign the user back in.
    pub fn revoke(&self, slot: Slot) -> Result<(), Error> {
        self.invalidate(slot);
        let _guard = self.inner.markers.lock().unwrap_or_else(|p| p.into_inner());
        let marker = self.write_marker(slot, true);
        let legacy = self.remove_legacy(slot);
        marker.and(legacy)
    }

    pub fn revoke_spotify(&self) -> Result<(), Error> {
        let mut error = None;
        for slot in Slot::SPOTIFY {
            if let Err(failure) = self.revoke(slot) {
                error = Some(failure);
            }
        }
        // Sign-out also removes corrupt/partial old combined records, whose
        // client identity cannot be established by a migration reader.
        let legacy = self.inner.dirs.legacy_web_token_file();
        for path in [legacy.clone(), legacy.with_extension("json.tmp")] {
            if let Err(failure) = remove_file(&path) {
                error = Some(failure);
            }
        }
        error.map_or(Ok(()), Err)
    }

    fn marker_path(&self, slot: Slot) -> PathBuf {
        self.inner
            .dirs
            .state
            .join("credential-storage")
            .join(format!("{}.json", slot.name()))
    }

    fn marker(&self, slot: Slot) -> Result<Option<Marker>, Error> {
        match std::fs::read(self.marker_path(slot)) {
            Ok(bytes) => {
                let marker: Marker = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
                if marker.version != 1 {
                    return Err(Error::Invalid);
                }
                Ok(Some(marker))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(Error::Filesystem),
        }
    }

    fn write_marker(&self, slot: Slot, revoked: bool) -> Result<(), Error> {
        let path = self.marker_path(slot);
        let parent = path.parent().ok_or(Error::Filesystem)?;
        std::fs::create_dir_all(parent).map_err(|_| Error::Filesystem)?;
        let bytes = serde_json::to_vec(&Marker {
            version: 1,
            revoked,
        })
        .map_err(|_| Error::Invalid)?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes).map_err(|_| Error::Filesystem)?;
        crate::util::replace_file(&temporary, &path).map_err(|_| Error::Filesystem)
    }

    fn legacy_paths(&self, slot: Slot) -> Vec<PathBuf> {
        if slot == Slot::Proxy {
            let path = self.inner.dirs.proxy_secret_file();
            return vec![path.clone(), path.with_extension("tmp")];
        }
        let dirs = &self.inner.dirs;
        let path = match slot {
            Slot::Shared => dirs.shared_web_token_file(),
            Slot::Personal => dirs.personal_web_token_file(),
            Slot::Playback => dirs.credentials_dir().join("credentials.json"),
            Slot::Proxy => unreachable!("proxy legacy paths handled above"),
        };
        let mut paths = vec![path.clone(), path.with_extension("json.tmp")];
        if slot != Slot::Playback {
            // The old combined path belongs to whichever client identity it contains.
            let legacy = dirs.legacy_web_token_file();
            if StoredToken::load(&legacy).is_some_and(|token| {
                (slot == Slot::Shared) == (token.client_id == crate::auth::DEFAULT_WEB_CLIENT_ID)
            }) {
                paths.push(legacy.clone());
                paths.push(legacy.with_extension("json.tmp"));
            }
        }
        paths
    }

    fn remove_legacy(&self, slot: Slot) -> Result<(), Error> {
        let mut result = Ok(());
        for path in self.legacy_paths(slot) {
            if let Err(error) = remove_file(&path) {
                result = Err(error);
            }
        }
        result
    }

    fn legacy_proxy(&self) -> Result<Option<Grant>, Error> {
        let text = match std::fs::read_to_string(self.inner.dirs.settings_file()) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return match self.inner.dirs.proxy_secret_file().try_exists() {
                    Ok(false) => Ok(None),
                    Ok(true) => Err(Error::Invalid),
                    Err(_) => Err(Error::Filesystem),
                };
            }
            Err(_) => return Err(Error::Filesystem),
        };
        let mut settings: crate::settings::Settings =
            serde_json::from_str(&text).map_err(|_| Error::Invalid)?;
        settings.migrate_proxy(&text);
        match std::fs::read_to_string(self.inner.dirs.proxy_secret_file()) {
            Ok(password) => settings.proxy_password = password,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Filesystem),
        }
        settings
            .proxy_password_record()
            .map(|password| password.map(Grant::Proxy))
            .map_err(|_| Error::Invalid)
    }

    fn legacy(&self, slot: Slot) -> Result<Option<Grant>, Error> {
        if slot == Slot::Proxy {
            return self.legacy_proxy();
        }
        for path in self
            .legacy_paths(slot)
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        {
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(Error::Filesystem),
            };
            let grant = if slot == Slot::Playback {
                Grant::Playback(serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?)
            } else {
                Grant::Web(serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?)
            };
            if !grant.valid_for(slot) {
                return Err(Error::Invalid);
            }
            return Ok(Some(grant));
        }
        Ok(None)
    }
}

fn remove_file(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Error::Filesystem),
    }
}

impl Lease {
    pub fn current(&self) -> bool {
        self.store.inner.generations[self.slot.index()].load(Ordering::SeqCst) == self.generation
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    fn check(&self) -> Result<(), Error> {
        if self.current() {
            Ok(())
        } else {
            Err(Error::Stale)
        }
    }
    fn key(&self) -> String {
        format!("{}:{}", self.store.inner.profile, self.slot.name())
    }

    fn request<T: Send + 'static>(
        &self,
        guard_generation: bool,
        operation: impl FnOnce(Self, &mut dyn ProtectedStore) -> Result<T, Error> + Send + 'static,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, Error>> + Send>> {
        let pending = (|| {
            if guard_generation {
                self.check()?;
            }
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let lease = self.clone();
            self.store
                .inner
                .jobs
                .try_send(Box::new(move |backend| {
                    let result = (if guard_generation {
                        lease.check()
                    } else {
                        Ok(())
                    })
                    .and_then(|()| operation(lease.clone(), backend));
                    let result = result.and_then(|value| {
                        if guard_generation {
                            lease.check()?;
                        }
                        Ok(value)
                    });
                    let _ = sender.send(result);
                }))
                .map_err(|_| Error::Unavailable)?;
            Ok::<_, Error>(receiver)
        })();
        Box::pin(async move {
            tokio::time::timeout(TIMEOUT, pending?)
                .await
                .map_err(|_| Error::Timeout)?
                .map_err(|_| Error::Unavailable)?
        })
    }

    pub async fn load(&self) -> Result<Loaded, Error> {
        self.request(true, |lease, backend| {
            let marker = lease.store.marker(lease.slot)?;
            if marker.as_ref().is_some_and(|marker| marker.revoked) {
                return Ok(Loaded {
                    grant: None,
                    warning: None,
                });
            }
            let existing = backend.read(&lease.key());
            if let Ok(Some(bytes)) = &existing {
                let record: Record = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
                if record.version != 1
                    || record.slot != lease.slot
                    || !record.grant.valid_for(lease.slot)
                {
                    return Err(Error::Invalid);
                }
                let _guard = lease
                    .store
                    .inner
                    .markers
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                lease.check()?;
                lease.store.write_marker(lease.slot, false)?;
                let warning = lease.store.remove_legacy(lease.slot).err();
                return Ok(Loaded {
                    grant: Some(record.grant),
                    warning,
                });
            }
            if marker.is_some() {
                return existing.map(|_| Loaded {
                    grant: None,
                    warning: None,
                });
            }
            let Some(grant) = lease.store.legacy(lease.slot)? else {
                return existing.map(|_| Loaded {
                    grant: None,
                    warning: None,
                });
            };
            // Keep a failed migration recoverable. No new plaintext is written,
            // and the caller must show the warning about persistence.
            let warning = match existing {
                Err(error) => Some(error),
                Ok(_) => lease.save_inner(backend, grant.clone()).err(),
            };
            Ok(Loaded {
                grant: Some(grant),
                warning,
            })
        })
        .await
    }

    /// `load` for startup. Credential Manager can refuse the first reads
    /// while Windows is still signing in, so retry an unavailable or locked
    /// store briefly before treating the grant as missing. A timeout is not
    /// retried: the worker is still busy with the call that timed out.
    pub async fn load_retrying(&self) -> Result<Loaded, Error> {
        let mut wait = STARTUP_RETRY;
        for _ in 0..STARTUP_RETRIES {
            match self.load().await {
                Err(error @ (Error::Unavailable | Error::Locked)) if self.current() => {
                    log::warn!("credential store read failed, retrying: {error:?}");
                    tokio::time::sleep(wait).await;
                    wait *= 2;
                }
                result => return result,
            }
        }
        self.load().await
    }

    fn save_inner(&self, backend: &mut dyn ProtectedStore, grant: Grant) -> Result<(), Error> {
        self.check()?;
        if !grant.valid_for(self.slot) {
            return Err(Error::Invalid);
        }
        let bytes = serde_json::to_vec(&Record {
            version: 1,
            slot: self.slot,
            grant,
        })
        .map_err(|_| Error::Invalid)?;
        backend.write(&self.key(), &bytes)?;
        if !self.current() {
            // Operations on this store are serialized. No newer write can be
            // erased here; it is still queued behind this one.
            let _ = backend.delete(&self.key());
            return Err(Error::Stale);
        }
        if backend.read(&self.key())?.as_deref() != Some(bytes.as_slice()) {
            return Err(Error::Verification);
        }
        let _guard = self
            .store
            .inner
            .markers
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.check()?;
        self.store.write_marker(self.slot, false)?;
        self.store.remove_legacy(self.slot)
    }

    /// Enqueue immediately so refreshes preserve write order even when callers
    /// await completion outside their token mutex.
    pub fn save(
        &self,
        grant: Grant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send>> {
        self.request(true, move |lease, backend| lease.save_inner(backend, grant))
    }

    /// Call `Store::revoke` before this. Deletion failure remains visible while
    /// the local revocation marker prevents restoration on the next launch.
    pub fn delete(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send>> {
        // Deletion is enqueued before any replacement write. Run it even if a
        // subsequent authorization increments the generation while it waits.
        self.request(false, |lease, backend| backend.delete(&lease.key()))
    }
}

#[cfg(test)]
mod tests {
    /// The Keychain is read once per item, an unchanged save is not sent,
    /// and a failed write is not remembered.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_store_reads_each_item_once_and_skips_unchanged_saves() {
        #[derive(Default)]
        struct Counting {
            values: std::collections::HashMap<String, Vec<u8>>,
            reads: u32,
            writes: u32,
            fail_writes: bool,
        }
        impl ProtectedStore for Counting {
            fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
                self.reads += 1;
                Ok(self.values.get(key).cloned())
            }
            fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), Error> {
                self.writes += 1;
                if self.fail_writes {
                    return Err(Error::Denied);
                }
                self.values.insert(key.to_owned(), secret.to_vec());
                Ok(())
            }
            fn delete(&mut self, key: &str) -> Result<(), Error> {
                self.values.remove(key);
                Ok(())
            }
        }
        let mut inner = Counting::default();
        inner.values.insert("k".into(), b"one".to_vec());
        let mut store = Remembering::new(inner);
        assert_eq!(store.read("k").unwrap().as_deref(), Some(b"one".as_slice()));
        assert_eq!(store.read("k").unwrap().as_deref(), Some(b"one".as_slice()));
        assert_eq!(store.inner.reads, 1);
        store.write("k", b"one").unwrap();
        assert_eq!(store.inner.writes, 0, "an unchanged save is not sent");
        store.write("k", b"two").unwrap();
        assert_eq!(store.read("k").unwrap().as_deref(), Some(b"two".as_slice()));
        assert_eq!((store.inner.reads, store.inner.writes), (1, 1));
        store.inner.fail_writes = true;
        assert_eq!(store.write("k", b"three"), Err(Error::Denied));
        store.inner.fail_writes = false;
        assert_eq!(store.read("k").unwrap().as_deref(), Some(b"two".as_slice()));
        assert_eq!(store.inner.reads, 2, "a failed write is not remembered");
        store.delete("k").unwrap();
        assert_eq!(store.read("k").unwrap(), None);
        assert_eq!(store.inner.reads, 2);
    }

    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake {
        values: HashMap<String, Vec<u8>>,
        read_error: Option<Error>,
        /// Fail this many reads as unavailable, then recover.
        read_failures: usize,
        write_error: Option<Error>,
        delete_error: Option<Error>,
        discard_write: bool,
        entered: Option<tokio::sync::oneshot::Sender<()>>,
        release: Option<mpsc::Receiver<()>>,
    }
    pub(super) fn memory_store(dirs: AppDirs) -> Store {
        Store::with_backend(
            dirs,
            Box::new(Backend(Arc::new(Mutex::new(Fake::default())))),
        )
    }
    struct Backend(Arc<Mutex<Fake>>);
    impl ProtectedStore for Backend {
        fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
            let mut fake = self.0.lock().unwrap();
            if fake.read_failures > 0 {
                fake.read_failures -= 1;
                return Err(Error::Unavailable);
            }
            if let Some(error) = fake.read_error {
                return Err(error);
            }
            Ok(fake.values.get(key).cloned())
        }
        fn write(&mut self, key: &str, value: &[u8]) -> Result<(), Error> {
            let (entered, release) = {
                let mut fake = self.0.lock().unwrap();
                if let Some(error) = fake.write_error {
                    return Err(error);
                }
                (fake.entered.take(), fake.release.take())
            };
            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            if let Some(release) = release {
                release.recv().unwrap();
            }
            let mut fake = self.0.lock().unwrap();
            if !fake.discard_write {
                fake.values.insert(key.to_owned(), value.to_vec());
            }
            Ok(())
        }
        fn delete(&mut self, key: &str) -> Result<(), Error> {
            let mut fake = self.0.lock().unwrap();
            if let Some(error) = fake.delete_error {
                return Err(error);
            }
            fake.values.remove(key);
            Ok(())
        }
    }

    struct Fixture {
        dirs: AppDirs,
        fake: Arc<Mutex<Fake>>,
        store: Store,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "lwspot-credential-tests-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let dirs = AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            };
            dirs.ensure().unwrap();
            let fake = Arc::new(Mutex::new(Fake::default()));
            let store = Store::with_backend(dirs.clone(), Box::new(Backend(fake.clone())));
            Self { dirs, fake, store }
        }
        fn restart(&self) -> Store {
            Store::with_backend(self.dirs.clone(), Box::new(Backend(self.fake.clone())))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.dirs.state.parent().unwrap());
        }
    }

    fn web(client: &str) -> Grant {
        Grant::Web(StoredToken {
            client_id: client.into(),
            access_token: "dummy-access".into(),
            refresh_token: "dummy-refresh".into(),
            expires_at: u64::MAX,
            scope: "dummy-scope".into(),
        })
    }
    fn playback() -> Grant {
        Grant::Playback(Credentials { username: Some("dummy-account".into()), auth_data: b"dummy-reusable-grant".to_vec(), auth_type: librespot_protocol::authentication::AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS })
    }

    #[tokio::test]
    async fn a_recorded_credential_profile_keeps_grants_and_revocation_markers() {
        let fixture = Fixture::new();
        let grant = web(crate::auth::DEFAULT_WEB_CLIENT_ID);
        fixture
            .store
            .lease(Slot::Shared)
            .save(grant.clone())
            .await
            .unwrap();
        fixture.store.revoke(Slot::Playback).unwrap();
        let parent = fixture.dirs.state.parent().unwrap();
        let renamed = AppDirs {
            config: parent.join("new-config"),
            state: parent.join("new-state"),
            cache: parent.join("new-cache"),
        };
        // Profiles moved by the rename record the account identity of their
        // original state folder; grants saved under it must stay reachable.
        std::fs::rename(&fixture.dirs.state, &renamed.state).unwrap();
        std::fs::write(
            renamed.state.join("credential-profile"),
            &fixture.store.inner.profile,
        )
        .unwrap();
        let store = Store::with_backend(renamed, Box::new(Backend(fixture.fake.clone())));
        assert_eq!(store.inner.profile, fixture.store.inner.profile);
        assert!(store.lease(Slot::Shared).load().await.unwrap().grant == Some(grant));
        assert!(
            store
                .lease(Slot::Playback)
                .load()
                .await
                .unwrap()
                .grant
                .is_none()
        );
    }
    fn proxy_password() -> Grant {
        Grant::Proxy(ProxyPassword {
            host: "127.0.0.1".into(),
            port: 8080,
            username: "dummy-user".into(),
            password: " dummy-private-password\n".into(),
        })
    }

    fn write_legacy(path: &Path, grant: &Grant) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = match grant {
            Grant::Web(token) => serde_json::to_vec(token).unwrap(),
            Grant::Playback(credentials) => serde_json::to_vec(credentials).unwrap(),
            Grant::Proxy(password) => password.password.as_bytes().to_vec(),
        };
        std::fs::write(path, bytes).unwrap();
    }

    fn write_proxy_legacy(f: &Fixture, combined: bool) -> Grant {
        let Grant::Proxy(password) = proxy_password() else {
            unreachable!()
        };
        let settings = crate::settings::Settings {
            proxy_mode: crate::settings::ProxyMode::Http,
            proxy_host: password.host.clone(),
            proxy_port: password.port.to_string(),
            proxy_username: password.username.clone(),
            ..Default::default()
        };
        let mut json = serde_json::to_value(settings).unwrap();
        if combined {
            json["proxy_password"] = password.password.clone().into();
        }
        std::fs::write(f.dirs.settings_file(), serde_json::to_vec(&json).unwrap()).unwrap();
        if !combined {
            std::fs::write(f.dirs.proxy_secret_file(), password.password.as_bytes()).unwrap();
        }
        Grant::Proxy(password)
    }

    #[tokio::test]
    async fn proxy_password_migration_verifies_before_removing_legacy_data() {
        for combined in [false, true] {
            let f = Fixture::new();
            let expected = write_proxy_legacy(&f, combined);
            let settings_before = std::fs::read(f.dirs.settings_file()).unwrap();
            let loaded = f.store.lease(Slot::Proxy).load().await.unwrap();
            assert!(loaded.grant == Some(expected.clone()));
            assert_eq!(loaded.warning, None);
            assert!(!f.dirs.proxy_secret_file().exists());
            // Settings belong to the UI thread. A verified event permits its
            // next atomic save to remove the legacy JSON password.
            assert_eq!(
                std::fs::read(f.dirs.settings_file()).unwrap(),
                settings_before
            );
            assert!(f.restart().lease(Slot::Proxy).load().await.unwrap().grant == Some(expected));
        }
    }

    #[tokio::test]
    async fn a_failed_proxy_migration_retains_the_exact_original_and_can_retry() {
        for combined in [false, true] {
            let f = Fixture::new();
            let expected = write_proxy_legacy(&f, combined);
            let path = if combined {
                f.dirs.settings_file()
            } else {
                f.dirs.proxy_secret_file()
            };
            let before = std::fs::read(&path).unwrap();
            f.fake.lock().unwrap().write_error = Some(Error::Locked);
            let loaded = f.store.lease(Slot::Proxy).load().await.unwrap();
            assert!(
                loaded.grant == Some(expected.clone()),
                "legacy password still serves this session"
            );
            assert_eq!(loaded.warning, Some(Error::Locked));
            assert_eq!(std::fs::read(&path).unwrap(), before);
            f.fake.lock().unwrap().write_error = None;
            let loaded = f.restart().lease(Slot::Proxy).load().await.unwrap();
            assert!(loaded.grant == Some(expected));
            assert_eq!(loaded.warning, None);
        }
    }

    #[tokio::test]
    async fn spotify_sign_out_retains_the_network_password() {
        let f = Fixture::new();
        let password = proxy_password();
        f.store
            .lease(Slot::Proxy)
            .save(password.clone())
            .await
            .unwrap();
        f.store
            .lease(Slot::Shared)
            .save(web(crate::auth::DEFAULT_WEB_CLIENT_ID))
            .await
            .unwrap();
        f.store.revoke_spotify().unwrap();
        let restarted = f.restart();
        assert!(restarted.lease(Slot::Proxy).load().await.unwrap().grant == Some(password));
        assert!(
            restarted
                .lease(Slot::Shared)
                .load()
                .await
                .unwrap()
                .grant
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_failed_proxy_delete_cannot_restore_the_forgotten_password() {
        let f = Fixture::new();
        let password = write_proxy_legacy(&f, true);
        f.store.lease(Slot::Proxy).save(password).await.unwrap();
        f.fake.lock().unwrap().delete_error = Some(Error::Locked);
        f.store.revoke(Slot::Proxy).unwrap();
        assert_eq!(
            f.store.lease(Slot::Proxy).delete().await,
            Err(Error::Locked)
        );
        assert!(
            f.restart()
                .lease(Slot::Proxy)
                .load()
                .await
                .unwrap()
                .grant
                .is_none()
        );
    }

    #[tokio::test]
    #[ignore = "requires an unlocked platform credential store; uses dummy grants only"]
    async fn native_store_round_trip() {
        let f = Fixture::new();
        let key = f.store.lease(Slot::Shared).key();
        tokio::task::spawn_blocking(move || {
            let mut native = NativeStore::default();
            assert_eq!(native.read(&key).unwrap(), None);
            native.write(&key, b"dummy-probe").unwrap();
            assert_eq!(
                native.read(&key).unwrap().as_deref(),
                Some(b"dummy-probe".as_slice())
            );
            assert_eq!(native.persistence(&key).unwrap(), Some(Persistence::Local));
            native.delete(&key).unwrap();
            assert_eq!(native.read(&key).unwrap(), None);
        })
        .await
        .unwrap();
        let store = Store::new(f.dirs.clone());
        let grants = [
            web(crate::auth::DEFAULT_WEB_CLIENT_ID),
            web("dummy-personal-client"),
            playback(),
            proxy_password(),
        ];
        for (slot, grant) in Slot::ALL.into_iter().zip(grants.iter()) {
            store.lease(slot).save(grant.clone()).await.unwrap();
        }
        let restarted = Store::new(f.dirs.clone());
        for (slot, grant) in Slot::ALL.into_iter().zip(grants) {
            assert!(restarted.lease(slot).load().await.unwrap().grant == Some(grant));
        }
        store.revoke_spotify().unwrap();
        store.revoke(Slot::Proxy).unwrap();
        for slot in Slot::ALL {
            let lease = store.lease(slot);
            lease.delete().await.unwrap();
            assert!(
                lease
                    .request(false, |lease, backend| backend.read(&lease.key()))
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn queued_deletion_cannot_erase_a_new_authorization() {
        let f = Fixture::new();
        let old = web("dummy-old-personal-client");
        let new = web("dummy-new-personal-client");
        f.store.lease(Slot::Personal).save(old).await.unwrap();
        f.store.revoke(Slot::Personal).unwrap();
        let deletion = f.store.lease(Slot::Personal).delete();
        f.store.invalidate(Slot::Personal);
        let saving = f.store.lease(Slot::Personal).save(new.clone());
        deletion.await.unwrap();
        saving.await.unwrap();
        assert!(
            f.restart()
                .lease(Slot::Personal)
                .load()
                .await
                .unwrap()
                .grant
                == Some(new)
        );
    }

    #[test]
    fn a_failed_revocation_marker_does_not_prevent_legacy_cleanup() {
        let f = Fixture::new();
        let path = f.dirs.shared_web_token_file();
        write_legacy(&path, &web(crate::auth::DEFAULT_WEB_CLIENT_ID));
        std::fs::write(f.dirs.state.join("credential-storage"), b"not a directory").unwrap();
        assert_eq!(f.store.revoke(Slot::Shared), Err(Error::Filesystem));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn all_grants_migrate_and_survive_restart_without_plaintext_files() {
        let f = Fixture::new();
        let grants = [
            web(crate::auth::DEFAULT_WEB_CLIENT_ID),
            web("dummy-personal-client"),
            playback(),
        ];
        let paths = [
            f.dirs.shared_web_token_file(),
            f.dirs.personal_web_token_file(),
            f.dirs.credentials_dir().join("credentials.json"),
        ];
        for ((slot, grant), path) in Slot::SPOTIFY
            .into_iter()
            .zip(grants.iter())
            .zip(paths.iter())
        {
            write_legacy(path, grant);
            let loaded = f.store.lease(slot).load().await.unwrap();
            assert!(loaded.grant.as_ref() == Some(grant));
            assert_eq!(loaded.warning, None);
            assert!(!path.exists());
            let marker = std::fs::read_to_string(f.store.marker_path(slot)).unwrap();
            assert!(!marker.contains("dummy"));
        }
        let restarted = f.restart();
        for (slot, grant) in Slot::SPOTIFY.into_iter().zip(grants) {
            assert!(restarted.lease(slot).load().await.unwrap().grant == Some(grant));
        }
        assert_eq!(f.fake.lock().unwrap().values.len(), 3);
    }

    #[tokio::test]
    async fn combined_legacy_path_migrates_to_its_client_identity() {
        for (slot, client) in [
            (Slot::Shared, crate::auth::DEFAULT_WEB_CLIENT_ID),
            (Slot::Personal, "dummy-personal-client"),
        ] {
            let f = Fixture::new();
            let grant = web(client);
            write_legacy(&f.dirs.legacy_web_token_file(), &grant);
            assert!(f.store.lease(slot).load().await.unwrap().grant == Some(grant));
            assert!(!f.dirs.legacy_web_token_file().exists());
        }
    }

    #[tokio::test]
    async fn failed_migration_keeps_original_and_reports_warning_then_can_retry() {
        let f = Fixture::new();
        let grant = web(crate::auth::DEFAULT_WEB_CLIENT_ID);
        let path = f.dirs.shared_web_token_file();
        write_legacy(&path, &grant);
        f.fake.lock().unwrap().write_error = Some(Error::Locked);
        let loaded = f.store.lease(Slot::Shared).load().await.unwrap();
        assert!(loaded.grant == Some(grant.clone()));
        assert_eq!(loaded.warning, Some(Error::Locked));
        assert!(path.exists());
        assert!(!f.store.marker_path(Slot::Shared).exists());
        f.fake.lock().unwrap().write_error = None;
        let loaded = f.store.lease(Slot::Shared).load().await.unwrap();
        assert!(loaded.grant == Some(grant));
        assert_eq!(loaded.warning, None);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn migration_requires_read_back_before_removing_original() {
        let f = Fixture::new();
        write_legacy(
            &f.dirs.shared_web_token_file(),
            &web(crate::auth::DEFAULT_WEB_CLIENT_ID),
        );
        f.fake.lock().unwrap().discard_write = true;
        let loaded = f.store.lease(Slot::Shared).load().await.unwrap();
        assert_eq!(loaded.warning, Some(Error::Verification));
        assert!(f.dirs.shared_web_token_file().exists());
        assert!(!f.store.marker_path(Slot::Shared).exists());
    }

    #[tokio::test]
    async fn late_write_after_signout_cannot_recreate_grant() {
        let f = Fixture::new();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        {
            let mut fake = f.fake.lock().unwrap();
            fake.entered = Some(entered_tx);
            fake.release = Some(release_rx);
        }
        let lease = f.store.lease(Slot::Shared);
        let pending_lease = lease.clone();
        let pending = tokio::spawn(async move {
            pending_lease
                .save(web(crate::auth::DEFAULT_WEB_CLIENT_ID))
                .await
        });
        entered_rx.await.unwrap();
        f.store.revoke_spotify().unwrap();
        assert!(!lease.current());
        release_tx.send(()).unwrap();
        assert_eq!(pending.await.unwrap(), Err(Error::Stale));
        assert!(f.fake.lock().unwrap().values.is_empty());
        assert!(
            f.restart()
                .lease(Slot::Shared)
                .load()
                .await
                .unwrap()
                .grant
                .is_none()
        );
    }

    #[tokio::test]
    async fn failed_native_deletion_remains_revoked_across_restart() {
        let f = Fixture::new();
        f.store
            .lease(Slot::Shared)
            .save(web(crate::auth::DEFAULT_WEB_CLIENT_ID))
            .await
            .unwrap();
        f.fake.lock().unwrap().delete_error = Some(Error::Locked);
        f.store.revoke_spotify().unwrap();
        assert_eq!(
            f.store.lease(Slot::Shared).delete().await,
            Err(Error::Locked)
        );
        assert_eq!(f.fake.lock().unwrap().values.len(), 1);
        // A revoked load must not even require access to the locked service.
        f.fake.lock().unwrap().read_error = Some(Error::Locked);
        assert!(
            f.restart()
                .lease(Slot::Shared)
                .load()
                .await
                .unwrap()
                .grant
                .is_none()
        );
    }

    #[tokio::test]
    async fn revoked_or_protected_slots_never_revive_stale_legacy_files() {
        let f = Fixture::new();
        let grant = web(crate::auth::DEFAULT_WEB_CLIENT_ID);
        f.store
            .lease(Slot::Shared)
            .save(grant.clone())
            .await
            .unwrap();
        f.fake.lock().unwrap().values.clear();
        write_legacy(&f.dirs.shared_web_token_file(), &grant);
        assert!(
            f.restart()
                .lease(Slot::Shared)
                .load()
                .await
                .unwrap()
                .grant
                .is_none()
        );
        f.store.revoke_spotify().unwrap();
        write_legacy(&f.dirs.shared_web_token_file(), &grant);
        assert!(
            f.restart()
                .lease(Slot::Shared)
                .load()
                .await
                .unwrap()
                .grant
                .is_none()
        );
    }

    #[tokio::test]
    async fn signout_removes_partial_and_corrupt_legacy_files_and_can_repeat() {
        let f = Fixture::new();
        let paths = [
            f.dirs.shared_web_token_file(),
            f.dirs.personal_web_token_file(),
            f.dirs.legacy_web_token_file(),
            f.dirs.credentials_dir().join("credentials.json"),
        ];
        for path in &paths {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"corrupt-dummy-grant").unwrap();
            std::fs::write(path.with_extension("json.tmp"), b"partial-dummy-grant").unwrap();
        }
        f.store.revoke_spotify().unwrap();
        f.store.revoke_spotify().unwrap();
        for path in paths {
            assert!(!path.exists());
            assert!(!path.with_extension("json.tmp").exists());
        }
    }

    #[tokio::test]
    async fn profiles_are_isolated_and_invalid_records_are_rejected() {
        let f = Fixture::new();
        let other = Fixture::new();
        let isolated = Store::with_backend(other.dirs.clone(), Box::new(Backend(f.fake.clone())));
        f.store
            .lease(Slot::Shared)
            .save(web(crate::auth::DEFAULT_WEB_CLIENT_ID))
            .await
            .unwrap();
        assert!(
            isolated
                .lease(Slot::Shared)
                .load()
                .await
                .unwrap()
                .grant
                .is_none()
        );
        for bytes in [
            b"not-json".to_vec(),
            serde_json::to_vec(&Record {
                version: 2,
                slot: Slot::Shared,
                grant: web(crate::auth::DEFAULT_WEB_CLIENT_ID),
            })
            .unwrap(),
            serde_json::to_vec(&Record {
                version: 1,
                slot: Slot::Playback,
                grant: playback(),
            })
            .unwrap(),
        ] {
            f.fake
                .lock()
                .unwrap()
                .values
                .insert(f.store.lease(Slot::Shared).key(), bytes);
            assert!(matches!(
                f.store.lease(Slot::Shared).load().await,
                Err(Error::Invalid)
            ));
        }
    }

    #[tokio::test]
    async fn startup_reads_retry_a_store_that_refuses_at_first() {
        let f = Fixture::new();
        let grant = web(crate::auth::DEFAULT_WEB_CLIENT_ID);
        f.store
            .lease(Slot::Shared)
            .save(grant.clone())
            .await
            .unwrap();
        f.fake.lock().unwrap().read_failures = 1;
        assert!(matches!(
            f.store.lease(Slot::Shared).load().await,
            Err(Error::Unavailable)
        ));
        f.fake.lock().unwrap().read_failures = 2;
        let loaded = f.store.lease(Slot::Shared).load_retrying().await.unwrap();
        assert!(loaded.grant == Some(grant));
        // A store that keeps refusing is reported, not mistaken for no grant.
        f.fake.lock().unwrap().read_failures = 10;
        assert!(matches!(
            f.store.lease(Slot::Shared).load_retrying().await,
            Err(Error::Unavailable)
        ));
    }

    #[test]
    fn native_errors_keep_only_their_variant_code_or_length() {
        use keyring_core::Error as E;
        let secret = b"dummy-native-secret".to_vec();
        let platform =
            |text: &str| -> keyring_core::error::PlatformError { text.to_string().into() };
        let cases = [
            (
                E::PlatformFailure(platform("Windows error code 1783")),
                "PlatformFailure (Win32 error 1783)",
                Error::Unavailable,
            ),
            (
                E::NoStorageAccess(platform("Windows ERROR_NO_SUCH_LOGON_SESSION")),
                "NoStorageAccess (Win32 error 1312)",
                Error::Locked,
            ),
            (
                E::PlatformFailure(platform("dummy-native-secret")),
                "PlatformFailure",
                Error::Unavailable,
            ),
            (
                E::BadEncoding(secret.clone()),
                "BadEncoding",
                Error::Unavailable,
            ),
            (
                E::BadDataFormat(secret.clone(), platform("dummy-native-secret")),
                "BadDataFormat",
                Error::Unavailable,
            ),
            (
                E::TooLong("dummy-native-secret".into(), 2560),
                "TooLong (limit 2560)",
                Error::Unavailable,
            ),
            (
                E::Invalid("dummy-native-secret".into(), "dummy-native-secret".into()),
                "Invalid",
                Error::Unavailable,
            ),
        ];
        for (error, expected, mapped) in cases {
            let cause = diagnose(&error);
            assert_eq!(cause.to_string(), expected);
            assert!(!format!("{cause:?}").contains("dummy-native-secret"));
            assert_eq!(native_error("test", error), mapped);
        }
    }

    // The protected fallback file is DPAPI, so Windows only.
    #[cfg(windows)]
    #[test]
    fn dpapi_round_trips_only_for_its_purpose() {
        let secret = b"dummy-dpapi-secret".to_vec();
        let protected = dpapi::protect(&secret, b"purpose-a").unwrap();
        assert!(!protected.windows(secret.len()).any(|part| part == secret));
        assert_eq!(dpapi::unprotect(&protected, b"purpose-a").unwrap(), secret);
        assert!(dpapi::unprotect(&protected, b"purpose-b").is_err());
        assert!(dpapi::unprotect(b"not protected", b"purpose-a").is_err());
    }

    #[derive(Default)]
    struct FakeNative {
        values: HashMap<String, (Vec<u8>, Persistence)>,
        error: Option<Error>,
        write_error: Option<Error>,
        /// What Credential Manager makes of a write; Local when unset.
        saves_as: Option<Persistence>,
        writes: usize,
    }
    struct NativeHandle(Arc<Mutex<FakeNative>>);
    impl Native for NativeHandle {
        fn read(&mut self, key: &str) -> Result<Option<Vec<u8>>, Error> {
            let fake = self.0.lock().unwrap();
            if let Some(error) = fake.error {
                return Err(error);
            }
            Ok(fake.values.get(key).map(|(value, _)| value.clone()))
        }
        fn persistence(&mut self, key: &str) -> Result<Option<Persistence>, Error> {
            let fake = self.0.lock().unwrap();
            if let Some(error) = fake.error {
                return Err(error);
            }
            Ok(fake.values.get(key).map(|(_, persistence)| *persistence))
        }
        fn write(&mut self, key: &str, secret: &[u8]) -> Result<(), Error> {
            let mut fake = self.0.lock().unwrap();
            if let Some(error) = fake.error.or(fake.write_error) {
                return Err(error);
            }
            fake.writes += 1;
            let persistence = fake.saves_as.unwrap_or(Persistence::Local);
            fake.values
                .insert(key.to_owned(), (secret.to_vec(), persistence));
            Ok(())
        }
        fn delete(&mut self, key: &str) -> Result<(), Error> {
            let mut fake = self.0.lock().unwrap();
            if let Some(error) = fake.error {
                return Err(error);
            }
            fake.values.remove(key);
            Ok(())
        }
    }

    fn durable(f: &Fixture) -> (Arc<Mutex<FakeNative>>, Durable<NativeHandle>) {
        let native = Arc::new(Mutex::new(FakeNative::default()));
        let store = Durable {
            native: NativeHandle(native.clone()),
            folder: f.dirs.state.join("credential-storage"),
        };
        (native, store)
    }

    // The protected fallback file is DPAPI, so Windows only.
    #[cfg(windows)]
    #[test]
    fn the_fallback_file_is_read_after_the_native_store_and_encrypted() {
        let f = Fixture::new();
        let (native, mut store) = durable(&f);
        let key = f.store.lease(Slot::Shared).key();
        let file = f
            .dirs
            .state
            .join("credential-storage")
            .join("shared-web.bin");
        let secret = b"dummy-fallback-secret".to_vec();

        // A durable native save needs no file.
        store.write(&key, &secret).unwrap();
        assert!(!file.exists());
        assert_eq!(store.read(&key).unwrap(), Some(secret.clone()));

        // A failed native save lands in the encrypted file, never plaintext.
        native.lock().unwrap().error = Some(Error::Locked);
        let newer = b"dummy-fallback-newer".to_vec();
        store.write(&key, &newer).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        assert!(!bytes.windows(newer.len()).any(|part| part == newer));
        assert_eq!(store.read(&key).unwrap(), Some(newer.clone()));

        // The older native entry left behind cannot shadow the newer file,
        // which moves back into the native store once that works again.
        native.lock().unwrap().error = None;
        assert_eq!(store.read(&key).unwrap(), Some(newer.clone()));
        assert!(!file.exists());
        assert_eq!(
            native.lock().unwrap().values.get(&key),
            Some(&(newer.clone(), Persistence::Local))
        );
        // An unreadable file never hides the native entry, and is reported
        // only when it is the sole copy.
        std::fs::write(&file, b"dummy unreadable").unwrap();
        assert_eq!(store.read(&key).unwrap(), Some(newer.clone()));
        native.lock().unwrap().values.clear();
        assert_eq!(store.read(&key), Err(Error::Invalid));
        std::fs::remove_file(&file).unwrap();

        // A later durable native save retires the file.
        store.write(&key, &secret).unwrap();
        assert!(!file.exists());

        // Forgetting removes both copies.
        native.lock().unwrap().write_error = Some(Error::Unavailable);
        store.write(&key, &newer).unwrap();
        assert!(file.exists());
        native.lock().unwrap().write_error = None;
        store.delete(&key).unwrap();
        assert!(!file.exists());
        assert_eq!(store.read(&key).unwrap(), None);
        // With neither copy, a failing native store is reported.
        native.lock().unwrap().error = Some(Error::Unavailable);
        assert_eq!(store.read(&key), Err(Error::Unavailable));
    }

    // The protected fallback file is DPAPI, so Windows only.
    #[cfg(windows)]
    #[test]
    fn session_only_and_oversized_saves_use_the_fallback_file() {
        let f = Fixture::new();
        let (native, mut store) = durable(&f);
        let key = f.store.lease(Slot::Playback).key();
        let file = f.dirs.state.join("credential-storage").join("playback.bin");

        native.lock().unwrap().saves_as = Some(Persistence::Session);
        store.write(&key, b"dummy-session-secret").unwrap();
        assert!(file.exists());
        assert!(native.lock().unwrap().values.is_empty());
        assert_eq!(
            store.read(&key).unwrap().as_deref(),
            Some(b"dummy-session-secret".as_slice())
        );

        native.lock().unwrap().saves_as = None;
        let writes = native.lock().unwrap().writes;
        let large = vec![b'x'; NATIVE_LIMIT.unwrap() + 1];
        store.write(&key, &large).unwrap();
        assert_eq!(native.lock().unwrap().writes, writes, "never sent natively");
        assert_eq!(store.read(&key).unwrap(), Some(large));
    }

    #[test]
    fn roaming_entries_move_to_local_persistence_when_read() {
        let f = Fixture::new();
        let (native, mut store) = durable(&f);
        let key = f.store.lease(Slot::Personal).key();
        native.lock().unwrap().values.insert(
            key.clone(),
            (b"dummy-roaming".to_vec(), Persistence::Enterprise),
        );
        assert_eq!(
            store.read(&key).unwrap().as_deref(),
            Some(b"dummy-roaming".as_slice())
        );
        let fake = native.lock().unwrap();
        assert_eq!(
            fake.values.get(&key),
            Some(&(b"dummy-roaming".to_vec(), Persistence::Local))
        );
        assert!(
            !f.dirs
                .state
                .join("credential-storage")
                .join("personal-web.bin")
                .exists()
        );
    }

    // The protected fallback file is DPAPI, so Windows only.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_locked_native_store_still_remembers_the_grant_across_restart() {
        let f = Fixture::new();
        let (native, backend) = durable(&f);
        native.lock().unwrap().error = Some(Error::Locked);
        let store = Store::with_backend(f.dirs.clone(), Box::new(backend));
        let grant = web(crate::auth::DEFAULT_WEB_CLIENT_ID);
        store.lease(Slot::Shared).save(grant.clone()).await.unwrap();
        let (_, backend) = durable(&f);
        let restarted = Store::with_backend(f.dirs.clone(), Box::new(backend));
        assert!(restarted.lease(Slot::Shared).load().await.unwrap().grant == Some(grant));
        restarted.revoke_spotify().unwrap();
        restarted.lease(Slot::Shared).delete().await.unwrap();
        assert!(
            !f.dirs
                .state
                .join("credential-storage")
                .join("shared-web.bin")
                .exists()
        );
    }
}
