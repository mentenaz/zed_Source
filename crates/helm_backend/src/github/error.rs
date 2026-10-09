//! What can go wrong when asking GitHub for something, as a type.
//!
//! Errors used to be strings, and callers told them apart by looking for
//! "403" in the text. [`GhError`] says which kind of failure it was, so a
//! caller can decide what to do: ask for a missing scope, wait for the rate
//! limit, or just show the message.
//!
//! [`interpret`] is the whole rule for turning an HTTP answer into a value
//! or an error. It takes plain values, not a live response, so it is tested
//! here without a network.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GhError {
    /// `gh` could not be run, or would not hand over a token.
    Cli(String),
    /// The request never got an answer: no connection, a timeout, DNS.
    Network(String),
    /// 401. GitHub no longer accepts the token.
    Unauthorized { message: String },
    /// 403 that is not a rate limit. The token lacks a scope, or the account
    /// lacks the right, or a policy forbids it.
    Forbidden { message: String },
    /// The organisation requires this token to be authorized for SSO.
    SsoRequired {
        message: String,
        authorization_url: Option<String>,
    },
    /// 404. The thing does not exist, or the token is not allowed to see
    /// that it does: GitHub answers both the same way.
    NotFound { message: String },
    /// The repository has not had its first commit, so it has no Git tree yet.
    EmptyRepository,
    /// 403 or 429 with the rate limit used up.
    RateLimited {
        /// When the limit resets, in seconds since the Unix epoch.
        reset_at: Option<u64>,
        /// How long GitHub asked us to wait, in seconds (its secondary
        /// limits say this instead of a reset time).
        retry_after: Option<u64>,
    },
    /// 422. GitHub understood the request and rejected what was in it.
    Validation { message: String },
    /// Any other status of 400 or above.
    Status { status: u16, message: String },
    /// The answer was not the JSON expected.
    Parse(String),
    /// Something that went wrong before GitHub was asked at all, such as
    /// an action that needs a repository when none is selected.
    Other(String),
}

impl GhError {
    /// Whether this is the kind of failure a missing token scope produces.
    /// GitHub answers 403 for an insufficient scope, 404 when the scope is
    /// so insufficient the resource is not visible to the token at all, and
    /// 401 for a token it no longer accepts.
    ///
    /// A rate limit also arrives as a 403, and is not one of these: asking
    /// the user to re-authorize would not help.
    pub fn is_permission(&self) -> bool {
        matches!(
            self,
            GhError::Unauthorized { .. }
                | GhError::Forbidden { .. }
                | GhError::SsoRequired { .. }
                | GhError::NotFound { .. }
        )
    }

    /// The HTTP status behind this error, when there was one.
    pub fn status(&self) -> Option<u16> {
        match self {
            GhError::Unauthorized { .. } => Some(401),
            GhError::Forbidden { .. } => Some(403),
            GhError::SsoRequired { .. } => Some(403),
            GhError::NotFound { .. } => Some(404),
            GhError::EmptyRepository => Some(409),
            GhError::Validation { .. } => Some(422),
            GhError::Status { status, .. } => Some(*status),
            GhError::RateLimited { .. }
            | GhError::Cli(_)
            | GhError::Network(_)
            | GhError::Parse(_)
            | GhError::Other(_) => None,
        }
    }
}

pub(super) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The sentence shown for a used-up rate limit, given the time now.
pub fn rate_limit_message(reset_at: Option<u64>, retry_after: Option<u64>, now: u64) -> String {
    let wait = retry_after.or_else(|| reset_at.map(|reset| reset.saturating_sub(now)));
    match wait {
        None => "GitHub's rate limit is used up. Try again later.".to_string(),
        Some(seconds) if seconds < 60 => {
            "GitHub's rate limit is used up. It resets in under a minute.".to_string()
        }
        Some(seconds) => {
            let minutes = seconds.div_ceil(60);
            format!(
                "GitHub's rate limit is used up. It resets in about {minutes} minute{}.",
                if minutes == 1 { "" } else { "s" }
            )
        }
    }
}

impl fmt::Display for GhError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GhError::Cli(message) | GhError::Other(message) => write!(f, "{message}"),
            GhError::Network(message) => write!(f, "Network error: {message}"),
            GhError::Unauthorized { message } => write!(f, "GitHub API 401: {message}"),
            GhError::Forbidden { message } => write!(f, "GitHub API 403: {message}"),
            GhError::SsoRequired { message, .. } => {
                write!(f, "GitHub API 403 (single sign-on required): {message}")
            }
            GhError::NotFound { message } => write!(f, "GitHub API 404: {message}"),
            GhError::EmptyRepository => write!(f, "This repository is empty."),
            GhError::Validation { message } => write!(f, "GitHub API 422: {message}"),
            GhError::Status { status, message } => write!(f, "GitHub API {status}: {message}"),
            GhError::RateLimited {
                reset_at,
                retry_after,
            } => write!(
                f,
                "{}",
                rate_limit_message(*reset_at, *retry_after, unix_now())
            ),
            GhError::Parse(message) => write!(f, "Failed to parse JSON: {message}"),
        }
    }
}

impl std::error::Error for GhError {}

/// So code that still carries errors as text can use `?` on these.
impl From<GhError> for String {
    fn from(error: GhError) -> String {
        error.to_string()
    }
}

/// The parts of an HTTP answer that decide what it means.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawResponse {
    pub status: u16,
    /// Text view used for JSON responses and GitHub error messages.
    pub body: String,
    /// Exact response bytes, including non-UTF-8 file contents.
    pub body_bytes: Vec<u8>,
    /// `x-ratelimit-remaining`: requests left in the current window.
    pub rate_remaining: Option<u64>,
    /// `x-ratelimit-reset`: when the window ends, in Unix seconds.
    pub rate_reset: Option<u64>,
    /// `retry-after`, in seconds.
    pub retry_after: Option<u64>,
    /// `link`: where the next, previous and last pages of a list are.
    pub link: Option<String>,
    /// `X-GitHub-SSO`, which identifies SSO authorization failures.
    pub github_sso: Option<String>,
}

/// What GitHub said was wrong: the `message` of its JSON error body, with
/// any per-field `errors` it listed, or the raw body when it is not that
/// shape.
fn error_message(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        let trimmed = body.trim();
        return if trimmed.is_empty() {
            "<no body>".to_string()
        } else {
            trimmed.to_string()
        };
    };
    let mut message = json
        .get("message")
        .and_then(|message| message.as_str())
        .unwrap_or("<no message>")
        .to_string();
    // A 422 carries the useful part here: which field, and why.
    let details: Vec<String> = json
        .get("errors")
        .and_then(|errors| errors.as_array())
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| {
                    if let Some(text) = error.as_str() {
                        return Some(text.to_string());
                    }
                    if let Some(text) = error.get("message").and_then(|m| m.as_str()) {
                        return Some(text.to_string());
                    }
                    let field = error.get("field").and_then(|f| f.as_str())?;
                    let code = error.get("code").and_then(|c| c.as_str())?;
                    Some(format!("{field} {code}"))
                })
                .collect()
        })
        .unwrap_or_default();
    if !details.is_empty() {
        message.push_str(&format!(" ({})", details.join("; ")));
    }
    message
}

/// The error for an answer with a status of 400 or above.
fn failure(response: &RawResponse) -> GhError {
    let message = error_message(&response.body);
    // A primary limit answers 403 or 429 with nothing remaining. A secondary
    // limit answers the same statuses with `retry-after`.
    let limited = matches!(response.status, 403 | 429)
        && (response.rate_remaining == Some(0) || response.retry_after.is_some());
    if limited || response.status == 429 {
        return GhError::RateLimited {
            reset_at: response.rate_reset,
            retry_after: response.retry_after,
        };
    }
    if response.status == 403
        && let Some(header) = response.github_sso.as_deref()
        && header
            .split(';')
            .next()
            .is_some_and(|status| status.trim().eq_ignore_ascii_case("required"))
    {
        let authorization_url = header.split(';').skip(1).find_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.trim()
                .eq_ignore_ascii_case("url")
                .then(|| value.trim().trim_matches('"').to_string())
        });
        return GhError::SsoRequired {
            message,
            authorization_url,
        };
    }
    match response.status {
        401 => GhError::Unauthorized { message },
        403 => GhError::Forbidden { message },
        404 => GhError::NotFound { message },
        409 if message
            .to_ascii_lowercase()
            .contains("git repository is empty") =>
        {
            GhError::EmptyRepository
        }
        422 => GhError::Validation { message },
        status => GhError::Status { status, message },
    }
}

/// Turns an answer into the value it carries, or the error it describes.
pub fn interpret<T: DeserializeOwned>(response: &RawResponse) -> Result<T, GhError> {
    if response.status >= 400 {
        return Err(failure(response));
    }
    serde_json::from_str(&response.body).map_err(|error| GhError::Parse(error.to_string()))
}

/// [`interpret`] for a request whose answer carries nothing worth reading:
/// GitHub replies 204 with no body to most deletes and some updates, and an
/// empty body is not JSON.
pub fn interpret_empty(response: &RawResponse) -> Result<(), GhError> {
    if response.status >= 400 {
        return Err(failure(response));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, body: &str) -> RawResponse {
        RawResponse {
            status,
            body: body.to_string(),
            ..RawResponse::default()
        }
    }

    #[test]
    fn a_good_answer_is_parsed() {
        let value: Vec<u32> = interpret(&answer(200, "[1, 2, 3]")).unwrap();
        assert_eq!(value, vec![1, 2, 3]);
        let value: serde_json::Value = interpret(&answer(201, r#"{"id": 7}"#)).unwrap();
        assert_eq!(value["id"], 7);
    }

    #[test]
    fn an_answer_that_is_not_the_json_expected_is_a_parse_error() {
        let error = interpret::<Vec<u32>>(&answer(200, r#"{"not": "a list"}"#)).unwrap_err();
        assert!(matches!(error, GhError::Parse(_)), "{error:?}");
        assert!(error.to_string().starts_with("Failed to parse JSON"));
        assert_eq!(error.status(), None);
    }

    #[test]
    fn statuses_become_the_matching_error() {
        let message = r#"{"message": "Nope"}"#;
        let error = |status| interpret::<serde_json::Value>(&answer(status, message)).unwrap_err();
        assert_eq!(
            error(401),
            GhError::Unauthorized {
                message: "Nope".into()
            }
        );
        assert_eq!(
            error(403),
            GhError::Forbidden {
                message: "Nope".into()
            }
        );
        assert_eq!(
            error(404),
            GhError::NotFound {
                message: "Nope".into()
            }
        );
        assert_eq!(
            error(422),
            GhError::Validation {
                message: "Nope".into()
            }
        );
        assert_eq!(
            error(500),
            GhError::Status {
                status: 500,
                message: "Nope".into()
            }
        );
        for status in [401, 403, 404, 422, 500] {
            assert_eq!(error(status).status(), Some(status));
        }
    }

    #[test]
    fn required_sso_header_is_distinguished_from_other_forbidden_responses() {
        let mut response = answer(403, r#"{"message":"Resource not accessible"}"#);
        response.github_sso =
            Some(r#"required; url="https://github.com/orgs/example/sso?request=abc""#.into());
        let error = interpret::<serde_json::Value>(&response).unwrap_err();
        assert_eq!(
            error,
            GhError::SsoRequired {
                message: "Resource not accessible".into(),
                authorization_url: Some("https://github.com/orgs/example/sso?request=abc".into()),
            }
        );
        assert!(error.is_permission());
        assert_eq!(error.status(), Some(403));

        let mut partial = answer(403, r#"{"message":"Resource not accessible"}"#);
        partial.github_sso = Some("partial-results; organizations=example".into());
        assert_eq!(
            interpret::<serde_json::Value>(&partial).unwrap_err(),
            GhError::Forbidden {
                message: "Resource not accessible".into()
            }
        );
    }

    #[test]
    fn only_the_empty_git_repository_conflict_is_classified_as_empty() {
        let empty = answer(409, r#"{"message":"Git Repository is empty."}"#);
        assert_eq!(
            interpret::<serde_json::Value>(&empty).unwrap_err(),
            GhError::EmptyRepository
        );
        assert_eq!(GhError::EmptyRepository.status(), Some(409));
        assert!(!GhError::EmptyRepository.is_permission());

        let other_conflict = answer(409, r#"{"message":"Conflict"}"#);
        assert_eq!(
            interpret::<serde_json::Value>(&other_conflict).unwrap_err(),
            GhError::Status {
                status: 409,
                message: "Conflict".into(),
            }
        );
    }

    #[test]
    fn only_401_403_and_404_look_like_a_missing_scope() {
        let error = |status| interpret::<serde_json::Value>(&answer(status, "{}")).unwrap_err();
        assert!(error(401).is_permission());
        assert!(error(403).is_permission());
        assert!(error(404).is_permission());
        // Validation, server and transport failures are not about the token.
        assert!(!error(422).is_permission());
        assert!(!error(500).is_permission());
        assert!(!GhError::Network("timed out".into()).is_permission());
        assert!(!GhError::Cli("gh auth token failed: GitHub API 403".into()).is_permission());
    }

    #[test]
    fn a_used_up_rate_limit_is_not_a_permission_error() {
        // The same 403 status as a missing scope, told apart by the headers.
        let limited = RawResponse {
            status: 403,
            body: r#"{"message": "API rate limit exceeded"}"#.into(),
            body_bytes: br#"{"message": "API rate limit exceeded"}"#.to_vec(),
            rate_remaining: Some(0),
            rate_reset: Some(1_800_000_600),
            retry_after: None,
            link: None,
            github_sso: None,
        };
        let error = interpret::<serde_json::Value>(&limited).unwrap_err();
        assert_eq!(
            error,
            GhError::RateLimited {
                reset_at: Some(1_800_000_600),
                retry_after: None
            }
        );
        assert!(!error.is_permission());

        // With requests still left, a 403 is about permission.
        let forbidden = RawResponse {
            rate_remaining: Some(4_000),
            ..limited.clone()
        };
        assert!(
            interpret::<serde_json::Value>(&forbidden)
                .unwrap_err()
                .is_permission()
        );

        // A secondary limit says how long to wait instead.
        let secondary = RawResponse {
            status: 403,
            body: "{}".into(),
            body_bytes: b"{}".to_vec(),
            rate_remaining: Some(4_000),
            rate_reset: None,
            retry_after: Some(30),
            link: None,
            github_sso: None,
        };
        assert_eq!(
            interpret::<serde_json::Value>(&secondary).unwrap_err(),
            GhError::RateLimited {
                reset_at: None,
                retry_after: Some(30)
            }
        );
        assert!(matches!(
            interpret::<serde_json::Value>(&answer(429, "{}")).unwrap_err(),
            GhError::RateLimited { .. }
        ));
    }

    #[test]
    fn a_rate_limit_says_when_it_resets() {
        let now = 1_800_000_000;
        assert_eq!(
            rate_limit_message(Some(now + 25 * 60), None, now),
            "GitHub's rate limit is used up. It resets in about 25 minutes."
        );
        assert_eq!(
            rate_limit_message(Some(now + 61), None, now),
            "GitHub's rate limit is used up. It resets in about 2 minutes."
        );
        assert_eq!(
            rate_limit_message(Some(now + 60), None, now),
            "GitHub's rate limit is used up. It resets in about 1 minute."
        );
        assert_eq!(
            rate_limit_message(Some(now + 10), None, now),
            "GitHub's rate limit is used up. It resets in under a minute."
        );
        // A reset time already past is "under a minute", not a negative wait.
        assert_eq!(
            rate_limit_message(Some(now - 500), None, now),
            "GitHub's rate limit is used up. It resets in under a minute."
        );
        // `retry-after` wins: it is what GitHub asked for.
        assert_eq!(
            rate_limit_message(Some(now + 3_600), Some(120), now),
            "GitHub's rate limit is used up. It resets in about 2 minutes."
        );
        assert_eq!(
            rate_limit_message(None, None, now),
            "GitHub's rate limit is used up. Try again later."
        );
    }

    #[test]
    fn the_message_is_githubs_own_with_its_field_errors() {
        let body = r#"{
            "message": "Validation Failed",
            "errors": [
                { "resource": "Repository", "field": "name", "code": "already_exists" },
                { "message": "name is too long" },
                "something else"
            ]
        }"#;
        assert_eq!(
            error_message(body),
            "Validation Failed (name already_exists; name is too long; something else)"
        );
        assert_eq!(error_message(r#"{"message": "Not Found"}"#), "Not Found");
        assert_eq!(error_message("{}"), "<no message>");
        // Not JSON at all: a proxy's error page, say.
        assert_eq!(error_message("  Bad Gateway \n"), "Bad Gateway");
        assert_eq!(error_message(""), "<no body>");
    }

    #[test]
    fn an_empty_answer_is_fine_where_nothing_is_expected() {
        assert_eq!(interpret_empty(&answer(204, "")), Ok(()));
        assert_eq!(
            interpret_empty(&answer(200, r#"{"ignored": true}"#)),
            Ok(())
        );
        // The same empty body is an error where a value was expected...
        assert!(matches!(
            interpret::<serde_json::Value>(&answer(204, "")),
            Err(GhError::Parse(_))
        ));
        // ...and a failure is still a failure.
        assert_eq!(
            interpret_empty(&answer(404, r#"{"message": "Not Found"}"#)),
            Err(GhError::NotFound {
                message: "Not Found".into()
            })
        );
    }

    #[test]
    fn errors_read_the_way_they_used_to() {
        // Code that still matches on the text keeps working.
        assert_eq!(
            GhError::Forbidden {
                message: "Forbidden".into()
            }
            .to_string(),
            "GitHub API 403: Forbidden"
        );
        assert_eq!(
            GhError::Network("timed out".into()).to_string(),
            "Network error: timed out"
        );
        let text: String = GhError::NotFound {
            message: "Not Found".into(),
        }
        .into();
        assert_eq!(text, "GitHub API 404: Not Found");
    }
}
