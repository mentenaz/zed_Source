//! Remembering answers, and how much of the rate limit is left.
//!
//! GitHub allows a signed-in user 5,000 requests an hour. Two things here
//! make that go further:
//!
//! - **Conditional requests.** Every answer to a `GET` comes with an `ETag`,
//!   a tag for that version of the data. Sending it back as `If-None-Match`
//!   gets a `304 Not Modified` with no body when nothing changed, and a 304
//!   does not count against the limit. [`ResponseCache`] keeps the tag and
//!   the body it belongs to.
//! - **Knowing what is left.** Every answer says how many requests remain
//!   and when the count resets. [`RateLimit`] is that, as a value.
//!
//! Both are plain data with no network in them, so they are tested here.

use std::collections::HashMap;

use super::requests::ApiResponseFormat;

/// How many answers are remembered. When one more arrives, the one used
/// longest ago is dropped.
pub const MAX_CACHED_RESPONSES: usize = 200;
/// Keep the in-memory cache from ballooning on large tree/blob answers.
pub const MAX_CACHED_BYTES: usize = 32 * 1024 * 1024;

/// A remembered answer to a `GET`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedResponse {
    pub etag: String,
    pub body: Vec<u8>,
    /// The `Link` header that came with it, for paged lists.
    pub link: Option<String>,
}

fn cache_key(path: &str, accept: ApiResponseFormat) -> String {
    format!("{path}\0{:?}", accept)
}

#[derive(Default)]
pub struct ResponseCache {
    entries: HashMap<String, (CachedResponse, u64)>,
    /// Tracks the number of bytes held in memory so huge tree/blob payloads do
    /// not exhaust the app with a handful of cached answers.
    total_bytes: usize,
    /// Counts uses, so the entry used longest ago can be found.
    clock: u64,
}

impl ResponseCache {
    /// The remembered answer for `path` and `accept`, marking it as just used.
    pub fn get(&mut self, path: &str, accept: ApiResponseFormat) -> Option<CachedResponse> {
        self.clock += 1;
        let clock = self.clock;
        let key = cache_key(path, accept);
        let (response, used) = self.entries.get_mut(&key)?;
        *used = clock;
        Some(response.clone())
    }

    /// The tag to send as `If-None-Match` for a response format.
    pub fn etag(&self, path: &str, accept: ApiResponseFormat) -> Option<String> {
        let key = cache_key(path, accept);
        self.entries
            .get(&key)
            .map(|(response, _)| response.etag.clone())
    }

    pub fn put(&mut self, path: &str, accept: ApiResponseFormat, response: CachedResponse) {
        let key = cache_key(path, accept);
        self.clock += 1;
        self.entries.insert(key, (response, self.clock));
        self.total_bytes = self
            .entries
            .values()
            .map(|(response, _)| response_size(response))
            .sum();

        while self.entries.len() > MAX_CACHED_RESPONSES || self.total_bytes > MAX_CACHED_BYTES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(path, _)| path.clone());
            match oldest {
                Some(oldest_key) => {
                    self.entries.remove(&oldest_key);
                    self.total_bytes = self
                        .entries
                        .values()
                        .map(|(response, _)| response_size(response))
                        .sum();
                }
                None => break,
            }
        }
    }

    /// Forgets everything. Called after any change is sent to GitHub, and
    /// when the signed-in account changes: a remembered answer may no longer
    /// be true, or may belong to someone else.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn response_size(response: &CachedResponse) -> usize {
    response.etag.len() + response.body.len() + response.link.as_deref().map_or(0, str::len)
}

/// How much of GitHub's request allowance is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    /// Requests allowed per window (5,000 an hour when signed in).
    pub limit: u64,
    pub remaining: u64,
    /// When the window ends, in seconds since the Unix epoch.
    pub reset_at: u64,
}

/// Rate-limit snapshots and secondary-limit cooldowns, keyed by the endpoint
/// family selected by the request builder.
#[derive(Default)]
pub struct RateLimitTracker {
    limits: HashMap<String, RateLimit>,
    retry_at: HashMap<String, u64>,
}

impl RateLimitTracker {
    pub fn record(
        &mut self,
        resource: &str,
        limit: Option<RateLimit>,
        retry_after: Option<u64>,
        now: u64,
    ) {
        if let Some(limit) = limit {
            self.limits.insert(resource.to_string(), limit);
        }
        if let Some(retry_after) = retry_after {
            self.retry_at
                .insert(resource.to_string(), now.saturating_add(retry_after));
        }
    }

    pub fn get(&self, resource: &str) -> Option<RateLimit> {
        self.limits.get(resource).copied()
    }

    pub fn retry_at(&self, resource: &str) -> Option<u64> {
        self.retry_at.get(resource).copied()
    }

    pub fn clear(&mut self) {
        self.limits.clear();
        self.retry_at.clear();
    }
}

impl RateLimit {
    /// From the headers of an answer. `None` when the answer did not carry
    /// the complete allowance; the caller keeps it under the request's
    /// resource key.
    pub fn from_headers(
        limit: Option<u64>,
        remaining: Option<u64>,
        reset_at: Option<u64>,
    ) -> Option<RateLimit> {
        Some(RateLimit {
            limit: limit?,
            remaining: remaining?,
            reset_at: reset_at?,
        })
    }

    /// Whether less than a tenth is left: worth drawing attention to.
    pub fn is_low(&self) -> bool {
        self.remaining.saturating_mul(10) < self.limit
    }

    /// Whether nothing is left and the reset has not come yet. A request
    /// sent now would be refused.
    pub fn is_used_up(&self, now: u64) -> bool {
        self.remaining == 0 && now < self.reset_at
    }

    /// Minutes until the allowance resets, rounded up. Zero once it has.
    pub fn minutes_to_reset(&self, now: u64) -> u64 {
        self.reset_at.saturating_sub(now).div_ceil(60)
    }

    /// One line for a status area: "4,812 of 5,000 requests left · resets
    /// in 23 min".
    pub fn summary(&self, now: u64) -> String {
        let minutes = self.minutes_to_reset(now);
        format!(
            "{} of {} requests left · resets in {} min",
            thousands(self.remaining),
            thousands(self.limit),
            minutes
        )
    }
}

/// `4812` as `4,812`.
fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(etag: &str, body: &str) -> CachedResponse {
        CachedResponse {
            etag: etag.to_string(),
            body: body.as_bytes().to_vec(),
            link: None,
        }
    }

    #[test]
    fn an_answer_is_remembered_with_its_tag() {
        let mut cache = ResponseCache::default();
        assert_eq!(cache.etag("/user", ApiResponseFormat::Json), None);
        assert_eq!(cache.get("/user", ApiResponseFormat::Json), None);

        cache.put("/user", ApiResponseFormat::Json, response("\"abc\"", "{}"));
        assert_eq!(
            cache.etag("/user", ApiResponseFormat::Json).as_deref(),
            Some("\"abc\"")
        );
        assert_eq!(
            cache.get("/user", ApiResponseFormat::Json),
            Some(response("\"abc\"", "{}"))
        );

        // A newer answer replaces the old one.
        cache.put(
            "/user",
            ApiResponseFormat::Json,
            response("\"def\"", "{\"x\":1}"),
        );
        assert_eq!(
            cache.etag("/user", ApiResponseFormat::Json).as_deref(),
            Some("\"def\"")
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn various_accept_formats_are_cached_separately() {
        let mut cache = ResponseCache::default();
        cache.put(
            "/repos/o/r/readme",
            ApiResponseFormat::Json,
            response("\"json\"", "{}"),
        );
        cache.put(
            "/repos/o/r/readme",
            ApiResponseFormat::Raw,
            response("\"raw\"", "README"),
        );
        assert_eq!(cache.len(), 2);
        assert_eq!(
            cache.get("/repos/o/r/readme", ApiResponseFormat::Json),
            Some(response("\"json\"", "{}"))
        );
        assert_eq!(
            cache.get("/repos/o/r/readme", ApiResponseFormat::Raw),
            Some(response("\"raw\"", "README"))
        );
    }

    #[test]
    fn raw_cache_entries_preserve_binary_bytes() {
        let mut cache = ResponseCache::default();
        let bytes = vec![0x00, 0xFF, 0x80, b'\n'];
        cache.put(
            "/repos/o/r/git/blobs/sha",
            ApiResponseFormat::Raw,
            CachedResponse {
                etag: "\"binary\"".into(),
                body: bytes.clone(),
                link: None,
            },
        );
        assert_eq!(
            cache
                .get("/repos/o/r/git/blobs/sha", ApiResponseFormat::Raw)
                .unwrap()
                .body,
            bytes
        );
    }

    #[test]
    fn pages_and_filters_are_separate_answers() {
        let mut cache = ResponseCache::default();
        cache.put(
            "/repos/o/r/issues?state=open&per_page=10&page=1",
            ApiResponseFormat::Json,
            response("\"p1\"", "[1]"),
        );
        cache.put(
            "/repos/o/r/issues?state=open&per_page=10&page=2",
            ApiResponseFormat::Json,
            response("\"p2\"", "[2]"),
        );
        cache.put(
            "/repos/o/r/issues?state=closed&per_page=10&page=1",
            ApiResponseFormat::Json,
            response("\"c1\"", "[3]"),
        );
        assert_eq!(cache.len(), 3);
        assert_eq!(
            cache
                .get(
                    "/repos/o/r/issues?state=open&per_page=10&page=2",
                    ApiResponseFormat::Json
                )
                .map(|r| r.body),
            Some(b"[2]".to_vec())
        );
    }

    #[test]
    fn the_answer_used_longest_ago_makes_room() {
        let mut cache = ResponseCache::default();
        for n in 0..MAX_CACHED_RESPONSES {
            cache.put(
                &format!("/item/{n}"),
                ApiResponseFormat::Json,
                response("\"t\"", ""),
            );
        }
        assert_eq!(cache.len(), MAX_CACHED_RESPONSES);

        // Using the oldest saves it; the next oldest goes in its place.
        assert!(cache.get("/item/0", ApiResponseFormat::Json).is_some());
        cache.put("/item/new", ApiResponseFormat::Json, response("\"t\"", ""));
        assert_eq!(cache.len(), MAX_CACHED_RESPONSES);
        assert!(cache.etag("/item/0", ApiResponseFormat::Json).is_some());
        assert!(cache.etag("/item/1", ApiResponseFormat::Json).is_none());
        assert!(cache.etag("/item/new", ApiResponseFormat::Json).is_some());
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut cache = ResponseCache::default();
        cache.put("/user", ApiResponseFormat::Json, response("\"abc\"", "{}"));
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.etag("/user", ApiResponseFormat::Json), None);
    }

    #[test]
    fn the_cache_enforces_a_byte_limit() {
        let mut cache = ResponseCache::default();
        let large = "x".repeat(MAX_CACHED_BYTES + 1024);
        cache.put(
            "/blob/huge",
            ApiResponseFormat::Raw,
            response("\"a\"", &large),
        );
        assert!(cache.len() <= MAX_CACHED_RESPONSES);
        assert!(cache.total_bytes <= MAX_CACHED_BYTES);
    }

    #[test]
    fn rate_limit_resources_keep_independent_allowances_and_cooldowns() {
        let mut tracker = RateLimitTracker::default();
        let core = RateLimit {
            limit: 5000,
            remaining: 0,
            reset_at: 200,
        };
        let search = RateLimit {
            limit: 30,
            remaining: 0,
            reset_at: 150,
        };
        let code_search = RateLimit {
            limit: 10,
            remaining: 7,
            reset_at: 160,
        };
        tracker.record("core", Some(core), None, 100);
        tracker.record("search", Some(search), Some(30), 100);
        tracker.record("code_search", Some(code_search), None, 100);

        assert_eq!(tracker.get("core"), Some(core));
        assert_eq!(tracker.get("search"), Some(search));
        assert_eq!(tracker.get("code_search"), Some(code_search));
        assert_eq!(tracker.retry_at("search"), Some(130));
        assert_eq!(tracker.retry_at("code_search"), None);

        tracker.clear();
        assert_eq!(tracker.get("core"), None);
        assert_eq!(tracker.retry_at("search"), None);
    }

    #[test]
    fn the_rate_limit_is_read_from_an_answers_headers() {
        let limit = RateLimit::from_headers(Some(5000), Some(4812), Some(1_800_001_380));
        assert_eq!(
            limit,
            Some(RateLimit {
                limit: 5000,
                remaining: 4812,
                reset_at: 1_800_001_380
            })
        );
        assert_eq!(
            RateLimit::from_headers(Some(5000), Some(4812), Some(1)),
            Some(RateLimit {
                limit: 5000,
                remaining: 4812,
                reset_at: 1
            })
        );
        assert_eq!(
            RateLimit::from_headers(Some(30), Some(29), Some(1)),
            Some(RateLimit {
                limit: 30,
                remaining: 29,
                reset_at: 1
            })
        );
        // An answer without the headers says nothing.
        assert_eq!(RateLimit::from_headers(None, Some(1), Some(1)), None);
        assert_eq!(RateLimit::from_headers(Some(1), None, Some(1)), None);
    }

    #[test]
    fn the_rate_limit_reads_as_one_line() {
        let now = 1_800_000_000;
        let limit = RateLimit {
            limit: 5000,
            remaining: 4812,
            reset_at: now + 23 * 60,
        };
        assert_eq!(
            limit.summary(now),
            "4,812 of 5,000 requests left · resets in 23 min"
        );
        assert!(!limit.is_low());

        let nearly_out = RateLimit {
            limit: 5000,
            remaining: 499,
            reset_at: now + 61,
        };
        assert!(nearly_out.is_low());
        assert_eq!(nearly_out.minutes_to_reset(now), 2);
        // A reset time already past is zero minutes, not a negative wait.
        assert_eq!(nearly_out.minutes_to_reset(now + 3_600), 0);
        assert_eq!(
            RateLimit {
                limit: 60,
                remaining: 0,
                reset_at: now
            }
            .summary(now),
            "0 of 60 requests left · resets in 0 min"
        );
    }

    #[test]
    fn a_used_up_allowance_stays_used_up_until_it_resets() {
        let now = 1_800_000_000;
        let out = RateLimit {
            limit: 5000,
            remaining: 0,
            reset_at: now + 600,
        };
        assert!(out.is_used_up(now));
        assert!(out.is_used_up(now + 599));
        // At the reset the old count no longer says anything.
        assert!(!out.is_used_up(now + 600));
        assert!(
            !RateLimit {
                remaining: 1,
                ..out
            }
            .is_used_up(now)
        );
    }

    #[test]
    fn numbers_are_grouped_in_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(12345), "12,345");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
