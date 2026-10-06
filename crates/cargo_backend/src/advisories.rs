//! Security advisories from OSV.dev for the packages a crate pulls in.
//!
//! Two round trips, both made by the host (this module builds the requests
//! and reads the answers, like [`crate::index`]):
//!
//! 1. **Which advisories apply?** `POST` [`OSV_BATCH_URL`] with up to
//!    [`OSV_BATCH_LIMIT`] package/version pairs per request
//!    ([`osv_batches`]). The answer names advisory ids only.
//! 2. **What are they?** `GET` [`advisory_url`] per distinct id
//!    ([`parse_advisory`]). Each id comes with a `modified` stamp, so a
//!    record can be cached until that stamp changes.
//!
//! The details are not optional. Without them there is no way to tell that
//! two ids are the same advisory, or that an id isn't a vulnerability at
//! all. On this fork (October 2026) OSV returned 44 records for 31 packages;
//! they are 39 distinct findings, of which 16 are RustSec *informational*
//! notices ("unmaintained", "unsound") rather than vulnerabilities, and only
//! 6 of the remaining 23 arrive with a severity label. [`merge_findings`]
//! does that sorting-out.

use std::collections::{BTreeSet, HashMap};

use semver::Version;
use serde_json::{Value, json};

use crate::lockfile::LockedPackage;

pub const OSV_BATCH_URL: &str = "https://api.osv.dev/v1/querybatch";
/// OSV's name for the crates.io ecosystem. Anything else is rejected as
/// "invalid ecosystem".
pub const OSV_ECOSYSTEM: &str = "crates.io";
/// The most queries one batch request may hold; one more is rejected with
/// "too many queries" (tested October 2026).
pub const OSV_BATCH_LIMIT: usize = 1000;

/// The API address of one advisory's full record.
pub fn advisory_url(id: &str) -> String {
    format!("https://api.osv.dev/v1/vulns/{id}")
}

/// The human-readable page for an advisory, used when a record names no
/// better link of its own.
pub fn advisory_page_url(id: &str) -> String {
    format!("https://osv.dev/vulnerability/{id}")
}

// ── Step 1: the batch query ────────────────────────────────────────────

/// One batch request: its body, and the packages it asks about in the order
/// OSV will answer them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsvBatch {
    /// `(name, version)` pairs, in query order.
    pub packages: Vec<(String, String)>,
    /// The JSON to `POST` to [`OSV_BATCH_URL`].
    pub body: String,
}

/// Splits `packages` into as many batch requests as OSV's limit requires.
/// Duplicates are asked about once.
pub fn osv_batches(packages: &[&LockedPackage]) -> Vec<OsvBatch> {
    let mut seen = BTreeSet::new();
    let unique: Vec<(String, String)> = packages
        .iter()
        .map(|package| (package.name.clone(), package.version.clone()))
        .filter(|pair| seen.insert(pair.clone()))
        .collect();

    unique
        .chunks(OSV_BATCH_LIMIT)
        .map(|chunk| OsvBatch {
            packages: chunk.to_vec(),
            body: json!({
                "queries": chunk
                    .iter()
                    .map(|(name, version)| {
                        json!({
                            "package": { "name": name, "ecosystem": OSV_ECOSYSTEM },
                            "version": version,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .to_string(),
        })
        .collect()
}

/// An advisory id as the batch query reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvisoryRef {
    pub id: String,
    /// When the record last changed. Cache a fetched record under its id and
    /// this stamp; refetch only when the stamp differs.
    pub modified: Option<String>,
}

/// The advisories OSV lists for one locked package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageAdvisories {
    pub name: String,
    pub version: String,
    pub advisories: Vec<AdvisoryRef>,
    /// OSV had more results than it returned for this package. The list is
    /// incomplete and the UI must say so rather than present it as whole.
    pub truncated: bool,
}

impl OsvBatch {
    /// Reads OSV's answer to this batch. Only packages with at least one
    /// advisory (or a truncated answer) are returned.
    ///
    /// The answer is positional — the *n*th result belongs to the *n*th
    /// query — so a result count that doesn't match is an error rather than
    /// something to guess an alignment for.
    pub fn parse_response(&self, body: &str) -> Result<Vec<PackageAdvisories>, String> {
        let value: Value = serde_json::from_str(body)
            .map_err(|error| format!("Could not read the advisory service's answer: {error}"))?;
        let results = value
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| match value.get("message").and_then(Value::as_str) {
                Some(message) => format!("The advisory service refused the request: {message}"),
                None => "The advisory service's answer had no results".to_string(),
            })?;
        if results.len() != self.packages.len() {
            return Err(format!(
                "The advisory service answered {} of {} queries",
                results.len(),
                self.packages.len()
            ));
        }

        Ok(self
            .packages
            .iter()
            .zip(results)
            .filter_map(|((name, version), result)| {
                let advisories: Vec<AdvisoryRef> = result
                    .get("vulns")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|vuln| {
                        Some(AdvisoryRef {
                            id: vuln.get("id")?.as_str()?.to_string(),
                            modified: vuln
                                .get("modified")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        })
                    })
                    .collect();
                let truncated = result
                    .get("next_page_token")
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty());
                (!advisories.is_empty() || truncated).then(|| PackageAdvisories {
                    name: name.clone(),
                    version: version.clone(),
                    advisories,
                    truncated,
                })
            })
            .collect())
    }
}

/// The distinct advisory ids across `hits`, sorted — one detail request each.
pub fn advisory_ids(hits: &[PackageAdvisories]) -> Vec<&str> {
    let ids: BTreeSet<&str> = hits
        .iter()
        .flat_map(|hit| hit.advisories.iter().map(|advisory| advisory.id.as_str()))
        .collect();
    ids.into_iter().collect()
}

// ── Severity ───────────────────────────────────────────────────────────

/// How serious an advisory is. `None` wherever this appears as an `Option`
/// means *unknown* — shown in the warning colour, never as "fine".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Moderate,
    High,
    Critical,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Moderate => "moderate",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// Reads a label as GitHub writes it (`MODERATE`) or as CVSS does
    /// (`Medium`).
    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Severity::Low),
            "moderate" | "medium" => Some(Severity::Moderate),
            "high" => Some(Severity::High),
            "critical" => Some(Severity::Critical),
            _ => None,
        }
    }

    /// The CVSS qualitative band for a base score given in tenths (98 for
    /// 9.8). A score of zero is "none", which is no severity.
    pub fn from_cvss_tenths(tenths: u32) -> Option<Self> {
        match tenths {
            0 => None,
            1..=39 => Some(Severity::Low),
            40..=69 => Some(Severity::Moderate),
            70..=89 => Some(Severity::High),
            _ => Some(Severity::Critical),
        }
    }
}

/// The CVSS v3.0/v3.1 base score of a vector such as
/// `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H`, in tenths (98 for 9.8).
///
/// RustSec records usually carry a vector but no severity label, so this is
/// what turns most "unknown" severities into real ones. Implements the base
/// metrics of the published specification; temporal and environmental
/// metrics, if present, are ignored.
///
/// `None` for anything that isn't a complete v3 vector — CVSS v4 included,
/// whose scoring is a lookup table rather than a formula. Those stay
/// unknown rather than being approximated.
pub fn cvss3_base_tenths(vector: &str) -> Option<u32> {
    let mut parts = vector.trim().split('/');
    if !matches!(parts.next()?, "CVSS:3.0" | "CVSS:3.1") {
        return None;
    }
    let metrics: HashMap<&str, &str> = parts.filter_map(|part| part.split_once(':')).collect();
    let metric = |name: &str| metrics.get(name).copied();

    let scope_changed = match metric("S")? {
        "U" => false,
        "C" => true,
        _ => return None,
    };
    let attack_vector = match metric("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.2,
        _ => return None,
    };
    let attack_complexity = match metric("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let privileges_required = match (metric("PR")?, scope_changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let user_interaction = match metric("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };
    let impact_weight = |name: &str| match metric(name)? {
        "H" => Some(0.56),
        "L" => Some(0.22),
        "N" => Some(0.0),
        _ => None,
    };
    let (confidentiality, integrity, availability) =
        (impact_weight("C")?, impact_weight("I")?, impact_weight("A")?);

    let impact_sub_score = 1.0 - (1.0 - confidentiality) * (1.0 - integrity) * (1.0 - availability);
    let impact = if scope_changed {
        7.52 * (impact_sub_score - 0.029) - 3.25 * (impact_sub_score - 0.02_f64).powi(15)
    } else {
        6.42 * impact_sub_score
    };
    if impact <= 0.0 {
        return Some(0);
    }
    let exploitability =
        8.22 * attack_vector * attack_complexity * privileges_required * user_interaction;
    let base = if scope_changed {
        (1.08 * (impact + exploitability)).min(10.0)
    } else {
        (impact + exploitability).min(10.0)
    };

    // The specification's "round up to one decimal", done on integers so
    // that a value like 4.000000000001 doesn't become 4.1.
    let scaled = (base * 100_000.0).round() as u64;
    Some(if scaled.is_multiple_of(10_000) {
        (scaled / 10_000) as u32
    } else {
        (scaled / 10_000 + 1) as u32
    })
}

// ── Step 2: one advisory's record ──────────────────────────────────────

/// What an advisory says about one crates.io package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffectedPackage {
    pub name: String,
    /// Versions the advisory names as fixed. Often more than one, one per
    /// maintained release line.
    pub fixed: Vec<String>,
    /// RustSec's marker for a record that is a notice rather than a
    /// vulnerability: `unmaintained`, `unsound`, or `notice`.
    pub informational: Option<String>,
}

/// One advisory record from OSV, reduced to what the manager uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvisoryRecord {
    pub id: String,
    /// Other ids for the same advisory (CVE, GHSA, RUSTSEC).
    pub aliases: Vec<String>,
    pub summary: Option<String>,
    pub details: Option<String>,
    /// Retracted after publication. Never shown.
    pub withdrawn: bool,
    /// The record's own label, when it has one (GitHub records do).
    pub severity_label: Option<Severity>,
    /// Every CVSS vector the record carries, any version.
    pub cvss_vectors: Vec<String>,
    /// Only crates.io packages; an advisory can cover other ecosystems too.
    pub affected: Vec<AffectedPackage>,
    /// The advisory's own page, when the record links one.
    pub url: Option<String>,
}

impl AdvisoryRecord {
    /// The severity this record supports: its label, or failing that the
    /// band of its highest computable CVSS v3 score.
    pub fn severity(&self) -> Option<Severity> {
        self.severity_label.or_else(|| {
            self.cvss_vectors
                .iter()
                .filter_map(|vector| cvss3_base_tenths(vector))
                .max()
                .and_then(Severity::from_cvss_tenths)
        })
    }
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Parses the JSON of one advisory (`GET` [`advisory_url`]).
pub fn parse_advisory(json: &str) -> Result<AdvisoryRecord, String> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| format!("Could not read the advisory: {error}"))?;
    let id = non_empty(value.get("id")).ok_or_else(|| {
        match value.get("message").and_then(Value::as_str) {
            Some(message) => format!("The advisory service refused the request: {message}"),
            None => "The advisory has no id".to_string(),
        }
    })?;

    let strings = |value: Option<&Value>| -> Vec<String> {
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.as_str().map(str::to_string))
            .collect()
    };

    let mut cvss_vectors: Vec<String> = value
        .get("severity")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| non_empty(entry.get("score")))
        .collect();

    let mut affected = Vec::new();
    for entry in value
        .get("affected")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let package = entry.get("package");
        let ecosystem = package
            .and_then(|package| package.get("ecosystem"))
            .and_then(Value::as_str);
        if ecosystem != Some(OSV_ECOSYSTEM) {
            continue;
        }
        let Some(name) = non_empty(package.and_then(|package| package.get("name"))) else {
            continue;
        };
        let fixed = entry
            .get("ranges")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|range| {
                range
                    .get("events")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|event| non_empty(event.get("fixed")))
            .collect();
        let database_specific = entry.get("database_specific");
        // RustSec also keeps its vector here, sometimes only here.
        if let Some(vector) = non_empty(database_specific.and_then(|specific| specific.get("cvss")))
            && !cvss_vectors.contains(&vector)
        {
            cvss_vectors.push(vector);
        }
        affected.push(AffectedPackage {
            name,
            fixed,
            informational: non_empty(
                database_specific.and_then(|specific| specific.get("informational")),
            ),
        });
    }

    let references: Vec<(&str, &str)> = value
        .get("references")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|reference| {
            Some((
                reference.get("type")?.as_str()?,
                reference.get("url")?.as_str()?,
            ))
        })
        .collect();
    let url = references
        .iter()
        .find(|(kind, _)| *kind == "ADVISORY")
        .or_else(|| references.iter().find(|(kind, _)| *kind == "WEB"))
        .map(|(_, url)| url.to_string());

    Ok(AdvisoryRecord {
        id,
        aliases: strings(value.get("aliases")),
        summary: non_empty(value.get("summary")),
        details: non_empty(value.get("details")),
        withdrawn: value.get("withdrawn").is_some_and(|stamp| !stamp.is_null()),
        severity_label: value
            .get("database_specific")
            .and_then(|specific| specific.get("severity"))
            .and_then(Value::as_str)
            .and_then(Severity::from_label),
        cvss_vectors,
        affected,
        url,
    })
}

// ── Findings ───────────────────────────────────────────────────────────

/// What kind of thing a finding is. Only the first is a vulnerability; the
/// others are RustSec informational notices, kept apart so that an
/// unmaintained crate is never counted as a security hole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FindingKind {
    /// A security vulnerability.
    Vulnerability,
    /// The package can cause undefined behaviour from safe code.
    Unsound,
    /// The package is no longer maintained.
    Unmaintained,
    /// Some other notice from the advisory database.
    Notice,
}

impl FindingKind {
    pub fn label(self) -> &'static str {
        match self {
            FindingKind::Vulnerability => "vulnerability",
            FindingKind::Unsound => "unsound",
            FindingKind::Unmaintained => "unmaintained",
            FindingKind::Notice => "notice",
        }
    }

    fn from_informational(marker: &str) -> Self {
        match marker.trim().to_ascii_lowercase().as_str() {
            "unsound" => FindingKind::Unsound,
            "unmaintained" => FindingKind::Unmaintained,
            _ => FindingKind::Notice,
        }
    }
}

/// One row of the Vulnerabilities page: one advisory, for one locked
/// version of one package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub package: String,
    /// The locked version it applies to. Two locked versions of a package
    /// are separate findings (decision 11 in the design note).
    pub version: String,
    /// The id to show: the RustSec one when there is one, since that is the
    /// Rust ecosystem's own.
    pub id: String,
    /// Every other id the same advisory goes by, sorted.
    pub other_ids: Vec<String>,
    pub kind: FindingKind,
    /// `None` is unknown, not "none".
    pub severity: Option<Severity>,
    pub summary: Option<String>,
    /// Fixed versions newer than the locked one, oldest first. Empty when
    /// the advisory names no fix (common for unmaintained crates).
    pub fixed_in: Vec<String>,
    pub url: String,
    /// No record could be read for this advisory, so only its id is known.
    /// Kind and severity are then unknown too, and the row must say so.
    pub details_missing: bool,
}

/// How many findings there are of each kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindingCounts {
    pub vulnerabilities: usize,
    pub unsound: usize,
    pub unmaintained: usize,
    pub notices: usize,
}

impl FindingCounts {
    pub fn of(findings: &[Finding]) -> Self {
        let mut counts = FindingCounts::default();
        for finding in findings {
            match finding.kind {
                FindingKind::Vulnerability => counts.vulnerabilities += 1,
                FindingKind::Unsound => counts.unsound += 1,
                FindingKind::Unmaintained => counts.unmaintained += 1,
                FindingKind::Notice => counts.notices += 1,
            }
        }
        counts
    }
}

fn id_rank(id: &str) -> u8 {
    if id.starts_with("RUSTSEC-") {
        0
    } else if id.starts_with("GHSA-") {
        1
    } else {
        2
    }
}

/// Turns raw batch hits plus fetched records into the rows to show.
///
/// - The same advisory published under several ids (GitHub's and RustSec's
///   list each other as aliases, or share a CVE) becomes **one** finding.
/// - Withdrawn records are dropped.
/// - Kind comes from RustSec's informational marker; severity from a label,
///   else a computed CVSS v3 score, else unknown.
/// - An id with no record in `records` — the detail request failed — is
///   still reported, flagged `details_missing`, rather than silently lost.
///
/// Sorted with vulnerabilities first, most severe first (unknown last
/// within a kind), then by package and version.
pub fn merge_findings(hits: &[PackageAdvisories], records: &[AdvisoryRecord]) -> Vec<Finding> {
    let by_id: HashMap<&str, &AdvisoryRecord> = records
        .iter()
        .map(|record| (record.id.as_str(), record))
        .collect();

    let mut findings = Vec::new();
    for hit in hits {
        // Group this package's ids by shared identity: an id and its aliases
        // form a set, and sets that overlap are the same advisory.
        let mut groups: Vec<(BTreeSet<&str>, Vec<&str>)> = Vec::new();
        for advisory in &hit.advisories {
            let id = advisory.id.as_str();
            if by_id.get(id).is_some_and(|record| record.withdrawn) {
                continue;
            }
            let mut identity: BTreeSet<&str> = BTreeSet::from([id]);
            if let Some(record) = by_id.get(id) {
                identity.extend(record.aliases.iter().map(String::as_str));
            }
            let mut members = vec![id];
            // Fold in every existing group this one overlaps.
            let mut index = 0;
            while index < groups.len() {
                if groups[index].0.is_disjoint(&identity) {
                    index += 1;
                } else {
                    let (other_identity, other_members) = groups.remove(index);
                    identity.extend(other_identity);
                    members.extend(other_members);
                }
            }
            groups.push((identity, members));
        }

        for (identity, mut members) in groups {
            members.sort_by(|a, b| id_rank(a).cmp(&id_rank(b)).then(a.cmp(b)));
            members.dedup();
            let primary = members[0];
            let known: Vec<&AdvisoryRecord> = members
                .iter()
                .filter_map(|id| by_id.get(id).copied())
                .collect();

            let affected: Vec<&AffectedPackage> = known
                .iter()
                .flat_map(|record| record.affected.iter())
                .filter(|affected| affected.name == hit.name)
                .collect();

            let kind = affected
                .iter()
                .find_map(|affected| affected.informational.as_deref())
                .map_or(FindingKind::Vulnerability, FindingKind::from_informational);
            let severity = known.iter().filter_map(|record| record.severity()).max();

            let locked = Version::parse(&hit.version).ok();
            let mut fixed: Vec<(Option<Version>, &str)> = affected
                .iter()
                .flat_map(|affected| affected.fixed.iter())
                .map(|fixed| (Version::parse(fixed).ok(), fixed.as_str()))
                // Keep what can't be compared; drop only what is provably
                // not newer than the locked version.
                .filter(|(version, _)| match (version, &locked) {
                    (Some(version), Some(locked)) => version > locked,
                    _ => true,
                })
                .collect();
            fixed.sort();
            fixed.dedup();

            let other_ids: Vec<String> = identity
                .iter()
                .filter(|id| **id != primary)
                .map(|id| id.to_string())
                .collect();

            findings.push(Finding {
                package: hit.name.clone(),
                version: hit.version.clone(),
                id: primary.to_string(),
                other_ids,
                kind,
                severity,
                summary: known.iter().find_map(|record| record.summary.clone()),
                fixed_in: fixed.into_iter().map(|(_, text)| text.to_string()).collect(),
                url: known
                    .iter()
                    .find_map(|record| record.url.clone())
                    .unwrap_or_else(|| advisory_page_url(primary)),
                details_missing: known.is_empty(),
            });
        }
    }

    findings.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            // `Some` sorts after `None`, so reverse to put the most severe
            // first and unknown last.
            .then(b.severity.cmp(&a.severity))
            .then(a.package.to_lowercase().cmp(&b.package.to_lowercase()))
            .then(a.version.cmp(&b.version))
            .then(a.id.cmp(&b.id))
    });
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    fn package(name: &str, version: &str) -> LockedPackage {
        LockedPackage {
            name: name.to_string(),
            version: version.to_string(),
            source: Some(CRATES_IO.to_string()),
            dependencies: Vec::new(),
        }
    }

    fn hit(name: &str, version: &str, ids: &[&str]) -> PackageAdvisories {
        PackageAdvisories {
            name: name.to_string(),
            version: version.to_string(),
            advisories: ids
                .iter()
                .map(|id| AdvisoryRef { id: id.to_string(), modified: None })
                .collect(),
            truncated: false,
        }
    }

    /// Records shaped like the real ones OSV returned for this fork.
    fn rustls_github() -> AdvisoryRecord {
        parse_advisory(
            &json!({
                "id": "GHSA-2mjx-qc3c-rqvc",
                "summary": "Rustls: TLS 1.3 handshake messages incorrectly accepted",
                "details": "Long description.",
                "aliases": ["RUSTSEC-2026-0285"],
                "modified": "2026-09-10T03:49:08Z",
                "database_specific": { "severity": "MODERATE", "cwe_ids": ["CWE-20"] },
                "severity": [{ "type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:L/A:N" }],
                "affected": [{
                    "package": { "name": "rustls", "ecosystem": "crates.io" },
                    "ranges": [{ "type": "SEMVER", "events": [{ "introduced": "0.23.0" }, { "fixed": "0.23.45" }] }]
                }],
                "references": [
                    { "type": "WEB", "url": "https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc" },
                    { "type": "PACKAGE", "url": "https://github.com/rustls/rustls" }
                ]
            })
            .to_string(),
        )
        .unwrap()
    }

    fn rustls_rustsec() -> AdvisoryRecord {
        parse_advisory(
            &json!({
                "id": "RUSTSEC-2026-0285",
                "summary": "TLS 1.3 handshake messages incorrectly accepted",
                "aliases": ["GHSA-2mjx-qc3c-rqvc"],
                "affected": [{
                    "package": { "name": "rustls", "ecosystem": "crates.io", "purl": "pkg:cargo/rustls" },
                    "ranges": [{ "type": "SEMVER", "events": [{ "introduced": "0.0.0-0" }, { "fixed": "0.23.45" }] }],
                    "database_specific": { "informational": null, "categories": [], "cvss": null }
                }],
                "references": [
                    { "type": "PACKAGE", "url": "https://crates.io/crates/rustls" },
                    { "type": "ADVISORY", "url": "https://rustsec.org/advisories/RUSTSEC-2026-0285.html" }
                ]
            })
            .to_string(),
        )
        .unwrap()
    }

    fn rsa_rustsec() -> AdvisoryRecord {
        parse_advisory(
            &json!({
                "id": "RUSTSEC-2023-0071",
                "summary": "Marvin Attack: potential key recovery through timing sidechannels",
                "aliases": ["CVE-2023-49092", "GHSA-4grx-2x9w-596c"],
                "severity": [{ "type": "CVSS_V3", "score": "CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:H/I:N/A:N" }],
                "affected": [{
                    "package": { "name": "rsa", "ecosystem": "crates.io" },
                    "ranges": [{ "type": "SEMVER", "events": [{ "introduced": "0.0.0-0" }] }],
                    "database_specific": {
                        "informational": null,
                        "cvss": "CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:H/I:N/A:N"
                    }
                }],
                "references": [{ "type": "ADVISORY", "url": "https://rustsec.org/advisories/RUSTSEC-2023-0071.html" }]
            })
            .to_string(),
        )
        .unwrap()
    }

    fn informational(id: &str, name: &str, marker: &str) -> AdvisoryRecord {
        parse_advisory(
            &json!({
                "id": id,
                "summary": format!("{name} is {marker}"),
                "affected": [{
                    "package": { "name": name, "ecosystem": "crates.io" },
                    "ranges": [{ "type": "SEMVER", "events": [{ "introduced": "0.0.0-0" }] }],
                    "database_specific": { "informational": marker }
                }]
            })
            .to_string(),
        )
        .unwrap()
    }

    // ── batch ──

    #[test]
    fn batches_ask_about_each_package_once_in_order() {
        let (serde, log) = (package("serde", "1.0.210"), package("log", "0.4.22"));
        let batches = osv_batches(&[&serde, &log, &serde]);
        assert_eq!(batches.len(), 1);
        assert_eq!(
            batches[0].packages,
            vec![("serde".to_string(), "1.0.210".to_string()), ("log".to_string(), "0.4.22".to_string())]
        );
        let body: Value = serde_json::from_str(&batches[0].body).unwrap();
        assert_eq!(
            body,
            json!({ "queries": [
                { "package": { "name": "serde", "ecosystem": "crates.io" }, "version": "1.0.210" },
                { "package": { "name": "log", "ecosystem": "crates.io" }, "version": "0.4.22" }
            ]})
        );
    }

    #[test]
    fn two_versions_of_a_package_are_two_queries() {
        let (old, new) = (package("windows-sys", "0.52.0"), package("windows-sys", "0.59.0"));
        assert_eq!(osv_batches(&[&old, &new])[0].packages.len(), 2);
    }

    #[test]
    fn batches_never_exceed_the_limit() {
        let packages: Vec<LockedPackage> = (0..2_500)
            .map(|n| package(&format!("crate-{n}"), "1.0.0"))
            .collect();
        let refs: Vec<&LockedPackage> = packages.iter().collect();
        let batches = osv_batches(&refs);

        let sizes: Vec<usize> = batches.iter().map(|batch| batch.packages.len()).collect();
        assert_eq!(sizes, vec![1000, 1000, 500]);
        for batch in &batches {
            let body: Value = serde_json::from_str(&batch.body).unwrap();
            assert_eq!(body["queries"].as_array().unwrap().len(), batch.packages.len());
        }
        // Nothing lost or repeated across the split.
        let asked: usize = sizes.iter().sum();
        assert_eq!(asked, 2_500);
        assert!(osv_batches(&[]).is_empty());
    }

    #[test]
    fn batch_answers_are_matched_by_position() {
        let (time, serde, log) = (package("time", "0.1.43"), package("serde", "1.0.210"), package("log", "0.4.22"));
        let batch = osv_batches(&[&time, &serde, &log]).remove(0);
        let hits = batch
            .parse_response(
                &json!({ "results": [
                    { "vulns": [
                        { "id": "GHSA-wcg3-cvx6-7396", "modified": "2026-09-10T03:49:08.359875Z" },
                        { "id": "RUSTSEC-2020-0071", "modified": "2026-02-04T02:31:56.682937Z" }
                    ]},
                    {},
                    { "vulns": [{ "id": "RUSTSEC-0000-0001" }], "next_page_token": "abc" }
                ]})
                .to_string(),
            )
            .unwrap();

        // The clean package in the middle is not reported.
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[0].name.as_str(), hits[0].version.as_str()), ("time", "0.1.43"));
        assert_eq!(hits[0].advisories.len(), 2);
        assert_eq!(hits[0].advisories[0].id, "GHSA-wcg3-cvx6-7396");
        assert_eq!(
            hits[0].advisories[0].modified.as_deref(),
            Some("2026-09-10T03:49:08.359875Z")
        );
        assert!(!hits[0].truncated);

        assert_eq!(hits[1].name, "log");
        assert_eq!(hits[1].advisories[0].modified, None);
        assert!(hits[1].truncated);

        assert_eq!(
            advisory_ids(&hits),
            vec!["GHSA-wcg3-cvx6-7396", "RUSTSEC-0000-0001", "RUSTSEC-2020-0071"]
        );
    }

    #[test]
    fn a_misaligned_or_refused_batch_answer_is_an_error() {
        let (a, b) = (package("a", "1.0.0"), package("b", "1.0.0"));
        let batch = osv_batches(&[&a, &b]).remove(0);

        let short = batch.parse_response(r#"{"results":[{}]}"#).unwrap_err();
        assert!(short.contains("1 of 2"), "{short}");

        let refused = batch
            .parse_response(r#"{"code":3,"message":"too many queries"}"#)
            .unwrap_err();
        assert!(refused.contains("too many queries"), "{refused}");

        assert!(batch.parse_response("<html>502</html>").is_err());
        assert!(batch.parse_response("{}").is_err());
        // All clean is a valid answer.
        assert!(batch.parse_response(r#"{"results":[{},{}]}"#).unwrap().is_empty());
    }

    // ── severity ──

    #[test]
    fn cvss3_scores_match_the_published_calculator() {
        let score = |vector: &str| cvss3_base_tenths(vector);
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), Some(98));
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H"), Some(100));
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"), Some(75));
        assert_eq!(score("CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:H/I:N/A:N"), Some(59));
        assert_eq!(score("CVSS:3.1/AV:L/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:H"), Some(78));
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:C/C:L/I:L/A:N"), Some(61));
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:L/A:N"), Some(53));
        assert_eq!(score("CVSS:3.0/AV:P/AC:H/PR:H/UI:R/S:U/C:L/I:N/A:N"), Some(16));
        // No impact at all scores zero, whatever the exploitability.
        assert_eq!(score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N"), Some(0));
    }

    #[test]
    fn privileges_weigh_more_when_scope_changes() {
        assert_eq!(
            cvss3_base_tenths("CVSS:3.1/AV:N/AC:L/PR:L/UI:N/S:U/C:H/I:H/A:H"),
            Some(88)
        );
        assert_eq!(
            cvss3_base_tenths("CVSS:3.1/AV:N/AC:L/PR:L/UI:N/S:C/C:H/I:H/A:H"),
            Some(99)
        );
    }

    #[test]
    fn cvss_vectors_that_cannot_be_scored() {
        // v4 is a lookup table, not this formula: unknown, not approximated.
        assert_eq!(
            cvss3_base_tenths("CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N"),
            None
        );
        assert_eq!(cvss3_base_tenths("CVSS:2.0/AV:N/AC:L/Au:N/C:P/I:P/A:P"), None);
        // Incomplete or malformed.
        assert_eq!(cvss3_base_tenths("CVSS:3.1/AV:N/AC:L"), None);
        assert_eq!(cvss3_base_tenths("CVSS:3.1/AV:X/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), None);
        assert_eq!(cvss3_base_tenths(""), None);
        assert_eq!(cvss3_base_tenths("high"), None);
    }

    #[test]
    fn severity_bands_and_labels() {
        assert_eq!(Severity::from_cvss_tenths(0), None);
        assert_eq!(Severity::from_cvss_tenths(1), Some(Severity::Low));
        assert_eq!(Severity::from_cvss_tenths(39), Some(Severity::Low));
        assert_eq!(Severity::from_cvss_tenths(40), Some(Severity::Moderate));
        assert_eq!(Severity::from_cvss_tenths(69), Some(Severity::Moderate));
        assert_eq!(Severity::from_cvss_tenths(70), Some(Severity::High));
        assert_eq!(Severity::from_cvss_tenths(89), Some(Severity::High));
        assert_eq!(Severity::from_cvss_tenths(90), Some(Severity::Critical));
        assert_eq!(Severity::from_cvss_tenths(100), Some(Severity::Critical));

        assert_eq!(Severity::from_label("MODERATE"), Some(Severity::Moderate));
        assert_eq!(Severity::from_label("Medium"), Some(Severity::Moderate));
        assert_eq!(Severity::from_label(" critical "), Some(Severity::Critical));
        assert_eq!(Severity::from_label("unknown"), None);
        assert_eq!(Severity::from_label(""), None);
        assert!(Severity::Critical > Severity::High && Severity::High > Severity::Moderate);
        assert_eq!(Severity::Moderate.label(), "moderate");
    }

    // ── records ──

    #[test]
    fn a_github_record() {
        let record = rustls_github();
        assert_eq!(record.id, "GHSA-2mjx-qc3c-rqvc");
        assert_eq!(record.aliases, vec!["RUSTSEC-2026-0285"]);
        assert_eq!(record.severity_label, Some(Severity::Moderate));
        assert_eq!(record.severity(), Some(Severity::Moderate));
        assert!(!record.withdrawn);
        assert_eq!(record.affected.len(), 1);
        assert_eq!(record.affected[0].name, "rustls");
        assert_eq!(record.affected[0].fixed, vec!["0.23.45"]);
        assert_eq!(record.affected[0].informational, None);
        // No ADVISORY link, so the first WEB one.
        assert_eq!(
            record.url.as_deref(),
            Some("https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc")
        );
    }

    #[test]
    fn a_rustsec_record_gets_its_severity_from_its_vector() {
        let record = rsa_rustsec();
        assert_eq!(record.severity_label, None);
        // Listed twice in the record (top level and per package), kept once.
        assert_eq!(record.cvss_vectors.len(), 1);
        assert_eq!(cvss3_base_tenths(&record.cvss_vectors[0]), Some(59));
        assert_eq!(record.severity(), Some(Severity::Moderate));
        // No fix has been published.
        assert!(record.affected[0].fixed.is_empty());
        assert_eq!(
            record.url.as_deref(),
            Some("https://rustsec.org/advisories/RUSTSEC-2023-0071.html")
        );
    }

    #[test]
    fn a_record_with_no_label_and_no_scorable_vector_has_unknown_severity() {
        assert_eq!(rustls_rustsec().severity(), None);
        let v4_only = parse_advisory(
            &json!({
                "id": "RUSTSEC-0000-0002",
                "severity": [{ "type": "CVSS_V4", "score": "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:N/SI:N/SA:N" }]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(v4_only.cvss_vectors.len(), 1);
        assert_eq!(v4_only.severity(), None);
    }

    #[test]
    fn other_ecosystems_and_withdrawn_records() {
        let record = parse_advisory(
            &json!({
                "id": "GHSA-xxxx",
                "withdrawn": "2026-01-01T00:00:00Z",
                "affected": [
                    { "package": { "name": "left-pad", "ecosystem": "npm" },
                      "ranges": [{ "events": [{ "fixed": "9.9.9" }] }] },
                    { "package": { "name": "demo", "ecosystem": "crates.io" },
                      "ranges": [
                          { "events": [{ "introduced": "0" }, { "fixed": "0.103.13" }] },
                          { "events": [{ "introduced": "0.104.0-alpha.1" }, { "fixed": "0.104.0-alpha.7" }] }
                      ] }
                ]
            })
            .to_string(),
        )
        .unwrap();
        assert!(record.withdrawn);
        assert_eq!(record.affected.len(), 1);
        assert_eq!(record.affected[0].name, "demo");
        assert_eq!(record.affected[0].fixed, vec!["0.103.13", "0.104.0-alpha.7"]);
        assert_eq!(record.url, None);
    }

    #[test]
    fn unreadable_records_are_errors() {
        assert!(parse_advisory("").is_err());
        assert!(parse_advisory("{}").is_err());
        let refused = parse_advisory(r#"{"code":5,"message":"Bug not found."}"#).unwrap_err();
        assert!(refused.contains("Bug not found"), "{refused}");
    }

    // ── findings ──

    #[test]
    fn the_same_advisory_under_two_ids_is_one_finding() {
        let hits = [hit("rustls", "0.23.31", &["GHSA-2mjx-qc3c-rqvc", "RUSTSEC-2026-0285"])];
        let findings = merge_findings(&hits, &[rustls_github(), rustls_rustsec()]);

        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.package, "rustls");
        assert_eq!(finding.version, "0.23.31");
        // Shown under the RustSec id, with the other alongside.
        assert_eq!(finding.id, "RUSTSEC-2026-0285");
        assert_eq!(finding.other_ids, vec!["GHSA-2mjx-qc3c-rqvc"]);
        assert_eq!(finding.kind, FindingKind::Vulnerability);
        // Only the GitHub record rates it; the merged finding keeps that.
        assert_eq!(finding.severity, Some(Severity::Moderate));
        assert_eq!(finding.fixed_in, vec!["0.23.45"]);
        assert_eq!(finding.url, "https://rustsec.org/advisories/RUSTSEC-2026-0285.html");
        assert!(!finding.details_missing);
    }

    #[test]
    fn records_sharing_only_a_cve_are_still_one_finding() {
        let github = parse_advisory(
            &json!({ "id": "GHSA-aaaa", "aliases": ["CVE-2026-1"],
                     "database_specific": { "severity": "HIGH" } })
            .to_string(),
        )
        .unwrap();
        let rustsec = parse_advisory(
            &json!({ "id": "RUSTSEC-2026-0001", "aliases": ["CVE-2026-1"] }).to_string(),
        )
        .unwrap();
        let unrelated = parse_advisory(&json!({ "id": "RUSTSEC-2026-0002" }).to_string()).unwrap();

        let hits = [hit("demo", "1.0.0", &["GHSA-aaaa", "RUSTSEC-2026-0002", "RUSTSEC-2026-0001"])];
        let findings = merge_findings(&hits, &[github, rustsec, unrelated]);

        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].id, "RUSTSEC-2026-0001");
        assert_eq!(findings[0].other_ids, vec!["CVE-2026-1", "GHSA-aaaa"]);
        assert_eq!(findings[0].severity, Some(Severity::High));
        assert_eq!(findings[1].id, "RUSTSEC-2026-0002");
        assert!(findings[1].other_ids.is_empty());
    }

    #[test]
    fn informational_notices_are_not_vulnerabilities() {
        let hits = [
            hit("old-crate", "0.1.0", &["RUSTSEC-2024-0001"]),
            hit("shaky", "0.2.0", &["RUSTSEC-2024-0002"]),
            hit("odd", "0.3.0", &["RUSTSEC-2024-0003"]),
            hit("rsa", "0.9.6", &["RUSTSEC-2023-0071"]),
        ];
        let records = [
            informational("RUSTSEC-2024-0001", "old-crate", "unmaintained"),
            informational("RUSTSEC-2024-0002", "shaky", "unsound"),
            informational("RUSTSEC-2024-0003", "odd", "notice"),
            rsa_rustsec(),
        ];
        let findings = merge_findings(&hits, &records);

        let kinds: Vec<(&str, FindingKind)> = findings
            .iter()
            .map(|finding| (finding.package.as_str(), finding.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("rsa", FindingKind::Vulnerability),
                ("shaky", FindingKind::Unsound),
                ("old-crate", FindingKind::Unmaintained),
                ("odd", FindingKind::Notice),
            ]
        );
        assert_eq!(
            FindingCounts::of(&findings),
            FindingCounts { vulnerabilities: 1, unsound: 1, unmaintained: 1, notices: 1 }
        );
        assert_eq!(FindingKind::Unmaintained.label(), "unmaintained");
    }

    #[test]
    fn a_rated_unsoundness_keeps_both_its_kind_and_its_severity() {
        // Real shape: GitHub rates it, RustSec marks it informational.
        let github = parse_advisory(
            &json!({ "id": "GHSA-wrw7", "aliases": ["RUSTSEC-2024-0429"],
                     "database_specific": { "severity": "MODERATE" },
                     "affected": [{ "package": { "name": "glib", "ecosystem": "crates.io" },
                                    "ranges": [{ "events": [{ "fixed": "0.20.0" }] }] }] })
            .to_string(),
        )
        .unwrap();
        let rustsec = informational("RUSTSEC-2024-0429", "glib", "unsound");
        let findings = merge_findings(
            &[hit("glib", "0.18.5", &["GHSA-wrw7", "RUSTSEC-2024-0429"])],
            &[github, rustsec],
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::Unsound);
        assert_eq!(findings[0].severity, Some(Severity::Moderate));
        assert_eq!(findings[0].fixed_in, vec!["0.20.0"]);
    }

    #[test]
    fn fixed_versions_are_only_those_newer_than_the_locked_one() {
        let record = parse_advisory(
            &json!({
                "id": "RUSTSEC-2026-0104",
                "affected": [{
                    "package": { "name": "rustls-webpki", "ecosystem": "crates.io" },
                    "ranges": [{ "events": [
                        { "fixed": "0.104.0-alpha.7" }, { "fixed": "0.102.9" },
                        { "fixed": "0.103.13" }, { "fixed": "0.103.13" }
                    ]}]
                }]
            })
            .to_string(),
        )
        .unwrap();

        // Locked on the 0.103 line: the 0.102 fix is behind it.
        let findings = merge_findings(
            &[hit("rustls-webpki", "0.103.4", &["RUSTSEC-2026-0104"])],
            std::slice::from_ref(&record),
        );
        assert_eq!(findings[0].fixed_in, vec!["0.103.13", "0.104.0-alpha.7"]);

        // A locked version that isn't semver can't be compared: keep them all.
        let findings = merge_findings(
            &[hit("rustls-webpki", "weird", &["RUSTSEC-2026-0104"])],
            std::slice::from_ref(&record),
        );
        assert_eq!(findings[0].fixed_in.len(), 3);
    }

    #[test]
    fn advice_about_another_package_does_not_leak_into_this_one() {
        // One advisory covering two crates: each gets its own fix versions.
        let record = parse_advisory(
            &json!({
                "id": "RUSTSEC-2026-0500",
                "affected": [
                    { "package": { "name": "alpha", "ecosystem": "crates.io" },
                      "ranges": [{ "events": [{ "fixed": "1.2.0" }] }] },
                    { "package": { "name": "beta", "ecosystem": "crates.io" },
                      "ranges": [{ "events": [{ "fixed": "7.0.0" }] }],
                      "database_specific": { "informational": "unmaintained" } }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let findings = merge_findings(
            &[hit("alpha", "1.0.0", &["RUSTSEC-2026-0500"])],
            std::slice::from_ref(&record),
        );
        assert_eq!(findings[0].fixed_in, vec!["1.2.0"]);
        assert_eq!(findings[0].kind, FindingKind::Vulnerability);
    }

    #[test]
    fn withdrawn_advisories_are_dropped() {
        let withdrawn = parse_advisory(
            &json!({ "id": "RUSTSEC-2020-0001", "withdrawn": "2021-01-01T00:00:00Z" }).to_string(),
        )
        .unwrap();
        let findings = merge_findings(
            &[hit("demo", "1.0.0", &["RUSTSEC-2020-0001", "RUSTSEC-2023-0071"])],
            &[withdrawn, rsa_rustsec()],
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "RUSTSEC-2023-0071");
    }

    #[test]
    fn an_advisory_whose_details_could_not_be_fetched_is_still_reported() {
        let findings = merge_findings(&[hit("demo", "1.0.0", &["RUSTSEC-2026-0999"])], &[]);
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert!(finding.details_missing);
        assert_eq!(finding.id, "RUSTSEC-2026-0999");
        assert_eq!(finding.severity, None);
        assert_eq!(finding.summary, None);
        assert!(finding.fixed_in.is_empty());
        assert_eq!(finding.url, "https://osv.dev/vulnerability/RUSTSEC-2026-0999");
    }

    #[test]
    fn two_locked_versions_of_a_package_are_separate_findings() {
        let hits = [
            hit("rsa", "0.8.2", &["RUSTSEC-2023-0071"]),
            hit("rsa", "0.9.6", &["RUSTSEC-2023-0071"]),
        ];
        let findings = merge_findings(&hits, &[rsa_rustsec()]);
        let versions: Vec<&str> = findings.iter().map(|finding| finding.version.as_str()).collect();
        assert_eq!(versions, vec!["0.8.2", "0.9.6"]);
    }

    #[test]
    fn findings_are_ordered_worst_first_with_unknown_last() {
        let record = |id: &str, label: Option<&str>| {
            let mut value = json!({ "id": id });
            if let Some(label) = label {
                value["database_specific"] = json!({ "severity": label });
            }
            parse_advisory(&value.to_string()).unwrap()
        };
        let hits = [
            hit("zeta", "1.0.0", &["A-LOW"]),
            hit("alpha", "1.0.0", &["A-UNKNOWN"]),
            hit("mid", "1.0.0", &["A-CRITICAL"]),
            hit("beta", "1.0.0", &["A-HIGH"]),
            hit("stale", "1.0.0", &["RUSTSEC-2024-0001"]),
        ];
        let records = [
            record("A-LOW", Some("LOW")),
            record("A-UNKNOWN", None),
            record("A-CRITICAL", Some("CRITICAL")),
            record("A-HIGH", Some("HIGH")),
            informational("RUSTSEC-2024-0001", "stale", "unmaintained"),
        ];
        let findings = merge_findings(&hits, &records);
        let order: Vec<&str> = findings
            .iter()
            .map(|finding| finding.package.as_str())
            .collect();
        assert_eq!(order, vec!["mid", "beta", "zeta", "alpha", "stale"]);
    }

    #[test]
    fn no_hits_no_findings() {
        assert!(merge_findings(&[], &[rsa_rustsec()]).is_empty());
        assert_eq!(FindingCounts::of(&[]), FindingCounts::default());
        assert!(advisory_ids(&[]).is_empty());
    }
}
