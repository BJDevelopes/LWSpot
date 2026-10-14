//! Self-update from GitHub releases, through fastframe-update.
//!
//! The previous release's helper installs the first release built on the
//! crate, so what it relies on stays the same: `--version` prints
//! `<command> <version>`, the new app accepts `--update-receipt` and
//! `--update-error` (stripped by [`fastframe_update::intercept`] before the
//! argument parser), and the handoff and receipt files keep their format.

pub use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Installation, Kind, Prepared, Release, Source, Unsupported,
    Updater,
};
use fastframe_update::{ReqwestTransport, UpdateConfig};

/// Updates come only from LWSpot's own releases: the installer puts the app
/// at `%LOCALAPPDATA%\Programs\LWSpot\lwspot.exe`, and a portable copy is
/// recognised by the `lwspot-portable.txt` marker beside it. On macOS the
/// app is `LWSpot.app` (packaging/macos), updated from the release's
/// `lwspot-v<version>-macos-universal.dmg`.
pub const CONFIG: UpdateConfig = UpdateConfig {
    // Releases are verified against checksums.txt alone until they are
    // signed. Only a version shipped after the first signed release may
    // carry the key: from then on an unsigned release is refused.
    publisher_key: None,
    macos: fastframe_update::MacConfig {
        bundle_ids: &["io.github.BJDevelopes.LWSpot"],
        executable_names: &["lwspot"],
        legacy_bundle_names: &[],
    },
    ..UpdateConfig::new(
        // GitHub's own casing, which its asset URLs use.
        "BJDevelopes/LWSpot",
        "LWSpot",
        "lwspot",
        env!("CARGO_PKG_VERSION"),
    )
};

/// An updater on LWSpot's HTTP client, through the configured proxy.
pub fn updater(proxy: &crate::settings::ProxyConfig) -> anyhow::Result<Updater> {
    let builder = crate::http::blocking_builder(proxy).map_err(anyhow::Error::msg)?;
    Ok(Updater::new(CONFIG, ReqwestTransport::new(builder)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_config_is_valid() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, env!("CARGO_PKG_VERSION"));
    }
}
