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

/// How many answers are remembered. When one more arrives, the one used
/// longest ago is dropped.
pub const MAX_CACHED_RESPONSES: usize = 200;

/// A remembered answer to a `GET`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedResponse {
    pub etag: String,
    pub body: String,
    /// The `Link` header that came with it, for paged lists.
    pub link: Option<String>,
}

#[derive(Default)]
pub struct ResponseCache {
    entries: HashMap<String, (CachedResponse, u64)>,
    /// Counts uses, so the entry used longest ago can be found.
    clock: u64,
}

impl ResponseCache {
    /// The remembered answer for `path`, marking it as just used.
    pub fn get(&mut self, path: &str) -> Option<CachedResponse> {
        self.clock += 1;
        let clock = self.clock;
        let (response, used) = self.entries.get_mut(path)?;
        *used = clock;
        Some(response.clone())
    }

    /// The tag to send as `If-None-Match` for `path`.
    pub fn etag(&self, path: &str) -> Option<String> {
        self.entries
            .get(path)
            .map(|(response, _)| response.etag.clone())
    }

    pub fn put(&mut self, path: &str, response: CachedResponse) {
        self.clock += 1;
        self.entries
            .insert(path.to_string(), (response, self.clock));
        while self.entries.len() > MAX_CACHED_RESPONSES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(path, _)| path.clone());
            match oldest {
                Some(path) => self.entries.remove(&path),
                None => break,
            };
        }
    }

    /// Forgets everything. Called after any change is sent to GitHub, and
    /// when the signed-in account changes: a remembered answer may no longer
    /// be true, or may belong to someone else.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
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

impl RateLimit {
    /// From the headers of an answer. `None` when the answer did not carry
    /// them, or they describe a different allowance than the main one
    /// (search, for one, has its own much smaller limit).
    pub fn from_headers(
        limit: Option<u64>,
        remaining: Option<u64>,
        reset_at: Option<u64>,
        resource: Option<&str>,
    ) -> Option<RateLimit> {
        if resource.is_some_and(|resource| resource != "core") {
            return None;
        }
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
            body: body.to_string(),
            link: None,
        }
    }

    #[test]
    fn an_answer_is_remembered_with_its_tag() {
        let mut cache = ResponseCache::default();
        assert_eq!(cache.etag("/user"), None);
        assert_eq!(cache.get("/user"), None);

        cache.put("/user", response("\"abc\"", "{}"));
        assert_eq!(cache.etag("/user").as_deref(), Some("\"abc\""));
        assert_eq!(cache.get("/user"), Some(response("\"abc\"", "{}")));

        // A newer answer replaces the old one.
        cache.put("/user", response("\"def\"", "{\"x\":1}"));
        assert_eq!(cache.etag("/user").as_deref(), Some("\"def\""));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn pages_and_filters_are_separate_answers() {
        let mut cache = ResponseCache::default();
        cache.put("/repos/o/r/issues?state=open&per_page=10&page=1", response("\"p1\"", "[1]"));
        cache.put("/repos/o/r/issues?state=open&per_page=10&page=2", response("\"p2\"", "[2]"));
        cache.put("/repos/o/r/issues?state=closed&per_page=10&page=1", response("\"c1\"", "[3]"));
        assert_eq!(cache.len(), 3);
        assert_eq!(
            cache.get("/repos/o/r/issues?state=open&per_page=10&page=2").map(|r| r.body),
            Some("[2]".to_string())
        );
    }

    #[test]
    fn the_answer_used_longest_ago_makes_room() {
        let mut cache = ResponseCache::default();
        for n in 0..MAX_CACHED_RESPONSES {
            cache.put(&format!("/item/{n}"), response("\"t\"", ""));
        }
        assert_eq!(cache.len(), MAX_CACHED_RESPONSES);

        // Using the oldest saves it; the next oldest goes in its place.
        assert!(cache.get("/item/0").is_some());
        cache.put("/item/new", response("\"t\"", ""));
        assert_eq!(cache.len(), MAX_CACHED_RESPONSES);
        assert!(cache.etag("/item/0").is_some());
        assert!(cache.etag("/item/1").is_none());
        assert!(cache.etag("/item/new").is_some());
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut cache = ResponseCache::default();
        cache.put("/user", response("\"abc\"", "{}"));
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.etag("/user"), None);
    }

    #[test]
    fn the_rate_limit_is_read_from_an_answers_headers() {
        let limit = RateLimit::from_headers(Some(5000), Some(4812), Some(1_800_001_380), None);
        assert_eq!(
            limit,
            Some(RateLimit {
                limit: 5000,
                remaining: 4812,
                reset_at: 1_800_001_380
            })
        );
        assert_eq!(
            RateLimit::from_headers(Some(5000), Some(4812), Some(1), Some("core")),
            Some(RateLimit {
                limit: 5000,
                remaining: 4812,
                reset_at: 1
            })
        );
        // Search has its own allowance; it must not be shown as the main one.
        assert_eq!(RateLimit::from_headers(Some(30), Some(29), Some(1), Some("search")), None);
        // An answer without the headers says nothing.
        assert_eq!(RateLimit::from_headers(None, Some(1), Some(1), None), None);
        assert_eq!(RateLimit::from_headers(Some(1), None, Some(1), None), None);
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
