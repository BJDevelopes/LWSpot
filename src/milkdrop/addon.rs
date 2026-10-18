//! The MilkDrop add-on, `lwspot-milkdrop.exe`.
//!
//! projectM stays out of `lwspot.exe`; the window is this separate program.
//! LWSpot looks for it beside its own executable first (development builds,
//! and anyone who puts it there), then in the add-ons folder, and uses one
//! only when its `--version` names this app's version. So nothing is fetched
//! until someone turns MilkDrop on, and after an app update the stale add-on
//! is replaced the next time they do.
//!
//! A download comes from this version's GitHub release and is checked
//! against the release's `checksums.txt` before anything is written. Only
//! then is the add-on run, for its `--version`, and moved into place.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// The add-on's file name, wherever it is.
pub const FILE_NAME: &str = if cfg!(windows) {
    "lwspot-milkdrop.exe"
} else {
    "lwspot-milkdrop"
};
/// The name its `--version` starts with.
pub const COMMAND: &str = "lwspot-milkdrop";
/// The version an add-on has to report: the app's own.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where releases keep their files, by tag.
const RELEASES: &str = "https://github.com/BJDevelopes/lwspot/releases/download";
/// The download is written here first, run, and only then moved into place.
const STAGED_NAME: &str = if cfg!(windows) {
    "lwspot-milkdrop.new.exe"
} else {
    "lwspot-milkdrop.new"
};
/// How long `--version` may take. A fresh download can wait on the virus
/// scanner the first time it runs.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the whole download may take on a slow line.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// More than `checksums.txt` ever holds.
const CHECKSUMS_LIMIT: u64 = 64 << 10;
/// More than the add-on's zip ever weighs.
const ZIP_LIMIT: u64 = 64 << 20;

/// What `--version` prints: `lwspot-milkdrop <version>`.
pub fn version_line(version: &str) -> String {
    format!("{COMMAND} {version}")
}

/// Whether what an add-on printed for `--version` names this version.
pub fn reports_version(output: &str, version: &str) -> bool {
    let mut words = output.split_whitespace();
    words.next() == Some(COMMAND) && words.next() == Some(version) && words.next().is_none()
}

/// The Rust target the release names its files by, the same as this build's.
pub fn target() -> &'static str {
    match (cfg!(target_os = "macos"), cfg!(target_arch = "aarch64")) {
        (true, true) => "aarch64-apple-darwin",
        (true, false) => "x86_64-apple-darwin",
        (false, true) => "aarch64-pc-windows-msvc",
        (false, false) => "x86_64-pc-windows-msvc",
    }
}

/// The release file that holds the add-on for a version and target.
pub fn zip_name(version: &str, target: &str) -> String {
    format!("{COMMAND}-v{version}-{target}.zip")
}

/// Where a file of a version's release is downloaded from.
pub fn release_url(version: &str, file: &str) -> String {
    format!("{RELEASES}/v{version}/{file}")
}

/// Why the add-on could not be had.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// The release has no add-on for this version and target, or there is no
    /// release of this version at all (a development build).
    #[error("no MilkDrop add-on is published for this version")]
    Unavailable,
    #[error("{0}")]
    Network(String),
    #[error("the download is larger than an add-on can be")]
    TooLarge,
    #[error("the download does not match checksums.txt")]
    Checksum,
    #[error("the download is damaged: {0}")]
    Archive(String),
    #[error("the downloaded add-on is not for LWSpot {0}")]
    WrongVersion(String),
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// The two places an add-on can be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Places {
    /// The folder `lwspot.exe` is in.
    pub beside: Option<PathBuf>,
    /// The add-ons folder, where downloads go.
    pub addons: PathBuf,
}

impl Places {
    pub fn new(dirs: &crate::paths::AppDirs) -> Self {
        Self {
            beside: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf)),
            addons: dirs.addons_dir(),
        }
    }

    /// Where to look, in order: beside the app, then the add-ons folder.
    pub fn candidates(&self) -> Vec<PathBuf> {
        self.beside
            .iter()
            .map(|dir| dir.join(FILE_NAME))
            .chain(std::iter::once(self.installed()))
            .collect()
    }

    /// Where a downloaded add-on lives, and what Remove deletes.
    pub fn installed(&self) -> PathBuf {
        self.addons.join(FILE_NAME)
    }
}

/// The first candidate that exists and reports `version`. `probe` runs one
/// and returns what it printed.
pub fn find(
    candidates: &[PathBuf],
    version: &str,
    mut probe: impl FnMut(&Path) -> Option<String>,
) -> Option<PathBuf> {
    candidates
        .iter()
        .filter(|path| path.is_file())
        .find(|path| probe(path).is_some_and(|output| reports_version(&output, version)))
        .cloned()
}

/// Runs an add-on with `--version`, with no console window, and returns what
/// it printed; `None` if it would not run or took too long.
pub fn probe(path: &Path) -> Option<String> {
    let mut command = Command::new(path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            log::warn!("MilkDrop add-on {} would not run: {error}", path.display());
            return None;
        }
    };
    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                log::warn!("MilkDrop add-on {} did not answer", path.display());
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    // One short line; it fits in the pipe, so the add-on has exited
    // without waiting for it to be read.
    let mut output = String::new();
    child
        .stdout
        .take()?
        .take(1024)
        .read_to_string(&mut output)
        .ok()?;
    Some(output)
}

/// The SHA-256 that `checksums.txt` lists for a file. Each line is
/// `<sha256>  <file name>`, as `sha256sum` writes it.
pub fn checksum_for(manifest: &str, file: &str) -> Option<[u8; 32]> {
    manifest.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        // A `*` marks a file hashed in binary mode.
        let name = name.trim_start().trim_start_matches('*');
        if name == file {
            decode_sha256(hash)
        } else {
            None
        }
    })
}

fn decode_sha256(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (byte, pair) in out.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Checks a download against `checksums.txt`.
pub fn verify(bytes: &[u8], manifest: &str, file: &str) -> Result<(), Failure> {
    // A release with no line for the add-on has no add-on.
    let expected = checksum_for(manifest, file).ok_or(Failure::Unavailable)?;
    if Sha256::digest(bytes).as_slice() == expected {
        Ok(())
    } else {
        Err(Failure::Checksum)
    }
}

/// The add-on's executable out of its zip, which also carries the licence.
pub fn extract(zip: &[u8]) -> Result<Vec<u8>, Failure> {
    let archive = crate::skin::zip::Archive::parse(zip)
        .map_err(|error| Failure::Archive(error.to_string()))?;
    let entry = archive
        .entries()
        .iter()
        .find(|entry| !entry.is_dir() && entry.base_name().eq_ignore_ascii_case(FILE_NAME))
        .ok_or_else(|| Failure::Archive(format!("it holds no {FILE_NAME}")))?;
    archive
        .read(entry)
        .map_err(|error| Failure::Archive(error.to_string()))
}

/// Writes a verified add-on into the folder: under a staging name first,
/// which must report `version`, then over the old add-on in one move, so a
/// failure anywhere leaves either the old add-on or none, never half of one.
pub fn place(
    folder: &Path,
    exe: &[u8],
    version: &str,
    probe: impl FnOnce(&Path) -> Option<String>,
) -> Result<PathBuf, Failure> {
    std::fs::create_dir_all(folder)?;
    let staged = folder.join(STAGED_NAME);
    let path = folder.join(FILE_NAME);
    std::fs::write(&staged, exe)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    if !probe(&staged).is_some_and(|output| reports_version(&output, version)) {
        let _ = std::fs::remove_file(&staged);
        return Err(Failure::WrongVersion(version.to_string()));
    }
    if let Err(error) = crate::util::replace_file(&staged, &path) {
        let _ = std::fs::remove_file(&staged);
        return Err(error.into());
    }
    Ok(path)
}

/// Downloads, verifies, and installs the add-on for this version.
/// `progress` hears the bytes of the zip as they arrive, and its size when
/// the server says.
pub fn install(
    places: &Places,
    proxy: &crate::settings::ProxyConfig,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, Failure> {
    let http = crate::http::build_blocking(proxy, DOWNLOAD_TIMEOUT).map_err(Failure::Network)?;
    let manifest = download(
        &http,
        &release_url(VERSION, "checksums.txt"),
        CHECKSUMS_LIMIT,
        |_, _| {},
    )?;
    let manifest = String::from_utf8_lossy(&manifest);
    let name = zip_name(VERSION, target());
    // Before the zip, so a release without the add-on costs one small file.
    checksum_for(&manifest, &name).ok_or(Failure::Unavailable)?;
    let zip = download(&http, &release_url(VERSION, &name), ZIP_LIMIT, progress)?;
    verify(&zip, &manifest, &name)?;
    let exe = extract(&zip)?;
    place(&places.addons, &exe, VERSION, probe)
}

fn download(
    http: &reqwest::blocking::Client,
    url: &str,
    limit: u64,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, Failure> {
    let network = |error: reqwest::Error| Failure::Network(error.without_url().to_string());
    let response = http.get(url).send().map_err(network)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Failure::Unavailable);
    }
    let mut response = response.error_for_status().map_err(network)?;
    let total = response.content_length();
    if total.is_some_and(|total| total > limit) {
        return Err(Failure::TooLarge);
    }
    let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut chunk = vec![0u8; 64 << 10];
    loop {
        let read = response
            .read(&mut chunk)
            .map_err(|error| Failure::Network(error.to_string()))?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() as u64 > limit {
            return Err(Failure::TooLarge);
        }
        progress(bytes.len() as u64, total);
    }
}

/// Where the add-on stands for this run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not looked for yet.
    Unknown,
    /// Looking for a usable one.
    Checking,
    /// On its way: the bytes so far, and the size when known.
    Downloading { received: u64, total: Option<u64> },
    /// None that reports this version.
    Missing,
    /// Ready to run, from here.
    Ready(PathBuf),
}

/// What a finished look or download comes to, for the app to act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The download has started.
    Downloading,
    /// A usable add-on, just downloaded or found already there.
    Ready {
        downloaded: bool,
    },
    /// A look found none; nothing was downloaded.
    Missing,
    Failed(Failure),
}

/// The worker's reports.
enum Message {
    Progress(u64, Option<u64>),
    Done(Result<Option<(PathBuf, bool)>, Failure>),
}

/// The add-on for this run: where it is, and the look or download on its
/// way. The work runs on a thread of its own; the app polls it each frame.
pub struct Addon {
    places: Places,
    status: Status,
    job: Option<mpsc::Receiver<Message>>,
    /// Whether the job on its way downloads when it finds nothing.
    fetching: bool,
    /// Open the window as soon as the add-on is ready: someone turned
    /// MilkDrop on before it was.
    pub open_when_ready: bool,
    /// Tests stand in for the worker through this end of its channel.
    #[cfg(test)]
    worker: Option<mpsc::Sender<Message>>,
}

impl Addon {
    pub fn new(places: Places) -> Self {
        Self {
            places,
            status: Status::Unknown,
            job: None,
            fetching: false,
            open_when_ready: false,
            #[cfg(test)]
            worker: None,
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn places(&self) -> &Places {
        &self.places
    }

    /// The add-on to run, once one has been found or installed.
    pub fn ready(&self) -> Option<&Path> {
        match &self.status {
            Status::Ready(path) => Some(path),
            _ => None,
        }
    }

    /// Whether a look or a download is on its way.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Whether a download has been asked for and is not done yet.
    pub fn fetching(&self) -> bool {
        self.job.is_some() && self.fetching
    }

    /// Whether the add-on that is ready is the one Remove deletes, rather
    /// than one placed beside the app.
    pub fn removable(&self) -> bool {
        self.ready() == Some(self.places.installed().as_path())
    }

    /// Looks for a usable add-on, without downloading, once a run.
    pub fn look(&mut self, ctx: &egui::Context) {
        if self.job.is_none() && self.status == Status::Unknown {
            self.start(None, ctx);
        }
    }

    /// Looks for a usable add-on and downloads one when there is none.
    pub fn fetch(&mut self, proxy: crate::settings::ProxyConfig, ctx: &egui::Context) {
        if self.fetching() {
            return;
        }
        // A look on its way is overtaken: the download looks first anyway.
        self.start(Some(proxy), ctx);
    }

    fn start(&mut self, proxy: Option<crate::settings::ProxyConfig>, ctx: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.fetching = proxy.is_some();
        self.status = Status::Checking;
        self.job = Some(receiver);
        #[cfg(test)]
        {
            // Tests never run add-ons or reach GitHub.
            let _ = (proxy, ctx);
            self.worker = Some(sender);
        }
        #[cfg(not(test))]
        {
            let places = self.places.clone();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("milkdrop-addon".into())
                .spawn(move || {
                    let result = resolve(&places, proxy.as_ref(), |received, total| {
                        let _ = sender.send(Message::Progress(received, total));
                        ctx.request_repaint();
                    });
                    let _ = sender.send(Message::Done(result));
                    ctx.request_repaint();
                });
            if let Err(error) = spawned {
                log::warn!("could not start looking for the MilkDrop add-on: {error}");
                self.job = None;
                self.status = Status::Unknown;
            }
        }
    }

    /// Takes the worker's news: progress, and how the job ended.
    pub fn poll(&mut self) -> Option<Outcome> {
        let receiver = self.job.as_ref()?;
        let mut outcome = None;
        loop {
            match receiver.try_recv() {
                Ok(Message::Progress(received, total)) => {
                    if !matches!(self.status, Status::Downloading { .. }) {
                        outcome = Some(Outcome::Downloading);
                    }
                    self.status = Status::Downloading { received, total };
                }
                Ok(Message::Done(result)) => {
                    self.job = None;
                    let (status, done) = match result {
                        Ok(Some((path, downloaded))) => {
                            (Status::Ready(path), Outcome::Ready { downloaded })
                        }
                        Ok(None) => (Status::Missing, Outcome::Missing),
                        Err(failure) => (Status::Missing, Outcome::Failed(failure)),
                    };
                    self.status = status;
                    return Some(done);
                }
                Err(mpsc::TryRecvError::Empty) => return outcome,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.status = Status::Unknown;
                    return outcome;
                }
            }
        }
    }

    /// Ends the job on its way the way a worker would, for the app's tests.
    #[cfg(test)]
    pub(crate) fn finish(&mut self, result: Result<Option<(PathBuf, bool)>, Failure>) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.send(Message::Done(result));
        }
    }

    /// Looks again next time: the add-on that was found did not start.
    pub fn forget(&mut self) {
        if self.job.is_none() {
            self.status = Status::Unknown;
        }
    }

    /// Deletes the downloaded add-on. One beside the app is the user's own
    /// and stays; the next look finds it again.
    pub fn remove(&mut self) -> std::io::Result<()> {
        self.open_when_ready = false;
        let removed = remove_if_there(&self.places.installed())
            .and_then(|()| remove_if_there(&self.places.addons.join(STAGED_NAME)));
        self.forget();
        removed
    }
}

fn remove_if_there(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The worker's job: a usable add-on already there, or, when `proxy` allows
/// a download, a fresh one. `Ok(None)` when there is none and none was
/// fetched. The flag says whether it was downloaded.
#[cfg_attr(test, allow(dead_code))]
fn resolve(
    places: &Places,
    proxy: Option<&crate::settings::ProxyConfig>,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<Option<(PathBuf, bool)>, Failure> {
    if let Some(found) = find(&places.candidates(), VERSION, probe) {
        return Ok(Some((found, false)));
    }
    let Some(proxy) = proxy else {
        return Ok(None);
    };
    install(places, proxy, progress).map(|path| Some((path, true)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = crate::test_support::temp_dir()
            .join(format!("lwspot-addon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn only_this_version_is_good_enough() {
        assert!(reports_version("lwspot-milkdrop 1.2.3\r\n", "1.2.3"));
        assert!(reports_version(&version_line(VERSION), VERSION));
        assert!(!reports_version("lwspot-milkdrop 1.2.2\n", "1.2.3"));
        assert!(!reports_version("lwspot-milkdrop 1.2.3-rc1", "1.2.3"));
        assert!(
            !reports_version("lwspot 1.2.3", "1.2.3"),
            "the app is not the add-on"
        );
        assert!(!reports_version("", "1.2.3"));
        assert!(!reports_version("lwspot-milkdrop 1.2.3 extra", "1.2.3"));
    }

    /// Beside the app first, where a development build puts it, then the
    /// add-ons folder; a file that is there but reports another version
    /// is passed over, and nothing that isn't there is run.
    #[test]
    fn the_add_on_beside_the_app_wins_and_a_stale_one_is_passed_over() {
        let root = temp_dir("lookup");
        let places = Places {
            beside: Some(root.join("app")),
            addons: root.join("addons"),
        };
        let candidates = places.candidates();
        assert_eq!(
            candidates,
            [
                root.join("app").join(FILE_NAME),
                root.join("addons").join(FILE_NAME)
            ]
        );
        assert_eq!(places.installed(), candidates[1]);

        let mut ran = Vec::new();
        assert_eq!(
            find(&candidates, "1.0.0", |path| {
                ran.push(path.to_path_buf());
                Some(version_line("1.0.0"))
            }),
            None
        );
        assert!(ran.is_empty(), "nothing that isn't there is run");

        std::fs::create_dir_all(&places.addons).unwrap();
        std::fs::write(&candidates[1], b"addon").unwrap();
        let app = root.join("app");
        let reports = |beside: &str, installed: &str| {
            let (beside, installed, app) = (beside.to_string(), installed.to_string(), app.clone());
            move |path: &Path| {
                Some(version_line(if path.starts_with(&app) {
                    &beside
                } else {
                    &installed
                }))
            }
        };
        assert_eq!(
            find(&candidates, "1.0.0", reports("1.0.0", "1.0.0")),
            Some(candidates[1].clone())
        );
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(&candidates[0], b"addon").unwrap();
        assert_eq!(
            find(&candidates, "1.0.0", reports("1.0.0", "1.0.0")),
            Some(candidates[0].clone()),
            "beside the app comes first"
        );
        assert_eq!(
            find(&candidates, "1.0.0", reports("0.9.0", "1.0.0")),
            Some(candidates[1].clone()),
            "an old one beside the app is passed over"
        );
        assert_eq!(
            find(&candidates, "1.0.0", reports("0.9.0", "0.9.0")),
            None,
            "after an app update neither is usable"
        );
        assert_eq!(
            find(&candidates, "1.0.0", |_| None),
            None,
            "one that won't run"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn downloads_are_named_for_the_version_and_the_processor() {
        assert_eq!(
            zip_name("0.2.0", "x86_64-pc-windows-msvc"),
            "lwspot-milkdrop-v0.2.0-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            release_url("0.2.0", &zip_name("0.2.0", "aarch64-pc-windows-msvc")),
            "https://github.com/BJDevelopes/lwspot/releases/download/v0.2.0/lwspot-milkdrop-v0.2.0-aarch64-pc-windows-msvc.zip"
        );
        assert_eq!(
            release_url("0.2.0", "checksums.txt"),
            "https://github.com/BJDevelopes/lwspot/releases/download/v0.2.0/checksums.txt"
        );
        let expected = match (cfg!(target_os = "macos"), cfg!(target_arch = "aarch64")) {
            (true, true) => "aarch64-apple-darwin",
            (true, false) => "x86_64-apple-darwin",
            (false, true) => "aarch64-pc-windows-msvc",
            (false, false) => "x86_64-pc-windows-msvc",
        };
        assert_eq!(target(), expected);
    }

    /// checksums.txt is `sha256sum`'s output for the whole release.
    #[test]
    fn a_download_must_match_its_line_in_checksums() {
        let zip = b"the add-on".as_slice();
        let sum = hex(&Sha256::digest(zip));
        let name = zip_name("1.0.0", "x86_64-pc-windows-msvc");
        let manifest = format!(
            "{}  lwspot-v1.0.0-x86_64-pc-windows-msvc.zip\n{sum}  {name}\n{}  lwspot-v1.0.0-setup.exe\n",
            "0".repeat(64),
            "1".repeat(64)
        );
        assert_eq!(
            checksum_for(&manifest, &name).map(|sum| hex(&sum)),
            Some(sum.clone())
        );
        assert_eq!(verify(zip, &manifest, &name), Ok(()));
        assert_eq!(
            verify(b"something else", &manifest, &name),
            Err(Failure::Checksum)
        );
        assert_eq!(
            verify(
                zip,
                &manifest,
                "lwspot-milkdrop-v1.0.0-aarch64-pc-windows-msvc.zip"
            ),
            Err(Failure::Unavailable),
            "a release without the add-on has none to offer"
        );
        // Binary-mode marks, upper case, and Windows line ends read the same.
        let binary = format!("{}  *{name}\r\n", sum.to_uppercase());
        assert_eq!(verify(zip, &binary, &name), Ok(()));
        // A broken hash is no hash.
        assert_eq!(
            checksum_for(&format!("{}  {name}", &sum[..63]), &name),
            None
        );
        assert_eq!(
            checksum_for(&format!("{}zz  {name}", &sum[..62]), &name),
            None
        );
        assert_eq!(checksum_for("", &name), None);
    }

    #[test]
    fn the_executable_comes_out_of_its_zip() {
        let zip = crate::skin::zip::write(&[
            ("LICENSE", b"MIT".as_slice(), true),
            (FILE_NAME, b"MZ program".as_slice(), true),
        ]);
        assert_eq!(extract(&zip), Ok(b"MZ program".to_vec()));
        let without = crate::skin::zip::write(&[("LICENSE", b"MIT".as_slice(), false)]);
        assert!(matches!(extract(&without), Err(Failure::Archive(_))));
        assert!(matches!(extract(b"not a zip"), Err(Failure::Archive(_))));
    }

    /// The new add-on replaces the old one only once it has answered with
    /// the right version; one that doesn't leaves nothing behind.
    #[test]
    fn an_add_on_is_put_in_place_only_once_it_answers() {
        let root = temp_dir("place");
        let installed = root.join(FILE_NAME);
        std::fs::write(&installed, b"old").unwrap();

        let wrong = place(&root, b"new", "1.0.0", |_| Some(version_line("0.9.0")));
        assert_eq!(wrong, Err(Failure::WrongVersion("1.0.0".into())));
        assert_eq!(std::fs::read(&installed).unwrap(), b"old");
        assert!(!root.join(STAGED_NAME).exists());

        let mut asked = None;
        let placed = place(&root, b"new", "1.0.0", |path| {
            asked = Some(path.to_path_buf());
            Some(version_line("1.0.0"))
        });
        assert_eq!(placed, Ok(installed.clone()));
        assert_eq!(
            asked,
            Some(root.join(STAGED_NAME)),
            "the staged copy answers, not the old one"
        );
        assert_eq!(std::fs::read(&installed).unwrap(), b"new");
        assert!(!root.join(STAGED_NAME).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn addon(root: &Path) -> Addon {
        Addon::new(Places {
            beside: None,
            addons: root.join("addons"),
        })
    }

    #[test]
    fn a_download_reports_its_progress_and_ends_ready() {
        let root = temp_dir("job");
        let ctx = egui::Context::default();
        let mut addon = addon(&root);
        assert_eq!(addon.status(), &Status::Unknown);
        addon.fetch(crate::settings::ProxyConfig::default(), &ctx);
        assert!(addon.fetching());
        assert_eq!(addon.status(), &Status::Checking);
        assert_eq!(addon.poll(), None);

        let worker = addon.worker.take().unwrap();
        worker.send(Message::Progress(10, Some(100))).unwrap();
        assert_eq!(addon.poll(), Some(Outcome::Downloading));
        worker.send(Message::Progress(50, Some(100))).unwrap();
        assert_eq!(addon.poll(), None, "the start is news once");
        assert_eq!(
            addon.status(),
            &Status::Downloading {
                received: 50,
                total: Some(100)
            }
        );
        let installed = addon.places().installed();
        worker
            .send(Message::Done(Ok(Some((installed.clone(), true)))))
            .unwrap();
        assert_eq!(addon.poll(), Some(Outcome::Ready { downloaded: true }));
        assert_eq!(addon.ready(), Some(installed.as_path()));
        assert!(addon.removable());
        assert!(!addon.busy());

        std::fs::create_dir_all(&addon.places().addons).unwrap();
        std::fs::write(&installed, b"addon").unwrap();
        addon.open_when_ready = true;
        addon.remove().unwrap();
        assert!(!installed.exists());
        assert!(!addon.open_when_ready);
        assert_eq!(addon.status(), &Status::Unknown, "the next use looks again");
        assert!(addon.remove().is_ok(), "nothing there is nothing to remove");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_look_finds_nothing_and_a_failure_is_reported() {
        let root = temp_dir("look");
        let ctx = egui::Context::default();
        let mut addon = addon(&root);
        addon.look(&ctx);
        assert!(addon.busy());
        assert!(!addon.fetching(), "a look downloads nothing");
        let worker = addon.worker.take().unwrap();
        worker.send(Message::Done(Ok(None))).unwrap();
        assert_eq!(addon.poll(), Some(Outcome::Missing));
        assert_eq!(addon.status(), &Status::Missing);
        addon.look(&ctx);
        assert!(!addon.busy(), "one look a run");

        addon.fetch(crate::settings::ProxyConfig::default(), &ctx);
        let worker = addon.worker.take().unwrap();
        worker
            .send(Message::Done(Err(Failure::Unavailable)))
            .unwrap();
        assert_eq!(addon.poll(), Some(Outcome::Failed(Failure::Unavailable)));
        assert_eq!(addon.ready(), None);

        // An add-on beside the app is ready but not Remove's to delete.
        addon.fetch(crate::settings::ProxyConfig::default(), &ctx);
        let worker = addon.worker.take().unwrap();
        worker
            .send(Message::Done(Ok(Some((root.join(FILE_NAME), false)))))
            .unwrap();
        assert_eq!(addon.poll(), Some(Outcome::Ready { downloaded: false }));
        assert!(!addon.removable());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
