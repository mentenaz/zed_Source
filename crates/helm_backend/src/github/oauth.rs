//! Signing in to GitHub with the OAuth device flow, through Helm's own OAuth
//! App, so no `gh` install is needed.
//!
//! The flow is two requests: [`request_device_code`] gets a one-time code
//! for the user to enter on GitHub, and [`poll_for_token`] is asked
//! repeatedly (every [`DeviceFlow::interval`] seconds) until the user has
//! entered it. The host does the waiting between polls, so dropping its task
//! cancels the sign-in.
//!
//! Both requests go through the host's HTTP client, so they use the same
//! proxy settings as everything else.

use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient, Method, Request};
use serde::Deserialize;

use super::error::GhError;

/// The client ID of the `Helm_GPUI` OAuth App. Public: the device flow
/// uses no client secret.
pub const HELM_OAUTH_CLIENT_ID: &str = "Ov23li4HBhs95CCtVsmP";

/// Helm's page in the user's GitHub settings: what it may access, which
/// organisations have approved it, and where to request approval from one
/// that has not, or revoke Helm's access.
pub fn helm_app_settings_url() -> String {
    format!("https://github.com/settings/connections/applications/{HELM_OAUTH_CLIENT_ID}")
}

/// The scopes a first sign-in asks for.
pub const HELM_OAUTH_SCOPES: &[&str] = &["repo", "read:org", "user"];

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// A sign-in waiting for the user to enter [`Self::user_code`] at
/// [`Self::verification_uri`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceFlow {
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until the code stops working.
    pub expires_in: u64,
    /// Seconds to wait between polls. [`PollOutcome::SlowDown`] raises it.
    pub interval: u64,
    device_code: String,
}

/// What one poll found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollOutcome {
    /// The user has not entered the code yet.
    Pending,
    /// Polling too often: wait this many seconds between polls from now on.
    SlowDown { interval: u64 },
    /// Signed in.
    Token(String),
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Starts a sign-in asking for `scopes`.
pub async fn request_device_code(
    http: &dyn HttpClient,
    scopes: &[&str],
) -> Result<DeviceFlow, GhError> {
    let scope = scopes.join(" ");
    let body = post_form(
        http,
        DEVICE_CODE_URL,
        &[("client_id", HELM_OAUTH_CLIENT_ID), ("scope", &scope)],
    )
    .await?;
    let parsed: DeviceCodeResponse = serde_json::from_slice(&body)
        .map_err(|e| GhError::Parse(format!("GitHub's device-code answer: {e}")))?;
    if let Some(error) = parsed.error {
        return Err(oauth_error(&error, parsed.error_description));
    }
    let (Some(device_code), Some(user_code), Some(verification_uri)) = (
        parsed.device_code,
        parsed.user_code,
        parsed.verification_uri,
    ) else {
        return Err(GhError::Parse(
            "GitHub's device-code answer is missing the code".into(),
        ));
    };
    Ok(DeviceFlow {
        user_code,
        verification_uri,
        expires_in: parsed.expires_in.unwrap_or(900),
        interval: parsed.interval.unwrap_or(5).max(1),
        device_code,
    })
}

/// Asks once whether the user has entered the code.
pub async fn poll_for_token(
    http: &dyn HttpClient,
    flow: &DeviceFlow,
) -> Result<PollOutcome, GhError> {
    let body = post_form(
        http,
        ACCESS_TOKEN_URL,
        &[
            ("client_id", HELM_OAUTH_CLIENT_ID),
            ("device_code", &flow.device_code),
            ("grant_type", DEVICE_CODE_GRANT_TYPE),
        ],
    )
    .await?;
    let parsed: AccessTokenResponse = serde_json::from_slice(&body)
        .map_err(|e| GhError::Parse(format!("GitHub's access-token answer: {e}")))?;
    if let Some(token) = parsed.access_token {
        return Ok(PollOutcome::Token(token));
    }
    match parsed.error.as_deref() {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        // GitHub says how long to wait now; without that, 5 seconds more.
        Some("slow_down") => Ok(PollOutcome::SlowDown {
            interval: parsed.interval.unwrap_or(flow.interval + 5),
        }),
        Some(error) => Err(oauth_error(error, parsed.error_description)),
        None => Err(GhError::Parse(
            "GitHub's access-token answer has neither a token nor an error".into(),
        )),
    }
}

/// A device-flow error code as a message for the sign-in screen.
fn oauth_error(code: &str, description: Option<String>) -> GhError {
    let message = match code {
        "expired_token" => "The sign-in code expired. Start the sign-in again.".to_string(),
        "access_denied" => "The sign-in was cancelled on GitHub.".to_string(),
        "device_flow_disabled" => "Device sign-in is turned off for Helm's GitHub app.".to_string(),
        "incorrect_client_credentials" | "unsupported_grant_type" | "incorrect_device_code" => {
            format!("GitHub refused the sign-in ({code}).")
        }
        _ => match description {
            Some(description) => format!("GitHub sign-in failed: {description}"),
            None => format!("GitHub sign-in failed ({code})."),
        },
    };
    GhError::Other(message)
}

/// Posts `fields` as a form and returns the body of a successful answer.
/// GitHub answers device-flow errors with 200 and an `error` field, so only
/// a transport failure or another status is an error here.
async fn post_form(
    http: &dyn HttpClient,
    url: &str,
    fields: &[(&str, &str)],
) -> Result<Vec<u8>, GhError> {
    let form = fields
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                urlencoding::encode(key),
                urlencoding::encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let request = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", "mentenaz-forge")
        .body(AsyncBody::from(form))
        .map_err(|e| GhError::Other(format!("Could not build the request to {url}: {e}")))?;
    let mut response = http
        .send(request)
        .await
        .map_err(|e| GhError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .map_err(|e| GhError::Network(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(GhError::Status {
            status,
            message: String::from_utf8_lossy(&body).into_owned(),
        });
    }
    Ok(body)
}
