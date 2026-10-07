//! Finding crates on crates.io, and where a crate's README is.
//!
//! Search goes through the crates.io Web API, not the sparse index, which
//! can only be asked about a name already known. As everywhere else in this
//! crate, the URLs are built and the answers parsed here; the requests are
//! the host's to make. Two things the host must get right, both confirmed
//! against the live API:
//!
//! - A request without a `User-Agent` is answered with HTTP 403.
//! - The README request is answered with a redirect to another host
//!   (`static.crates.io`), so the client has to follow redirects. The body
//!   is rendered HTML, not Markdown.

use serde::Deserialize;

use crate::actions::{check_crate_name, check_exact_version};

/// The crates.io endpoint that lists and searches crates.
pub const SEARCH_BASE_URL: &str = "https://crates.io/api/v1/crates";
/// Results asked for per request.
pub const SEARCH_PAGE_SIZE: usize = 20;

/// The URL of the first page of results for `query`.
pub fn search_url(query: &str) -> String {
    format!(
        "{SEARCH_BASE_URL}?q={}&per_page={SEARCH_PAGE_SIZE}",
        urlencoding::encode(query.trim())
    )
}

/// The URL of the page after one whose answer carried `next_page`.
///
/// crates.io pages with an opaque `seek` token and hands back the whole
/// query string to use (`?q=serde&per_page=20&seek=…`), so that string is
/// appended as it is. `None` if it isn't a query string, since it comes
/// from the network and ends up in a URL.
pub fn next_search_url(next_page: &str) -> Option<String> {
    let plain = next_page.starts_with('?')
        && next_page
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '#' | '/' | '\\' | '@'));
    plain.then(|| format!("{SEARCH_BASE_URL}{next_page}"))
}

/// One crate in a page of search results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub name: String,
    /// The newest stable version, or the newest of any kind for a crate that
    /// has only published pre-releases.
    pub version: String,
    pub description: Option<String>,
    /// All-time downloads.
    pub downloads: u64,
    /// Downloads over the last 90 days, when the API gives them.
    pub recent_downloads: Option<u64>,
    /// The crate's name is exactly what was searched for.
    pub exact_match: bool,
    pub repository: Option<String>,
    pub documentation: Option<String>,
}

/// One page of search results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPage {
    pub results: Vec<SearchResult>,
    /// How many crates match in all, across every page.
    pub total: usize,
    /// Pass to [`next_search_url`] for the next page. `None` on the last.
    pub next_page: Option<String>,
}

#[derive(Deserialize)]
struct RawSearch {
    #[serde(default)]
    crates: Vec<RawCrate>,
    #[serde(default)]
    meta: RawMeta,
}

#[derive(Deserialize, Default)]
struct RawMeta {
    #[serde(default)]
    total: usize,
    #[serde(default)]
    next_page: Option<String>,
}

#[derive(Deserialize)]
struct RawCrate {
    name: String,
    #[serde(default)]
    max_stable_version: Option<String>,
    #[serde(default)]
    max_version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    recent_downloads: Option<u64>,
    #[serde(default)]
    exact_match: bool,
    /// Every version of the crate has been yanked.
    #[serde(default)]
    yanked: bool,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    documentation: Option<String>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Parses one page of the search API's answer.
///
/// A crate with every version yanked, or with no version at all, is left
/// out: there is nothing to add.
pub fn parse_search(json: &str) -> Result<SearchPage, String> {
    let raw: RawSearch = serde_json::from_str(json)
        .map_err(|error| format!("Could not read the crates.io answer: {error}"))?;
    let results = raw
        .crates
        .into_iter()
        .filter(|krate| !krate.yanked)
        .filter_map(|krate| {
            // crates.io reports "no version" as `0.0.0`.
            let version = non_empty(krate.max_stable_version)
                .or_else(|| non_empty(krate.max_version))
                .filter(|version| version != "0.0.0")?;
            Some(SearchResult {
                name: krate.name,
                version,
                description: non_empty(krate.description),
                downloads: krate.downloads,
                recent_downloads: krate.recent_downloads,
                exact_match: krate.exact_match,
                repository: non_empty(krate.repository),
                documentation: non_empty(krate.documentation),
            })
        })
        .collect();
    Ok(SearchPage {
        results,
        total: raw.meta.total,
        next_page: non_empty(raw.meta.next_page),
    })
}

/// Where the README of `name` at `version` is. Refuses a name or version
/// that isn't one, since both are placed in the URL.
pub fn readme_url(name: &str, version: &str) -> Result<String, String> {
    check_crate_name(name)?;
    check_exact_version(version)?;
    Ok(format!("{SEARCH_BASE_URL}/{name}/{version}/readme"))
}

/// A count shortened for a list: `1.4B`, `345M`, `12K`, `870`.
pub fn fmt_count(count: u64) -> String {
    let scaled = |divisor: u64, suffix: &str| {
        let tenths = count / (divisor / 10);
        if tenths >= 100 {
            format!("{}{suffix}", tenths / 10)
        } else {
            format!("{}.{}{suffix}", tenths / 10, tenths % 10)
        }
    };
    match count {
        1_000_000_000.. => scaled(1_000_000_000, "B"),
        1_000_000.. => scaled(1_000_000, "M"),
        1_000.. => scaled(1_000, "K"),
        _ => count.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_urls_encode_the_query() {
        assert_eq!(
            search_url("serde json"),
            "https://crates.io/api/v1/crates?q=serde%20json&per_page=20"
        );
        assert_eq!(
            search_url("  a&b=c#d  "),
            "https://crates.io/api/v1/crates?q=a%26b%3Dc%23d&per_page=20"
        );
    }

    #[test]
    fn the_next_page_is_the_query_string_crates_io_handed_back() {
        assert_eq!(
            next_search_url("?q=serde+json&per_page=2&seek=W2ZhbHNlLDUuMDgw").as_deref(),
            Some("https://crates.io/api/v1/crates?q=serde+json&per_page=2&seek=W2ZhbHNlLDUuMDgw")
        );
        // Anything that could point the request somewhere else is refused.
        assert_eq!(next_search_url("https://example.test/?q=x"), None);
        assert_eq!(next_search_url("?q=x@example.test/"), None);
        assert_eq!(next_search_url("?q=x y"), None);
        assert_eq!(next_search_url("q=x"), None);
        assert_eq!(next_search_url(""), None);
    }

    fn sample() -> String {
        // The shape of a real answer, trimmed to what is read plus a few
        // fields that are not.
        json!({
            "crates": [
                {
                    "id": "serde_json",
                    "name": "serde_json",
                    "downloads": 1394763516u64,
                    "recent_downloads": 345158046,
                    "max_version": "2.0.0-rc.1",
                    "max_stable_version": "1.0.151",
                    "newest_version": "1.0.151",
                    "description": "A JSON serialization file format\n",
                    "homepage": null,
                    "documentation": "https://docs.rs/serde_json",
                    "repository": "https://github.com/serde-rs/json",
                    "exact_match": true,
                    "yanked": false,
                    "badges": [],
                    "links": { "owners": "/api/v1/crates/serde_json/owners" }
                },
                {
                    "name": "only-prereleases",
                    "downloads": 12,
                    "recent_downloads": null,
                    "max_version": "0.1.0-alpha.2",
                    "max_stable_version": null,
                    "description": "",
                    "repository": "  "
                },
                { "name": "all-yanked", "max_version": "1.0.0", "yanked": true },
                { "name": "no-versions", "max_version": "0.0.0" }
            ],
            "meta": {
                "total": 13946,
                "next_page": "?q=serde+json&per_page=2&seek=abc",
                "prev_page": null
            }
        })
        .to_string()
    }

    #[test]
    fn search_results_are_parsed() {
        let page = parse_search(&sample()).unwrap();
        assert_eq!(page.total, 13946);
        assert_eq!(page.next_page.as_deref(), Some("?q=serde+json&per_page=2&seek=abc"));

        let names: Vec<&str> = page.results.iter().map(|result| result.name.as_str()).collect();
        assert_eq!(names, vec!["serde_json", "only-prereleases"]);

        let serde_json = &page.results[0];
        // The stable version, not the newer pre-release.
        assert_eq!(serde_json.version, "1.0.151");
        assert_eq!(serde_json.description.as_deref(), Some("A JSON serialization file format"));
        assert_eq!(serde_json.downloads, 1_394_763_516);
        assert_eq!(serde_json.recent_downloads, Some(345_158_046));
        assert!(serde_json.exact_match);
        assert_eq!(serde_json.repository.as_deref(), Some("https://github.com/serde-rs/json"));

        let prerelease = &page.results[1];
        assert_eq!(prerelease.version, "0.1.0-alpha.2");
        // Empty and blank strings are "none", not something to show.
        assert_eq!(prerelease.description, None);
        assert_eq!(prerelease.repository, None);
        assert_eq!(prerelease.recent_downloads, None);
        assert!(!prerelease.exact_match);
    }

    #[test]
    fn the_last_page_and_an_empty_answer() {
        let last = parse_search(r#"{"crates":[],"meta":{"total":3,"next_page":null}}"#).unwrap();
        assert!(last.results.is_empty());
        assert_eq!(last.total, 3);
        assert_eq!(last.next_page, None);

        assert_eq!(parse_search("{}").unwrap(), SearchPage::default());
        assert!(parse_search("<html>rate limited</html>").is_err());
    }

    #[test]
    fn readme_urls_refuse_anything_that_is_not_a_name_and_version() {
        assert_eq!(
            readme_url("itoa", "1.0.15").as_deref(),
            Ok("https://crates.io/api/v1/crates/itoa/1.0.15/readme")
        );
        assert!(readme_url("../../etc", "1.0.0").is_err());
        assert!(readme_url("itoa", "1.0.0/../x").is_err());
        assert!(readme_url("itoa?x=1", "1.0.0").is_err());
        assert!(readme_url("itoa", "latest").is_err());
    }

    #[test]
    fn counts_are_shortened() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(870), "870");
        assert_eq!(fmt_count(1_000), "1.0K");
        assert_eq!(fmt_count(12_345), "12K");
        assert_eq!(fmt_count(999_999), "999K");
        assert_eq!(fmt_count(345_158_046), "345M");
        assert_eq!(fmt_count(1_394_763_516), "1.3B");
        assert!(fmt_count(u64::MAX).ends_with("B"));
    }
}
