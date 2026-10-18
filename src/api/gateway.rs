use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use super::ApiSource;
use super::client::{ApiClient, ApiError, NetActivity, TokenProvider};
use super::models::Playlist;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountId(String);

impl AccountId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlaylistId(String);

impl PlaylistId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for PlaylistId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    Unavailable,
    Authorizing,
    Ready { account: AccountId },
}

impl SessionState {
    pub fn account(&self) -> Option<&AccountId> {
        match self {
            Self::Ready { account } => Some(account),
            Self::Unavailable | Self::Authorizing => None,
        }
    }
}

#[derive(Clone, Copy)]
struct ApiProfile {
    source: ApiSource,
    search_limit: u32,
    artist_albums_limit: u32,
}

impl ApiProfile {
    pub const SHARED: Self = Self {
        source: ApiSource::Shared,
        search_limit: 20,
        artist_albums_limit: 50,
    };
    pub const PERSONAL: Self = Self {
        source: ApiSource::Personal,
        search_limit: 10,
        artist_albums_limit: 10,
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaylistAccess {
    Owned,
    Collaborative,
    External,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    CanonicalAccount,
    Playback,
    UserData,
    PlaylistLibrary,
    PlaylistCreation,
    PlaylistSearch,
    CatalogSearch,
    Catalog,
    PlaylistMetadata(PlaylistAccess),
    PlaylistItems(PlaylistAccess),
    PlaylistMutation(PlaylistAccess),
    UnsupportedDevelopmentMode,
}

/// The streaming session reads every playlist the shared app would have
/// been asked for: other people's, which no personal app may read, and the
/// account's own when it has no personal app. A personal app keeps its own
/// playlists, which it reads quickly and with every field.
fn session_serves(operation: Operation, personal_ready: bool) -> bool {
    matches!(
        operation,
        Operation::PlaylistMetadata(_) | Operation::PlaylistItems(_)
    ) && plan(operation, personal_ready) == ApiSource::Shared
}

/// A playlist with unknown access dispatches to the shared app, which can
/// serve anything; later requests move once a response reveals the owner.
fn plan(operation: Operation, personal_ready: bool) -> ApiSource {
    use Operation::*;
    match operation {
        CanonicalAccount | PlaylistLibrary | PlaylistSearch | UnsupportedDevelopmentMode => {
            ApiSource::Shared
        }
        PlaylistMetadata(PlaylistAccess::External | PlaylistAccess::Unknown)
        | PlaylistItems(PlaylistAccess::External | PlaylistAccess::Unknown)
        | PlaylistMutation(PlaylistAccess::External | PlaylistAccess::Unknown) => ApiSource::Shared,
        PlaylistMetadata(_) | PlaylistItems(_) | PlaylistMutation(_) if personal_ready => {
            ApiSource::Personal
        }
        Playback | UserData | PlaylistCreation | Catalog | CatalogSearch if personal_ready => {
            ApiSource::Personal
        }
        Playback | UserData | PlaylistCreation | Catalog | CatalogSearch | PlaylistMetadata(_)
        | PlaylistItems(_) | PlaylistMutation(_) => ApiSource::Shared,
    }
}

/// How long a request the personal app would serve waits for that app's
/// grant to finish verifying before it settles for the shared app. Both
/// grants verify at startup; whichever answers first signs the app in, and
/// without this wait the first burst of requests went to the shared app
/// whenever its answer came first.
pub const PERSONAL_GRACE: Duration = Duration::from_secs(10);

/// How long a playlist request waits for the account's saved playlist
/// classification to be read from disk.
const ACCESS_SEED_WAIT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Go(ApiSource),
    /// The personal app would serve this and is still verifying: wait for
    /// it until this instant, then plan again.
    AwaitPersonal(Instant),
}

/// Where an operation goes given the personal app's state right now.
/// `grace` is when waiting for a personal grant being verified stops.
fn route(
    operation: Operation,
    personal: &SessionState,
    grace: Option<Instant>,
    now: Instant,
) -> Route {
    if matches!(personal, SessionState::Ready { .. }) {
        return Route::Go(plan(operation, true));
    }
    if matches!(personal, SessionState::Authorizing)
        && plan(operation, true) == ApiSource::Personal
        && let Some(until) = grace.filter(|until| *until > now)
    {
        return Route::AwaitPersonal(until);
    }
    Route::Go(plan(operation, false))
}

fn classify_playlist(account: &AccountId, playlist: &Playlist) -> PlaylistAccess {
    if playlist.owner.id.as_deref() == Some(account.as_str()) {
        PlaylistAccess::Owned
    } else if playlist.collaborative {
        PlaylistAccess::Collaborative
    } else if playlist.owner.id.is_some() {
        PlaylistAccess::External
    } else {
        PlaylistAccess::Unknown
    }
}

struct Session {
    // The counter remembers a sign-out even if a new sign-in becomes ready
    // before a waiting request wakes up.
    state: tokio::sync::watch::Sender<(u64, SessionState)>,
    client: RwLock<Arc<ApiClient>>,
}

impl Session {
    fn new(http: crate::http::Http, activity: Arc<NetActivity>, profile: ApiProfile) -> Self {
        Self {
            state: tokio::sync::watch::channel((0, SessionState::Unavailable)).0,
            client: RwLock::new(Arc::new(ApiClient::new(
                http,
                activity,
                profile.search_limit,
                profile.artist_albums_limit,
                profile.source,
            ))),
        }
    }

    fn state(&self) -> SessionState {
        self.state.borrow().1.clone()
    }

    fn set_state(&self, state: SessionState) {
        self.state.send_modify(|current| current.1 = state);
    }

    fn client(&self) -> Arc<ApiClient> {
        Arc::clone(&self.client.read().unwrap_or_else(|lock| lock.into_inner()))
    }
}

pub struct ApiGateway {
    shared: Session,
    personal: Session,
    playlist_access: Mutex<HashMap<PlaylistId, PlaylistAccess>>,
    /// Until when requests the personal app would serve wait for its grant
    /// to verify. Set when a personal grant is restored or verified.
    personal_grace: Mutex<Option<Instant>>,
    /// True while the account's saved playlist classification is read.
    access_seeding: tokio::sync::watch::Sender<bool>,
}

impl ApiGateway {
    pub fn new(http: impl Into<crate::http::Http>, activity: Arc<NetActivity>) -> Self {
        let http = http.into();
        Self {
            shared: Session::new(http.clone(), activity.clone(), ApiProfile::SHARED),
            personal: Session::new(http, activity, ApiProfile::PERSONAL),
            playlist_access: Mutex::new(HashMap::new()),
            personal_grace: Mutex::new(None),
            access_seeding: tokio::sync::watch::channel(false).0,
        }
    }

    fn session(&self, source: ApiSource) -> &Session {
        match source {
            ApiSource::Shared => &self.shared,
            ApiSource::Personal => &self.personal,
        }
    }

    pub fn state(&self, source: ApiSource) -> SessionState {
        self.session(source).state()
    }

    pub fn set_state(&self, source: ApiSource, state: SessionState) {
        self.session(source).set_state(state);
    }

    /// Marks a verified session ready, keeping the token provider that
    /// `begin_verification` installed.
    pub fn install(&self, source: ApiSource, account: AccountId) -> Result<(), ApiError> {
        let other = match source {
            ApiSource::Shared => ApiSource::Personal,
            ApiSource::Personal => ApiSource::Shared,
        };
        if self
            .state(other)
            .account()
            .is_some_and(|active| active != &account)
        {
            return Err(ApiError::Status {
                status: 403,
                message: "The Spotify grants belong to different accounts".into(),
            });
        }
        self.session(source)
            .set_state(SessionState::Ready { account });
        Ok(())
    }

    pub fn begin_verification(&self, source: ApiSource, provider: TokenProvider) {
        if source == ApiSource::Personal {
            self.expect_personal(PERSONAL_GRACE);
        }
        let session = self.session(source);
        let client = session.client().for_authorization(provider);
        *session
            .client
            .write()
            .unwrap_or_else(|lock| lock.into_inner()) = Arc::new(client);
        session.set_state(SessionState::Authorizing);
    }

    pub fn verification_client(&self, source: ApiSource) -> Arc<ApiClient> {
        self.session(source).client()
    }

    pub fn clear(&self, source: ApiSource) {
        let session = self.session(source);
        session.client().set_token_provider(None);
        session.state.send_modify(|current| {
            current.0 += 1;
            current.1 = SessionState::Unavailable;
        });
    }

    pub fn clear_all(&self) {
        self.clear(ApiSource::Shared);
        self.clear(ApiSource::Personal);
        self.playlist_access
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .clear();
        self.access_seeding.send_replace(false);
    }

    /// Requests the personal app would serve wait up to `within` for a
    /// personal grant that is being restored or verified.
    pub fn expect_personal(&self, within: Duration) {
        *self
            .personal_grace
            .lock()
            .unwrap_or_else(|lock| lock.into_inner()) = Some(Instant::now() + within);
    }

    fn personal_grace(&self) -> Option<Instant> {
        *self
            .personal_grace
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
    }

    /// The app the operation goes to. While a personal grant is still
    /// verifying, an operation it would serve waits for it (bounded by
    /// [`PERSONAL_GRACE`]) instead of committing to the shared app.
    async fn source_for(&self, operation: Operation) -> ApiSource {
        let mut personal = self.personal.state.subscribe();
        loop {
            let state = personal.borrow_and_update().1.clone();
            match route(operation, &state, self.personal_grace(), Instant::now()) {
                Route::Go(source) => return source,
                Route::AwaitPersonal(until) => {
                    tokio::select! {
                        changed = personal.changed() => {
                            if changed.is_err() {
                                return plan(operation, false);
                            }
                        }
                        () = tokio::time::sleep_until(tokio::time::Instant::from_std(until)) => {}
                    }
                }
            }
        }
    }

    pub fn account(&self) -> Option<AccountId> {
        self.state(ApiSource::Shared)
            .account()
            .cloned()
            .or_else(|| self.state(ApiSource::Personal).account().cloned())
    }

    pub fn personal_ready(&self) -> bool {
        matches!(self.state(ApiSource::Personal), SessionState::Ready { .. })
    }

    pub async fn client_for(&self, operation: Operation) -> Result<Arc<ApiClient>, ApiError> {
        let source = self.source_for(operation).await;
        let session = self.session(source);
        let mut state = session.state.subscribe();
        let generation = state.borrow().0;
        loop {
            let (current, status) = state.borrow_and_update().clone();
            if current != generation {
                return Err(ApiError::NotSignedIn);
            }
            match status {
                SessionState::Ready { .. } => break,
                SessionState::Unavailable => return Err(ApiError::NotSignedIn),
                // A personal grant can open the app before the shared grant
                // finishes verification. Keep shared-only views loading.
                SessionState::Authorizing => {
                    state.changed().await.map_err(|_| ApiError::NotSignedIn)?;
                }
            }
        }
        log::debug!("Spotify route operation={operation:?} source={source}");
        Ok(session.client())
    }

    /// Whether the streaming session, when up, should answer an operation.
    pub fn session_serves(&self, operation: Operation) -> bool {
        session_serves(operation, self.personal_ready())
    }

    pub fn playlist_access(&self, id: &str) -> PlaylistAccess {
        self.playlist_access
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .get(id)
            .copied()
            .unwrap_or_default()
    }

    pub fn observe_playlist(&self, playlist: &Playlist) {
        let Some(account) = self.account() else {
            return;
        };
        let access = classify_playlist(&account, playlist);
        self.playlist_access
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .insert(PlaylistId::new(playlist.id.clone()), access);
    }

    pub fn observe_playlists<'a>(&self, playlists: impl IntoIterator<Item = &'a Playlist>) {
        for playlist in playlists {
            self.observe_playlist(playlist);
        }
    }

    /// The saved classification of the account's playlists is being read;
    /// playlist requests of unknown access wait briefly for it.
    pub fn begin_access_seed(&self) {
        self.access_seeding.send_replace(true);
    }

    /// Classifies the playlists read from the account's saved list, unless
    /// another account has signed in meanwhile, and releases waiting requests.
    pub fn finish_access_seed(&self, account: &str, playlists: &[Playlist]) {
        if self
            .account()
            .is_some_and(|current| current.as_str() == account)
        {
            let account = AccountId::new(account);
            let mut known = self
                .playlist_access
                .lock()
                .unwrap_or_else(|lock| lock.into_inner());
            for playlist in playlists {
                // A live answer that already arrived is newer than the disk.
                known
                    .entry(PlaylistId::new(playlist.id.clone()))
                    .and_modify(|access| {
                        if *access == PlaylistAccess::Unknown {
                            *access = classify_playlist(&account, playlist);
                        }
                    })
                    .or_insert_with(|| classify_playlist(&account, playlist));
            }
        }
        self.access_seeding.send_replace(false);
    }

    /// Waits, briefly, while the saved playlist classification is read.
    pub async fn wait_for_access_seed(&self) {
        let mut seeding = self.access_seeding.subscribe();
        if !*seeding.borrow_and_update() {
            return;
        }
        let _ = tokio::time::timeout(ACCESS_SEED_WAIT, seeding.wait_for(|seeding| !*seeding)).await;
    }

    pub fn invalidate_playlist_access(&self, id: &PlaylistId) {
        self.playlist_access
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .insert(id.clone(), PlaylistAccess::Unknown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;

    #[tokio::test]
    async fn shared_requests_wait_while_a_personal_session_is_already_ready() {
        let gateway = ApiGateway::new(reqwest::Client::new(), Arc::new(NetActivity::default()));
        gateway.set_state(ApiSource::Shared, SessionState::Authorizing);
        gateway.begin_verification(
            ApiSource::Personal,
            provider("ready-personal", ApiSource::Personal),
        );
        gateway
            .install(ApiSource::Personal, AccountId::new("same"))
            .unwrap();
        let mut waiting = Box::pin(gateway.client_for(Operation::PlaylistLibrary));
        std::future::poll_fn(|cx| {
            assert!(waiting.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        gateway.begin_verification(
            ApiSource::Shared,
            provider("slow-shared", ApiSource::Shared),
        );
        gateway
            .install(ApiSource::Shared, AccountId::new("same"))
            .unwrap();
        let client = waiting.await.unwrap();
        assert!(Arc::ptr_eq(
            &client,
            &gateway.verification_client(ApiSource::Shared)
        ));
    }

    #[tokio::test]
    async fn sign_out_cancels_waiting_requests_even_if_sign_in_finishes_before_they_wake() {
        let gateway = ApiGateway::new(reqwest::Client::new(), Arc::new(NetActivity::default()));
        gateway.begin_verification(ApiSource::Shared, provider("old-shared", ApiSource::Shared));
        let old_client = gateway.verification_client(ApiSource::Shared);
        let mut waiting = Box::pin(gateway.client_for(Operation::PlaylistLibrary));
        std::future::poll_fn(|cx| {
            assert!(waiting.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        gateway.clear_all();
        gateway.begin_verification(ApiSource::Shared, provider("new-shared", ApiSource::Shared));
        gateway
            .install(ApiSource::Shared, AccountId::new("new-account"))
            .unwrap();
        assert!(matches!(waiting.await, Err(ApiError::NotSignedIn)));
        assert!(matches!(old_client.me().await, Err(ApiError::NotSignedIn)));
        let new_client = gateway
            .client_for(Operation::PlaylistLibrary)
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&old_client, &new_client));
    }

    fn provider(name: &str, source: ApiSource) -> TokenProvider {
        let path = crate::test_support::temp_dir().join(format!("lwspot-{name}-unused-token"));
        let store = crate::credentials::Store::in_memory(crate::paths::AppDirs {
            config: path.join("config"),
            state: path.join("state"),
            cache: path.join("cache"),
        });
        TokenProvider::Web(super::super::client::WebTokens::new(
            reqwest::Client::new(),
            crate::auth::StoredToken::default(),
            store.lease(if source == ApiSource::Shared {
                crate::credentials::Slot::Shared
            } else {
                crate::credentials::Slot::Personal
            }),
            source,
            std::sync::Arc::new(|_| {}),
        ))
    }

    #[test]
    fn the_session_reads_what_the_shared_app_would_have() {
        for (operation, personal) in [
            (Operation::PlaylistItems(PlaylistAccess::External), true),
            (Operation::PlaylistMetadata(PlaylistAccess::Unknown), false),
            (Operation::PlaylistMetadata(PlaylistAccess::Owned), false),
            (
                Operation::PlaylistItems(PlaylistAccess::Collaborative),
                false,
            ),
        ] {
            assert!(session_serves(operation, personal), "{operation:?}");
        }
        for (operation, personal) in [
            (Operation::PlaylistItems(PlaylistAccess::Owned), true),
            (
                Operation::PlaylistItems(PlaylistAccess::Collaborative),
                true,
            ),
            (Operation::PlaylistMutation(PlaylistAccess::External), true),
            (Operation::Catalog, false),
            (Operation::PlaylistLibrary, false),
        ] {
            assert!(!session_serves(operation, personal), "{operation:?}");
        }
    }

    #[test]
    fn routing_matrix_selects_source() {
        let personal = true;
        for operation in [
            Operation::Playback,
            Operation::UserData,
            Operation::PlaylistCreation,
            Operation::Catalog,
            Operation::CatalogSearch,
            Operation::PlaylistMetadata(PlaylistAccess::Owned),
            Operation::PlaylistMetadata(PlaylistAccess::Collaborative),
            Operation::PlaylistItems(PlaylistAccess::Owned),
            Operation::PlaylistItems(PlaylistAccess::Collaborative),
        ] {
            assert_eq!(plan(operation, personal), ApiSource::Personal);
        }
        for operation in [
            Operation::CanonicalAccount,
            Operation::PlaylistLibrary,
            Operation::PlaylistSearch,
            Operation::UnsupportedDevelopmentMode,
            Operation::PlaylistMetadata(PlaylistAccess::External),
            Operation::PlaylistMetadata(PlaylistAccess::Unknown),
            Operation::PlaylistItems(PlaylistAccess::External),
            Operation::PlaylistItems(PlaylistAccess::Unknown),
            Operation::PlaylistMutation(PlaylistAccess::External),
            Operation::PlaylistMutation(PlaylistAccess::Unknown),
        ] {
            assert_eq!(plan(operation, personal), ApiSource::Shared);
        }
        for operation in [
            Operation::Playback,
            Operation::UserData,
            Operation::PlaylistLibrary,
            Operation::PlaylistCreation,
            Operation::PlaylistSearch,
            Operation::Catalog,
            Operation::CatalogSearch,
            Operation::PlaylistMetadata(PlaylistAccess::Unknown),
            Operation::PlaylistItems(PlaylistAccess::Unknown),
        ] {
            assert_eq!(plan(operation, false), ApiSource::Shared);
        }
    }

    #[test]
    fn a_verifying_personal_grant_holds_what_it_would_serve() {
        let now = Instant::now();
        let until = now + Duration::from_secs(5);
        let verifying = SessionState::Authorizing;
        for operation in [
            Operation::Playback,
            Operation::UserData,
            Operation::Catalog,
            Operation::CatalogSearch,
            Operation::PlaylistItems(PlaylistAccess::Owned),
        ] {
            assert_eq!(
                route(operation, &verifying, Some(until), now),
                Route::AwaitPersonal(until),
                "{operation:?}"
            );
        }
        // What only the shared app serves never waits for the personal one.
        for operation in [
            Operation::PlaylistLibrary,
            Operation::PlaylistSearch,
            Operation::UnsupportedDevelopmentMode,
            Operation::PlaylistItems(PlaylistAccess::Unknown),
        ] {
            assert_eq!(
                route(operation, &verifying, Some(until), now),
                Route::Go(ApiSource::Shared),
                "{operation:?}"
            );
        }
    }

    #[test]
    fn the_wait_for_a_personal_grant_is_bounded() {
        let now = Instant::now();
        let verifying = SessionState::Authorizing;
        // Past the grace, or with none (a browser sign-in in progress), the
        // shared app answers as before.
        for grace in [None, Some(now), Some(now - Duration::from_secs(1))] {
            assert_eq!(
                route(Operation::Playback, &verifying, grace, now),
                Route::Go(ApiSource::Shared)
            );
        }
        // No personal app at all: nothing waits.
        assert_eq!(
            route(
                Operation::Playback,
                &SessionState::Unavailable,
                Some(now + PERSONAL_GRACE),
                now
            ),
            Route::Go(ApiSource::Shared)
        );
        let ready = SessionState::Ready {
            account: AccountId::new("me"),
        };
        assert_eq!(
            route(Operation::Playback, &ready, None, now),
            Route::Go(ApiSource::Personal)
        );
        assert_eq!(
            route(Operation::PlaylistLibrary, &ready, None, now),
            Route::Go(ApiSource::Shared)
        );
    }

    #[tokio::test]
    async fn a_request_waits_for_the_personal_grant_instead_of_the_shared_app() {
        let gateway = ApiGateway::new(reqwest::Client::new(), Arc::new(NetActivity::default()));
        gateway.begin_verification(
            ApiSource::Shared,
            provider("race-shared", ApiSource::Shared),
        );
        gateway.begin_verification(
            ApiSource::Personal,
            provider("race-personal", ApiSource::Personal),
        );
        // The shared grant verifies first.
        gateway
            .install(ApiSource::Shared, AccountId::new("same"))
            .unwrap();
        let mut waiting = Box::pin(gateway.client_for(Operation::Playback));
        std::future::poll_fn(|cx| {
            assert!(waiting.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        gateway
            .install(ApiSource::Personal, AccountId::new("same"))
            .unwrap();
        let client = waiting.await.unwrap();
        assert!(Arc::ptr_eq(
            &client,
            &gateway.verification_client(ApiSource::Personal)
        ));
    }

    #[tokio::test]
    async fn a_failed_personal_grant_releases_waiting_requests_to_the_shared_app() {
        let gateway = ApiGateway::new(reqwest::Client::new(), Arc::new(NetActivity::default()));
        gateway.begin_verification(
            ApiSource::Shared,
            provider("fail-shared", ApiSource::Shared),
        );
        gateway.begin_verification(
            ApiSource::Personal,
            provider("fail-personal", ApiSource::Personal),
        );
        gateway
            .install(ApiSource::Shared, AccountId::new("same"))
            .unwrap();
        let mut waiting = Box::pin(gateway.client_for(Operation::UserData));
        std::future::poll_fn(|cx| {
            assert!(waiting.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        gateway.clear(ApiSource::Personal);
        let client = waiting.await.unwrap();
        assert!(Arc::ptr_eq(
            &client,
            &gateway.verification_client(ApiSource::Shared)
        ));
    }

    #[tokio::test]
    async fn saved_classification_routes_own_playlists_from_the_start() {
        let gateway = ApiGateway::new(reqwest::Client::new(), Arc::new(NetActivity::default()));
        gateway.begin_verification(
            ApiSource::Shared,
            provider("seed-shared", ApiSource::Shared),
        );
        gateway
            .install(ApiSource::Shared, AccountId::new("me"))
            .unwrap();
        gateway.begin_access_seed();
        let mut mine = Playlist {
            id: "mine".into(),
            ..Playlist::default()
        };
        mine.owner.id = Some("me".into());
        let mut waiting = Box::pin(gateway.wait_for_access_seed());
        std::future::poll_fn(|cx| {
            assert!(waiting.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        gateway.finish_access_seed("me", &[mine]);
        waiting.await;
        assert_eq!(gateway.playlist_access("mine"), PlaylistAccess::Owned);
        // Another account's saved list classifies nothing.
        gateway.begin_access_seed();
        let mut theirs = Playlist {
            id: "theirs".into(),
            ..Playlist::default()
        };
        theirs.owner.id = Some("other".into());
        gateway.finish_access_seed("other", &[theirs]);
        assert_eq!(gateway.playlist_access("theirs"), PlaylistAccess::Unknown);
        gateway.wait_for_access_seed().await;
    }

    #[test]
    fn playlist_access_keeps_unknown_explicit() {
        let account = AccountId::new("me");
        let mut playlist = Playlist {
            id: "p".into(),
            ..Playlist::default()
        };
        assert_eq!(
            classify_playlist(&account, &playlist),
            PlaylistAccess::Unknown
        );
        playlist.owner.id = Some("other".into());
        assert_eq!(
            classify_playlist(&account, &playlist),
            PlaylistAccess::External
        );
        playlist.collaborative = true;
        assert_eq!(
            classify_playlist(&account, &playlist),
            PlaylistAccess::Collaborative
        );
        playlist.owner.id = Some("me".into());
        assert_eq!(
            classify_playlist(&account, &playlist),
            PlaylistAccess::Owned
        );
    }

    #[test]
    fn dual_sessions_require_the_same_account() {
        let gateway = ApiGateway::new(reqwest::Client::new(), Arc::new(NetActivity::default()));
        gateway.begin_verification(ApiSource::Shared, provider("shared", ApiSource::Shared));
        gateway
            .install(ApiSource::Shared, AccountId::new("same"))
            .unwrap();
        gateway.begin_verification(
            ApiSource::Personal,
            provider("personal", ApiSource::Personal),
        );
        gateway
            .install(ApiSource::Personal, AccountId::new("same"))
            .unwrap();
        assert!(gateway.personal_ready());
        gateway.clear(ApiSource::Personal);
        gateway.begin_verification(ApiSource::Personal, provider("other", ApiSource::Personal));
        assert!(
            gateway
                .install(ApiSource::Personal, AccountId::new("other"))
                .is_err()
        );
        assert!(matches!(
            gateway.state(ApiSource::Shared),
            SessionState::Ready { .. }
        ));

        gateway.begin_verification(
            ApiSource::Personal,
            provider("personal-again", ApiSource::Personal),
        );
        gateway
            .install(ApiSource::Personal, AccountId::new("same"))
            .unwrap();
        gateway.clear(ApiSource::Shared);
        gateway.begin_verification(
            ApiSource::Shared,
            provider("other-shared", ApiSource::Shared),
        );
        assert!(
            gateway
                .install(ApiSource::Shared, AccountId::new("other"))
                .is_err()
        );
        assert!(matches!(
            gateway.state(ApiSource::Personal),
            SessionState::Ready { .. }
        ));
    }
}
