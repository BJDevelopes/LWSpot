//! Web API answers kept per account so the shared app is asked less.
//!
//! Several requests can only go to the shared Web API app, whose quota
//! Spotify divides among every user: the playlist library, Spotify-owned
//! playlists, recommendations, and parts of artist pages. Their answers
//! change slowly, so they are kept on disk under the account and reused:
//!
//! - the library's playlist list, shown at once on startup and read again
//!   in the background when it is older than a few minutes;
//! - Home's "Made for you" playlists for a day and recommendations for a few
//!   hours;
//! - an artist's top tracks and related artists for a day.
//!
//! Manual refreshes still ask Spotify. Files are versioned JSON, written
//! atomically, and safe to delete.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::api::models::{Artist, Playlist, Track};
use crate::model::Loadable;
use crate::paths::AppDirs;

const VERSION: u32 = 1;
const HOUR: i64 = 60 * 60;
/// "Made for you" playlists change weekly or daily.
pub const DISCOVER_TTL: i64 = 24 * HOUR;
pub const RECOMMENDATIONS_TTL: i64 = 4 * HOUR;
pub const ARTIST_TTL: i64 = 24 * HOUR;
/// How old the saved playlist list may be before startup reads it again.
pub const PLAYLIST_LIST_FRESH: i64 = 5 * 60;
/// Artist answers kept in memory; the rest stay on disk.
const ARTISTS_IN_MEMORY: usize = 64;
/// Artist files kept on disk per account; the oldest go first.
const ARTIST_FILES_KEPT: usize = 200;
/// Playlist searches answered from memory, and for how long.
pub const PLAYLIST_SEARCHES_KEPT: usize = 32;
pub const PLAYLIST_SEARCH_TTL: Duration = Duration::from_secs(10 * 60);

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// Whether something saved at `saved_at` is younger than `ttl` at `now`. A
/// time in the future (the clock moved back) is not fresh.
pub fn fresh(saved_at: i64, now: i64, ttl: i64) -> bool {
    (0..ttl).contains(&now.saturating_sub(saved_at))
}

async fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    write_bytes(
        path,
        serde_json::to_vec(value).map_err(std::io::Error::other)?,
    )
    .await
}

// ---- the playlist list ------------------------------------------------------

/// The library's playlists as last read in full from Spotify, with the
/// changes made here since.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaylistList {
    version: u32,
    pub account_id: String,
    /// When the list was last read in full from Spotify. Changes made here
    /// keep this time, so a startup still reads the list again in time.
    pub read_at: i64,
    pub items: Vec<Playlist>,
}

impl PlaylistList {
    pub fn new(account_id: String, read_at: i64, items: Vec<Playlist>) -> Self {
        Self {
            version: VERSION,
            account_id,
            read_at,
            items,
        }
    }

    pub fn valid_for(&self, account: &str) -> bool {
        self.version == VERSION && self.account_id == account
    }
}

pub fn playlist_list_file(dirs: &AppDirs, account: &str) -> PathBuf {
    dirs.web_api_cache_dir(account).join("playlists.json")
}

pub async fn read_playlist_list(dirs: &AppDirs, account: &str) -> Option<PlaylistList> {
    read_json::<PlaylistList>(&playlist_list_file(dirs, account))
        .await
        .filter(|list| list.valid_for(account))
}

pub async fn write_playlist_list(dirs: &AppDirs, list: &PlaylistList) -> std::io::Result<()> {
    write_json(&playlist_list_file(dirs, &list.account_id), list).await
}

/// A change to the library made here, applied to the list in memory
/// instead of reading every page again.
#[derive(Clone, Debug, PartialEq)]
pub enum ListChange {
    /// Spotify lists a newly followed or created playlist first.
    Followed(Playlist),
    Unfollowed(String),
    Details {
        id: String,
        name: Option<String>,
        description: Option<String>,
        public: Option<bool>,
    },
}

pub fn apply_change(list: &mut Vec<Playlist>, change: ListChange) {
    match change {
        ListChange::Followed(playlist) => {
            if !list.iter().any(|listed| listed.id == playlist.id) {
                list.insert(0, playlist);
            }
        }
        ListChange::Unfollowed(id) => list.retain(|listed| listed.id != id),
        ListChange::Details {
            id,
            name,
            description,
            public,
        } => {
            for listed in list.iter_mut().filter(|listed| listed.id == id) {
                if let Some(name) = &name {
                    listed.name = name.clone();
                }
                if let Some(description) = &description {
                    listed.description = Some(description.clone());
                }
                if let Some(public) = public {
                    listed.public = Some(public);
                }
            }
        }
    }
}

/// Playlist URIs a refreshed list no longer has.
pub fn dropped_uris(old: &[Playlist], new: &[Playlist]) -> Vec<String> {
    let kept: HashSet<&str> = new.iter().map(|playlist| playlist.uri.as_str()).collect();
    old.iter()
        .map(|playlist| playlist.uri.as_str())
        .filter(|uri| !uri.is_empty() && !kept.contains(uri))
        .map(str::to_owned)
        .collect()
}

/// How much of the library's playlist list is known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ListView<'a> {
    /// Every page has been read (a background refresh may be under way).
    Complete(&'a [Playlist]),
    Loading,
    /// Not asked for, or it failed.
    Unavailable,
}

pub fn list_view<'a>(
    playlists: &'a Loadable<Vec<Playlist>>,
    next_offset: Option<u32>,
    asked: Option<u32>,
    refreshing: bool,
) -> ListView<'a> {
    match playlists {
        Loadable::Loaded(items) if next_offset.is_none() && (asked.is_none() || refreshing) => {
            ListView::Complete(items)
        }
        Loadable::Loaded(_) | Loadable::Loading => ListView::Loading,
        Loadable::NotLoaded | Loadable::Failed(_) => ListView::Unavailable,
    }
}

/// How the saved state of a playlist is found out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedAnswer {
    /// The complete list answers: followed playlists are the ones listed.
    Known(bool),
    /// The list is still loading; ask it once it is complete.
    Wait,
    /// Only Spotify can answer.
    Ask,
}

pub fn playlist_saved(list: ListView<'_>, uri: &str) -> SavedAnswer {
    match list {
        ListView::Complete(items) => SavedAnswer::Known(items.iter().any(|item| item.uri == uri)),
        ListView::Loading => SavedAnswer::Wait,
        ListView::Unavailable => SavedAnswer::Ask,
    }
}

/// "Made for you" terms Spotify makes a single playlist for, so a
/// followed copy answers for the search. There are several Daily Mixes.
pub fn single_playlist_term(term: &str) -> bool {
    !term.eq_ignore_ascii_case("daily mix")
}

/// The Spotify-owned playlist in the library that is `term`'s only one.
pub fn followed_made_for_you(
    list: &[Playlist],
    term: &str,
    matches: impl Fn(&str, &str) -> bool,
) -> Option<Playlist> {
    if !single_playlist_term(term) {
        return None;
    }
    list.iter()
        .find(|playlist| {
            let owner = playlist.owner.id.as_deref().unwrap_or("");
            (owner == "spotify" || playlist.owner_name() == "Spotify")
                && matches(&playlist.name, term)
        })
        .cloned()
}

/// A playlist detail edit sent to Spotify, applied to the list once
/// Spotify confirms it.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailEdit {
    pub name: Option<String>,
    pub description: Option<String>,
    pub public: Option<bool>,
}

/// What the interface keeps for the playlist list and its saved states.
#[derive(Debug, Default)]
pub struct CacheState {
    /// When the list on screen was last read in full from Spotify.
    pub list_read_at: Option<i64>,
    /// Pages of a background refresh, gathered while the list stays shown.
    pub list_refresh: Option<Vec<Playlist>>,
    /// A later page failed: the list shown is not the whole library.
    pub list_incomplete: bool,
    /// Playlist saved-state questions waiting for the list to complete.
    pub held_contains: Vec<String>,
    pub detail_edits: HashMap<String, DetailEdit>,
}

// ---- answers read through the cache --------------------------------------

/// A request whose answer is kept.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Discover(String),
    Recommendations,
    TopTracks(String),
    Related(String),
}

impl Key {
    fn ttl(&self) -> i64 {
        match self {
            Self::Discover(_) => DISCOVER_TTL,
            Self::Recommendations => RECOMMENDATIONS_TTL,
            Self::TopTracks(_) | Self::Related(_) => ARTIST_TTL,
        }
    }

    fn artist(&self) -> Option<&str> {
        match self {
            Self::TopTracks(id) | Self::Related(id) => Some(id),
            Self::Discover(_) | Self::Recommendations => None,
        }
    }
}

/// What a manual refresh asks Spotify for again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Home,
    Artist(String),
}

impl Scope {
    fn keys(&self) -> Vec<Key> {
        match self {
            Self::Home => crate::model::DISCOVER_TERMS
                .iter()
                .map(|term| Key::Discover((*term).to_string()))
                .chain([Key::Recommendations])
                .collect(),
            Self::Artist(id) => vec![Key::TopTracks(id.clone()), Key::Related(id.clone())],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Cached {
    Playlists(Vec<Playlist>),
    Tracks(Vec<Track>),
    Artists(Vec<Artist>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    Fresh(Cached),
    /// Kept, but too old to use.
    Stale,
    Miss,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stamped<T> {
    saved_at: i64,
    value: T,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct HomeFile {
    version: u32,
    account_id: String,
    discover: BTreeMap<String, Stamped<Vec<Playlist>>>,
    recommendations: Option<Stamped<Vec<Track>>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ArtistFile {
    version: u32,
    account_id: String,
    top_tracks: Option<Stamped<Vec<Track>>>,
    related: Option<Stamped<Vec<Artist>>>,
}

fn stamped_lookup<T: Clone>(
    stamped: Option<&Stamped<T>>,
    now: i64,
    ttl: i64,
    wrap: fn(T) -> Cached,
) -> Lookup {
    match stamped {
        Some(stamped) if fresh(stamped.saved_at, now, ttl) => {
            Lookup::Fresh(wrap(stamped.value.clone()))
        }
        Some(_) => Lookup::Stale,
        None => Lookup::Miss,
    }
}

impl HomeFile {
    fn lookup(&self, key: &Key, now: i64) -> Lookup {
        match key {
            Key::Discover(term) => {
                stamped_lookup(self.discover.get(term), now, key.ttl(), Cached::Playlists)
            }
            Key::Recommendations => stamped_lookup(
                self.recommendations.as_ref(),
                now,
                key.ttl(),
                Cached::Tracks,
            ),
            Key::TopTracks(_) | Key::Related(_) => Lookup::Miss,
        }
    }

    fn store(&mut self, key: &Key, value: Cached, now: i64) {
        match (key, value) {
            (Key::Discover(term), Cached::Playlists(value)) => {
                self.discover.insert(
                    term.clone(),
                    Stamped {
                        saved_at: now,
                        value,
                    },
                );
            }
            (Key::Recommendations, Cached::Tracks(value)) => {
                self.recommendations = Some(Stamped {
                    saved_at: now,
                    value,
                });
            }
            _ => {}
        }
    }
}

impl ArtistFile {
    fn lookup(&self, key: &Key, now: i64) -> Lookup {
        match key {
            Key::TopTracks(_) => {
                stamped_lookup(self.top_tracks.as_ref(), now, key.ttl(), Cached::Tracks)
            }
            Key::Related(_) => {
                stamped_lookup(self.related.as_ref(), now, key.ttl(), Cached::Artists)
            }
            Key::Discover(_) | Key::Recommendations => Lookup::Miss,
        }
    }

    fn store(&mut self, key: &Key, value: Cached, now: i64) {
        match (key, value) {
            (Key::TopTracks(_), Cached::Tracks(value)) => {
                self.top_tracks = Some(Stamped {
                    saved_at: now,
                    value,
                });
            }
            (Key::Related(_), Cached::Artists(value)) => {
                self.related = Some(Stamped {
                    saved_at: now,
                    value,
                });
            }
            _ => {}
        }
    }
}

/// Spotify IDs are base62; anything else is not used as a file name.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[derive(Default)]
struct Memory {
    account: String,
    /// `None` until the account's file has been read.
    home: Option<HomeFile>,
    artists: HashMap<String, ArtistFile>,
    /// Least recently used first.
    artist_order: VecDeque<String>,
    bypass: HashSet<Key>,
}

impl Memory {
    fn switch_to(&mut self, account: &str) {
        if self.account != account {
            let bypass = std::mem::take(&mut self.bypass);
            *self = Self {
                account: account.to_string(),
                bypass,
                ..Self::default()
            };
        }
    }

    fn loaded(&self, account: &str, key: &Key) -> bool {
        self.account == account
            && match key.artist() {
                Some(id) => self.artists.contains_key(id),
                None => self.home.is_some(),
            }
    }

    fn touch_artist(&mut self, id: &str) {
        self.artist_order.retain(|known| known != id);
        self.artist_order.push_back(id.to_string());
        while self.artist_order.len() > ARTISTS_IN_MEMORY {
            if let Some(oldest) = self.artist_order.pop_front() {
                self.artists.remove(&oldest);
            }
        }
    }
}

/// Answers kept in memory and on disk for the signed-in account.
pub struct ReadThrough {
    dirs: AppDirs,
    memory: Mutex<Memory>,
    /// Orders file writes, so a later state is never overwritten by an
    /// earlier one.
    writes: tokio::sync::Mutex<()>,
}

impl ReadThrough {
    pub fn new(dirs: AppDirs) -> Self {
        Self {
            dirs,
            memory: Mutex::new(Memory::default()),
            writes: tokio::sync::Mutex::new(()),
        }
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.memory.lock().unwrap_or_else(|lock| lock.into_inner())
    }

    /// Forget everything in memory, for a sign-out.
    pub fn clear(&self) {
        *self.memory() = Memory::default();
    }

    /// The next request for each of these asks Spotify.
    pub fn bypass(&self, scope: &Scope) {
        self.memory().bypass.extend(scope.keys());
    }

    fn home_file(&self, account: &str) -> PathBuf {
        self.dirs.web_api_cache_dir(account).join("home.json")
    }

    fn artist_file(&self, account: &str, id: &str) -> PathBuf {
        self.dirs
            .web_api_cache_dir(account)
            .join("artists")
            .join(format!("{id}.json"))
    }

    /// Reads the file holding `key` into memory, once per account.
    async fn load(&self, account: &str, key: &Key) {
        if self.memory().loaded(account, key) {
            return;
        }
        match key.artist() {
            Some(id) => {
                let file = if safe_id(id) {
                    read_json::<ArtistFile>(&self.artist_file(account, id))
                        .await
                        .filter(|file| file.version == VERSION && file.account_id == account)
                } else {
                    None
                };
                let mut memory = self.memory();
                memory.switch_to(account);
                if !memory.artists.contains_key(id) {
                    memory.artists.insert(
                        id.to_string(),
                        file.unwrap_or_else(|| ArtistFile {
                            version: VERSION,
                            account_id: account.to_string(),
                            ..ArtistFile::default()
                        }),
                    );
                }
                memory.touch_artist(id);
            }
            None => {
                let file = read_json::<HomeFile>(&self.home_file(account))
                    .await
                    .filter(|file| file.version == VERSION && file.account_id == account);
                let mut memory = self.memory();
                memory.switch_to(account);
                if memory.home.is_none() {
                    memory.home = Some(file.unwrap_or_else(|| HomeFile {
                        version: VERSION,
                        account_id: account.to_string(),
                        ..HomeFile::default()
                    }));
                }
            }
        }
    }

    pub async fn lookup(&self, account: &str, key: &Key, now: i64) -> Lookup {
        if self.memory().bypass.remove(key) {
            return Lookup::Miss;
        }
        self.load(account, key).await;
        let memory = self.memory();
        if memory.account != account {
            return Lookup::Miss;
        }
        match key.artist() {
            Some(id) => memory
                .artists
                .get(id)
                .map_or(Lookup::Miss, |file| file.lookup(key, now)),
            None => memory
                .home
                .as_ref()
                .map_or(Lookup::Miss, |file| file.lookup(key, now)),
        }
    }

    pub async fn store(&self, account: &str, key: &Key, value: Cached, now: i64) {
        if key.artist().is_some_and(|id| !safe_id(id)) {
            return;
        }
        let _write = self.writes.lock().await;
        self.load(account, key).await;
        let written = {
            let mut memory = self.memory();
            if memory.account != account {
                return;
            }
            match key.artist() {
                Some(id) => memory.artists.get_mut(id).map(|file| {
                    file.store(key, value, now);
                    serde_json::to_vec(file)
                }),
                None => memory.home.as_mut().map(|file| {
                    file.store(key, value, now);
                    serde_json::to_vec(file)
                }),
            }
        };
        let Some(Ok(bytes)) = written else {
            return;
        };
        let path = match key.artist() {
            Some(id) => self.artist_file(account, id),
            None => self.home_file(account),
        };
        if let Err(error) = write_bytes(&path, bytes).await {
            log::warn!("unable to store {}: {error}", path.display());
        }
        if key.artist().is_some()
            && let Some(directory) = path.parent()
        {
            prune_oldest(directory, ARTIST_FILES_KEPT).await;
        }
    }
}

async fn write_bytes(path: &Path, bytes: Vec<u8>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let temporary = path.with_extension("json.tmp");
    tokio::fs::write(&temporary, bytes).await?;
    let result = crate::util::replace_file(&temporary, path);
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

/// Removes the least recently written files beyond `keep`.
async fn prune_oldest(directory: &Path, keep: usize) {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return;
    };
    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(metadata) = entry.metadata().await
            && metadata.is_file()
        {
            let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
            files.push((modified, entry.path()));
        }
    }
    for path in files_to_prune(files, keep) {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn files_to_prune<T: Ord>(mut files: Vec<(T, PathBuf)>, keep: usize) -> Vec<PathBuf> {
    if files.len() <= keep {
        return Vec::new();
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let excess = files.len() - keep;
    files
        .into_iter()
        .take(excess)
        .map(|(_, path)| path)
        .collect()
}

// ---- playlist searches -------------------------------------------------------

/// The latest answers to a few queries, for one account, least recently
/// used first.
pub struct QueryCache<T> {
    capacity: usize,
    ttl: Duration,
    entries: VecDeque<(String, String, Instant, T)>,
}

impl<T: Clone> QueryCache<T> {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            capacity,
            ttl,
            entries: VecDeque::new(),
        }
    }

    pub fn get(&mut self, account: &str, query: &str, now: Instant) -> Option<T> {
        let ttl = self.ttl;
        self.entries
            .retain(|(_, _, at, _)| now.saturating_duration_since(*at) < ttl);
        let index = self
            .entries
            .iter()
            .position(|(owner, key, _, _)| owner == account && key == query)?;
        let entry = self.entries.remove(index)?;
        let value = entry.3.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    pub fn insert(&mut self, account: &str, query: &str, value: T, now: Instant) {
        self.entries
            .retain(|(owner, key, _, _)| !(owner == account && key == query));
        self.entries
            .push_back((account.to_string(), query.to_string(), now, value));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(id: &str) -> Playlist {
        Playlist {
            id: id.into(),
            uri: format!("spotify:playlist:{id}"),
            name: id.into(),
            ..Playlist::default()
        }
    }

    fn ids(list: &[Playlist]) -> Vec<&str> {
        list.iter().map(|playlist| playlist.id.as_str()).collect()
    }

    fn dirs(name: &str) -> AppDirs {
        let root = crate::test_support::temp_dir()
            .join(format!("lwspot-web-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        AppDirs {
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
        }
    }

    #[test]
    fn freshness_is_younger_than_the_ttl_and_never_from_the_future() {
        assert!(fresh(1_000, 1_000, 60));
        assert!(fresh(1_000, 1_059, 60));
        assert!(!fresh(1_000, 1_060, 60));
        assert!(!fresh(1_000, 999, 60), "a clock moved back is not fresh");
    }

    #[test]
    fn known_changes_update_the_list_in_place() {
        let mut list = vec![playlist("a"), playlist("b")];
        apply_change(&mut list, ListChange::Followed(playlist("new")));
        assert_eq!(ids(&list), ["new", "a", "b"]);
        // Following something already listed adds nothing.
        apply_change(&mut list, ListChange::Followed(playlist("b")));
        assert_eq!(ids(&list), ["new", "a", "b"]);
        apply_change(&mut list, ListChange::Unfollowed("a".into()));
        assert_eq!(ids(&list), ["new", "b"]);
        apply_change(
            &mut list,
            ListChange::Details {
                id: "b".into(),
                name: Some("Renamed".into()),
                description: None,
                public: Some(false),
            },
        );
        assert_eq!(list[1].name, "Renamed");
        assert_eq!(list[1].public, Some(false));
        assert_eq!(list[1].description, None, "what was not edited stays");
        assert_eq!(list[0].name, "new");
    }

    #[test]
    fn a_refresh_reports_the_playlists_it_no_longer_lists() {
        let old = [playlist("a"), playlist("b"), playlist("c")];
        let new = [playlist("c"), playlist("a")];
        assert_eq!(dropped_uris(&old, &new), ["spotify:playlist:b"]);
    }

    #[test]
    fn saved_state_comes_from_a_complete_list() {
        let items = vec![playlist("a")];
        let loaded = Loadable::Loaded(items);
        let complete = list_view(&loaded, None, None, false);
        assert_eq!(
            playlist_saved(complete, "spotify:playlist:a"),
            SavedAnswer::Known(true)
        );
        assert_eq!(
            playlist_saved(complete, "spotify:playlist:z"),
            SavedAnswer::Known(false)
        );
        // More pages to come, or a page on its way: wait for the list.
        for view in [
            list_view(&loaded, Some(50), None, false),
            list_view(&loaded, None, Some(50), false),
            list_view(&Loadable::Loading, None, None, false),
        ] {
            assert_eq!(
                playlist_saved(view, "spotify:playlist:z"),
                SavedAnswer::Wait
            );
        }
        // A background refresh's page leaves the shown list complete.
        assert_eq!(
            playlist_saved(
                list_view(&loaded, None, Some(50), true),
                "spotify:playlist:z"
            ),
            SavedAnswer::Known(false)
        );
        for unavailable in [Loadable::NotLoaded, Loadable::Failed("no".into())] {
            assert_eq!(
                playlist_saved(
                    list_view(&unavailable, None, None, false),
                    "spotify:playlist:a"
                ),
                SavedAnswer::Ask
            );
        }
    }

    #[test]
    fn a_followed_made_for_you_playlist_answers_for_its_search() {
        let same = |name: &str, term: &str| name.eq_ignore_ascii_case(term);
        let mut weekly = playlist("dw");
        weekly.name = "Discover Weekly".into();
        weekly.owner.id = Some("spotify".into());
        let mut copy = playlist("copy");
        copy.name = "Release Radar".into();
        copy.owner.id = Some("someone".into());
        copy.owner.display_name = Some("Someone".into());
        let mut mix = playlist("mix");
        mix.name = "Daily Mix".into();
        mix.owner.id = Some("spotify".into());
        let list = [copy, weekly, mix];
        assert_eq!(
            followed_made_for_you(&list, "Discover Weekly", same).map(|p| p.id),
            Some("dw".into())
        );
        assert_eq!(
            followed_made_for_you(&list, "Release Radar", same),
            None,
            "only Spotify's own"
        );
        assert_eq!(
            followed_made_for_you(&list, "Daily Mix", same),
            None,
            "one followed mix does not stand for all of them"
        );
    }

    #[test]
    fn manual_refresh_scopes_name_their_keys() {
        let home = Scope::Home.keys();
        assert!(home.contains(&Key::Recommendations));
        assert!(home.contains(&Key::Discover("Discover Weekly".into())));
        assert_eq!(
            Scope::Artist("x".into()).keys(),
            [Key::TopTracks("x".into()), Key::Related("x".into())]
        );
    }

    #[test]
    fn only_spotify_ids_become_file_names() {
        assert!(safe_id("4Z8W4fKeB5YxbusRsdQVPb"));
        assert!(!safe_id("../x"));
        assert!(!safe_id(""));
        assert!(!safe_id("a.b"));
    }

    #[test]
    fn pruning_removes_the_oldest_beyond_the_limit() {
        let files = vec![
            (3, PathBuf::from("c")),
            (1, PathBuf::from("a")),
            (2, PathBuf::from("b")),
        ];
        assert_eq!(files_to_prune(files.clone(), 2), [PathBuf::from("a")]);
        assert!(files_to_prune(files, 3).is_empty());
    }

    #[test]
    fn playlist_searches_are_kept_per_account_for_a_while() {
        let start = Instant::now();
        let mut cache = QueryCache::new(2, Duration::from_secs(600));
        cache.insert("me", "jazz", 1, start);
        cache.insert("me", "rock", 2, start);
        assert_eq!(cache.get("me", "jazz", start), Some(1));
        assert_eq!(cache.get("other", "jazz", start), None);
        // "jazz" was used last, so "rock" makes room for "pop".
        cache.insert("me", "pop", 3, start);
        assert_eq!(cache.get("me", "rock", start), None);
        assert_eq!(cache.get("me", "jazz", start), Some(1));
        assert_eq!(
            cache.get("me", "pop", start + Duration::from_secs(600)),
            None
        );
        cache.insert("me", "pop", 4, start);
        cache.clear();
        assert_eq!(cache.get("me", "pop", start), None);
    }

    #[tokio::test]
    async fn answers_are_kept_per_account_and_age_out() {
        let dirs = dirs("read-through");
        let cache = ReadThrough::new(dirs.clone());
        let key = Key::Discover("Discover Weekly".into());
        assert_eq!(cache.lookup("me", &key, 100).await, Lookup::Miss);
        let value = Cached::Playlists(vec![playlist("dw")]);
        cache.store("me", &key, value.clone(), 100).await;
        assert_eq!(
            cache.lookup("me", &key, 100 + DISCOVER_TTL - 1).await,
            Lookup::Fresh(value.clone())
        );
        assert_eq!(
            cache.lookup("me", &key, 100 + DISCOVER_TTL).await,
            Lookup::Stale
        );
        assert_eq!(cache.lookup("other", &key, 100).await, Lookup::Miss);

        // It is on disk: a new run reads it back.
        let restarted = ReadThrough::new(dirs.clone());
        assert_eq!(
            restarted.lookup("me", &key, 101).await,
            Lookup::Fresh(value.clone())
        );
        // A manual refresh asks Spotify once.
        restarted.bypass(&Scope::Home);
        assert_eq!(restarted.lookup("me", &key, 101).await, Lookup::Miss);
        assert_eq!(
            restarted.lookup("me", &key, 101).await,
            Lookup::Fresh(value)
        );

        let top = Key::TopTracks("artist1".into());
        let related = Key::Related("artist1".into());
        let tracks = Cached::Tracks(vec![Track::default()]);
        let artists = Cached::Artists(vec![Artist::default()]);
        restarted.store("me", &top, tracks.clone(), 200).await;
        restarted.store("me", &related, artists.clone(), 200).await;
        let again = ReadThrough::new(dirs.clone());
        assert_eq!(again.lookup("me", &top, 201).await, Lookup::Fresh(tracks));
        assert_eq!(
            again.lookup("me", &related, 201).await,
            Lookup::Fresh(artists)
        );
        let _ = std::fs::remove_dir_all(dirs.cache.parent().unwrap());
    }

    #[tokio::test]
    async fn the_playlist_list_is_saved_per_account() {
        let dirs = dirs("playlist-list");
        let list = PlaylistList::new("me".into(), 42, vec![playlist("a")]);
        write_playlist_list(&dirs, &list).await.unwrap();
        let read = read_playlist_list(&dirs, "me").await.unwrap();
        assert_eq!(read.read_at, 42);
        assert_eq!(ids(&read.items), ["a"]);
        assert!(read_playlist_list(&dirs, "other").await.is_none());
        let _ = std::fs::remove_dir_all(dirs.cache.parent().unwrap());
    }
}
