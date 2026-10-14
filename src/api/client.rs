//! An authenticated, rate-limited transport for the Spotify Web API.
//!
//! Typed calls share one `request` helper that adds the bearer token, limits
//! concurrency, honors `Retry-After`, and formats API errors. The gateway
//! handles capability differences before dispatch.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::Semaphore;

use super::ApiSource;
use super::models::*;
use crate::http::Http;

const BASE_URL: &str = "https://api.spotify.com/v1";
const MAX_IN_FLIGHT: usize = 6;
/// Requests the app makes on its own (polls, Home shelves, saved-state
/// checks) in flight at once on one Web API app. Each app has its own, so
/// a shared app waiting out a rate limit never holds up the personal one.
const BACKGROUND_IN_FLIGHT: usize = 4;
const RATE_LIMIT_RETRIES: u32 = 3;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

tokio::task_local! {
    /// Set for requests nobody is waiting on: they yield to user actions
    /// after a rate limit instead of all retrying at once.
    static BACKGROUND: bool;
}

/// Runs `request` as background work: it takes one of its app's background
/// slots only while it is actually talking to Spotify, never while waiting
/// out a cooldown, and after a rate limit such requests resume one at a
/// time until one succeeds.
pub async fn in_background<F: std::future::Future>(request: F) -> F::Output {
    BACKGROUND.scope(true, request).await
}

fn is_background() -> bool {
    BACKGROUND
        .try_with(|background| *background)
        .unwrap_or(false)
}

/// Whether background requests must go one at a time: a rate limit was
/// seen and no request has succeeded since.
#[derive(Debug, Default)]
struct Recovery(std::sync::atomic::AtomicBool);

impl Recovery {
    fn rate_limited(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn succeeded(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    fn probing(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What a background request holds while it is sent.
struct BackgroundTicket<'a> {
    _permit: tokio::sync::SemaphorePermit<'a>,
    _probe: Option<tokio::sync::MutexGuard<'a, ()>>,
}

#[derive(Clone, Debug, Error)]
pub enum ApiError {
    #[error("not signed in")]
    NotSignedIn,
    #[error("{message}")]
    Status { status: u16, message: String },
    #[error("Spotify is rate limiting requests; try again in a moment")]
    RateLimited,
    #[error("Spotify's Development Mode quota is exhausted; try again after the quota resets")]
    QuotaExhausted,
    #[error("your Spotify sign-in expired; please sign in again")]
    SignInExpired { api_source: ApiSource },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response from Spotify: {0}")]
    Decode(String),
}

impl ApiError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_decode() {
            Self::Decode(error.to_string())
        } else {
            Self::Network(error.to_string())
        }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

fn is_quota_exhausted(body: &str) -> bool {
    serde_json::from_str::<ApiErrorBody>(body)
        .ok()
        .and_then(|body| body.error.reason)
        .is_some_and(|reason| reason == "QUOTA_EXCEEDED")
}

/// Where bearer tokens come from.
///
/// The Web API is driven by a registered application's PKCE grant, refreshed
/// on demand and persisted so the browser is needed once per machine. Tokens
/// minted for Spotify's own desktop client are throttled on the Web API, so
/// they are never used here. `Fixed` exists only for tests.
#[derive(Clone)]
pub enum TokenProvider {
    Web(std::sync::Arc<WebTokens>),
}

impl TokenProvider {
    async fn access_token(&self) -> Result<String> {
        match self {
            Self::Web(tokens) => tokens.access_token(None).await,
        }
    }

    /// `rejected` is the access token Spotify answered 401 to. Concurrent
    /// requests that saw the same token share one refresh.
    async fn invalidate(&self, rejected: &str) {
        let Self::Web(tokens) = self;
        let _ = tokens.access_token(Some(rejected)).await;
    }
}

/// The Web API grant, refreshed and persisted as it ages.
pub struct WebTokens {
    http: Http,
    token: tokio::sync::Mutex<crate::auth::StoredToken>,
    lease: crate::credentials::Lease,
    remember: std::sync::atomic::AtomicBool,
    storage_error: std::sync::Arc<dyn Fn(crate::credentials::Error) + Send + Sync>,
    source: ApiSource,
    /// No token request before this, after Spotify's token endpoint asked
    /// for a pause. Read and written only while `token` is locked.
    refresh_after: Mutex<Option<Instant>>,
    #[cfg(test)]
    token_url: Option<String>,
}

/// The longest pause honored from the token endpoint's `Retry-After`.
const MAX_REFRESH_PAUSE: Duration = Duration::from_secs(300);

impl WebTokens {
    pub fn new(
        http: impl Into<Http>,
        token: crate::auth::StoredToken,
        lease: crate::credentials::Lease,
        source: ApiSource,
        storage_error: std::sync::Arc<dyn Fn(crate::credentials::Error) + Send + Sync>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            http: http.into(),
            token: tokio::sync::Mutex::new(token),
            lease,
            remember: std::sync::atomic::AtomicBool::new(false),
            storage_error,
            source,
            refresh_after: Mutex::new(None),
            #[cfg(test)]
            token_url: None,
        })
    }

    /// A grant restored from protected storage is already remembered. Save
    /// rotated refresh tokens even while the account is being verified, or a
    /// rotation during startup is lost and the stored grant is spent.
    pub fn persist_rotations(&self) {
        self.remember
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    async fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> std::result::Result<crate::auth::TokenResponse, crate::auth::TokenEndpointError> {
        let http = self
            .http
            .client()
            .map_err(crate::auth::TokenEndpointError::Unreachable)?;
        #[cfg(test)]
        if let Some(url) = &self.token_url {
            return crate::auth::refresh_at(&http, url, client_id, refresh_token).await;
        }
        crate::auth::refresh(&http, client_id, refresh_token).await
    }

    /// Start persistence only after the Web API has verified the account.
    pub async fn remember(&self) -> std::result::Result<(), crate::credentials::Error> {
        let token = self.token.lock().await;
        self.remember
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let pending = self
            .lease
            .save(crate::credentials::Grant::Web(token.clone()));
        drop(token);
        pending.await
    }

    /// A valid access token, refreshing first when it is close to expiry or
    /// after Spotify answered 401 to `rejected`. A rejected token that is no
    /// longer current was already replaced by another request's refresh.
    async fn access_token(&self, rejected: Option<&str>) -> Result<String> {
        let mut guard = self.token.lock().await;
        if !self.lease.current() {
            return Err(ApiError::SignInExpired {
                api_source: self.source,
            });
        }
        let force = rejected.is_some_and(|rejected| rejected == guard.access_token);
        if force || guard.needs_refresh() {
            let paused = {
                let mut after = self.refresh_after.lock().unwrap_or_else(|p| p.into_inner());
                if after.is_some_and(|after| Instant::now() >= after) {
                    *after = None;
                }
                after.is_some()
            };
            if paused {
                if force || guard.expired() {
                    return Err(ApiError::RateLimited);
                }
                return Ok(guard.access_token.clone());
            }
            let client_id = guard.client_id.clone();
            let refresh_token = guard.refresh_token.clone();
            match self.refresh(&client_id, &refresh_token).await {
                Ok(response) => match crate::auth::StoredToken::from_response(
                    &client_id,
                    response,
                    Some(&refresh_token),
                ) {
                    Ok(updated) => {
                        if !self.lease.current() {
                            return Err(ApiError::SignInExpired {
                                api_source: self.source,
                            });
                        }
                        if self.remember.load(std::sync::atomic::Ordering::Relaxed) {
                            let pending = self
                                .lease
                                .save(crate::credentials::Grant::Web(updated.clone()));
                            let notice = self.storage_error.clone();
                            let lease = self.lease.clone();
                            tokio::spawn(async move {
                                if let Err(error) = pending.await
                                    && lease.current()
                                {
                                    notice(error);
                                }
                            });
                        }
                        *guard = updated;
                    }
                    Err(error) => {
                        log::warn!("token refresh returned an unusable response: {error}")
                    }
                },
                Err(crate::auth::TokenEndpointError::Rejected { .. }) => {
                    return Err(ApiError::SignInExpired {
                        api_source: self.source,
                    });
                }
                Err(crate::auth::TokenEndpointError::RateLimited { retry_after }) => {
                    let pause = retry_after
                        .unwrap_or(Duration::from_secs(5))
                        .min(MAX_REFRESH_PAUSE);
                    log::warn!(
                        "Spotify token endpoint rate limit source={} wait={pause:?}",
                        self.source
                    );
                    *self.refresh_after.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(Instant::now() + pause);
                    if force || guard.expired() {
                        return Err(ApiError::RateLimited);
                    }
                }
                Err(crate::auth::TokenEndpointError::Unreachable(detail)) => {
                    if force || guard.expired() {
                        return Err(ApiError::Network(detail));
                    }
                    log::warn!("token refresh failed, using the current token: {detail}");
                }
            }
        }
        Ok(guard.access_token.clone())
    }
}

/// What to start playing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayRequest {
    pub context_uri: Option<String>,
    pub uris: Vec<String>,
    pub offset_uri: Option<String>,
    pub offset_position: Option<u32>,
    pub position_ms: u32,
}

impl PlayRequest {
    pub fn context(uri: impl Into<String>) -> Self {
        Self {
            context_uri: Some(uri.into()),
            ..Self::default()
        }
    }

    pub fn tracks(uris: Vec<String>) -> Self {
        Self {
            uris,
            ..Self::default()
        }
    }

    pub fn starting_at_uri(mut self, uri: impl Into<String>) -> Self {
        self.offset_uri = Some(uri.into());
        self
    }

    pub fn starting_at_index(mut self, index: u32) -> Self {
        self.offset_position = Some(index);
        self
    }

    fn body(&self) -> Value {
        let mut body = serde_json::Map::new();
        // The Web API plays a lone track or episode only as `uris`; as a
        // `context_uri` it answers 400 "Non supported context uri". (librespot
        // takes a track as a context, so local playback keeps that form.)
        let single_item = self
            .context_uri
            .as_deref()
            .filter(|uri| matches!(crate::util::uri_kind(uri), Some("track" | "episode")))
            // A resume can name a later song than the context's (autoplay moved
            // on): the named song is the one to play.
            .map(|context| self.offset_uri.as_deref().unwrap_or(context));
        if let Some(item) = single_item {
            body.insert("uris".into(), json!([item]));
        } else if let Some(context) = &self.context_uri {
            body.insert("context_uri".into(), json!(context));
        } else if !self.uris.is_empty() {
            body.insert("uris".into(), json!(self.uris));
        }
        // A single item is the start: an offset naming it adds nothing.
        let offset = single_item.is_none();
        if let Some(uri) = self.offset_uri.as_ref().filter(|_| offset) {
            body.insert("offset".into(), json!({ "uri": uri }));
        } else if let Some(position) = self.offset_position.filter(|_| offset) {
            body.insert("offset".into(), json!({ "position": position }));
        }
        if self.position_ms > 0 {
            body.insert("position_ms".into(), json!(self.position_ms));
        }
        Value::Object(body)
    }
}

/// Live view of the client's traffic, shared with the interface so it can
/// show that the app is talking to Spotify rather than being slow itself.
pub struct NetActivity {
    started_at: Instant,
    in_flight: AtomicUsize,
    /// Milliseconds since `started_at` when the oldest current burst began.
    busy_since_ms: AtomicU64,
    /// Requests sent and rate limits received since the last
    /// [`NetActivity::take_traffic`], per app (shared, personal).
    sent: [AtomicU64; 2],
    limited: [AtomicU64; 2],
    /// Milliseconds since `started_at` when the latest Retry-After cooldown
    /// ends; requests in flight before then are only waiting it out.
    cooling_until_ms: AtomicU64,
}

impl Default for NetActivity {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            in_flight: AtomicUsize::new(0),
            busy_since_ms: AtomicU64::new(0),
            sent: Default::default(),
            limited: Default::default(),
            cooling_until_ms: AtomicU64::new(0),
        }
    }
}

/// Requests one app sent, and how many Spotify answered 429, in a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traffic {
    pub requests: u64,
    pub rate_limited: u64,
}

fn traffic_slot(source: ApiSource) -> usize {
    match source {
        ApiSource::Shared => 0,
        ApiSource::Personal => 1,
    }
}

/// The debug line for one window's traffic, or `None` when nothing was sent.
pub fn traffic_line(shared: Traffic, personal: Traffic) -> Option<String> {
    (shared != Traffic::default() || personal != Traffic::default()).then(|| {
        format!(
            "Spotify traffic last minute: shared requests={} rate_limited={}, personal requests={} rate_limited={}",
            shared.requests, shared.rate_limited, personal.requests, personal.rate_limited
        )
    })
}

impl NetActivity {
    fn record(&self, source: ApiSource, rate_limited: bool) {
        let slot = traffic_slot(source);
        self.sent[slot].fetch_add(1, Ordering::Relaxed);
        if rate_limited {
            self.limited[slot].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Traffic since the last call, per app, as (shared, personal).
    pub fn take_traffic(&self) -> (Traffic, Traffic) {
        let take = |source| {
            let slot = traffic_slot(source);
            Traffic {
                requests: self.sent[slot].swap(0, Ordering::Relaxed),
                rate_limited: self.limited[slot].swap(0, Ordering::Relaxed),
            }
        };
        (take(ApiSource::Shared), take(ApiSource::Personal))
    }

    fn now_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    fn begin(&self) {
        if self.in_flight.fetch_add(1, Ordering::SeqCst) == 0 {
            self.busy_since_ms.store(self.now_ms(), Ordering::SeqCst);
        }
    }

    fn end(&self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    fn note_cooldown(&self, until: Instant) {
        let until_ms = until.saturating_duration_since(self.started_at).as_millis() as u64;
        self.cooling_until_ms.fetch_max(until_ms, Ordering::SeqCst);
    }

    /// How long until a Spotify cooldown ends, while one is running. During
    /// it, requests in flight are only waiting, so there is nothing to
    /// animate.
    pub fn cooldown_left(&self) -> Option<Duration> {
        let left = self
            .cooling_until_ms
            .load(Ordering::SeqCst)
            .saturating_sub(self.now_ms());
        (left > 0).then(|| Duration::from_millis(left))
    }

    /// Requests have been in flight continuously for at least `for_at_least`.
    pub fn busy(&self, for_at_least: Duration) -> bool {
        self.in_flight.load(Ordering::SeqCst) > 0
            && self
                .now_ms()
                .saturating_sub(self.busy_since_ms.load(Ordering::SeqCst))
                >= for_at_least.as_millis() as u64
    }
}

/// Decrements the in-flight count even if the request future is dropped.
struct ActivityGuard<'a>(&'a NetActivity);

impl Drop for ActivityGuard<'_> {
    fn drop(&mut self) {
        self.0.end();
    }
}

pub struct ApiClient {
    #[cfg(test)]
    base_url: Option<String>,
    http: Http,
    tokens: Mutex<Option<TokenProvider>>,
    limiter: Semaphore,
    background: Semaphore,
    /// Held by the one background request allowed out while recovering.
    probe: tokio::sync::Mutex<()>,
    recovery: Recovery,
    queue_writes: tokio::sync::Mutex<()>,
    cooldown_until: tokio::sync::Mutex<Instant>,
    search_limit: u32,
    artist_albums_limit: u32,
    source: ApiSource,
    activity: Arc<NetActivity>,
}

impl ApiClient {
    pub fn new(
        http: impl Into<Http>,
        activity: Arc<NetActivity>,
        search_limit: u32,
        artist_albums_limit: u32,
        source: ApiSource,
    ) -> Self {
        Self {
            #[cfg(test)]
            base_url: None,
            http: http.into(),
            tokens: Mutex::new(None),
            limiter: Semaphore::new(MAX_IN_FLIGHT),
            background: Semaphore::new(BACKGROUND_IN_FLIGHT),
            probe: tokio::sync::Mutex::new(()),
            recovery: Recovery::default(),
            queue_writes: tokio::sync::Mutex::new(()),
            cooldown_until: tokio::sync::Mutex::new(Instant::now()),
            search_limit,
            artist_albums_limit,
            source,
            activity,
        }
    }

    pub fn set_token_provider(&self, provider: Option<TokenProvider>) {
        *self.tokens.lock().unwrap_or_else(|p| p.into_inner()) = provider;
    }

    /// A new authorization gets its own provider and request cooldown. An old
    /// request must never pick up a replacement account's credentials.
    pub fn for_authorization(&self, provider: TokenProvider) -> Self {
        let client = Self::new(
            self.http.clone(),
            self.activity.clone(),
            self.search_limit,
            self.artist_albums_limit,
            self.source,
        );
        client.set_token_provider(Some(provider));
        client
    }

    fn provider(&self) -> Result<TokenProvider> {
        self.tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or(ApiError::NotSignedIn)
    }

    async fn wait_for_cooldown(&self) {
        loop {
            let until = *self.cooldown_until.lock().await;
            let Some(wait) = until.checked_duration_since(Instant::now()) else {
                return;
            };
            tokio::time::sleep(wait).await;
        }
    }

    async fn extend_cooldown(&self, wait: Duration) {
        let mut until = self.cooldown_until.lock().await;
        *until = (*until).max(Instant::now() + wait);
        self.activity.note_cooldown(*until);
    }

    async fn cooling_down(&self) -> bool {
        *self.cooldown_until.lock().await > Instant::now()
    }

    /// A background slot for one request, or `None` when a rate limit
    /// arrived while waiting for it: the caller then waits out the cooldown
    /// holding nothing. While recovering from a rate limit only one
    /// background request is out at a time; the rest queue for the probe
    /// and go together once one has succeeded.
    async fn admit_background(&self) -> Option<BackgroundTicket<'_>> {
        let probe = if self.recovery.probing() {
            let guard = self.probe.lock().await;
            self.recovery.probing().then_some(guard)
        } else {
            None
        };
        let permit = self.background.acquire().await.ok()?;
        if self.cooling_down().await {
            return None;
        }
        Some(BackgroundTicket {
            _permit: permit,
            _probe: probe,
        })
    }

    // ---- transport -------------------------------------------------------

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<String> {
        self.send_body(method, path, query, body, None).await
    }

    async fn send_body(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        jpeg: Option<&str>,
    ) -> Result<String> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            #[cfg(test)]
            let base = self.base_url.as_deref().unwrap_or(BASE_URL);
            #[cfg(not(test))]
            let base = BASE_URL;
            format!("{base}{path}")
        };
        let provider = self.provider()?;
        let started = Instant::now();
        // This is one logical request even when it waits for another request
        // or for a Retry-After cooldown. Keep the interface's activity signal
        // alive for that whole wait, not only while bytes are on the wire.
        self.activity.begin();
        let _activity = ActivityGuard(&self.activity);

        let mut attempt = 0;
        let queue_write = method == Method::POST && path == "/me/player/queue";
        let background = is_background();
        loop {
            self.wait_for_cooldown().await;
            let ticket = if background {
                match self.admit_background().await {
                    Some(ticket) => Some(ticket),
                    None => continue,
                }
            } else {
                None
            };
            attempt = u32::saturating_add(attempt, 1);
            let permit = self
                .limiter
                .acquire()
                .await
                .map_err(|_| ApiError::NotSignedIn)?;
            let token = provider.access_token().await?;
            let mut request = self
                .http
                .client()
                .map_err(ApiError::Network)?
                .request(method.clone(), &url)
                .bearer_auth(&token)
                .query(query);
            if let Some(jpeg) = jpeg {
                request = request
                    .header(reqwest::header::CONTENT_TYPE, "image/jpeg")
                    .body(jpeg.to_owned());
            } else if let Some(body) = body {
                request = request.json(body);
            } else if matches!(method, Method::PUT | Method::POST | Method::DELETE) {
                request = request.header(reqwest::header::CONTENT_LENGTH, "0");
            }
            let response = request.send().await?;
            let status = response.status();
            let rate_limited = status == StatusCode::TOO_MANY_REQUESTS;
            self.activity.record(self.source, rate_limited);
            if rate_limited {
                self.recovery.rate_limited();
            } else {
                self.recovery.succeeded();
            }

            if status == StatusCode::UNAUTHORIZED && attempt == 1 {
                drop(ticket);
                drop(permit);
                provider.invalidate(&token).await;
                continue;
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                let wait = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .map_or(Duration::from_secs(1), Duration::from_secs);
                let text = response.text().await.unwrap_or_default();
                if is_quota_exhausted(&text) {
                    return Err(ApiError::QuotaExhausted);
                }
                // A rejected queue append is safe to retry. Keep its place
                // in the write lock and honor the full server-requested wait.
                // Other requests retain their existing bounded retry policy.
                let wait = if queue_write {
                    wait
                } else {
                    wait.min(MAX_RETRY_AFTER)
                };
                log::warn!("Spotify rate limit source={} wait={wait:?}", self.source);
                log::info!(
                    "Spotify cooldown source={} duration_ms={}",
                    self.source,
                    wait.as_millis()
                );
                // The cooldown is in place before the slots are released, so
                // a background request let in next waits it out first.
                self.extend_cooldown(wait).await;
                drop(ticket);
                drop(permit);
                if !queue_write && attempt > RATE_LIMIT_RETRIES {
                    return Err(ApiError::RateLimited);
                }
                continue;
            }
            if status.is_server_error() && method == Method::GET && attempt == 1 {
                drop(ticket);
                drop(permit);
                tokio::time::sleep(Duration::from_millis(800)).await;
                continue;
            }
            let text = response.text().await?;
            log::debug!(
                "Spotify request source={} method={} status={} duration_ms={}",
                self.source,
                method,
                status.as_u16(),
                started.elapsed().as_millis()
            );
            if status.is_success() {
                return Ok(text);
            }
            let message = serde_json::from_str::<ApiErrorBody>(&text)
                .ok()
                .map(|body| body.error.message)
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| {
                    status
                        .canonical_reason()
                        .unwrap_or("request failed")
                        .to_string()
                });
            return Err(ApiError::Status {
                status: status.as_u16(),
                message,
            });
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let text = self.send(Method::GET, path, query, None).await?;
        if text.trim().is_empty() {
            return serde_json::from_value(Value::Null)
                .map_err(|error| ApiError::Decode(error.to_string()));
        }
        serde_json::from_str(&text).map_err(|error| ApiError::Decode(error.to_string()))
    }

    async fn get_optional<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Option<T>> {
        // Spotify answers 204 with no body when nothing is playing.
        let text = self.send(Method::GET, path, query, None).await?;
        if text.trim().is_empty() || text.trim() == "null" {
            return Ok(None);
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| ApiError::Decode(error.to_string()))
    }

    /// Performs a change. The status decides success; the body is only
    /// consulted where a caller needs something from it, because Spotify's
    /// replies to player commands are not reliably JSON and contain nothing
    /// this client uses. Treating an unparseable body as failure told people
    /// their music had not started while it was already playing.
    async fn write(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Option<Value>> {
        let text = self.send(method, path, query, body).await?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        match serde_json::from_str(&text) {
            Ok(value) => Ok(Some(value)),
            Err(error) => {
                log::debug!(
                    "Spotify write source={} returned a non-JSON success body: {error}",
                    self.source
                );
                Ok(None)
            }
        }
    }

    // ---- identity and player ---------------------------------------------

    pub async fn me(&self) -> Result<User> {
        self.get("/me", &[]).await
    }

    pub async fn devices(&self) -> Result<Vec<Device>> {
        let list: DeviceList = self.get("/me/player/devices", &[]).await?;
        Ok(list.devices)
    }

    pub async fn playback_state(&self) -> Result<Option<PlaybackState>> {
        self.get_optional(
            "/me/player",
            &[("additional_types", "track,episode".to_string())],
        )
        .await
    }

    pub async fn queue(&self) -> Result<Queue> {
        self.get("/me/player/queue", &[]).await
    }

    pub async fn recently_played(
        &self,
        limit: u32,
        after: Option<&str>,
        before: Option<&str>,
    ) -> Result<CursorPage<PlayHistory>> {
        let mut query = vec![("limit", limit.to_string())];
        if let Some(after) = after {
            query.push(("after", after.to_string()));
        }
        if let Some(before) = before {
            query.push(("before", before.to_string()));
        }
        self.get("/me/player/recently-played", &query).await
    }

    fn device_query(device_id: Option<&str>) -> Vec<(&'static str, String)> {
        device_id
            .map(|id| vec![("device_id", id.to_string())])
            .unwrap_or_default()
    }

    pub async fn play(&self, device_id: Option<&str>, request: Option<&PlayRequest>) -> Result<()> {
        let body = request.map(PlayRequest::body);
        self.write(
            Method::PUT,
            "/me/player/play",
            &Self::device_query(device_id),
            body.as_ref(),
        )
        .await?;
        Ok(())
    }

    pub async fn pause(&self, device_id: Option<&str>) -> Result<()> {
        self.write(
            Method::PUT,
            "/me/player/pause",
            &Self::device_query(device_id),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn next(&self, device_id: Option<&str>) -> Result<()> {
        self.write(
            Method::POST,
            "/me/player/next",
            &Self::device_query(device_id),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn previous(&self, device_id: Option<&str>) -> Result<()> {
        self.write(
            Method::POST,
            "/me/player/previous",
            &Self::device_query(device_id),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn seek(&self, position_ms: u32, device_id: Option<&str>) -> Result<()> {
        let mut query = Self::device_query(device_id);
        query.push(("position_ms", position_ms.to_string()));
        self.write(Method::PUT, "/me/player/seek", &query, None)
            .await?;
        Ok(())
    }

    pub async fn set_volume(&self, percent: u8, device_id: Option<&str>) -> Result<()> {
        let mut query = Self::device_query(device_id);
        query.push(("volume_percent", percent.min(100).to_string()));
        self.write(Method::PUT, "/me/player/volume", &query, None)
            .await?;
        Ok(())
    }

    pub async fn set_shuffle(&self, state: bool, device_id: Option<&str>) -> Result<()> {
        let mut query = Self::device_query(device_id);
        query.push(("state", state.to_string()));
        self.write(Method::PUT, "/me/player/shuffle", &query, None)
            .await?;
        Ok(())
    }

    /// `state` is `off`, `context`, or `track`.
    pub async fn set_repeat(&self, state: &str, device_id: Option<&str>) -> Result<()> {
        let mut query = Self::device_query(device_id);
        query.push(("state", state.to_string()));
        self.write(Method::PUT, "/me/player/repeat", &query, None)
            .await?;
        Ok(())
    }

    pub async fn transfer(&self, device_id: &str, play: bool) -> Result<()> {
        let body = json!({ "device_ids": [device_id], "play": play });
        self.write(Method::PUT, "/me/player", &[], Some(&body))
            .await?;
        Ok(())
    }

    pub async fn add_to_queue(&self, uri: &str, device_id: Option<&str>) -> Result<()> {
        let _write = self.queue_writes.lock().await;
        self.append_to_queue(uri, device_id).await
    }

    async fn append_to_queue(&self, uri: &str, device_id: Option<&str>) -> Result<()> {
        let mut query = Self::device_query(device_id);
        query.push(("uri", uri.to_string()));
        self.write(Method::POST, "/me/player/queue", &query, None)
            .await?;
        Ok(())
    }

    /// Spotify appends one song per request. Await each write to keep an
    /// album's order, including repeated tracks, and stop on the first error.
    pub async fn add_many_to_queue(
        &self,
        uris: &[String],
        device_id: Option<&str>,
    ) -> (usize, Result<()>) {
        let _write = self.queue_writes.lock().await;
        for (added, uri) in uris.iter().enumerate() {
            if let Err(error) = self.append_to_queue(uri, device_id).await {
                return (added, Err(error));
            }
        }
        (uris.len(), Ok(()))
    }

    // ---- playlists ---------------------------------------------------------

    pub async fn my_playlists(&self, offset: u32, limit: u32) -> Result<Page<Playlist>> {
        self.get(
            "/me/playlists",
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    pub async fn playlist(&self, id: &str) -> Result<Playlist> {
        self.get(&format!("/playlists/{id}"), &[]).await
    }

    pub async fn playlist_items(
        &self,
        id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Page<PlaylistItem>> {
        self.get::<PositionedPage<PlaylistItem>>(
            &format!("/playlists/{id}/items"),
            &[
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
                ("additional_types", "track,episode".to_string()),
            ],
        )
        .await
        .map(Into::into)
    }

    /// Requested songs already present in a playlist.
    ///
    /// Spotify has no membership endpoint for playlists, so walk its pages
    /// until every requested URI has been found or the playlist ends.
    pub async fn playlist_duplicates(&self, id: &str, uris: &[String]) -> Result<Vec<String>> {
        let wanted: HashSet<&str> = uris.iter().map(String::as_str).collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let mut found = HashSet::new();
        let mut offset = 0;
        loop {
            let page = self.playlist_items(id, offset, 50).await?;
            for uri in page
                .items
                .iter()
                .filter_map(PlaylistItem::playable)
                .map(PlayableItem::uri)
            {
                if wanted.contains(uri) {
                    found.insert(uri.to_string());
                }
            }
            if found.len() == wanted.len() {
                break;
            }
            let Some(next) = page.next_offset() else {
                break;
            };
            offset = next;
        }
        Ok(uris
            .iter()
            .filter(|uri| found.contains(uri.as_str()))
            .cloned()
            .collect())
    }

    pub async fn create_playlist(
        &self,
        name: &str,
        public: bool,
        description: &str,
    ) -> Result<Playlist> {
        let body = json!({ "name": name, "public": public, "description": description });
        let value = self
            .write(Method::POST, "/me/playlists", &[], Some(&body))
            .await?
            .unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(|error| ApiError::Decode(error.to_string()))
    }

    pub async fn upload_playlist_cover(&self, id: &str, encoded: &str) -> Result<()> {
        if encoded.is_empty() || encoded.len() > crate::playlist_cover::MAX_PAYLOAD {
            return Err(ApiError::Status {
                status: 413,
                message: "Choose a smaller image.".into(),
            });
        }
        self.send_body(
            Method::PUT,
            &format!("/playlists/{id}/images"),
            &[],
            None,
            Some(encoded),
        )
        .await?;
        Ok(())
    }

    pub async fn update_playlist(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        public: Option<bool>,
    ) -> Result<()> {
        let mut body = serde_json::Map::new();
        if let Some(name) = name {
            body.insert("name".into(), json!(name));
        }
        if let Some(description) = description {
            body.insert("description".into(), json!(description));
        }
        if let Some(public) = public {
            body.insert("public".into(), json!(public));
        }
        self.write(
            Method::PUT,
            &format!("/playlists/{id}"),
            &[],
            Some(&Value::Object(body)),
        )
        .await?;
        Ok(())
    }

    pub async fn add_playlist_items(
        &self,
        id: &str,
        uris: &[String],
        position: Option<u32>,
    ) -> Result<Option<String>> {
        let mut body = json!({ "uris": uris });
        if let Some(position) = position {
            body["position"] = json!(position);
        }
        let value = self
            .write(
                Method::POST,
                &format!("/playlists/{id}/items"),
                &[],
                Some(&body),
            )
            .await?;
        Ok(Self::snapshot(value))
    }

    pub async fn remove_playlist_items(
        &self,
        id: &str,
        uris: &[String],
        snapshot_id: Option<&str>,
    ) -> Result<Option<String>> {
        let entries: Vec<Value> = uris.iter().map(|uri| json!({ "uri": uri })).collect();
        let mut body = json!({ "items": entries });
        if let Some(snapshot) = snapshot_id {
            body["snapshot_id"] = json!(snapshot);
        }
        let value = self
            .write(
                Method::DELETE,
                &format!("/playlists/{id}/items"),
                &[],
                Some(&body),
            )
            .await?;
        Ok(Self::snapshot(value))
    }

    pub async fn reorder_playlist(
        &self,
        id: &str,
        range_start: u32,
        insert_before: u32,
        snapshot_id: Option<&str>,
    ) -> Result<Option<String>> {
        let mut body = json!({
            "range_start": range_start,
            "insert_before": insert_before,
            "range_length": 1,
        });
        if let Some(snapshot) = snapshot_id {
            body["snapshot_id"] = json!(snapshot);
        }
        let value = self
            .write(
                Method::PUT,
                &format!("/playlists/{id}/items"),
                &[],
                Some(&body),
            )
            .await?;
        Ok(Self::snapshot(value))
    }

    fn snapshot(value: Option<Value>) -> Option<String> {
        value
            .and_then(|value| serde_json::from_value::<SnapshotId>(value).ok())
            .and_then(|snapshot| snapshot.snapshot_id)
    }

    pub async fn follow_playlist(&self, id: &str) -> Result<()> {
        self.write(
            Method::PUT,
            "/me/library",
            &[("uris", format!("spotify:playlist:{id}"))],
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn unfollow_playlist(&self, id: &str) -> Result<()> {
        self.write(
            Method::DELETE,
            "/me/library",
            &[("uris", format!("spotify:playlist:{id}"))],
            None,
        )
        .await?;
        Ok(())
    }

    // ---- library -----------------------------------------------------------

    pub async fn saved_tracks(&self, offset: u32, limit: u32) -> Result<Page<SavedTrack>> {
        self.get(
            "/me/tracks",
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    pub async fn saved_albums(&self, offset: u32, limit: u32) -> Result<Page<SavedAlbum>> {
        self.get(
            "/me/albums",
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    pub async fn followed_artists(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> Result<CursorPage<Artist>> {
        let mut query = vec![("type", "artist".to_string()), ("limit", limit.to_string())];
        if let Some(after) = after {
            query.push(("after", after.to_string()));
        }
        let followed: FollowedArtists = self.get("/me/following", &query).await?;
        Ok(followed.artists)
    }

    pub async fn saved_shows(&self, offset: u32, limit: u32) -> Result<Page<SavedShow>> {
        self.get(
            "/me/shows",
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    pub async fn saved_episodes(&self, offset: u32, limit: u32) -> Result<Page<SavedEpisode>> {
        self.get(
            "/me/episodes",
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    pub async fn top_tracks(
        &self,
        time_range: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Page<Track>> {
        self.get(
            "/me/top/tracks",
            &[
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
                ("time_range", time_range.to_string()),
            ],
        )
        .await
    }

    pub async fn top_artists(&self, time_range: &str, limit: u32) -> Result<Page<Artist>> {
        self.get(
            "/me/top/artists",
            &[
                ("limit", limit.to_string()),
                ("time_range", time_range.to_string()),
            ],
        )
        .await
    }

    async fn library_write(&self, method: Method, uris: &[String]) -> Result<()> {
        self.write(method, "/me/library", &[("uris", uris.join(","))], None)
            .await?;
        Ok(())
    }

    /// Saves tracks, albums, artists, shows, episodes, or playlists.
    pub async fn save(&self, uris: &[String]) -> Result<()> {
        self.library_write(Method::PUT, uris).await
    }

    pub async fn unsave(&self, uris: &[String]) -> Result<()> {
        self.library_write(Method::DELETE, uris).await
    }

    /// Whether each URI is in the library, in the same order as `uris`.
    pub async fn contains(&self, uris: &[String]) -> Result<Vec<bool>> {
        self.get("/me/library/contains", &[("uris", uris.join(","))])
            .await
    }

    // ---- catalog -----------------------------------------------------------

    pub async fn search(&self, query: &str, types: &[&str]) -> Result<SearchResults> {
        self.get(
            "/search",
            &[
                ("q", query.to_string()),
                ("type", types.join(",")),
                ("limit", self.search_limit.to_string()),
            ],
        )
        .await
    }

    pub async fn artist(&self, id: &str) -> Result<Artist> {
        self.get(&format!("/artists/{id}"), &[]).await
    }

    pub async fn artist_top_tracks(&self, id: &str) -> Result<Vec<Track>> {
        self.get::<TopTracks>(&format!("/artists/{id}/top-tracks"), &[])
            .await
            .map(|top| top.tracks)
    }

    pub async fn artist_albums(
        &self,
        id: &str,
        include_groups: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Page<Album>> {
        let limit = limit.min(self.artist_albums_limit);
        self.get(
            &format!("/artists/{id}/albums"),
            &[
                ("include_groups", include_groups.to_string()),
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
            ],
        )
        .await
    }

    pub async fn related_artists(&self, id: &str) -> Result<Vec<Artist>> {
        let related: RelatedArtists = self
            .get(&format!("/artists/{id}/related-artists"), &[])
            .await?;
        Ok(related.artists)
    }

    pub async fn album(&self, id: &str) -> Result<Album> {
        self.get(&format!("/albums/{id}"), &[]).await
    }

    pub async fn album_tracks(&self, id: &str, offset: u32, limit: u32) -> Result<Page<Track>> {
        self.get::<PositionedPage<Track>>(
            &format!("/albums/{id}/tracks"),
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
        .map(Into::into)
    }

    pub async fn show(&self, id: &str) -> Result<Show> {
        self.get(&format!("/shows/{id}"), &[]).await
    }

    pub async fn show_episodes(&self, id: &str, offset: u32, limit: u32) -> Result<Page<Episode>> {
        self.get(
            &format!("/shows/{id}/episodes"),
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    pub async fn track(&self, id: &str) -> Result<Track> {
        self.get(&format!("/tracks/{id}"), &[]).await
    }

    pub async fn episode(&self, id: &str) -> Result<Episode> {
        self.get(&format!("/episodes/{id}"), &[]).await
    }

    pub async fn recommendations(
        &self,
        seed_tracks: &[String],
        seed_artists: &[String],
        limit: u32,
    ) -> Result<Vec<Track>> {
        let mut query = vec![("limit", limit.to_string())];
        if !seed_tracks.is_empty() {
            query.push(("seed_tracks", seed_tracks.join(",")));
        }
        if !seed_artists.is_empty() {
            query.push(("seed_artists", seed_artists.join(",")));
        }
        let recommendations: Recommendations = self.get("/recommendations", &query).await?;
        Ok(recommendations.tracks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn revoked_provider_cannot_return_or_persist_its_token() {
        let root =
            std::env::temp_dir().join(format!("lwspot-revoked-provider-{}", std::process::id()));
        let store = crate::credentials::Store::in_memory(crate::paths::AppDirs {
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
        });
        let slot = crate::credentials::Slot::Shared;
        let tokens = WebTokens::new(
            reqwest::Client::new(),
            crate::auth::StoredToken {
                client_id: crate::auth::DEFAULT_WEB_CLIENT_ID.into(),
                access_token: "dummy-access".into(),
                refresh_token: "dummy-refresh".into(),
                expires_at: u64::MAX,
                scope: String::new(),
            },
            store.lease(slot),
            ApiSource::Shared,
            std::sync::Arc::new(|_| {}),
        );
        // Verification can use a token in memory without persisting it yet.
        assert_eq!(tokens.access_token(None).await.unwrap(), "dummy-access");
        assert!(store.lease(slot).load().await.unwrap().grant.is_none());
        tokens.remember().await.unwrap();
        assert!(store.lease(slot).load().await.unwrap().grant.is_some());
        store.revoke_spotify().unwrap();
        store.lease(slot).delete().await.unwrap();
        assert!(matches!(
            tokens.access_token(None).await,
            Err(ApiError::SignInExpired { .. })
        ));
        assert_eq!(
            tokens.remember().await,
            Err(crate::credentials::Error::Stale)
        );
        assert!(store.lease(slot).load().await.unwrap().grant.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    /// A token endpoint that answers each request with the next response
    /// and counts the requests.
    async fn token_endpoint(
        responses: Vec<&'static str>,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/token", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let server = tokio::spawn(async move {
            for response in responses {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                }
                let headers = String::from_utf8(request).unwrap().to_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map_or(0, |length| length.trim().parse().unwrap());
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                count.fetch_add(1, Ordering::SeqCst);
                // Hold the answer so concurrent callers overlap the refresh.
                tokio::time::sleep(Duration::from_millis(50)).await;
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (url, requests, server)
    }

    fn tokens_at(url: String, access_token: &str) -> Arc<WebTokens> {
        let mut tokens = WebTokens::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            crate::auth::StoredToken {
                client_id: "dummy-client".into(),
                access_token: access_token.into(),
                refresh_token: "dummy-refresh".into(),
                expires_at: u64::MAX,
                scope: String::new(),
            },
            crate::credentials::Store::in_memory(crate::paths::AppDirs {
                config: std::env::temp_dir().join("unused-refresh-token/config"),
                state: std::env::temp_dir().join("unused-refresh-token/state"),
                cache: std::env::temp_dir().join("unused-refresh-token/cache"),
            })
            .lease(crate::credentials::Slot::Shared),
            ApiSource::Shared,
            Arc::new(|_| {}),
        );
        Arc::get_mut(&mut tokens).unwrap().token_url = Some(url);
        tokens
    }

    const FRESH: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 72\r\nConnection: close\r\n\r\n{\"access_token\":\"dummy-fresh\",\"expires_in\":3600,\"refresh_token\":\"rot\"}  ";
    const NEWER: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 72\r\nConnection: close\r\n\r\n{\"access_token\":\"dummy-newer\",\"expires_in\":3600,\"refresh_token\":\"rot\"}  ";

    #[tokio::test]
    async fn concurrent_401s_for_one_token_share_one_refresh() {
        let (url, requests, server) = token_endpoint(vec![FRESH, NEWER]).await;
        let tokens = tokens_at(url, "dummy-stale");
        let (first, second) = tokio::join!(
            tokens.access_token(Some("dummy-stale")),
            tokens.access_token(Some("dummy-stale"))
        );
        assert_eq!(first.unwrap(), "dummy-fresh");
        assert_eq!(second.unwrap(), "dummy-fresh");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        // A late 401 for the replaced token needs no refresh either.
        assert_eq!(
            tokens.access_token(Some("dummy-stale")).await.unwrap(),
            "dummy-fresh"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        // A 401 for the current token still refreshes.
        assert_eq!(
            tokens.access_token(Some("dummy-fresh")).await.unwrap(),
            "dummy-newer"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_rate_limited_token_endpoint_keeps_the_grant_and_pauses_refreshes() {
        let (url, requests, server) = token_endpoint(vec![
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 60\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ])
        .await;
        let tokens = tokens_at(url, "dummy-stale");
        assert!(matches!(
            tokens.access_token(Some("dummy-stale")).await,
            Err(ApiError::RateLimited)
        ));
        // The pause holds: no second request, and the grant is not expired.
        assert!(matches!(
            tokens.access_token(Some("dummy-stale")).await,
            Err(ApiError::RateLimited)
        ));
        assert_eq!(tokens.access_token(None).await.unwrap(), "dummy-stale");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn queue_batch_preserves_order_and_duplicates_and_stops_on_failure() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (fail, rate_limit) in [(false, false), (true, false), (false, true)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let first_request = Arc::new(tokio::sync::Notify::new());
            let notify = Arc::clone(&first_request);
            let server = tokio::spawn(async move {
                let mut paths = Vec::new();
                let mut cooldown_started: Option<Instant> = None;
                for index in 0..if fail {
                    2
                } else if rate_limit {
                    8
                } else {
                    4
                } {
                    let (mut socket, _) =
                        tokio::time::timeout(Duration::from_secs(5), listener.accept())
                            .await
                            .unwrap()
                            .unwrap();
                    if index == 2 && rate_limit {
                        assert!(cooldown_started.unwrap().elapsed() >= Duration::from_secs(1));
                    }
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(socket.read_u8().await.unwrap());
                        assert!(request.len() < 8192);
                    }
                    paths.push(
                        String::from_utf8(request)
                            .unwrap()
                            .lines()
                            .next()
                            .unwrap()
                            .to_string(),
                    );
                    if index == 0 {
                        notify.notify_one();
                    }
                    assert!(
                        tokio::time::timeout(Duration::from_millis(10), listener.accept())
                            .await
                            .is_err(),
                        "the next append must wait for this response"
                    );
                    let limited = rate_limit && (1..=4).contains(&index);
                    let status = if limited {
                        "429 Too Many Requests"
                    } else if fail && index == 1 {
                        "403 Forbidden"
                    } else {
                        "204 No Content"
                    };
                    let retry_after = if limited && index == 1 { 1 } else { 0 };
                    cooldown_started = Some(Instant::now());
                    socket.write_all(format!("HTTP/1.1 {status}\r\nRetry-After: {retry_after}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                }
                paths
            });
            let http = reqwest::Client::builder().no_proxy().build().unwrap();
            let mut client = ApiClient::new(
                http.clone(),
                Arc::new(NetActivity::default()),
                20,
                50,
                ApiSource::Shared,
            );
            client.base_url = Some(format!("http://{address}"));
            client.set_token_provider(Some(TokenProvider::Web(WebTokens::new(
                http,
                crate::auth::StoredToken {
                    access_token: "test-only".into(),
                    expires_at: u64::MAX,
                    ..Default::default()
                },
                crate::credentials::Store::in_memory(crate::paths::AppDirs {
                    config: std::env::temp_dir().join("unused-queue-token/config"),
                    state: std::env::temp_dir().join("unused-queue-token/state"),
                    cache: std::env::temp_dir().join("unused-queue-token/cache"),
                })
                .lease(crate::credentials::Slot::Shared),
                ApiSource::Shared,
                Arc::new(|_| {}),
            ))));
            let mut uris = vec![
                "spotify:track:a".into(),
                "spotify:track:b".into(),
                "spotify:track:a".into(),
            ];
            let (added, result) = if fail {
                client.add_many_to_queue(&uris, Some("phone")).await
            } else {
                let (album, later_song) =
                    tokio::join!(client.add_many_to_queue(&uris, Some("phone")), async {
                        first_request.notified().await;
                        client
                            .add_to_queue("spotify:track:later", Some("phone"))
                            .await
                    });
                later_song.unwrap();
                uris.push("spotify:track:later".into());
                album
            };
            assert_eq!(result.is_err(), fail);
            assert_eq!(added, if fail { 1 } else { 3 });
            if fail {
                assert_eq!(result.unwrap_err().status(), Some(403));
            }
            let paths = server.await.unwrap();
            if rate_limit {
                // Only the rejected song is retried, even beyond the normal
                // request retry budget. The later single still follows the album.
                uris.splice(1..1, std::iter::repeat_n("spotify:track:b".into(), 4));
            }
            assert_eq!(paths.len(), if fail { 2 } else { uris.len() });
            for (path, expected) in paths.iter().zip(uris) {
                assert!(path.starts_with("POST /me/player/queue?"));
                let url = reqwest::Url::parse(&format!(
                    "http://test{}",
                    path.split_whitespace().nth(1).unwrap()
                ))
                .unwrap();
                let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
                assert_eq!(query["uri"], expected);
                assert_eq!(query["device_id"], "phone");
            }
        }
    }

    #[tokio::test]
    async fn cover_upload_sends_raw_base64_jpeg_and_reports_spotify_errors() {
        use std::io::{Read, Write};
        for status in [202, 403, 413] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut received = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    let count = socket.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    received.extend_from_slice(&buffer[..count]);
                    if let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&received[..end]).to_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if received.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                write!(
                    socket,
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                received
            });
            let http = reqwest::Client::new();
            let mut client = ApiClient::new(
                http.clone(),
                Arc::new(NetActivity::default()),
                20,
                50,
                ApiSource::Shared,
            );
            client.base_url = Some(format!("http://{address}"));
            client.set_token_provider(Some(TokenProvider::Web(WebTokens::new(
                http,
                crate::auth::StoredToken {
                    access_token: "test-only".into(),
                    expires_at: u64::MAX,
                    ..Default::default()
                },
                crate::credentials::Store::in_memory(crate::paths::AppDirs {
                    config: std::env::temp_dir().join("unused-cover-token/config"),
                    state: std::env::temp_dir().join("unused-cover-token/state"),
                    cache: std::env::temp_dir().join("unused-cover-token/cache"),
                })
                .lease(crate::credentials::Slot::Shared),
                ApiSource::Shared,
                Arc::new(|_| {}),
            ))));
            let result = client
                .upload_playlist_cover("test-playlist", "/9j/test")
                .await;
            if status == 202 {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.unwrap_err().status(), Some(status));
            }
            let received = String::from_utf8(server.join().unwrap()).unwrap();
            assert!(received.starts_with("PUT /playlists/test-playlist/images HTTP/1.1"));
            assert!(received.to_lowercase().contains("content-type: image/jpeg"));
            assert_eq!(received.split("\r\n\r\n").nth(1), Some("/9j/test"));
        }
    }

    #[tokio::test]
    async fn oversized_cover_is_rejected_before_authentication_or_network() {
        let client = ApiClient::new(
            reqwest::Client::new(),
            Arc::new(NetActivity::default()),
            20,
            50,
            ApiSource::Shared,
        );
        let result = client
            .upload_playlist_cover("test", &"x".repeat(crate::playlist_cover::MAX_PAYLOAD + 1))
            .await;
        assert_eq!(result.unwrap_err().status(), Some(413));
    }

    #[test]
    fn play_request_body_shapes() {
        let context = PlayRequest::context("spotify:album:x").starting_at_uri("spotify:track:y");
        assert_eq!(
            context.body(),
            json!({ "context_uri": "spotify:album:x", "offset": { "uri": "spotify:track:y" } })
        );
        let tracks = PlayRequest::tracks(vec!["spotify:track:a".into()]).starting_at_index(0);
        assert_eq!(
            tracks.body(),
            json!({ "uris": ["spotify:track:a"], "offset": { "position": 0 } })
        );
        // A lone track or episode as a context is sent as the only uri.
        let mut song = PlayRequest::context("spotify:track:t").starting_at_uri("spotify:track:t");
        song.position_ms = 5_000;
        assert_eq!(
            song.body(),
            json!({ "uris": ["spotify:track:t"], "position_ms": 5_000 })
        );
        assert_eq!(
            PlayRequest::context("spotify:episode:e").body(),
            json!({ "uris": ["spotify:episode:e"] })
        );
        // A resume whose song moved past the one-song context plays that song.
        let mut resumed =
            PlayRequest::context("spotify:track:a").starting_at_uri("spotify:track:b");
        resumed.position_ms = 42_000;
        assert_eq!(
            resumed.body(),
            json!({ "uris": ["spotify:track:b"], "position_ms": 42_000 })
        );
    }

    #[test]
    fn quota_exhaustion_is_distinct_from_an_ordinary_rate_limit() {
        assert!(is_quota_exhausted(
            r#"{"error":{"status":429,"reason":"QUOTA_EXCEEDED"}}"#
        ));
        assert!(!is_quota_exhausted(
            r#"{"error":{"status":429,"message":"Too many requests"}}"#
        ));
    }

    #[tokio::test]
    async fn cooldown_state_is_owned_by_one_session() {
        let activity = Arc::new(NetActivity::default());
        let shared = ApiClient::new(
            reqwest::Client::new(),
            activity.clone(),
            20,
            50,
            ApiSource::Shared,
        );
        let personal = ApiClient::new(
            reqwest::Client::new(),
            activity,
            10,
            10,
            ApiSource::Personal,
        );
        shared.extend_cooldown(Duration::from_secs(10)).await;
        assert!(*shared.cooldown_until.lock().await > Instant::now());
        assert!(*personal.cooldown_until.lock().await <= Instant::now());
    }

    fn bare_client(source: ApiSource) -> ApiClient {
        ApiClient::new(
            reqwest::Client::new(),
            Arc::new(NetActivity::default()),
            20,
            50,
            source,
        )
    }

    #[tokio::test]
    async fn each_app_has_its_own_background_slots() {
        let gateway =
            super::super::ApiGateway::new(reqwest::Client::new(), Arc::new(NetActivity::default()));
        let shared = gateway.verification_client(ApiSource::Shared);
        let personal = gateway.verification_client(ApiSource::Personal);
        let mut held = Vec::new();
        for _ in 0..BACKGROUND_IN_FLIGHT {
            held.push(shared.admit_background().await.expect("a free slot"));
        }
        assert_eq!(shared.background.available_permits(), 0);
        // The shared app being full, or cooling down, leaves the personal
        // app's background requests alone.
        shared.extend_cooldown(Duration::from_secs(30)).await;
        assert!(personal.admit_background().await.is_some());
        assert_eq!(
            personal.background.available_permits(),
            BACKGROUND_IN_FLIGHT
        );
        drop(held);
    }

    #[tokio::test]
    async fn a_background_request_holds_no_slot_while_cooling_down() {
        let client = bare_client(ApiSource::Shared);
        client.extend_cooldown(Duration::from_secs(30)).await;
        assert!(client.admit_background().await.is_none());
        assert_eq!(client.background.available_permits(), BACKGROUND_IN_FLIGHT);
    }

    #[tokio::test]
    async fn after_a_rate_limit_background_requests_resume_one_at_a_time() {
        let client = bare_client(ApiSource::Shared);
        client.recovery.rate_limited();
        let probe = client.admit_background().await.expect("the probe");
        let mut second = Box::pin(client.admit_background());
        std::future::poll_fn(|cx| {
            assert!(
                second.as_mut().poll(cx).is_pending(),
                "only the probe goes while recovering"
            );
            std::task::Poll::Ready(())
        })
        .await;
        // The probe got through: everyone waiting goes, without the probe.
        client.recovery.succeeded();
        drop(probe);
        let second = second.await.expect("released");
        assert!(second._probe.is_none());
        assert!(client.admit_background().await.is_some());
    }

    #[test]
    fn traffic_is_counted_per_app_and_reset_each_window() {
        let activity = NetActivity::default();
        assert_eq!(
            traffic_line(activity.take_traffic().0, activity.take_traffic().1),
            None
        );
        activity.record(ApiSource::Shared, false);
        activity.record(ApiSource::Shared, true);
        activity.record(ApiSource::Personal, false);
        let (shared, personal) = activity.take_traffic();
        assert_eq!(
            shared,
            Traffic {
                requests: 2,
                rate_limited: 1
            }
        );
        assert_eq!(
            personal,
            Traffic {
                requests: 1,
                rate_limited: 0
            }
        );
        assert_eq!(
            traffic_line(shared, personal).as_deref(),
            Some(
                "Spotify traffic last minute: shared requests=2 rate_limited=1, personal requests=1 rate_limited=0"
            )
        );
        assert_eq!(
            activity.take_traffic(),
            (Traffic::default(), Traffic::default())
        );
    }

    #[test]
    fn a_cooldown_is_reported_until_it_ends_and_the_latest_one_wins() {
        let activity = NetActivity::default();
        assert_eq!(activity.cooldown_left(), None);
        let now = Instant::now();
        activity.note_cooldown(now + Duration::from_secs(30));
        // An earlier end noted later does not cut the wait short.
        activity.note_cooldown(now + Duration::from_secs(5));
        let left = activity.cooldown_left().expect("cooling down");
        assert!(left > Duration::from_secs(25) && left <= Duration::from_secs(30));
        // One that has already ended reports nothing.
        let ended = NetActivity::default();
        ended.note_cooldown(Instant::now());
        assert_eq!(ended.cooldown_left(), None);
    }

    #[tokio::test]
    async fn only_background_work_is_marked_background() {
        assert!(!is_background());
        assert!(in_background(async { is_background() }).await);
    }
}
