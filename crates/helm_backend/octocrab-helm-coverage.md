# Octocrab vs. the `gh` CLI in the Helm backend

*Review of `mentenaz/zed_source` (`crates/helm_backend`, `crates/helm_panel`) against octocrab 0.54.x. Written 18 Oct 2026.*

> **How to read this document.** **Sections 9 and 10 are the agreed plan.** Sections 1 to 8 are background, written first: the inventory, endpoint coverage and gap analysis that led to the plan. Section 4 was corrected after reading the fork source (errors keep headers; the cache layer has four fixable problems). The recommendation in section 6 ("keep your REST layer") is **superseded** by section 9. Revised 19 Oct 2026 after a review against the `zed_source` tree.

---

## TL;DR

1. **Helm does not use `gh api`.** Every REST call already goes straight to `api.github.com` through Zed's own `http_client` (`helm_backend/src/github/api.rs`, `send_once`). The `gh` CLI is used for the "gh required" gate, getting the token, the sign-in lifecycle (login, scope refresh, logout, status), and `gh repo clone`.
2. **Decided direction (section 9):** fork octocrab (`mentenaz/octocrab`), patch its cache layer (P1 to P4) and any failing models (P5), and pin it by git revision as a Helm-only dependency (`octocrab_helm`). Helm's REST layer moves onto octocrab, plugged into Zed's `http_client` through a `tower::Service` adapter so proxy settings are kept. Sign-in uses your own OAuth App with device flow; cloning uses `git` with the token in env config.
3. **Stays in `zed_source`, not in the fork:** mapping octocrab errors into `GhError`, per-resource rate-limit tracking and the 8-request limiter, token storage, the device-flow UI, and cloning via `git`.
4. **Octocrab cannot remove the `gh` dependency by itself.** It has no token storage and cannot clone. Dropping `gh` needs your own OAuth App (device flow), token storage via `credentials_provider` (Windows Credential Manager in every release channel since 19 Oct; see section 10), and a `git` clone with credentials.
5. **Several accounts, bound per workspace (section 10):** sign in once per account, and each workspace window uses its own account and its own client.

---

## Background (sections 1 to 8)

## 1. Where Helm uses `gh` today (verified from source)

| Location | `gh` command | Purpose |
|---|---|---|
| `github/cli.rs` `gh_check_cli` | `gh --version` | "gh required" gate screen |
| `github/cli.rs` `gh_auth_status` | `gh auth status` | Parses account name and token scopes from text output |
| `github/cli.rs` `gh_login` | `gh auth login --hostname github.com --git-protocol https --scopes repo,read:org,user --web` | Device-code sign-in; output lines streamed to the panel |
| `github/cli.rs` `gh_refresh_scope` | `gh auth refresh -s <scope> --hostname github.com` | Add a missing scope (private) |
| `github/cli.rs` `gh_ensure_scope` | (via `gh_refresh_scope`) | Public entry point for adding a scope, exported from `github/mod.rs` |
| `github/cli.rs` `gh_logout` | `gh auth logout --hostname github.com` | Sign out |
| `github/api.rs` `token()` | `gh auth token` | Fetch the token once, cache it in `GhState.token` |
| `github/clone.rs` | `gh repo clone <repo> <path>` | Clone with streamed progress, then optional `npm install` |
| `helm_panel/src/auth.rs` (~line 73) | (via `gh_auth_status`) | Sign-in screen: reads the signed-in account and scopes |
| `helm_panel/src/changes.rs` `run_action` | (via `gh_auth_status`) | After a 401/403/404, decide whether the token lacks a scope |
| `helm_workspace/src/tab.rs` | (via `gh_auth_status`) | Same scope check |

Not part of Helm, but mentioned for completeness: `tooling/xtask` workflows and `CLAUDE.md` release notes use `gh` for CI and release tagging. Octocrab is irrelevant there.

**Already in your tree:** `octocrab = { version = "0.49", default-features = false }` is a workspace dependency (0.49.7 in `Cargo.lock`), used by `tooling/compliance` for GitHub App auth. That file has a comment (`github.rs` ~line 669) saying octocrab *"fails to deserialize the permission response"* for repository collaborator permissions. That is a real data point on how reliable its typed models are.

---

## 2. Endpoint coverage

Every endpoint Helm calls today (`requests.rs`) is reachable through octocrab's raw methods (`get`, `post`, `patch`, `put`, `delete`, plus `_get`/`_post` variants that give raw status and headers). The table says whether a typed handler also exists. The first version was based on octocrab's published module list; it was **re-checked on 19 Oct against the fork source** (`E:\octocrab`, v0.54.5, `src/api/`), which corrected several "raw only" rows. Individual method signatures and model fields were spot-checked, not checked for every endpoint.

| Helm endpoints | Octocrab typed handler (fork source) | Notes |
|---|---|---|
| Current user, get user, update user | `current`, `users` | Raw works either way |
| Org detail, org members, org memberships, org invitations (accept/decline) | `orgs`; `current.rs` has `get_org_membership` / `update_org_membership` (`/user/memberships/orgs/{org}`) | Accept/decline is `update_org_membership`. `orgs/invitations.rs` is the admin side (list, create, cancel) |
| List/get/create/update repo, topics | `repos`, `repos/topics.rs` | Org-vs-user create routing (`/orgs/{org}/repos` vs `/user/repos`) is your logic either way |
| Branches, tags, commits (by ref, by path), commit detail, compare | `repos`, `commits` (`commits/compare_commit.rs`) | |
| Git trees (incl. truncated-tree handling), blobs | `git` | Raw blob accept type (`application/vnd.github.raw+json`) needs custom headers |
| README (root and at path) | `repos` | |
| Repo search | `search` | |
| Code search with text-match fragments | `search` | **No text-match support anywhere in the fork.** Needs the `application/vnd.github.text-match+json` accept header, so a raw call with custom headers (`get_with_headers`) |
| Issues, issue comments | `issues` | |
| Pull requests, PR for head, create PR | `pulls` | |
| Combined commit status | `repos` (`combined_status_for_ref`, `/commits/{ref}/status`) | |
| Releases, create release | `repos` (releases handler) | |
| Collaborators (list/add/remove), repo invitations (list/accept/decline) | `repos/collaborators.rs`, `repos/invitations.rs`, `current` | |
| Packages (org/user list, versions) | `packages` | Package names are percent-encoded by the fork (`encode_package_name`, `packages.rs:14`), so `owner/image` names are handled |
| Workflow runs (all/by branch), single run, run jobs | `actions`, `workflows` | |
| Deployments | `repos/deployments.rs` | Was listed as raw only; corrected |
| Traffic: views, clones, referrers, paths | `repos/traffic.rs` | Was listed as raw only; corrected |
| Dependabot alerts | `repos/dependabot.rs` | Was listed as raw only; corrected |
| Secret scanning alerts | `repos/secret_scanning_alerts.rs` | Was listed as raw only; corrected |

**Summary:** almost all of Helm's surface has a typed handler in 0.54.5; only text-match code search and the raw/diff accept types need raw calls with custom headers. Because Helm's models in `types.rs` (about 1,200 lines) are shaped for the panel, you would still likely keep them and use octocrab mostly as transport, using typed handlers where their models match.

---

## 3. The `gh` touchpoints: what octocrab can and cannot replace

| `gh` use | Replaceable? | How / what's missing |
|---|---|---|
| `gh --version` gate | Yes, delete it | The gate screen and its error state go away |
| `gh auth login --web` | **Yes**, with a prerequisite | `Octocrab::authenticate_as_device(client_id, scopes)` returns `DeviceCodes` (`user_code`, `verification_uri`, `interval`, `expires_in`); `poll_until_available` returns an `OAuth` token. **You need your own OAuth App** with device flow enabled; you cannot rely on `gh`'s identity. Bonus: `parse_device_line` in `helm_panel/src/auth.rs` (which scrapes the code and URL out of CLI text) is no longer needed, because you get them as structured fields |
| `gh auth refresh -s` | Yes | There is no refresh command; re-run device flow requesting the combined scopes. The existing "authorize missing scope" screen can stay |
| `gh auth token` / token storage | **Not by octocrab** | Octocrab holds a token in memory only. Persist it with `credentials_provider` (`read_credentials` / `write_credentials` / `delete_credentials`). Note: that trait takes an `&AsyncApp`, but `GhState` is currently tokio-based with no app context, so token read/write has to happen panel-side and the token be handed to `GhState` |
| `gh auth status` (account, scopes) | Yes, differently | `GET /user` returns the login. **Verified:** the device-flow result (`auth::OAuth`) already carries `scope: Vec<String>`, plus `expires_in`, `refresh_token` and `refresh_token_expires_in`, so granted scopes are known at sign-in and expiring tokens are supported. For later checks, classic OAuth tokens also return `X-OAuth-Scopes` on any response (read it with the raw `_get`). Fine-grained and GitHub App user tokens do not report scopes this way, so the scope model would change if you go that route |
| `gh auth logout` | Partly | Delete the stored credential and clear `GhState`. Revoking the token on GitHub's side needs the OAuth App's client secret as far as I know, so a device-flow-only app would send the user to GitHub's settings to revoke |
| **`gh repo clone`** | **No** | Octocrab is an API client; it does not clone. See below |

### Clone is the real gap

`gh repo clone` also quietly provides **credentials for later `git push` / `git pull`** in those clones, because `gh auth login` configures git's HTTPS credential helper. If you drop `gh`:

**Decided (19 Oct): `git` subprocess, token passed through the environment.**

- **`git` is already required.** Zed shells out to the `git` binary for its git features and only bundles one on macOS (`crates/zed/src/main.rs:523`), so on Windows and Linux a system `git` is already needed. Requiring it for Helm's clone adds nothing new.
- **Token through env-based config, never in arguments or the URL.** Run `git clone --progress <https url> <path>` with `GIT_CONFIG_COUNT=1`, `GIT_CONFIG_KEY_0=http.https://github.com/.extraHeader` and `GIT_CONFIG_VALUE_0=Authorization: Basic <base64("x-access-token:" + token)>` (git 2.31 or later). The token is then not in the process list and not written to the clone's `.git/config`.
- Set `GIT_TERMINAL_PROMPT=0` so a bad token fails instead of hanging.
- Progress streaming: `git clone --progress` writes to stderr, which maps onto the existing `CloneEvent::Line` channel.
- **Not reusing `Fs::git_clone`** (`crates/fs/src/fs.rs:1273`): it already streams `--progress`, but it takes no credentials and picks the target folder from the URL.
- **No `npm install` in the clone itself any more:** the opened workspace gets `npm_bootstrap`'s offer instead (section 7, item 2).
- **Later push/pull** in cloned repos uses whatever credential helper git has, normally Git Credential Manager (ships with Git for Windows), the same as Zed's own git panel. Helm does not configure a credential helper.

---

## 4. Gaps: what you would have to rebuild or replace if octocrab becomes the REST layer

| Helm feature today | In octocrab |
|---|---|
| Requests go through Zed's `http_client` (shared proxy settings, `GhState.http`) | Octocrab uses hyper/rustls or reqwest. A custom transport is possible with `OctocrabBuilder::with_service` (a tower `Service` you would write as an adapter over `http_client`) or `build_with_reqwest`, but it is real work and the proxy behaviour must be re-verified. **Verified in the fork (19 Oct):** the custom-service `build()` (`lib.rs:675`) only maps the response body. It adds **no** base URI, auth header, redirect following, extra headers or cache layer; only the built-in clients stack those (`lib.rs:988-1014`). The layers are public (`octocrab::service::middleware::{base_uri, auth_header, cache, extra_headers}`), so the adapter must add them itself with `with_layer`. Also, `Octocrab::new` starts a tower `Buffer` worker with tokio (`lib.rs:1480`), so building the client from gpui code would panic: use `with_executor` to run the worker on `reqwest_client::runtime()`, or build inside `on_tokio` |
| ETag conditional GETs: 304 handling, cache invalidation after any successful mutation, `peek_page` for instant redisplay | **Verified in source** (`service/middleware/cache.rs`, `cache/mem.rs`): a real conditional-GET layer exists (`If-None-Match` / `If-Modified-Since`, replays the cached body on 304), but it has four problems for Helm: (1) entries are keyed by URI only, not by URI plus `Accept`, so raw and JSON answers for the same path collide; (2) on a 304 it calls `storage.load(..).expect("no body for cache hit")`, which **panics** if the entry was removed while the request was in flight, the exact race your `send()` handles; (3) the in-memory writer commits on `Drop`, so a body cut off mid-stream can be cached as complete; (4) nothing invalidates entries after a successful POST/PATCH/PUT/DELETE, and there is no `peek` without sending. All four are fork-patch candidates (section 9) |
| Per-resource rate limits (`core`, `search`, `code_search`) tracked passively from response headers, with secondary-limit `Retry-After` cooldowns that refuse requests locally | **Verified:** octocrab has `ratelimit()` (an explicit call to the rate-limit endpoint) and an optional retry layer (`RetryConfig::HandleRateLimits`) that *waits and retries* on 403/429. That is the opposite of Helm's behaviour, which fails fast with `RateLimited` so the panel can show it. No passive per-resource tracking exists, so this stays your own tower layer or observer |
| Concurrency cap of 8 in-flight requests (`RequestLimiter`) | A tower layer would be needed |
| Error taxonomy: `SsoRequired` (with authorization URL from `x-github-sso`), `RateLimited { reset_at, retry_after }`, `Forbidden` vs `NotFound` vs `Validation`, `EmptyRepository` | **Corrected after reading the fork (v0.54.5):** `GitHubError` carries `status_code`, `message`, `errors`, `rate_limit_reset` and the full response `headers` (`map_github_error`, `lib.rs`). So `x-github-sso` and `retry-after` are reachable. Octocrab does not classify errors; you would map `GitHubError` into your `GhError` variants yourself, which is a small function, not a rewrite |
| `Link`-header paging (`Page`, `last_page`, `fetch_page_under` for wrapped lists such as workflow runs) | Octocrab has `Page<T>`, `get_page`, `all_pages`. Wrapped-list endpoints still need checking |
| Accept media types (raw, diff, text-match) | Possible with `get_with_headers`, one call at a time |
| Request-as-data design (`requests.rs`): every endpoint tested with no network | Would be lost or need re-creating |
| Typed models fit for the panel | Octocrab models can lag GitHub. Evidence in your own tree: the compliance tool's comment about failed permission deserialization. Fine-grained scope or permission fields are another risk area |
| Version churn | The crate has had many breaking releases (43 of 95 as of its lib.rs listing). Your tree pins 0.49 (0.49.7 in `Cargo.lock`); the latest release is 0.54.5 (the fork's HEAD), so even your existing pin is behind |

---

## 5. What octocrab adds that Helm does not have

- **Device-flow helpers** (the one clear win; see section 3).
- **GraphQL**: `Octocrab::graphql(payload)`. Useful for fetching several things in one request (for example repo list plus PR counts), which would cut REST round trips.
- **GitHub App support**: JWT creation, installation tokens (already used in `tooling/compliance`). Only relevant if Helm ever acts as an app rather than as the signed-in user.
- **Webhook payload types**, `download_zip`, a retry layer, and a configurable upload URI: none needed today.

---

## 6. Recommended approach (superseded by section 9)

> Kept for the record. The decision on 18 Oct was to move the REST layer onto a patched octocrab fork instead (section 9). The `gh` replacement steps below (sign-in, token, scopes, clone, deletions) still apply.

**Don't replace the REST layer. Replace `gh`.**

1. **Sign-in:** register an OAuth App (device flow enabled) and implement sign-in with octocrab's device flow, or hand-roll the three HTTP calls through `http_client` so the proxy settings are honoured. Emit structured `{user_code, verification_uri}` events instead of scraping CLI text.
2. **Token:** store in `credentials_provider`; hand it to `GhState` at startup. As a migration nicety, keep an optional "import from `gh auth token`" if `gh` happens to be installed.
3. **Scopes:** read `X-OAuth-Scopes` from `GET /user` (add header capture to `send_once`, which already reads headers) and delete the `gh auth status` text parsing.
4. **Clone:** switch to `git clone` with env-based credentials and `--progress`; decide on the push/pull credential story.
5. **Delete** `gh_cmd`, the gate screen, `cli.rs`, and `parse_device_line`.
6. **Keep** `api.rs` / `requests.rs` / `cache.rs` / `paging.rs` / `error.rs` as they are. Optionally add octocrab later for GraphQL, behind the same `GhState`.

Result: Helm runs with no `gh` installed, no new transport stack, and no loss of your caching and error handling.

---

## 7. Decisions to talk through before any code

*Status as of 19 Oct.*

1. **OAuth App vs GitHub App: decided, OAuth App with device flow.** It keeps classic scopes (`repo`, `read:org`, `user`) and the scope-gating UI as is. A GitHub App would have given fine-grained permissions but removed scopes, changing `required_scope()` and the "missing scope" flow. The consequence is item 6.
   **Registered 19 Oct:** OAuth App **`Helm_GPUI`**, Client ID **`Ov23li4HBhs95CCtVsmP`** (public; safe to put in source, as Copilot's is in `copilot_chat/src/copilot_oauth.rs:21`). Device Flow enabled. No client secret generated, and none is needed: device flow uses only the client ID. The redirect URI is required by the form but never used. **Template for the sign-in code:** `copilot_chat/src/copilot_oauth.rs` (device flow) plus `copilot_chat.rs:566` (token read from `credentials_provider`).
2. **Goal: decided 19 Oct, move away from `gh` entirely, so Helm needs as few separate installs as possible.** Consequences:
   - **No `gh` import.** Helm never reads `gh`'s token, and no `gh` code path is kept. People who used Helm through `gh` sign in once with "Login with GitHub".
   - **Pasting a token as an advanced option.** A small "Use a token instead" link on the sign-in screen accepts a personal access token. It needs no install and no app approval, so it covers orgs that block the OAuth App and locked-down machines. It is stored like any other account (`helm://github.com/<login>`, login learned from `GET /user`). Scopes come from `X-OAuth-Scopes`; fine-grained tokens report none, so for them a missing permission shows up per request (403) rather than up front.
   - **External programs after `gh` is gone:** `git` only. Zed's own git panel already needs a system `git` on Windows and Linux (macOS bundles one), so it is not an extra install for Helm. **npm** was the other one (below).
   - **npm after a clone: decided, drop Helm's own step.** Helm's clone currently runs the system `npm.cmd install` itself (`clone.rs:97`, "Phase 2"). `npm_bootstrap` already offers `npm install` whenever a workspace with a `package.json` but no `node_modules` opens, a fresh clone included, so Helm's step is removed. `npm_bootstrap` itself also calls the system `npm.cmd` (`npm_bootstrap.rs:121`); it is switched to Zed's managed Node (`node_runtime`, `npm_command` / `run_npm_subcommand`), so `npm install` anywhere in Forge works without a system Node. To check when doing it: that `npm_bootstrap`'s offer fires for the workspace Helm opens after a clone, and the exact `NodeRuntime` call to use.
3. **Clone implementation: decided, `git` subprocess with the token in env-based config.** See "Clone is the real gap" in section 3.
4. **Token lifetime: depends on the app's "Expire user access tokens" setting.** GitHub now offers expiring tokens for OAuth Apps too (about 8 hours, with a refresh token), ticked by default on the registration form. The plan is to keep it **off**: tokens then last until the user signs out of Helm or revokes the app on GitHub, and no refresh logic is needed (the `OAuth` fields `refresh_token` / `expires_in` stay unused). With it on, refreshing would most likely need the client secret, which a desktop app cannot keep (not verified). *Confirm the setting is off in the app's settings page.*
5. **Where token I/O lives: settled.** Token reads and writes go through `credentials_provider` on the panel side (it needs an `AsyncApp`), and the token is handed to `GhState`; the backend's tokio code keeps running through `on_tokio` (section 8).
6. **Organisation approval of the new OAuth App (added 19 Oct).** An org with OAuth App access restrictions turned on (common for company orgs such as Sencorp) blocks your new app's tokens from its resources until an org owner approves the app. Orgs with SAML SSO also need each token authorised. `gh`'s own OAuth App is often already approved, so moving off `gh` can silently lose access to org repos. Needed:
   - Detect the restriction. GitHub answers with a 403 whose message names the OAuth App access restrictions. Map it to its own `GhError` variant next to `SsoRequired`. The exact message and status still need checking against a restricted org.
   - Show an "approval needed for <org>" state with a link to the app's page in the user's GitHub settings (`https://github.com/settings/connections/applications/<client_id>`), where the user can request approval from the org owners.
   - Mention it in the sign-in screen's help text, and test against a restricted org before dropping `gh`.

---

## 8. What I did and did not verify

**Verified by reading source:** all `gh` call sites (re-checked 19 Oct; `gh_ensure_scope` and the `auth.rs` caller were added to section 1); the REST send path; endpoint list in `requests.rs`; `GhState` fields; `credentials_provider` trait signatures; octocrab already a workspace dependency (0.49) and its compliance-tool usage and comment.

**Verified from octocrab docs (docs.rs, README):** module list; builder methods (`with_service`, `with_layer`, `build_with_reqwest`, `cache`, `base_uri`, `add_header`, auth variants); `authenticate_as_device`; `DeviceCodes` fields and `poll_once` / `poll_until_available`; raw and processed HTTP methods; `graphql`; `get_page` / `all_pages`; `ratelimit`.

**Verified later, in the fork source (`mentenaz/octocrab` @ `b15540c`, v0.54.5):** error type keeps headers and `rate_limit_reset`; the cache layer's behaviour and its four problems; device-flow types (`DeviceCodes`, `OAuth` with scopes and refresh fields, `TokenResponse`, `Continue`); `with_service` / `with_layer` signatures; the retry layer's wait-and-retry behaviour; Cargo features and dependency versions.

**Resolved on 19 Oct, from the fork and `zed_source` source:**

- **Typed handler coverage:** re-checked per file in the fork; section 2 corrected (traffic, deployments, Dependabot, secret scanning, invitations, memberships, compare, combined status are all typed; text-match is not).
- **Layers around a custom `with_service`:** none except body mapping; the adapter stacks base URI, auth header and cache itself (section 4). Building the client needs a tokio runtime or `with_executor`.
- **`git` as a requirement:** Zed already shells out to a system `git` on Windows and Linux (section 3).

**Still not verified:** whether the `X-OAuth-Scopes` header and token revocation behave as described (from knowledge of GitHub's API, not a source I checked); the exact 403 for OAuth App access restrictions (section 7, item 6); wrapped-list paging in octocrab (section 4); and individual model fields against live responses (P5).

**Redirects and timeouts: decided by the adapter design.** `send_once` itself follows redirects (`MAX_REDIRECTS = 10`; GitHub redirects renamed or moved repositories) and gives each request a 30-second timeout (`REQUEST_TIMEOUT`, `api.rs`). Since the custom-service build adds no redirect or timeout layer, the adapter keeps doing both exactly as `send_once` does today: it sets `.follow_redirects(RedirectPolicy::FollowLimit(MAX_REDIRECTS))` on the `http_client` request (`api.rs:83`), so Zed's `reqwest`-based client follows redirects, and it applies the same timeout. Do not add octocrab's `follow-redirect` feature or `FollowRedirectLayer`, or redirects would be handled twice. Because the `Authorization` header is set by octocrab's `AuthHeaderLayer` before the request reaches the adapter, dropping it on a redirect to another host is left to `reqwest`, which removes sensitive headers on cross-origin redirects; the same is true today.
- **Tokio runtime (resolved).** The backend already runs its tokio code on `reqwest_client::runtime()` through `on_tokio` (`helm_backend/src/helm_backend.rs:19`), which the panel uses to call into it (for example `gh_login` in `helm_panel/src/auth.rs`). Octocrab's `poll_until_available` and its tower stack can run through the same bridge. `helm_backend`'s own `tokio` features (`io-util`, `process`, `sync`) do not include `time`; octocrab enables what it needs itself.

---

## 9. Plan: use the fork as Helm's GitHub layer (decided 18 Oct)

**Direction:** fork octocrab, fix it, and point the workspace at the fork by git revision, so Helm needs no separately installed GitHub tooling. This is the same pattern as your fnm fork for Node. This replaces the "keep your REST layer" advice in section 6: if you take this route, the REST layer moves onto octocrab, and the patches below are what make that safe.

**State of the fork:** `mentenaz/octocrab` is currently identical to upstream `main` (HEAD `b15540cf8c644f90b6d89971da5e6c54d634cd14`, "chore: release v0.54.5", 17 Oct 2026). No custom commits yet.

### Patches to make in the fork (proposed, nothing applied yet)

| # | Patch | Why |
|---|---|---|
| P1 | Cache key = URI **plus `Accept`** (add a defaulted trait method so existing storages keep compiling) | Raw/diff/text-match answers for the same path currently collide |
| P2 | On 304 with no stored body: treat as a cache miss and re-request without the validator, instead of `.expect(..)` panicking | Entries can be dropped while a request is in flight; a panic in a UI app is not acceptable |
| P3 | Commit a cached body only on **successful end of stream** (add a `finish()` to `CacheWriter`; nothing is stored on a bare `Drop`) | Today a cancelled or failed read can store a truncated body as if complete |
| P4 | **Decided 19 Oct: clear the whole cache** after any successful POST/PATCH/PUT/DELETE (a defaulted `clear(&self)` on `CacheStorage`, called by the layer), plus a `peek(uri, accept)` for showing remembered data without a request | Same as `forget_answers()` today (`types.rs:1013`, which clears everything). Clearing per URI was rejected: `POST /repos/x/issues` would not clear the cached `GET /repos/x/issues?state=open&page=2`. The cost is a few extra 304 checks after a change |
| P5 | Fix the repository-permission deserialisation your compliance tool already works around, and any other model that fails against live responses as Helm exercises them | Typed models are the main reliability risk |
| P6 | *(optional)* A `RateLimitObserver` hook receiving per-resource `limit/remaining/reset/retry-after` from every response | Could instead live in your own tower layer outside the fork, which keeps the fork smaller and easier to rebase. I would start outside |

Keep upstream-able patches (P2, P3, P5) as clean commits so they can be sent as upstream PRs and dropped from the fork later.

### Things that stay in `zed_source`, not in the fork

- A `tower::Service` adapter over Zed's `http_client` (request type `Request<OctoBody>`, response a body implementing `http_body::Body<Data = Bytes>`), plugged in with `OctocrabBuilder::with_service`. This keeps Zed's proxy settings and avoids pulling in a second TLS stack. Note: your workspace pins a Zed-specific `reqwest` fork, which is another reason to avoid octocrab's own `reqwest` feature. The adapter's stack, outermost first: the fork's `HttpCacheLayer`, `AuthHeaderLayer`, `BaseUriLayer`, `ExtraHeadersLayer` (`User-Agent`, API version), Helm's own rate-limit/limiter layers, then the `http_client` service, which keeps redirects and the timeout (section 8). The client is built with `with_executor` running the `Buffer` worker on `reqwest_client::runtime()`.
- The mapping from `octocrab::Error::GitHub(GitHubError)` to Helm's `GhError` (SSO, rate limit, scope, validation).
- Per-resource rate-limit tracking, the 8-request limiter, and fail-fast cooldowns, as tower layers.
- Token storage (`credentials_provider`), per-account tokens and the account switcher, device-flow UI.
- Clone via `git` with env-based credentials.

### Wiring the dependency

**Decided 19 Oct: a Helm-only dependency; the workspace `octocrab` pin stays as it is.**

1. In the fork, keep `main` tracking upstream; do the work on a `helm` branch.
2. In `zed_source/Cargo.toml` **add** (do not replace the existing `octocrab = { version = "0.49", … }` line):
   `octocrab_helm = { package = "octocrab", git = "https://github.com/mentenaz/octocrab", rev = "<commit sha on the helm branch>", default-features = false }`
   and bump the `rev` deliberately, never track a branch.
3. **Why not bump the shared pin:** `tooling/compliance` (feature `octo-client`) comes from upstream Zed and stays on 0.49, so it needs no port to 0.54.5 and upstream merges touching the pin or compliance do not conflict. `compliance` is a tooling crate, not part of the app build, so the Zed binary still contains only one octocrab.
4. `helm_backend` depends on `octocrab_helm.workspace = true` (renamed in code with `use octocrab_helm as octocrab;` or used under its new name), with only the features it needs; sign-in via `poll_until_available` needs a `tokio` runtime, which the crate already reaches through `on_tokio` (`reqwest_client::runtime()`; see section 8).

### Suggested order of work

1. Add the `octocrab_helm` dependency on the unmodified fork and get `helm_backend` building with it (nothing else changes).
2. Spike the `http_client` adapter with the layer stack above and make one Helm call (`GET /user`) through it.
3. Land P2/P3/P1/P4 with tests in the fork; switch Helm's REST calls over group by group, keeping `requests.rs` as the single place that describes what is asked.
4. Replace `gh` for sign-in, token storage, scopes and account switching.
5. Replace `gh repo clone` with `git`; remove the clone's npm phase; delete `cli.rs`, `gh_cmd` and the gate screen.
6. Switch `npm_bootstrap` from the system `npm.cmd` to Zed's managed Node (`node_runtime`). Independent of the steps above; can be done at any time.

*Status (19 Oct):* step 4 is built (single-account UI): device-flow and pasted-token sign-in (`helm_backend/src/github/oauth.rs`, `helm_panel/src/auth.rs`), tokens under `helm://github.com/<login>`, `accounts` / `default_account` settings, the `HelmAccounts` event store (`helm_panel/src/accounts.rs`), scopes from `X-OAuth-Scopes`, and the wrong-account check. Still open from step 4: the `GhState` identity check for late results, the account picker and per-workspace binding, and the organisation-approval error. Clone still uses `gh` until step 5.

*Note (19 Oct):* step 4 does not depend on octocrab. The current REST layer only needs a token, and the device flow is two HTTP calls (as in `copilot_chat/src/copilot_oauth.rs`), so sign-in can be done **before** steps 1 to 3, which makes Helm work without `gh` for everything except cloning early on.

### Decisions still open (design talk before code)

Decided on 19 Oct: OAuth App with device flow (section 7), P4 clears the whole cache, Helm-only dependency, clone via `git` with the token in env config.

Also decided 19 Oct: move away from `gh` entirely, with no `gh` import; token pasting as an advanced sign-in option; Helm's npm step removed in favour of `npm_bootstrap` on Zed's managed Node (section 7, item 2).

Still open: how the adapter reports rate limits to the panel; whether the request-as-data tests in `requests.rs` stay as the source of truth for endpoints; and the UI for the organisation-approval state (section 7, item 6).

---

## 10. Several GitHub accounts, bound per workspace (agreed design, 18 Oct)

**Goal:** sign in to several accounts once (for example Sencorp, personal, Mentenaz), then let each Zed workspace use its own account, with no logout/login between them. This replaces the earlier "global switcher" idea and the "account switcher" bullet under "Things that stay in `zed_source`" in section 9.

**Why per workspace fits:** every workspace window gets its own `HelmPanel`, and the panel creates its own `GhState` when it loads (`GhState::new(cx.http_client())` in `helm_panel.rs`). So token cache, ETag cache and rate limits are already separate per workspace. Today they only look shared because `gh` has one active login.

### Where each thing lives

| What | Where | Why |
|---|---|---|
| Access tokens | The existing credential store (`credentials_provider`), one entry per account, named **`helm://github.com/<login>`** (user name = login, password = token), the same style as the database panel's `database://…` names | Already how database passwords are handled. On Windows this is Windows Credential Manager (see the note below) |
| Account list (the index), display labels, default account, dock, button | `settings.json` under `"helm_panel"` | Hand-editable and safe to sync across machines. **The list is the only index of accounts** (decided 19 Oct): the credential store can only look an entry up by exact name, not list what it holds |
| Which account a workspace uses | Zed.db key-value store, one entry per workspace, key `helm_panel_account-<id>` (see "Still to check") | Machine-local and tied to the workspace record; terminal, outline and git panels already persist this way |
| Live state (HTTP client, caches, rate limits) | In memory, one per workspace panel | Already how `GhState` works |

**Credential store note (changed 19 Oct).** The fork's builds, installers included, ship on the `dev` release channel (`crates/zed/RELEASE_CHANNEL`). Upstream Zed's `dev` channel stores credentials in a plain-text JSON file (`development_credentials` in the config directory), so until 19 Oct database passwords and the zed.dev sign-in were stored there unencrypted. `zed_credentials_provider` now uses the system store in every channel (Windows Credential Manager on Windows). The plain-text file is used only when `ZED_DEVELOPMENT_USE_FILE_CREDENTIALS=1` is set in a dev build, which replaces upstream's `ZED_DEVELOPMENT_USE_KEYCHAIN`. Entries in an existing file are moved into the system store at startup (or on first read, if that comes earlier), and the file is deleted once empty. Helm tokens must not be written until a build with this change is in use, or they would land in the file.

### Settings shape (snake_case, matching the other panel keys)

```json
"helm_panel": {
  "button": true,
  "dock": "left",
  "default_account": "login-a",
  "accounts": [
    { "login": "login-a", "label": "Sencorp" },
    { "login": "login-b", "label": "Personal" },
    { "login": "login-c", "label": "Mentenaz" }
  ]
}
```

- The **GitHub login is the id**, not a random GUID: it has to match the credential-store key and what `GET /user` returns. `label` is the friendly name.
- **No tokens** in this file, ever.
- An account listed here with no stored token shows as **"needs sign-in"**, which keeps a synced settings file safe on a new machine.
- Today the key holds only `button` and `dock` (shared side-panel settings type). `default_account` and `accounts` are new fields; a panel-order field is not present for Helm in the current source.

### Which account a workspace uses

1. The binding saved for that workspace (Zed.db), else
2. `default_account` from settings, else
3. the only account, if exactly one is listed, else
4. with no accounts, the "Login with GitHub" screen; with several, a picker.

*Optional extra:* a rule table such as "repos owned by org X use account B", matched against the workspace's git remote, tried between steps 1 and 2.

### Startup, replacing `do_auth` (`helm_panel/src/auth.rs`)

1. Pick the account as above.
2. Read its token from `helm://github.com/<login>`. None: show **"Sign in as <label>"** for that account (the "needs sign-in" state).
3. `GET /user` (user details, scopes from `X-OAuth-Scopes`) and org logins. A 401 means the token was revoked: delete the stored token and show "needs sign-in".
4. Build the panel's `GhState` for that account and show the account in the header.

The `gh` CLI gate (`check_cli`, `render_gate`) is removed; the code/copy/Open screen in `render_auth` stays, fed by the device-flow response instead of `parse_device_line`.

### Adding an account

1. Run the device flow (client ID `Ov23li4HBhs95CCtVsmP`, scopes `repo read:org user`) and get a token.
2. `GET /user` to learn **which** account actually signed in.
3. Store the token under that login; add `{ login, label }` to `accounts` with `update_settings_file`; if it is the first account, also set `default_account`.
4. Bind the current workspace to it.

### Behaviour rules

- **One octocrab client per workspace panel**, built for the bound account, with its own cache and rate-limit state. Nothing is shared across accounts, so no cache key needs the account in it.
- **Changing a workspace's account:** write the new binding, build a **new `GhState`** (fresh token, cache and rate limits), clear that panel's repo lists, selection and history, and **clear `pending_action`** (otherwise a parked "create release" could be re-sent under the wrong account).
- **Late results after a switch (decided 19 Oct): compare `GhState` identity.** Each task keeps the `Arc<GhState>` it started with and, when it returns, applies its result only if `Arc::ptr_eq` with the panel's current `gh_state`; otherwise the result is dropped. A switch builds a new `GhState` anyway, so no extra counter is needed. The panel's tasks are detached, so they cannot be cancelled; this check is what keeps an old account's answer off the new account's screen.
- **App-wide account store (decided 19 Oct):** one gpui `Global` entity (for example `HelmAccounts`) owns the account list, reads and writes tokens, and emits events (`SignedIn`, `SignedOut`, `Removed`, `ScopesChanged`, keyed by login). Every `HelmPanel` subscribes and, when its bound account changes, reloads or shows "needs sign-in". The store also observes settings, so a hand-edited `accounts` list is picked up. This fixes today's behaviour, where a logout in one window leaves other windows holding the old cached token.
- **Wrong account in the browser (decided 19 Oct): refuse, then offer choices.** GitHub's device flow cannot pre-select an account; the code page signs in whichever account the browser is using. So for "Sign in again as A" and "Authorize missing scope" for A, Helm checks that the login from `GET /user` is A. If it is B instead, A's token is **not** touched, and Helm shows "You signed in as B, not A" with **Add B as a new account** and **Try again** (with the hint to use a private browser window, or sign out of B on github.com first).
- **Sign out vs remove:** "sign out" deletes the stored token and leaves the account in the list as "needs sign-in"; "remove" also deletes the list entry.
- **Adding an account:** runs the device flow without replacing anyone; the login is learned from `GET /user`. If that login is already listed, its token is replaced (a re-sign-in), and nothing else changes.
- **Scopes are per account:** "authorize missing scope" upgrades only the account of the workspace that asked.
- **Always show the active account** in the panel header and on create, delete and release dialogs.

### Still to check before building

- ~~How to write the account list into `settings.json` from the sign-in flow.~~ Resolved: `settings::update_settings_file` (`crates/settings/src/settings_file.rs:269`), which Helm already uses (`helm_panel.rs:608`).
- Whether Zed's settings UI can edit a list of objects like `accounts`, or needs custom work. *Still open.*
- ~~The exact per-workspace key format the git panel uses with the key-value store.~~ Resolved: `"{GIT_PANEL_KEY}-{id}"`, where `id` is the workspace's `database_id()`, falling back to its `session_id()` (`git_ui/src/git_panel.rs:1988`), written with `KeyValueStore::write_kvp` off the main thread. Helm should use the same shape with its own prefix, for example `"helm_panel_account-{id}"`. The binding then belongs to the workspace record (its set of folders), not to one folder path.
- ~~That `credentials_provider` on Windows is backed by the credential store you expect.~~ Resolved: the keychain provider calls gpui's platform `read_credentials` / `write_credentials`, which on Windows is Windows Credential Manager, and it is now used in every release channel (see the credential store note above). Still to confirm in a real run: the move of the old file's entries, and the size limit of a Credential Manager entry (2,560 bytes) against a token plus refresh token.

### Where it fits in the work order

Build the storage and the per-workspace client as account-aware from the start, as part of step 4 (replacing `gh` sign-in), but ship the UI for a single account first. That first version already includes: tokens under `helm://github.com/<login>`, the `accounts` list as index, the `HelmAccounts` store with its events, the `GhState` identity check, and the wrong-account check. With one account none of it shows; the startup steps simply land on that account. The account picker, the header switcher and per-workspace binding then land as a follow-up with no data rework.
