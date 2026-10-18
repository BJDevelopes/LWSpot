//! Spotify sign-in with the Authorization Code + PKCE flow.
//!
//! Three independent grants may exist because Spotify treats them differently:
//!
//! - The shared and optional personal **Web API grants** use separate
//!   registered application identities, protected entries, and request sessions.
//! - The **playback grant** uses Spotify's desktop client identity, the one
//!   librespot streams with. Its access token is exchanged once for a
//!   reusable credential kept in the platform credential store.
//!
//! The browser does the password entry; this process only ever sees the
//! one-time authorization code that Spotify sends back to a loopback
//! listener.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::watch;

/// Spotify's own desktop client identity, the one librespot streams with.
pub const PLAYBACK_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
pub const PLAYBACK_REDIRECT_PORT: u16 = 8898;

/// The public Web API application shared by spotify-player, ncspot, and
/// Omarchy Spotify.
pub const DEFAULT_WEB_CLIENT_ID: &str = "d420a117a32841c2b3474932e49fb54b";
pub const WEB_REDIRECT_PORT: u16 = 8989;

pub const REDIRECT_PATH: &str = "/login";

/// Playback: what librespot needs to stream and join Spotify Connect.
pub const PLAYBACK_SCOPES: &[&str] = &["streaming"];

/// Web API: what visible features use, plus `user-read-private` for the
/// plan (Free or Premium), which decides whether local playback is offered
/// at all; no email.
pub const WEB_SCOPES: &[&str] = &[
    "ugc-image-upload",
    "playlist-modify-private",
    "playlist-modify-public",
    "playlist-read-collaborative",
    "playlist-read-private",
    "user-follow-modify",
    "user-follow-read",
    "user-library-modify",
    "user-library-read",
    "user-modify-playback-state",
    "user-read-playback-position",
    "user-read-playback-state",
    "user-read-private",
    "user-read-recently-played",
    "user-top-read",
];

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Refresh this long before the access token expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(90);

/// One OAuth application identity and what it is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub client_id: String,
    pub redirect_port: u16,
    pub scopes: &'static [&'static str],
}

impl Grant {
    pub fn playback() -> Self {
        Self {
            client_id: PLAYBACK_CLIENT_ID.to_string(),
            redirect_port: PLAYBACK_REDIRECT_PORT,
            scopes: PLAYBACK_SCOPES,
        }
    }

    pub fn shared_web_api() -> Self {
        Self {
            client_id: DEFAULT_WEB_CLIENT_ID.to_string(),
            redirect_port: WEB_REDIRECT_PORT,
            scopes: WEB_SCOPES,
        }
    }

    pub fn personal_web_api(client_id: &str) -> Result<Self> {
        let client_id = client_id.trim();
        if client_id.is_empty() {
            bail!("a personal Spotify Client ID is required");
        }
        Ok(Self {
            client_id: client_id.to_string(),
            redirect_port: WEB_REDIRECT_PORT,
            scopes: WEB_SCOPES,
        })
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{REDIRECT_PATH}", self.redirect_port)
    }
}

/// A started sign-in: the URL to open and the secrets needed to finish it.
#[derive(Clone, Debug)]
pub struct Flow {
    pub verifier: String,
    pub state: String,
    pub url: String,
}

fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}

pub fn begin(grant: Grant) -> Flow {
    let verifier = random_token(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token(18);
    let show_dialog = if grant.client_id == PLAYBACK_CLIENT_ID {
        "&show_dialog=true"
    } else {
        ""
    };
    let url = format!(
        "{AUTHORIZE_URL}?client_id={}&response_type=code&redirect_uri={}&code_challenge_method=S256&code_challenge={challenge}&state={state}&scope={}{show_dialog}",
        grant.client_id,
        urlencoding::encode(&grant.redirect_uri()),
        urlencoding::encode(&grant.scopes.join(" "))
    );
    Flow {
        verifier,
        state,
        url,
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// Listens for Spotify's redirect and returns the authorization code.
///
/// Ends early when `cancel` flips to true (the user gave up) or after ten
/// minutes.
pub async fn wait_for_code(
    port: u16,
    expected_state: &str,
    mut cancel: watch::Receiver<bool>,
) -> Result<String> {
    let address: SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("unable to listen on {address} for the Spotify redirect"))?;
    let deadline = tokio::time::sleep(LOGIN_TIMEOUT);
    tokio::pin!(deadline);
    // Each connection is answered on its own, so one that sends nothing (a
    // browser's speculative preconnect, say) holds up neither the real
    // redirect nor Cancel. Dropping the set ends the others.
    let mut connections = tokio::task::JoinSet::new();
    // Once the sender is gone only the redirect or the deadline ends the
    // wait: the worker drops it when it is done with the sign-in, which
    // is not the person giving up in the browser.
    let mut cancellable = true;

    loop {
        tokio::select! {
            // At the limit, new connections wait in the listen queue.
            accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                let (stream, _) = accepted.context("redirect listener failed")?;
                connections.spawn(answer(stream, expected_state.to_owned()));
            }
            Some(answered) = connections.join_next() => match answered {
                Ok(Ok(code)) => return Ok(code),
                // Spotify sent this attempt back with an error: the person
                // declined, or Spotify refused. Waiting longer is pointless.
                Ok(Err(error)) if error.is::<Declined>() => return Err(error),
                // A favicon request or a stale tab is not the redirect; keep waiting.
                Ok(Err(error)) => log::debug!("ignored request on the redirect listener: {error}"),
                Err(_) => {}
            },
            // A closed channel would answer at once, every time round.
            changed = cancel.changed(), if cancellable => match changed {
                Ok(()) if *cancel.borrow() => bail!("sign-in cancelled"),
                Ok(()) => {}
                Err(_) => cancellable = false,
            },
            _ = &mut deadline => bail!("sign-in timed out; try again"),
        }
    }
}

/// At most this many connections are answered at once; more wait in the
/// listen queue.
const MAX_CONNECTIONS: usize = 16;
/// How long a connection may take to send its request line.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The browser came back from Spotify with an error for this attempt.
#[derive(Debug, thiserror::Error)]
#[error("the sign-in was declined or refused in the browser")]
pub struct Declined;

/// Reads one request, answers it with the success or failure page, and
/// says what it was.
async fn answer(mut stream: tokio::net::TcpStream, expected_state: String) -> Result<String> {
    let mut request_line = String::new();
    let read = {
        let mut reader = BufReader::new(&mut stream);
        tokio::time::timeout(REQUEST_TIMEOUT, reader.read_line(&mut request_line)).await
    };
    if !matches!(read, Ok(Ok(_))) {
        bail!("no request on the redirect listener");
    }
    let outcome = parse_request_line(&request_line, &expected_state);
    let (status, body) = match &outcome {
        Ok(_) => ("200 OK", success_page()),
        Err(error) => ("400 Bad Request", failure_page(&error.to_string())),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
    outcome
}

fn parse_request_line(line: &str, expected_state: &str) -> Result<String> {
    let target = line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("malformed request"))?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != REDIRECT_PATH {
        bail!("unexpected redirect path");
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = urlencoding::decode(value)
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| value.to_string());
        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            _ => {}
        }
    }
    // The state first: a stale tab from an earlier attempt must not end
    // this one, whatever it carries.
    if state.as_deref() != Some(expected_state) {
        bail!("state mismatch");
    }
    if error.is_some() {
        return Err(Declined.into());
    }
    code.ok_or_else(|| anyhow!("Spotify did not return an authorization code"))
}

pub async fn exchange_code(
    http: &reqwest::Client,
    grant: &Grant,
    code: &str,
    verifier: &str,
) -> Result<TokenResponse> {
    token_request(
        http,
        &[
            ("client_id", grant.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", grant.redirect_uri().as_str()),
            ("code_verifier", verifier),
        ],
    )
    .await
    .map_err(|error| anyhow!("{error}"))
}

pub async fn refresh(
    http: &reqwest::Client,
    client_id: &str,
    refresh_token: &str,
) -> std::result::Result<TokenResponse, TokenEndpointError> {
    token_request(
        http,
        &[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

#[derive(Debug, thiserror::Error)]
pub enum TokenEndpointError {
    /// Spotify refused the grant itself; only the browser can mint a new one.
    #[error("Spotify rejected the token request ({status}): {detail}")]
    Rejected { status: u16, detail: String },
    /// Spotify asked for fewer token requests. The grant is still valid.
    #[error("Spotify is rate limiting token requests")]
    RateLimited { retry_after: Option<Duration> },
    /// The endpoint could not answer; the grant may still be valid.
    #[error("token request failed: {0}")]
    Unreachable(String),
}

async fn token_request(
    http: &reqwest::Client,
    form: &[(&str, &str)],
) -> std::result::Result<TokenResponse, TokenEndpointError> {
    token_request_at(http, TOKEN_URL, form).await
}

async fn token_request_at(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> std::result::Result<TokenResponse, TokenEndpointError> {
    let response = http
        .post(url)
        .form(form)
        .send()
        .await
        .map_err(|error| TokenEndpointError::Unreachable(error.to_string()))?;
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let text = response.text().await.unwrap_or_default();
    decode_token_response(status, retry_after, &text)
}

/// `refresh` against another token endpoint, for tests with a local server.
#[cfg(test)]
pub(crate) async fn refresh_at(
    http: &reqwest::Client,
    url: &str,
    client_id: &str,
    refresh_token: &str,
) -> std::result::Result<TokenResponse, TokenEndpointError> {
    token_request_at(
        http,
        url,
        &[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

/// Only an OAuth answer that names the grant or client as invalid ends a
/// grant. Rate limits, timeouts, server errors, and pages that are not
/// Spotify's JSON (a proxy or captive portal) leave it for a later retry.
fn decode_token_response(
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
    text: &str,
) -> std::result::Result<TokenResponse, TokenEndpointError> {
    if status.is_success() {
        return serde_json::from_str(text).map_err(|_| {
            TokenEndpointError::Unreachable("Spotify returned an unreadable token response".into())
        });
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(TokenEndpointError::RateLimited { retry_after });
    }
    let value = serde_json::from_str::<serde_json::Value>(text).ok();
    // OAuth descriptions and malformed bodies may contain usable grants.
    // Report only known classifications, never authorization response text.
    let code = value.as_ref().and_then(|value| value["error"].as_str());
    let detail = match code {
        Some("invalid_grant") => "The grant expired or was revoked. Sign in again.",
        Some("invalid_client") => "Check the Spotify application's Client ID.",
        Some("invalid_scope") => "Spotify did not accept the requested permissions.",
        Some("access_denied") => "Spotify authorization was declined.",
        Some(_) => "Spotify refused the token request.",
        None => "Spotify's answer was not a token response.",
    };
    if matches!(status.as_u16(), 400 | 401)
        && matches!(code, Some("invalid_grant" | "invalid_client"))
    {
        return Err(TokenEndpointError::Rejected {
            status: status.as_u16(),
            detail: detail.into(),
        });
    }
    Err(TokenEndpointError::Unreachable(format!(
        "Spotify answered {}: {detail}",
        status.as_u16()
    )))
}

/// The Web API grant, serialized only inside protected credential storage.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredToken {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
    #[serde(default)]
    pub scope: String,
}

impl StoredToken {
    pub fn from_response(
        client_id: &str,
        response: TokenResponse,
        previous_refresh: Option<&str>,
    ) -> Result<Self> {
        let refresh_token = response
            .refresh_token
            .or_else(|| previous_refresh.map(str::to_string))
            .ok_or_else(|| anyhow!("Spotify did not return a refresh token"))?;
        Ok(Self {
            client_id: client_id.to_string(),
            access_token: response.access_token,
            refresh_token,
            expires_at: now_secs() + response.expires_in.unwrap_or(3600),
            scope: response.scope.unwrap_or_default(),
        })
    }

    pub fn needs_refresh(&self) -> bool {
        now_secs() + REFRESH_MARGIN.as_secs() >= self.expires_at
    }

    pub fn expired(&self) -> bool {
        now_secs() >= self.expires_at
    }

    /// Whether the grant covers every scope in `scopes`. A grant cannot be
    /// widened by a refresh, only by the browser.
    pub fn has_scopes(&self, scopes: &[&str]) -> bool {
        let granted: Vec<&str> = self.scope.split_whitespace().collect();
        scopes.iter().all(|scope| granted.contains(scope))
    }

    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    // Legacy file fixtures only. Production persistence lives in credentials.rs.
    #[cfg(test)]
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let temporary = path.with_extension("json.tmp");
        write_private(&temporary, text.as_bytes())?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }

    // Legacy file fixtures only. Production persistence lives in credentials.rs.
    #[cfg(test)]
    pub fn remove(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    // Legacy file fixtures only. Production persistence lives in credentials.rs.
    #[cfg(test)]
    pub fn migrate_legacy(legacy: &Path, shared: &Path, personal: &Path) -> Result<()> {
        let Some(token) = Self::load(legacy) else {
            return Ok(());
        };
        let target = if token.client_id == DEFAULT_WEB_CLIENT_ID {
            shared
        } else {
            personal
        };
        if let Some(existing) = Self::load(target) {
            if existing != token {
                return Ok(());
            }
        } else {
            token.save(target)?;
        }
        Self::remove(legacy);
        Ok(())
    }
}

#[cfg(test)]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.flush()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn page(title: &str, heading: &str, body: &str, accent: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title>\
<style>:root{{color-scheme:dark}}body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#0f1114;color:#e8eaed;font-family:Inter,system-ui,sans-serif}}\
main{{max-width:28rem;padding:2.5rem;border-radius:1.25rem;background:#181b20;box-shadow:0 20px 60px rgba(0,0,0,.5);text-align:center}}\
.mark{{width:64px;height:64px;border-radius:50%;background:{accent};display:grid;place-items:center;margin:0 auto 1.25rem}}\
.mark svg{{width:30px;height:30px;fill:#0f1114}}h1{{font-size:1.4rem;margin:.25rem 0 .5rem}}p{{color:#a5adba;line-height:1.5;margin:0}}</style>\
<main><div class=\"mark\"><svg viewBox=\"0 0 24 24\"><path d=\"M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z\"/></svg></div>\
<h1>{heading}</h1><p>{body}</p></main><script>setTimeout(function(){{window.close()}},1500)</script></html>"
    )
}

fn success_page() -> String {
    page(
        "Signed in to LWSpot",
        "You're signed in",
        "You can close this tab and go back to LWSpot.",
        "#6d5bf0",
    )
}

fn failure_page(reason: &str) -> String {
    page(
        "Sign-in failed",
        "Sign-in didn't complete",
        &format!("{reason}. Return to LWSpot and try again."),
        "#f5717f",
    )
}

#[cfg(test)]
mod tests {
    /// A free loopback port for one listener test.
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn request(port: u16, line: &str) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(format!("{line}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut reply = Vec::new();
        let _ = stream.read_to_end(&mut reply).await;
    }

    /// A connection that sends nothing, as a browser's preconnect does,
    /// holds up neither the redirect nor Cancel.
    #[tokio::test]
    async fn an_idle_connection_does_not_hold_up_the_redirect_or_cancel() {
        let port = free_port();
        let (_cancel, cancel_rx) = tokio::sync::watch::channel(false);
        let waiting =
            tokio::spawn(async move { super::wait_for_code(port, "s1", cancel_rx).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _idle = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let started = std::time::Instant::now();
        request(port, "GET /login?code=abc&state=s1 HTTP/1.1").await;
        let code = waiting.await.unwrap().unwrap();
        assert_eq!(code, "abc");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        let port = free_port();
        let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
        let waiting =
            tokio::spawn(async move { super::wait_for_code(port, "s1", cancel_rx).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _idle = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let started = std::time::Instant::now();
        cancel.send(true).unwrap();
        let error = waiting.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    /// The worker dropping its Cancel neither cancels nor spins: the
    /// redirect still arrives.
    #[tokio::test]
    async fn a_dropped_cancel_keeps_waiting_for_the_redirect() {
        let port = free_port();
        let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
        let waiting =
            tokio::spawn(async move { super::wait_for_code(port, "s1", cancel_rx).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(cancel);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!waiting.is_finished(), "a dropped Cancel ended the wait");
        request(port, "GET /login?code=abc&state=s1 HTTP/1.1").await;
        assert_eq!(waiting.await.unwrap().unwrap(), "abc");
    }

    /// Declining on Spotify's page ends this attempt at once; a stale tab of
    /// another attempt is ignored.
    #[tokio::test]
    async fn a_declined_sign_in_ends_the_wait_and_a_stale_tab_does_not() {
        let port = free_port();
        let (_cancel, cancel_rx) = tokio::sync::watch::channel(false);
        let waiting =
            tokio::spawn(async move { super::wait_for_code(port, "s1", cancel_rx).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        request(port, "GET /login?error=access_denied&state=old HTTP/1.1").await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!waiting.is_finished(), "a stale tab ended the attempt");
        request(port, "GET /login?error=access_denied&state=s1 HTTP/1.1").await;
        let error = waiting.await.unwrap().unwrap_err();
        assert!(error.is::<super::Declined>());
    }

    #[test]
    fn token_errors_never_include_authorization_response_contents() {
        for (status, body) in [
            (
                400,
                r#"{"error":"invalid_grant","error_description":"dummy-refresh-secret"}"#,
            ),
            (400, r#"{"error":"dummy-refresh-secret"}"#),
            (503, "dummy-refresh-secret"),
            (
                200,
                r#"{"access_token":"dummy-refresh-secret","expires_in":"dummy-refresh-secret"}"#,
            ),
        ] {
            let error = super::decode_token_response(
                reqwest::StatusCode::from_u16(status).unwrap(),
                None,
                body,
            )
            .unwrap_err();
            assert!(!format!("{error:?} {error}").contains("dummy-refresh-secret"));
        }
        for request in [
            "GET /login?error=dummy-authorization-secret&state=ok HTTP/1.1",
            "GET /dummy-authorization-secret HTTP/1.1",
        ] {
            let error = super::parse_request_line(request, "ok").unwrap_err();
            assert!(!format!("{error:?} {error}").contains("dummy-authorization-secret"));
        }
    }

    use super::*;

    #[test]
    fn only_an_invalid_grant_or_client_ends_a_grant() {
        let html = "<html><body>Sign in to the network</body></html>";
        for (status, body, rejected) in [
            (400, r#"{"error":"invalid_grant"}"#, true),
            (401, r#"{"error":"invalid_client"}"#, true),
            (400, r#"{"error":"invalid_request"}"#, false),
            (400, html, false),
            (403, html, false),
            (403, r#"{"error":"invalid_grant"}"#, false),
            (404, "", false),
            (408, "", false),
            (429, r#"{"error":"rate_limited"}"#, false),
            (500, r#"{"error":"invalid_grant"}"#, false),
            (502, html, false),
            (503, "", false),
        ] {
            let result = decode_token_response(
                reqwest::StatusCode::from_u16(status).unwrap(),
                Some(Duration::from_secs(7)),
                body,
            );
            match result {
                Err(TokenEndpointError::Rejected { status: code, .. }) => {
                    assert!(rejected, "{status} {body} must keep the grant");
                    assert_eq!(code, status);
                }
                Err(TokenEndpointError::RateLimited { retry_after }) => {
                    assert!(!rejected);
                    assert_eq!(status, 429);
                    assert_eq!(retry_after, Some(Duration::from_secs(7)));
                }
                Err(TokenEndpointError::Unreachable(detail)) => {
                    assert!(!rejected, "{status} {body} must end the grant");
                    assert!(!detail.contains("network"), "body text is never reported");
                }
                Ok(_) => panic!("{status} is not a token"),
            }
        }
    }

    #[test]
    fn old_web_grants_require_image_upload_consent() {
        let token = StoredToken {
            scope: WEB_SCOPES
                .iter()
                .filter(|scope| **scope != "ugc-image-upload")
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
            ..Default::default()
        };
        assert!(!token.has_scopes(WEB_SCOPES));
        let renewed = StoredToken {
            scope: WEB_SCOPES.join(" "),
            ..token
        };
        assert!(renewed.has_scopes(WEB_SCOPES));
    }

    #[test]
    fn begin_produces_valid_pkce_material() {
        let flow = begin(Grant::shared_web_api());
        assert!(flow.verifier.len() >= 43);
        assert!(flow.url.contains("code_challenge_method=S256"));
        assert!(flow.url.contains(&format!("state={}", flow.state)));
        assert!(
            flow.url
                .contains(&format!("client_id={DEFAULT_WEB_CLIENT_ID}"))
        );
        assert!(flow.url.contains("8989"));
        assert!(!flow.url.contains("show_dialog=true"));
        let playback = begin(Grant::playback());
        assert!(
            playback
                .url
                .contains(&format!("client_id={PLAYBACK_CLIENT_ID}"))
        );
        assert!(playback.url.contains("8898"));
        assert!(playback.url.contains("show_dialog=true"));
    }

    #[test]
    fn playback_scopes_conform_to_spotify_streaming_contract() {
        assert_eq!(PLAYBACK_SCOPES, &["streaming"]);
        let playback = Grant::playback();
        assert_eq!(playback.scopes, vec!["streaming".to_string()]);
        let personal = Grant::personal_web_api("test-id").unwrap();
        let flow = begin(personal);
        assert!(!flow.url.contains("show_dialog=true"));
    }

    #[test]
    fn personal_client_id_is_validated_at_the_boundary() {
        assert_eq!(Grant::personal_web_api("  abc ").unwrap().client_id, "abc");
        assert!(Grant::personal_web_api("  ").is_err());
    }

    #[test]
    fn request_line_parsing() {
        let code =
            parse_request_line("GET /login?code=abc%20d&state=s1 HTTP/1.1\r\n", "s1").unwrap();
        assert_eq!(code, "abc d");
        assert!(parse_request_line("GET /login?code=abc&state=other HTTP/1.1", "s1").is_err());
        assert!(parse_request_line("GET /favicon.ico HTTP/1.1", "s1").is_err());
        assert!(
            parse_request_line("GET /login?error=access_denied&state=s1 HTTP/1.1", "s1").is_err()
        );
    }

    #[test]
    fn stored_token_round_trips_and_tracks_expiry() {
        let response = TokenResponse {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_in: Some(3600),
            scope: Some("x".into()),
        };
        let token = StoredToken::from_response("id", response, None).unwrap();
        assert!(!token.needs_refresh());
        assert!(token.has_scopes(&["x"]));
        assert!(!token.has_scopes(&["x", "y"]));
        let expired = StoredToken {
            expires_at: now_secs() + 10,
            ..token.clone()
        };
        assert!(expired.needs_refresh());
        let dir =
            crate::test_support::temp_dir().join(format!("lwspot-token-{}", std::process::id()));
        let path = dir.join("token.json");
        token.save(&path).unwrap();
        assert_eq!(StoredToken::load(&path), Some(token));
        StoredToken::remove(&path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_tokens_move_to_the_matching_session() {
        let dir = crate::test_support::temp_dir().join(format!(
            "lwspot-token-migration-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let legacy = dir.join("legacy.json");
        let shared = dir.join("shared.json");
        let personal = dir.join("personal.json");
        for (client_id, target) in [
            (DEFAULT_WEB_CLIENT_ID, &shared),
            ("personal-client", &personal),
        ] {
            let token = StoredToken {
                client_id: client_id.into(),
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                expires_at: now_secs() + 3600,
                ..StoredToken::default()
            };
            token.save(&legacy).unwrap();
            StoredToken::migrate_legacy(&legacy, &shared, &personal).unwrap();
            assert_eq!(StoredToken::load(target), Some(token));
            assert!(!legacy.exists());
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
