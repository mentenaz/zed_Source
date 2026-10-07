//! The network half of the Rust manager, shared by this tab and the Rust
//! dock panel (`rust_panel`): looking dependencies up in the crates.io
//! sparse index, and asking OSV.dev about advisories.
//!
//! `cargo_backend` builds the URLs and request bodies and parses the
//! answers; the functions here make the requests, through the app's shared
//! HTTP client (which carries Zed's `User-Agent`, as crates.io requires).

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cargo_backend::{
    AdvisoryRecord, DependencyList, Finding, IndexVersion, LockedPackage, PackageAdvisories,
    UpdateTarget, advisory_ids, advisory_url, index_url, osv_batches, parse_advisory, parse_index,
};
use futures::{AsyncReadExt as _, StreamExt as _, stream};
use gpui::{App, BorrowAppContext as _, Global};
use http_client::{AsyncBody, HttpClient};

/// How long a crate's index entry is reused before asking again — the
/// `max-age` the sparse index itself sends.
pub const INDEX_CACHE_LIFETIME: Duration = Duration::from_secs(600);
/// Requests in flight at once, for index lookups and advisory details alike.
pub const REQUEST_CONCURRENCY: usize = 8;

/// One package's index entries, with when they were fetched.
pub struct CachedIndex {
    pub versions: Vec<IndexVersion>,
    pub fetched: Instant,
}

/// Index entries by package name.
pub type IndexCache = HashMap<String, CachedIndex>;

/// Fetched advisory records by id, each with the `modified` stamp it was
/// fetched at, so a rescan only refetches what changed.
pub type AdvisoryRecords = HashMap<String, (Option<String>, AdvisoryRecord)>;

/// Where the registry check for newer versions stands.
pub enum OutdatedState {
    /// Nothing to check yet (no crate selected, or it lists nothing).
    Idle,
    Checking,
    /// Finished; `failed` lookups could not be completed (offline, usually).
    Done { failed: usize },
}

/// Where the advisory scan stands. Starts — and after any change to the
/// selected crate returns to — `NotScanned`, so that stale or missing
/// results never read as a clean bill of health.
pub enum AdvisoryState {
    NotScanned,
    Scanning,
    Done {
        findings: Vec<Finding>,
        /// Packages asked about.
        scanned: usize,
        /// OSV had more results for some package than it returned.
        truncated: bool,
    },
    Failed(String),
}

/// A finished advisory scan.
#[derive(Clone)]
pub struct ScanResult {
    pub findings: Vec<Finding>,
    pub scanned: usize,
    pub truncated: bool,
}

impl ScanResult {
    pub fn into_state(self) -> AdvisoryState {
        AdvisoryState::Done {
            findings: self.findings,
            scanned: self.scanned,
            truncated: self.truncated,
        }
    }
}

/// Finished scans, shared by every Rust panel and Cargo manager tab in the
/// app, so a scan run in one shows up in the other instead of leaving it at
/// "not scanned yet".
///
/// Keyed by [`scan_key`]: the exact set of packages that was asked about.
/// That is what makes sharing safe without any invalidation. A different
/// crate, or a lockfile that now resolves differently, is a different key,
/// so a result can only ever be shown for the packages it was computed for.
#[derive(Default)]
pub struct SharedScans(HashMap<u64, ScanResult>);

impl Global for SharedScans {}

/// Identifies a scan by what it covered: every reachable package's name and
/// version, in the order `Lockfile::reachable_crates_io_packages` returns
/// them (sorted).
pub fn scan_key(reachable: &[LockedPackage]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for package in reachable {
        package.name.hash(&mut hasher);
        package.version.hash(&mut hasher);
    }
    hasher.finish()
}

/// Records a finished scan and notifies every observer of [`SharedScans`].
pub fn publish_scan(key: u64, result: ScanResult, cx: &mut App) {
    cx.update_default_global::<SharedScans, _>(|scans, _| {
        scans.0.insert(key, result);
    });
}

/// The finished scan for exactly this set of packages, if any panel or tab
/// has run one.
pub fn shared_scan(key: u64, cx: &App) -> Option<ScanResult> {
    cx.try_global::<SharedScans>()?.0.get(&key).cloned()
}

/// One dependency that is behind the registry, or locked to a yanked
/// version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutdatedRow {
    pub name: String,
    /// The locked version, or `None` when the lockfile doesn't have it.
    pub locked: Option<String>,
    pub locked_yanked: bool,
    /// Newest version the declared requirement allows (an upper bound: see
    /// `cargo_backend::outdated`).
    pub in_range: Option<UpdateTarget>,
    /// Newest version the requirement excludes.
    pub out_of_range: Option<UpdateTarget>,
}

/// The dependencies that are behind the registry, or locked to a yanked
/// version, once each. A package declared twice (normal and dev, or under
/// two targets) has the same status both times and is shown once.
///
/// A dependency with no index entry yet is not a row: "not checked" is
/// neither outdated nor up to date.
pub fn outdated_rows(
    list: &DependencyList,
    index: &IndexCache,
    toolchain: Option<&str>,
) -> Vec<OutdatedRow> {
    let mut rows: Vec<OutdatedRow> = Vec::new();
    for dependency in &list.listed {
        if rows.iter().any(|row| row.name == dependency.name) {
            continue;
        }
        let Some(cached) = index.get(&dependency.name) else {
            continue;
        };
        let status = dependency.status(&cached.versions, toolchain);
        if !status.is_outdated() && !status.locked_yanked {
            continue;
        }
        rows.push(OutdatedRow {
            name: dependency.name.clone(),
            locked: dependency.locked_version.clone(),
            locked_yanked: status.locked_yanked,
            in_range: status.in_range,
            out_of_range: status.out_of_range,
        });
    }
    rows
}

/// The listed dependencies whose index entry is missing or older than
/// [`INDEX_CACHE_LIFETIME`] at `now` — the lookups a check has to make.
pub fn stale_names(list: &DependencyList, index: &IndexCache, now: Instant) -> Vec<String> {
    list.registry_names()
        .into_iter()
        .filter(|name| {
            index
                .get(*name)
                .is_none_or(|cached| now.duration_since(cached.fetched) > INDEX_CACHE_LIFETIME)
        })
        .map(str::to_string)
        .collect()
}

async fn read_body(
    response: anyhow::Result<http_client::Response<AsyncBody>>,
) -> Result<String, String> {
    let mut response = response.map_err(|error| format!("request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

async fn fetch_text(client: &Arc<dyn HttpClient>, url: &str) -> Result<String, String> {
    read_body(client.get(url, AsyncBody::default(), true).await).await
}

async fn post_json(client: &Arc<dyn HttpClient>, url: &str, body: String) -> Result<String, String> {
    read_body(client.post_json(url, body.into()).await).await
}

/// Looks each package up in the crates.io sparse index, several at a time.
pub async fn fetch_index_entries(
    client: &Arc<dyn HttpClient>,
    names: Vec<String>,
) -> Vec<(String, Result<Vec<IndexVersion>, String>)> {
    stream::iter(names)
        .map(|name| {
            let client = client.clone();
            async move {
                let versions = match fetch_text(&client, &index_url(&name)).await {
                    Ok(body) => parse_index(&body),
                    Err(error) => Err(error),
                };
                (name, versions)
            }
        })
        .buffer_unordered(REQUEST_CONCURRENCY)
        .collect()
        .await
}

/// Stores the lookups that succeeded and returns how many failed. A failed
/// lookup keeps whatever older entry there was: stale-but-labelled beats
/// blank.
pub fn store_index_entries(
    index: &mut IndexCache,
    results: Vec<(String, Result<Vec<IndexVersion>, String>)>,
) -> usize {
    let mut failed = 0;
    for (name, versions) in results {
        match versions {
            Ok(versions) => {
                index.insert(
                    name,
                    CachedIndex {
                        versions,
                        fetched: Instant::now(),
                    },
                );
            }
            Err(_) => failed += 1,
        }
    }
    failed
}

/// Both rounds of an advisory scan. Returns the batch hits and the record
/// cache, updated with whatever had to be (re)fetched.
///
/// A record whose detail request fails is simply absent from the cache;
/// `merge_findings` then reports that advisory as "details missing" rather
/// than dropping it. Only a failed *batch* request fails the scan, because
/// without it there is nothing to show at all.
pub async fn run_advisory_scan(
    client: &Arc<dyn HttpClient>,
    reachable: &[LockedPackage],
    mut records: AdvisoryRecords,
) -> Result<(Vec<PackageAdvisories>, AdvisoryRecords), String> {
    let refs: Vec<&LockedPackage> = reachable.iter().collect();
    let mut hits = Vec::new();
    for batch in osv_batches(&refs) {
        let answer = post_json(client, cargo_backend::OSV_BATCH_URL, batch.body.clone())
            .await
            .map_err(|error| format!("Could not reach the advisory service: {error}"))?;
        hits.extend(batch.parse_response(&answer)?);
    }

    // The newest `modified` stamp seen for each id, to decide what is stale.
    let mut stamps: HashMap<&str, Option<&str>> = HashMap::new();
    for advisory in hits.iter().flat_map(|hit| hit.advisories.iter()) {
        stamps.insert(advisory.id.as_str(), advisory.modified.as_deref());
    }
    let to_fetch: Vec<(String, Option<String>)> = advisory_ids(&hits)
        .into_iter()
        .filter(|id| {
            let stamp = stamps.get(id).copied().flatten();
            records
                .get(*id)
                .is_none_or(|(cached_stamp, _)| cached_stamp.as_deref() != stamp)
        })
        .map(|id| {
            (
                id.to_string(),
                stamps.get(id).copied().flatten().map(str::to_string),
            )
        })
        .collect();

    let fetched: Vec<(String, Option<String>, Result<AdvisoryRecord, String>)> =
        stream::iter(to_fetch)
            .map(|(id, stamp)| {
                let client = client.clone();
                async move {
                    let record = match fetch_text(&client, &advisory_url(&id)).await {
                        Ok(body) => parse_advisory(&body),
                        Err(error) => Err(error),
                    };
                    (id, stamp, record)
                }
            })
            .buffer_unordered(REQUEST_CONCURRENCY)
            .collect()
            .await;
    for (id, stamp, record) in fetched {
        if let Ok(record) = record {
            records.insert(id, (stamp, record));
        }
    }

    Ok((hits, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_backend::{DependencyKind, ListedDependency, UpdateKind, Version};

    fn listed(name: &str, requirement: &str, locked: Option<&str>, kind: DependencyKind) -> ListedDependency {
        ListedDependency {
            name: name.to_string(),
            rename: None,
            requirement: requirement.to_string(),
            kind,
            target: None,
            optional: false,
            locked_version: locked.map(str::to_string),
        }
    }

    fn cached(lines: &[&str]) -> CachedIndex {
        CachedIndex {
            versions: parse_index(&lines.join("\n")).unwrap(),
            fetched: Instant::now(),
        }
    }

    fn list(listed: Vec<ListedDependency>) -> DependencyList {
        DependencyList { listed, hidden: 0 }
    }

    #[test]
    fn outdated_rows_list_each_package_once() {
        let list = list(vec![
            listed("serde", "^1", Some("1.0.210"), DependencyKind::Normal),
            // Declared again as a dev-dependency: same status, one row.
            listed("serde", "^1", Some("1.0.210"), DependencyKind::Dev),
            listed("log", "^0.4", Some("0.4.29"), DependencyKind::Normal),
            listed("fresh", "^2", Some("2.0.0"), DependencyKind::Normal),
        ]);
        let mut index = IndexCache::new();
        index.insert(
            "serde".to_string(),
            cached(&[r#"{"vers":"1.0.210"}"#, r#"{"vers":"1.0.229"}"#, r#"{"vers":"2.0.0"}"#]),
        );
        index.insert("log".to_string(), cached(&[r#"{"vers":"0.4.29"}"#, r#"{"vers":"0.4.34"}"#]));
        index.insert("fresh".to_string(), cached(&[r#"{"vers":"2.0.0"}"#]));

        let rows = outdated_rows(&list, &index, Some("1.98.1"));
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["serde", "log"]);

        let serde = &rows[0];
        assert_eq!(serde.locked.as_deref(), Some("1.0.210"));
        let in_range = serde.in_range.as_ref().unwrap();
        assert_eq!(in_range.version, Version::new(1, 0, 229));
        assert_eq!(in_range.kind, UpdateKind::Patch);
        assert_eq!(serde.out_of_range.as_ref().unwrap().version, Version::new(2, 0, 0));
    }

    #[test]
    fn dependencies_not_looked_up_yet_are_not_reported() {
        // No index entry means "not checked", which is neither outdated nor
        // up to date — it must not appear as a row.
        let list = list(vec![listed("serde", "^1", Some("1.0.210"), DependencyKind::Normal)]);
        assert!(outdated_rows(&list, &IndexCache::new(), None).is_empty());
    }

    #[test]
    fn a_yanked_locked_version_is_a_row_even_with_nothing_newer() {
        let list = list(vec![listed("oops", "^1", Some("1.0.1"), DependencyKind::Normal)]);
        let mut index = IndexCache::new();
        index.insert(
            "oops".to_string(),
            cached(&[r#"{"vers":"1.0.0"}"#, r#"{"vers":"1.0.1","yanked":true}"#]),
        );
        let rows = outdated_rows(&list, &index, None);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].locked_yanked);
        assert_eq!(rows[0].in_range, None);
        assert_eq!(rows[0].out_of_range, None);
    }

    #[test]
    fn a_dependency_without_a_locked_version_is_not_called_outdated() {
        let list = list(vec![listed("serde", "^1", None, DependencyKind::Normal)]);
        let mut index = IndexCache::new();
        index.insert("serde".to_string(), cached(&[r#"{"vers":"1.0.229"}"#]));
        assert!(outdated_rows(&list, &index, None).is_empty());
    }

    #[test]
    fn a_scan_is_identified_by_the_packages_it_covered() {
        let package = |name: &str, version: &str| LockedPackage {
            name: name.to_string(),
            version: version.to_string(),
            source: None,
            dependencies: Vec::new(),
        };
        let before = [package("serde", "1.0.210"), package("time", "0.1.43")];
        let same = [package("serde", "1.0.210"), package("time", "0.1.43")];
        assert_eq!(scan_key(&before), scan_key(&same));

        // One package moved to another version: an old result no longer
        // describes this lockfile, so it must not be found.
        let updated = [package("serde", "1.0.210"), package("time", "0.3.41")];
        assert_ne!(scan_key(&before), scan_key(&updated));
        let fewer = [package("serde", "1.0.210")];
        assert_ne!(scan_key(&before), scan_key(&fewer));
        // Name and version are kept apart: `a` + `11.0` is not `a1` + `1.0`.
        assert_ne!(
            scan_key(&[package("a", "11.0.0")]),
            scan_key(&[package("a1", "1.0.0")])
        );
    }

    #[test]
    fn only_missing_or_expired_entries_are_looked_up() {
        let list = list(vec![
            listed("fresh", "^1", None, DependencyKind::Normal),
            listed("expired", "^1", None, DependencyKind::Normal),
            listed("missing", "^1", None, DependencyKind::Normal),
            // Listed twice: still one lookup.
            listed("missing", "^1", None, DependencyKind::Dev),
        ]);
        let fetched = Instant::now();
        let mut index = IndexCache::new();
        index.insert("fresh".to_string(), CachedIndex { versions: Vec::new(), fetched });
        index.insert("expired".to_string(), CachedIndex { versions: Vec::new(), fetched });

        let soon = fetched + Duration::from_secs(60);
        assert_eq!(stale_names(&list, &index, soon), vec!["missing"]);

        index.get_mut("fresh").unwrap().fetched = fetched + INDEX_CACHE_LIFETIME;
        let later = fetched + INDEX_CACHE_LIFETIME + Duration::from_secs(1);
        assert_eq!(stale_names(&list, &index, later), vec!["expired", "missing"]);
    }

    #[test]
    fn failed_lookups_are_counted_and_keep_the_older_entry() {
        let mut index = IndexCache::new();
        index.insert("kept".to_string(), cached(&[r#"{"vers":"1.0.0"}"#]));
        let failed = store_index_entries(
            &mut index,
            vec![
                ("kept".to_string(), Err("offline".to_string())),
                ("new".to_string(), parse_index(r#"{"vers":"2.0.0"}"#)),
            ],
        );
        assert_eq!(failed, 1);
        assert_eq!(index["kept"].versions[0].version, Version::new(1, 0, 0));
        assert_eq!(index["new"].versions[0].version, Version::new(2, 0, 0));
    }
}
