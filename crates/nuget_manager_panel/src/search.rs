//! Registry search and package-details fetching: `search_nuget` / `fetch_details`
//! run on the panel's context and store results on `NuGetManagerPanel`. The
//! JSON parsing (azuresearch response, registration catalog → details) lives in
//! `dotnet_backend` — this module only performs the HTTP calls.

use std::sync::Arc;

use dotnet_backend::{
    NugetPackageDetails, NugetSearchResult, nuget_flat_readme_url, nuget_registration_url,
    parse_nuget_details, parse_nuget_search, registration_items_from_pages,
};
use futures::io::AsyncReadExt as _;
use gpui::{AsyncApp, Context, WeakEntity};

use http_client::HttpClient;

use crate::NuGetManagerPanel;

const SEARCH_URL: &str = "https://azuresearch-usnc.nuget.org/query";
/// Results per page — the Forge search pages 15 at a time, and 15 is also
/// what nuget.org's own search UI returns per page.
const PAGE_SIZE: usize = 15;

impl NuGetManagerPanel {
    pub(crate) fn search_nuget(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().trim().to_string();
        if query.is_empty() {
            self.search_results.clear();
            self.search_total = 0;
            self.search_error = None;
            self.search_loading = false;
            cx.notify();
            return;
        }
        self.search_loading = true;
        self.search_error = None;
        cx.notify();

        let http_client = cx.http_client();
        let page = self.search_page;
        let url = format!(
            "{SEARCH_URL}?q={}&take={PAGE_SIZE}&skip={}&prerelease=false",
            urlencoding::encode(&query),
            page * PAGE_SIZE
        );
        let this = cx.weak_entity();

        cx.spawn(async move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome: Result<(Vec<NugetSearchResult>, usize), String> =
                fetch_json(&http_client, &url).await.and_then(|json| {
                    let (results, total) = parse_nuget_search(&json)?;
                    Ok((results, total as usize))
                });
            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.search_loading = false;
                        match outcome {
                            Ok((results, total)) => {
                                if panel.search_page == 0 {
                                    panel.search_results = results;
                                } else {
                                    panel.search_results.extend(results);
                                }
                                panel.search_total = total;
                            }
                            Err(e) => panel.search_error = Some(e),
                        }
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }

    pub(crate) fn fetch_details(&mut self, name: String, cx: &mut Context<Self>) {
        self.fetch_details_with_mode(name, false, cx);
    }

    /// Like [`Self::fetch_details`] but opens the details pane straight to the
    /// markdown-rendered README (when the package has one) — the search
    /// card's "More info" button.
    pub(crate) fn fetch_details_and_readme(&mut self, name: String, cx: &mut Context<Self>) {
        self.fetch_details_with_mode(name, true, cx);
    }

    fn fetch_details_with_mode(
        &mut self,
        name: String,
        open_readme_on_load: bool,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(name.clone());
        self.details = None;
        self.details_error = None;
        self.details_loading = true;
        self.show_readme = false;
        self.open_readme_on_load = open_readme_on_load;
        cx.notify();

        let http_client = cx.http_client();
        let index_url = nuget_registration_url(&name);
        let this = cx.weak_entity();

        cx.spawn(async move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Registration pages whose version list doesn't fit in the index
            // aren't inlined — only an "@id" pointing at a separate page
            // document is present. Popular packages (exactly what searches
            // surface) hit this, so the pages are fetched individually.
            let result: Result<NugetPackageDetails, String> = async {
                let index = fetch_json(&http_client, &index_url).await?;
                let mut pages: Vec<serde_json::Value> = Vec::new();
                if let Some(items) = index.get("items").and_then(|v| v.as_array()) {
                    for page in items {
                        let has_items = page
                            .get("items")
                            .and_then(|v| v.as_array())
                            .map(|items| !items.is_empty())
                            .unwrap_or(false);
                        if has_items {
                            pages.push(page.clone());
                        } else if let Some(page_url) = page.get("@id").and_then(|v| v.as_str()) {
                            if let Ok(page_json) = fetch_json(&http_client, page_url).await {
                                pages.push(page_json);
                            }
                        }
                    }
                }
                let catalog = registration_items_from_pages(&pages);
                let mut details = parse_nuget_details(&catalog)?;
                // The v3-flatcontainer README is always raw markdown (no HTML
                // gallery wrapper, unlike the catalog's readmeUrl) — a 404
                // just means the package ships no embedded README.
                let readme_url = nuget_flat_readme_url(&details.id, &details.version);
                details.readme = fetch_text(&http_client, &readme_url).await.ok().flatten();
                Ok(details)
            }
            .await;

            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.details_loading = false;
                        match result {
                            Ok(details) => {
                                let open_readme = open_readme_on_load && details.readme.is_some();
                                panel.show_readme = open_readme;
                                panel.open_readme_on_load = false;
                                panel.details = Some(details);
                            }
                            Err(e) => panel.details_error = Some(e),
                        }
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }
}

async fn fetch_json(
    http_client: &Arc<dyn HttpClient>,
    url: &str,
) -> Result<serde_json::Value, String> {
    let mut response = http_client
        .get(url, Default::default(), true)
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&body).map_err(|e| format!("invalid JSON: {e}"))
}

/// Like [`fetch_json`] but returns `None` for non-success statuses — used for
/// the flatcontainer README, where "not found" is a normal state.
async fn fetch_text(
    http_client: &Arc<dyn HttpClient>,
    url: &str,
) -> Result<Option<String>, String> {
    let mut response = http_client
        .get(url, Default::default(), true)
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some(String::from_utf8_lossy(&body).into_owned()))
}
