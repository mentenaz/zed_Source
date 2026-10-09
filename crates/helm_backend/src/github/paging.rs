//! One page of a list, and reading where the other pages are.
//!
//! GitHub returns lists a page at a time and says where the neighbouring
//! pages are in the `Link` header:
//!
//! ```text
//! <https://api.github.com/repositories/1/issues?per_page=10&page=3>; rel="next",
//! <https://api.github.com/repositories/1/issues?per_page=10&page=9>; rel="last"
//! ```
//!
//! Everything here is plain parsing, tested without a network.

use serde::de::DeserializeOwned;

use super::error::{GhError, RawResponse, interpret};

/// One page of a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Which page this is, counting from 1.
    pub page: u32,
    /// The last page there is. Equal to `page` on the last page, and 1 for
    /// a list that fits on one.
    pub last_page: u32,
}

impl<T> Page<T> {
    pub fn has_next(&self) -> bool {
        self.page < self.last_page
    }

    pub fn has_previous(&self) -> bool {
        self.page > 1
    }
}

/// The page numbers a `Link` header points at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageLinks {
    pub next: Option<u32>,
    pub previous: Option<u32>,
    pub last: Option<u32>,
}

/// The `page` parameter of a URL's query string.
fn page_of(url: &str) -> Option<u32> {
    let (_, query) = url.split_once('?')?;
    query
        .split('&')
        .find_map(|param| param.strip_prefix("page="))?
        .parse()
        .ok()
}

/// Reads a `Link` header. Relations other than `next`, `prev` and `last`,
/// and links without a page number, are ignored.
pub fn parse_link(header: &str) -> PageLinks {
    let mut links = PageLinks::default();
    for part in header.split(',') {
        let Some((url, relation)) = part.split_once(';') else {
            continue;
        };
        let url = url.trim().trim_start_matches('<').trim_end_matches('>');
        let Some(page) = page_of(url) else {
            continue;
        };
        let relation = relation.trim();
        if relation.contains("rel=\"next\"") {
            links.next = Some(page);
        } else if relation.contains("rel=\"prev\"") {
            links.previous = Some(page);
        } else if relation.contains("rel=\"last\"") {
            links.last = Some(page);
        }
    }
    links
}

/// The last page of a list, given the `Link` header of page `current`.
///
/// GitHub leaves `rel="last"` out on the last page itself, and sends no
/// header at all for a list that fits on one page; in both cases the
/// current page is the last. For a few very long lists it gives `next`
/// without `last`, and then all that is known is that there is at least one
/// more page.
pub fn last_page(link: Option<&str>, current: u32) -> u32 {
    let links = link.map(parse_link).unwrap_or_default();
    links.last.or(links.next).unwrap_or(current).max(current)
}

/// Turns the answer to a request for page `page` into that page.
pub fn interpret_page<T: DeserializeOwned>(
    response: &RawResponse,
    page: u32,
) -> Result<Page<T>, GhError> {
    let items: Vec<T> = interpret(response)?;
    Ok(Page {
        items,
        page,
        last_page: last_page(response.link.as_deref(), page),
    })
}

/// [`interpret_page`] for a list GitHub wraps in an object, as it does for
/// workflow runs (`{"total_count": 3, "workflow_runs": [...]}`). A missing
/// `key` is an empty page.
pub fn interpret_page_under<T: DeserializeOwned>(
    response: &RawResponse,
    key: &str,
    page: u32,
) -> Result<Page<T>, GhError> {
    let answer: serde_json::Value = interpret(response)?;
    let items = match answer.get(key) {
        Some(list) => serde_json::from_value(list.clone())
            .map_err(|error| GhError::Parse(error.to_string()))?,
        None => Vec::new(),
    };
    Ok(Page {
        items,
        page,
        last_page: last_page(response.link.as_deref(), page),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIDDLE: &str = "<https://api.github.com/repositories/1/issues?state=open&per_page=10&page=2>; rel=\"prev\", \
         <https://api.github.com/repositories/1/issues?state=open&per_page=10&page=4>; rel=\"next\", \
         <https://api.github.com/repositories/1/issues?state=open&per_page=10&page=9>; rel=\"last\", \
         <https://api.github.com/repositories/1/issues?state=open&per_page=10&page=1>; rel=\"first\"";
    const LAST: &str = "<https://api.github.com/repositories/1/issues?per_page=10&page=8>; rel=\"prev\", \
         <https://api.github.com/repositories/1/issues?per_page=10&page=1>; rel=\"first\"";

    fn answer(body: &str, link: Option<&str>) -> RawResponse {
        RawResponse {
            status: 200,
            body: body.to_string(),
            link: link.map(str::to_string),
            ..RawResponse::default()
        }
    }

    #[test]
    fn a_link_header_names_the_neighbouring_pages() {
        assert_eq!(
            parse_link(MIDDLE),
            PageLinks {
                next: Some(4),
                previous: Some(2),
                last: Some(9)
            }
        );
        assert_eq!(
            parse_link(LAST),
            PageLinks {
                next: None,
                previous: Some(8),
                last: None
            }
        );
        assert_eq!(parse_link(""), PageLinks::default());
        assert_eq!(parse_link("nonsense; rel=\"next\""), PageLinks::default());
        // `per_page=10` must not be read as the page.
        assert_eq!(
            parse_link("<https://x/y?per_page=10&page=7>; rel=\"next\"").next,
            Some(7)
        );
    }

    #[test]
    fn the_last_page_is_known_on_every_page() {
        assert_eq!(last_page(Some(MIDDLE), 3), 9);
        // On the last page GitHub leaves `last` out: this page is it.
        assert_eq!(last_page(Some(LAST), 9), 9);
        // A list that fits on one page has no header at all.
        assert_eq!(last_page(None, 1), 1);
        // `next` without `last`: at least one more page.
        assert_eq!(last_page(Some("<https://x/y?page=5>; rel=\"next\""), 4), 5);
    }

    #[test]
    fn an_answer_becomes_a_page() {
        let page: Page<u32> = interpret_page(&answer("[1, 2, 3]", Some(MIDDLE)), 3).unwrap();
        assert_eq!(page.items, vec![1, 2, 3]);
        assert_eq!((page.page, page.last_page), (3, 9));
        assert!(page.has_next() && page.has_previous());

        let only: Page<u32> = interpret_page(&answer("[1]", None), 1).unwrap();
        assert_eq!((only.page, only.last_page), (1, 1));
        assert!(!only.has_next() && !only.has_previous());

        let last: Page<u32> = interpret_page(&answer("[]", Some(LAST)), 9).unwrap();
        assert!(!last.has_next() && last.has_previous());
    }

    #[test]
    fn a_failed_page_is_the_usual_error() {
        let response = RawResponse {
            status: 404,
            body: r#"{"message": "Not Found"}"#.to_string(),
            ..RawResponse::default()
        };
        assert_eq!(
            interpret_page::<u32>(&response, 1),
            Err(GhError::NotFound {
                message: "Not Found".into()
            })
        );
    }

    #[test]
    fn a_wrapped_list_is_paged_the_same_way() {
        let body = r#"{"total_count": 25, "workflow_runs": [10, 11]}"#;
        let page: Page<u32> =
            interpret_page_under(&answer(body, Some(MIDDLE)), "workflow_runs", 3).unwrap();
        assert_eq!(page.items, vec![10, 11]);
        assert_eq!(page.last_page, 9);

        // A missing key is an empty page, not an error.
        let empty: Page<u32> =
            interpret_page_under(&answer(r#"{"total_count": 0}"#, None), "workflow_runs", 1)
                .unwrap();
        assert!(empty.items.is_empty());
        // The key holding something else is an error.
        assert!(matches!(
            interpret_page_under::<u32>(
                &answer(r#"{"workflow_runs": "x"}"#, None),
                "workflow_runs",
                1
            ),
            Err(GhError::Parse(_))
        ));
    }
}
