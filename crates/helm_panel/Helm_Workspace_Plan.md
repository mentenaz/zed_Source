# Helm Workspace — Phased Plan

**Written:** 7 October 2026. **Status:** the plan is being hardened; no code is to be written until that is done. Eleven of the fourteen decisions in section 12 are made; the other three wait on trials (rows 2, 3 and 5), and the API facts in section 3a were checked against GitHub's documentation and two real repositories (7 October 2026). Nothing here has been built; the sizes of the phases are estimates.
**Companions:** `Helm_Future_Developments.md` (the roadmap; this plan is its phase 2, plus the search gap left in phase 1) and `Helm_Refactor_Plan.md` (the groundwork, phases A to E, done).

---

## 1. What the Workspace is

A workspace tab, one per repository, for reading a GitHub repository without cloning it and without a browser: its files, branches, history and diffs, with the pull requests, issues and Actions that belong to what you are looking at beside the code.

The dock panel stays what it is: the place to find a repository and manage it. The Workspace is where you go to work with one.

```text
Helm panel (dock)                 Helm Workspace (tab)
find, manage, administer   ──►    read, navigate, hand over to the editor
```

### In this plan

- Repository tree, branch and tag selector, file viewer.
- Commit history, commit detail, diffs, comparing two refs.
- Search: file names, code, and repositories across GitHub.
- A context pane: the pull request, checks and issues for the branch in view.
- Handing a file over to the editor when a local clone exists, and cloning when it does not.

### Not in this plan

Anything that writes to the repository: creating branches, committing, pushing, reviewing, merging. That is the roadmap's phase 3 and gets its own plan once reading works. The one exception is cloning, which the panel already does.

## 2. Rules for the whole plan

- **Built on `gpui_component`.** The shell follows the package managers: `Settings` pages for the sections, `h_resizable` for the splits, `tree` for the file tree, `List` for every list. A hand-built `div` layout is a failed phase, as `CLAUDE.md` says.
- **Every request goes through `helm_backend`.** No screen talks to GitHub by itself. New endpoints are added as request builders with tests, the same way the existing 45 are.
- **Read-only until the plan says otherwise.** No phase here sends a change to GitHub.
- **Every phase ends in a working app** and can be released on its own.
- **No file over about 800 lines.**
- **Each phase is checked two ways:** unit tests for everything that is not UI, and the manual checklist in section 11 in the app started with `./script/run-isolated.ps1`.
- Fork conventions as elsewhere: `-j 8`, commit only when asked.

## 3. How it fits together

```text
┌──────────────────────────────────────────────────────────────────────┐
│ mentenaz / zed_Source                      [ main ▾ ]   Open on GitHub│
├───────────┬──────────────────────────────────────────┬───────────────┤
│ Overview  │ FILES          │ src/helm_panel.rs       │ CONTEXT       │
│ Code      │ ▼ crates       │                         │ PR #184  open │
│ Commits   │   ▼ helm_panel │ (read-only, highlighted)│ ✓ checks      │
│ Pull req. │     src        │                         │ 2 issues      │
│ Issues    │ README.md      │                         │ last commit   │
│ Actions   │                │                         │               │
└───────────┴────────────────┴─────────────────────────┴───────────────┘
  Settings     tree             viewer                    context
  pages        (h_resizable split)                        (h_resizable)
```

- **One tab per repository.** Opening a repository that already has a tab activates that tab, as the workflow-run tab does today.
- **What is in view is a ref** (a branch, tag or commit) and optionally a path. Every pane reads from that one piece of state, so switching branch changes all of them together.
- **Data comes from GitHub's API first.** A repository can be read without a clone. A local clone, when there is one, is used only for handing files to the editor (phase W4).

### Backend additions

| Need | GitHub endpoint | Note |
|---|---|---|
| Whole tree of a ref | Git Trees, recursive | One request for most repositories. GitHub marks very large trees as truncated; then load a directory at a time. |
| A file's contents | Blobs, by the hash the tree gives | Never changes for a given hash, so it can be kept without checking. The Contents endpoint is not needed for files. |
| Commit with its changed files | Commits (single) | Carries the patch per file. |
| Two refs compared | Compare | |
| Code and repository search | Search | Has its own, much smaller allowance than the main one. |
| The README of a ref | Readme | Finds the README whatever it is called; saves guessing from the tree. |

Five things in `helm_backend` have to change to carry this safely:

1. **Rate limits per kind of request.** The backend tracks only the main allowance and ignores the others. Search needs its own count, shown and respected separately.
2. **A size limit on remembered answers.** The cache holds up to 200 answers whatever their size. Trees and files can be megabytes each, so it needs a limit in bytes.
3. **Answers that never change.** A tree or blob fetched by hash is the same forever. These should be kept and served without asking GitHub again.
4. **Bytes, not only text.** The backend turns every answer into text, replacing anything that is not valid UTF-8. That would corrupt an image or any other binary file. Answers need to be carried as bytes, and turned into text only where text is expected.
5. **Asking for a format.** Every request is sent asking for GitHub's default JSON. A file's raw contents, a diff, and search results with match positions are each asked for with a different `Accept` value. A request needs to carry which format it wants, and the cache has to keep answers in different formats apart.

There is also a new rule the backend has to keep: **a limit on requests in flight.** See section 3a.

## 3a. API facts, checked

Checked on 7 October 2026 against GitHub's REST documentation (version 2022-11-28), and against two real repositories with `gh api`. Each fact is followed by what it means for the plan.

### Trees

- A recursive tree request returns at most **100,000 entries and 7 MB**. Past either, the answer has `truncated: true`, and GitHub's advice is to fetch one sub-tree at a time, without `recursive`.
- **Measured:** this fork returns 6,468 entries, not truncated. `torvalds/linux` is truncated at 71,638 entries, so the 7 MB limit is reached well before 100,000.
- Each entry carries `path`, `mode`, `type`, `sha` and, for a file, `size`.
- `type` is `blob`, `tree` or `commit`. A `commit` entry (mode `160000`) is a **submodule**. Mode `120000` is a **symlink**; `100755` is an executable file.
- **Measured:** this fork has 292 symlinks (the licence files) and a largest file of 6.1 MB.

What it means:

- One request draws the whole tree for ordinary repositories. The directory-at-a-time path is needed and has to be built in W1, not left for later: real repositories hit it.
- The size and hash of every file are known before it is opened. Whether a file is too large to show is decided without a request, and the file is fetched by its hash.
- Submodules and symlinks are identified from the tree. A submodule is shown as a link to the repository and commit it points at. A symlink is shown as a symlink, with its target (the contents of its blob).
- A truncated recursive answer is still usable for the part it holds, but it does not say what is missing, so it is not used: on `truncated`, the tree is loaded a directory at a time from the root.

### File contents

- **Blobs:** up to **100 MB**. Raw bytes with `Accept: application/vnd.github.raw+json`; otherwise JSON with the contents in Base64.
- **Contents:** all features up to 1 MB; from 1 to 100 MB only the raw and object formats; over 100 MB not supported. A directory listing stops at **1,000 files**.
- A file kept in **Git LFS** is, to these endpoints, a small text file that points at the real one.

What it means:

- Files are fetched as raw blobs by hash. The 1 MB limit of the Contents endpoint does not apply, so how large a file is shown inline is our choice (section 12, row 4).
- Directories are never listed through Contents, because of the 1,000-file stop. Trees are used throughout.
- An LFS pointer is recognised by its first line and shown as "stored in Git LFS", with its real size (the pointer states it) and Open on GitHub. The Workspace does not fetch LFS objects.

### Rate limits

- **5,000 requests an hour** when signed in. Headers: `x-ratelimit-limit`, `-remaining`, `-used`, `-reset`, `-resource`.
- **Measured** (in phase E of the refactor): a request answered `304 Not Modified` leaves the count where it was.
- **Secondary limits:** no more than **100 requests at once**; no more than **900 points a minute** (a read is 1 point, a change is 5); no more than 90 seconds of GitHub's CPU time per minute.
- Past the hourly limit: do not retry before `x-ratelimit-reset`. Past a secondary limit: obey `retry-after`, or wait at least a minute, and wait longer each time it happens again.

What it means:

- Loading a truncated tree a directory at a time, or fetching many files, could send hundreds of requests in a burst. The backend gets a limit on requests in flight (suggested: 8) that every caller shares.
- 900 reads a minute is far above what a person browsing causes, but not above what a careless loop does. Nothing in the Workspace fetches ahead of what is on screen, except the README.
- The backend already stops sending when the hourly allowance is used up. It needs the same for a secondary limit: hold everything for the time GitHub gives.

### Search

- **30 requests a minute** for search, and **10 a minute for code search**. Both are counted apart from the hourly allowance.
- At most **1,000 results** per search, however many match. A query is at most 256 characters, with no more than five `AND`, `OR` or `NOT`.
- **Code search looks only at the default branch**, only at files under **384 KB**, and needs at least one search word.
- Match positions for highlighting come with `Accept: application/vnd.github.text-match+json`.
- A search that takes too long returns what it found with `incomplete_results: true`.

What it means:

- Search sends on `enter`, never as you type, and the backend tracks the two search allowances separately from the main one.
- When the ref in view is not the default branch, code search says plainly that it searches the default branch. Results open the file on the default branch.
- The results screen says when there are more than 1,000 matches, and when results are incomplete.

### Commits and comparing

- **A commit** lists **300 changed files** a page, up to **3,000** in all. Asking for the whole diff as text can fail with a server error for a large commit.
- **Comparing two refs** returns 250 commits unless paged, and the changed files only on the first page, **300 at most for the whole comparison**. The form is `BASE...HEAD`; across forks, `USER:BASE...USER:HEAD`.
- A binary file has no patch. A file's patch is also left out when it is very large.

What it means:

- Diffs are taken per file from the JSON answer, never as one text for the whole commit.
- Commit detail pages its files. Compare shows up to 300 and says so when there are more.
- A file with no patch shows "binary file" or "diff too large to show", with Open on GitHub.

## 3b. Permissions: what the Workspace shows

The Workspace only reads, so the question is mostly "can this be opened, and what is said when it cannot". It also has to carry what the user may do, because the phases after this plan write.

### What GitHub tells us

Checked on 7 October 2026 unless marked.

- Every repository answer carries `permissions`: `pull`, `triage`, `push`, `maintain`, `admin`. **Measured:** all five true on this fork; only `pull` true on `zed-industries/zed`.
- It also carries `private`, `visibility`, `archived`, `disabled`, `fork` (with `parent`), `has_issues` and the like.
- **A private repository the sign-in cannot see answers `404 Not Found`, the same as one that does not exist.** GitHub does this on purpose, and the two cannot be told apart from the answer.
- A fine-grained token that lacks a permission answers `403` with "Resource not accessible by personal access token". Helm signs in through `gh` with classic scopes (`repo`, `read:org`, `user`), so this is rare, but a sign-in made outside Helm can have less.
- **Not checked, from memory:** an organisation that enforces single sign-on answers `403` until the token is authorised for it. An empty repository answers `409` to a request for its tree. Both to be confirmed in W1 by trying them.

### What the Workspace does

| Situation | Shown |
|---|---|
| Read access only (`pull`) | Everything in this plan works. Nothing is greyed out, because nothing here writes. |
| Private | A "private" tag in the header. Otherwise the same. |
| Archived | An "archived" tag in the header, and one line under it: "This repository is archived and read-only on GitHub." |
| A fork | "forked from owner/name" in the header, opening the parent in its own tab. |
| Disabled by GitHub (`disabled`) | The tab opens with the Overview only and says the repository is disabled. |
| `404` when opening | One message for the whole tab: "This repository does not exist, or this sign-in cannot see it." Followed by the account signed in, and, when the token lacks the `repo` scope, the same authorise prompt the panel uses. No guessing which of the two it is. |
| `403` for single sign-on | The message GitHub gives, and Open on GitHub, where the authorisation is done. |
| Empty repository | The tab opens. Code says "This repository is empty" with its clone URL. |
| Issues, or another feature, turned off | That page says so ("Issues are turned off for this repository") and is not hidden, so the pages do not move about between repositories. |
| A page needs more than the user has (Traffic needs `push`, security alerts need `admin`) | These stay in the panel and are not part of the Workspace. |

### What W1 has to add

- `permissions` read into a typed value (`pull`, `triage`, `push`, `maintain`, `admin`) in place of the untyped JSON it is kept as today, with one function that answers "may this user do X". Phase 3 of the roadmap will ask it for every write action; the Workspace asks it for nothing yet.
- An error that says "single sign-on needed", told apart from an ordinary `403`.
- "Empty repository" as an outcome of loading a tree, not as an error.

## 3c. The ref in view

What the tab is looking at, and what happens when GitHub moves underneath it.

### The rule

**The tab looks at a commit, and remembers which branch or tag led it there.**

When a branch is chosen, its name is resolved once to the commit it points at. The tree, every file, the history and the context pane are then fetched for that commit, not for the branch name.

Why:

- Every pane shows the same moment. A file opened ten minutes after the tree is the file from that tree, even if someone pushed in between.
- Everything fetched by commit never changes, so it is kept without asking again (section 3).
- Permalinks are always right, because they are made from the commit.

The cost is that the tab can fall behind its branch. That is handled openly, below.

### Falling behind

The tab checks where its branch points:

- when the tab is brought to the front, if the last check was more than a minute ago,
- when Refresh is pressed,
- and never while the tab is in the background.

The check is one request, and free when nothing changed.

| What the check finds | What the tab does |
|---|---|
| Same commit | Nothing. |
| The branch moved forward | A line under the header: "main has 3 new commits. **Update**". Nothing changes until Update is pressed: what is being read is not pulled away. |
| The branch was rewritten (force-pushed) | "main was rewritten on GitHub. **Update**". Told apart from moving forward by comparing the old commit with the new one: one extra request, made only when the branch moved. |
| The branch is gone (`404`) | "main was deleted on GitHub. You are looking at its last commit, a1b2c3d." The tab keeps working from that commit. The ref selector offers the default branch. |
| The repository is gone or no longer visible (`404` for the repository) | The whole-tab message from section 3b. |
| The repository was renamed or moved | GitHub redirects and the backend follows. The tab takes its new name from the answer. |

**Update** moves the tab to the branch's new commit and keeps the place: the same file if it still exists, otherwise its nearest folder that does, with one line saying the file is not in the new commit.

A commit that is no longer on any branch can be removed by GitHub after a time (from memory, not checked). If a request for the old commit fails after a rewrite or a deletion, the tab says the commit is no longer available and offers the default branch.

### The other kinds of ref

- **A tag** is handled as a branch is. Tags rarely move, but the same check applies.
- **A commit** (reached from history, or pasted) has nothing to fall behind. The ref selector shows its short hash and "not on a branch".
- **The default branch** is taken from the repository's answer each time the tab opens, so a renamed default branch is picked up.

### Switching ref

Choosing another branch keeps the path in view when it exists there. When it does not, the tab moves to the nearest folder that does and says so.

### Effect on W4

The comparison with a local clone is made on commits, not on branch names. If the clone's checked-out commit is the commit in view, the file opens without a question, whatever the branches are called. The question from section 12, row 8 is asked only when the commits differ, and it says by how much when that is known ("your clone is 3 commits behind").

## 3d. Screens

Sketches of every screen, to agree what goes where before any of it is built. They show content and arrangement, not exact sizes or colours. Each names the `gpui_component` widgets it is made of.

Two things in them depend on trial 1 (section 4): whether the section list down the left is the `Settings` sidebar, and whether Code sits inside it or beside it. The sketches assume the expected outcome: a `Settings` sidebar for the sections, with Code using the full area to its right.

### The frame, on every screen

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ ⎇ mentenaz / zed_Source  [public] [fork of zed-industries/zed]             │
│                                   [ main ▾ ]  a1b2c3d   ⟳   Open on GitHub │
├────────────────────────────────────────────────────────────────────────────┤
│ ⓘ main has 3 new commits.                                        [ Update ]│
├────────────┬───────────────────────────────────────────────────────────────┤
│ Overview   │                                                               │
│ Code       │                                                               │
│ Commits    │                      the section in view                      │
│ Pull req. 4│                                                               │
│ Issues  12 │                                                               │
│ Actions  ✓ │                                                               │
├────────────┴───────────────────────────────────────────────────────────────┤
│ 4,812 of 5,000 requests left · resets in 23 min                            │
└────────────────────────────────────────────────────────────────────────────┘
```

- **Header:** owner and name; `Tag`s for private, archived and fork (the fork tag opens the parent); the ref selector; the short hash of the commit in view (click copies the full one); Refresh; Open on GitHub.
- **The line under the header** is an `Alert`, shown only when there is something to say: the branch moved, was rewritten or was deleted (section 3c), or the repository is archived.
- **Sections** carry a count or a state where one is known: open pull requests, open issues, the latest run's result.
- **Foot:** the rate-limit line, as in the panel. When search is in use it also shows the search allowance.

### The ref selector

```text
[ main ▾ ]
┌──────────────────────────────┐
│ 🔍 Find a branch or tag…      │
├──────────────────────────────┤
│ [ Branches ]  [ Tags ]       │
├──────────────────────────────┤
│ ✓ main              default  │
│   phase-e-cache              │
│   fix/zed-release-asset-name │
│   …                          │
├──────────────────────────────┤
│ Page 1 of 3          ‹  ›    │
└──────────────────────────────┘
```

A `Popover` holding an `Input`, a two-button `ButtonGroup`, a `List` and `Pagination`. The filter box narrows what is loaded; a repository with hundreds of branches is paged, not loaded whole. Typing a full commit hash and pressing `enter` goes to that commit.

### Overview

```text
┌────────────┬───────────────────────────────────────────────────────────────┐
│ Overview ◂ │  About                                                        │
│ Code       │  ┌─────────────────────────────────────────────────────────┐  │
│ Commits    │  │ Description   Code at the speed of thought…             │  │
│ Pull req. 4│  │ Homepage      zed.dev                                   │  │
│ Issues  12 │  │ Topics        [rust] [editor] [gpui]                    │  │
│ Actions  ✓ │  │ Default       main                                      │  │
│            │  │ Clone URL     https://github.com/…/zed_Source.git   ⧉   │  │
│            │  └─────────────────────────────────────────────────────────┘  │
│            │                                                               │
│            │  At this commit                                               │
│            │  ┌─────────────────────────────────────────────────────────┐  │
│            │  │ a1b2c3d  helm_panel: Add the phased plan…   Francois 2h │  │
│            │  │ Checks   ✓ fork_tests                                   │  │
│            │  └─────────────────────────────────────────────────────────┘  │
│            │                                                               │
│            │  Activity                                                     │
│            │  ┌───────────────┬───────────────┬───────────────┬────────┐  │
│            │  │ Pull requests │ Issues        │ Releases      │ Stars  │  │
│            │  │ 4 open        │ 12 open       │ v0.3.1        │ 17     │  │
│            │  └───────────────┴───────────────┴───────────────┴────────┘  │
│            │                                                               │
│            │  On this computer                                             │
│            │  ┌─────────────────────────────────────────────────────────┐  │
│            │  │ E:\zed_Source · on main · same commit      [Open folder]│  │
│            │  └─────────────────────────────────────────────────────────┘  │
└────────────┴───────────────────────────────────────────────────────────────┘
```

A `SettingPage` of four `SettingGroup`s, each holding a `DescriptionList`. This is the screen the settings widgets fit best.

- **About:** what the panel's repository screen shows, read-only here.
- **At this commit:** the commit in view and its checks. Clicking it opens the commit (W5).
- **Activity:** four counts; each opens its section, and Releases opens the panel's release list.
- **On this computer:** appears with W4. Without a clone it reads "Not cloned" with a Clone button.

### Code

```text
┌────────────┬──────────────────┬────────────────────────────┬──────────────┐
│ Overview   │ 🔍 Filter files… │ crates › helm_panel › src › │ CONTEXT      │
│ Code     ◂ │──────────────────│ section.rs                 │              │
│ Commits    │ ▸ .github        │ 471 lines · 15.2 KB        │ Pull request │
│ Pull req. 4│ ▾ crates         │ [History] [⧉ Path] [🔗] [↗]│ #184 open    │
│ Issues  12 │   ▸ helm_backend │ [ Open in editor ]         │ ✓ 3 checks   │
│ Actions  ✓ │   ▾ helm_panel   │────────────────────────────│              │
│            │     ▾ src        │  1  //! One list screen's  │ Last change  │
│            │       auth.rs    │  2  //! worth of data.     │ to this file │
│            │       section.rs◂│  3  //!                    │ f5365ed 3h   │
│            │       …          │  4  //! Every list in Helm │ Remember     │
│            │     Cargo.toml   │  5  use std::future::Fut…  │ answers…     │
│            │ ▸ docs           │  6                         │              │
│            │ ↪ LICENSE-GPL    │  7  use super::*;          │ Latest run   │
│            │ ⧉ vendor/lib @9f │  …                         │ ✓ fork_tests │
│            │ README.md        │                            │ 9 min        │
└────────────┴──────────────────┴────────────────────────────┴──────────────┘
```

Three panes in an `h_resizable`: `tree`, the viewer, and the context pane, which can be closed.

- **Tree:** folders first, then files. `↪` marks a symlink and `⧉ name @hash` a submodule. The filter box narrows by name as you type, with no requests.
- **Above the file:** the path as a `Breadcrumb` (each part opens that folder), size, and actions: History (W5), copy path, copy permalink, Open on GitHub, and Open in editor (W4).
- **Viewer:** read-only, with line numbers. Selecting lines and copying a permalink gives a link to those lines.
- **Nothing selected:** the viewer shows the README of the folder in view, rendered.
- **A folder selected:** the viewer lists its contents with each entry's size, then its README.

In place of the viewer, when a file cannot be shown as text:

```text
  Image                       Binary / too large           Git LFS
┌────────────────────────┐  ┌────────────────────────┐  ┌────────────────────────┐
│                        │  │   zeddev.exe           │  │   demo.mp4             │
│     (the image,        │  │   Binary file · 84 MB  │  │   Stored in Git LFS    │
│      fitted)           │  │                        │  │   212 MB               │
│                        │  │   [ Open on GitHub ]   │  │   [ Open on GitHub ]   │
│  1280 × 720 · 340 KB   │  │                        │  │                        │
└────────────────────────┘  └────────────────────────┘  └────────────────────────┘

  Symlink                     Submodule
┌────────────────────────┐  ┌──────────────────────────────────────┐
│   LICENSE-GPL          │  │   vendor/lib                         │
│   Link to              │  │   Submodule: owner/lib at 9f8e7d6    │
│   ../../LICENSE-GPL    │  │                                      │
│   [ Go to target ]     │  │   [ Open in a Workspace tab ]        │
└────────────────────────┘  └──────────────────────────────────────┘
```

A tree too large for one request (section 3a) looks the same, with folders loading when opened and a small spinner on the folder while it does. The filter box then says "Filtering loaded folders only".

### Commits

```text
┌────────────┬───────────────────────────────────────────────────────────────┐
│ Overview   │ History of  crates/helm_panel/src/section.rs  ✕   [Compare…]  │
│ Code       │───────────────────────────────────────────────────────────────│
│ Commits  ◂ │ Today                                                         │
│ Pull req. 4│   helm_panel: Show remembered pages at once…      7a88b4d  ✓  │
│ Issues  12 │   Francois · 3 hours ago                                      │
│ Actions  ✓ │   helm_backend: Remember answers and track…       f5365ed  ✓  │
│            │   Francois · 3 hours ago                                      │
│            │ Yesterday                                                     │
│            │   helm_panel: Show lists ten rows at a time  [v0.3.1] e3168c1 │
│            │   Francois · 1 day ago                                        │
│            │   …                                                           │
│            │───────────────────────────────────────────────────────────────│
│            │ Page 1 of 404                                         ‹  ›    │
└────────────┴───────────────────────────────────────────────────────────────┘
```

A `List` with day headings and `Pagination`, the same parts as the panel's lists.

- **The top line** says what the history is of: the whole ref, or the file or folder it was opened from, with `✕` to widen it back to the ref.
- **A row:** the first line of the message, the author and when, a `Tag` for any tag on that commit, the short hash, and the checks result when known.
- **`enter` or a click** opens the commit.

### A commit

```text
┌────────────┬─────────────────────────┬─────────────────────────────────────┐
│ Overview   │ ‹ Commits               │ crates/helm_panel/src/section.rs    │
│ Code       │ helm_panel: Show        │ +141 −6            [ View file ] [↗]│
│ Commits  ◂ │ remembered pages at     │─────────────────────────────────────│
│ Pull req. 4│ once, and the requests  │  23 │ 23 │      pub(super) page: u32│
│ Issues  12 │ left                    │  24 │ 24 │      /// The last page  │
│ Actions  ✓ │                         │  25 │ 25 │      pub(super) last_pa…│
│            │ A paged list whose page │     │ 26 │ +    /// The rows are a │
│            │ is remembered is shown… │     │ 27 │ +    /// remembered ans…│
│            │                         │     │ 28 │ +    pub(super) refresh…│
│            │ Francois · 3 hours ago  │  26 │ 29 │  }                      │
│            │ 7a88b4d ⧉  parent f5365e│ ⋯                                   │
│            │ ✓ fork_tests            │  40 │ 43 │      /// Drops the rows │
│            │ [ Browse files here ]   │                                     │
│            │─────────────────────────│                                     │
│            │ 8 files  +445 −53       │                                     │
│            │ M activity.rs      +4   │                                     │
│            │ M helm_panel.rs   +34 −1│                                     │
│            │ M section.rs ◂   +141 −6│                                     │
│            │ A cache.rs       +289   │                                     │
│            │ …                       │                                     │
│            │ Page 1 of 1             │                                     │
└────────────┴─────────────────────────┴─────────────────────────────────────┘
```

An `h_resizable` of two panes.

- **Left:** the full message, author, hash (click copies), the parent or parents (each opens that commit), checks, and "Browse files here", which moves the tab to this commit. Below, the changed files as a `List`: `A`dded, `M`odified, `D`eleted, `R`enamed, with lines added and removed. Paged at 300.
- **Right:** the diff of the selected file. How it is drawn is decision 5; the sketch shows one column with old and new line numbers. A file with no diff shows "Binary file" or "Diff too large to show" in its place.

### Comparing

```text
│ Compare   [ main ▾ ]  …  [ phase-e-cache ▾ ]              ⇄ swap           │
│────────────────────────────────────────────────────────────────────────────│
│ phase-e-cache is 5 commits ahead of main and 2 behind.                     │
│ [ Commits 5 ]  [ Files 12 ]                                                │
```

Opened from Commits. Two ref selectors, one line stating the relationship, then two tabs (`TabBar`): the commits, as on the Commits screen, and the changed files with their diffs, as on a commit. When more than 300 files changed, a line says so.

### Pull requests, Issues, Actions

```text
┌────────────┬───────────────────────────────────────────────────────────────┐
│ Overview   │ [ Open | Closed | All ]                     ☐ This branch only│
│ Code       │───────────────────────────────────────────────────────────────│
│ Pull req.◂ │   Fix release asset name            #19  mentenaz   ✓  2 💬   │
│ Issues  12 │   fix/zed-release-asset-name → main                           │
│ Actions  ✓ │   …                                                           │
│            │───────────────────────────────────────────────────────────────│
│            │ Page 1 of 1                                           ‹  ›    │
└────────────┴───────────────────────────────────────────────────────────────┘
```

The panel's lists, drawn by the same row code from `helm_ui`, with one addition: "This branch only" narrows to what belongs to the ref in view. A row opens the detail beside the list in an `h_resizable`, where the panel replaces the list with it; the tab has the width to show both. An Actions run still opens its own tab with the job graph.

### The context pane

```text
┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐
│ CONTEXT        ✕ │   │ CONTEXT        ✕ │   │ CONTEXT        ✕ │
│                  │   │                  │   │                  │
│ Pull request     │   │ Pull request     │   │ Pull request     │
│ #184  open       │   │ None for this    │   │ —                │
│ Add the cache    │   │ branch           │   │ (default branch) │
│ ✓ 3 checks       │   │                  │   │                  │
│ 1 approval       │   │ Latest run       │   │ Latest run       │
│                  │   │ ✗ fork_tests     │   │ ✓ fork_tests     │
│ Last change to   │   │ 2 of 14 failed   │   │ 9 min            │
│ this file        │   │                  │   │                  │
│ f5365ed · 3h     │   │ Last change to   │   │ Open issues  12  │
│ Francois         │   │ this file        │   │ Open PRs      4  │
│                  │   │ …                │   │                  │
│ Latest run       │   │                  │   │                  │
│ ✓ fork_tests     │   │                  │   │                  │
└──────────────────┘   └──────────────────┘   └──────────────────┘
   a branch with          a branch with          the default
   a pull request         none                   branch
```

A column of `GroupBox`es beside Code, narrow and closable. Each block opens the thing it names. It shows what belongs to the ref and file in view and nothing else; it is not a second navigation.

### When the repository cannot be opened

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ ⎇ someone / private-thing                                                  │
├────────────────────────────────────────────────────────────────────────────┤
│                                                                            │
│        This repository does not exist, or this sign-in cannot see it.      │
│                                                                            │
│        Signed in as mentenaz                                               │
│                                                                            │
│        [ Open on GitHub ]   [ Retry ]                                      │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

The whole tab, with no sections. The same layout serves "disabled" and "single sign-on needed", with their own sentence. An empty repository is not this: it gets the normal frame, with Code saying it is empty.

### Search GitHub, in the panel

```text
┌──────────────────────────────┐
│ ⎈ Helm            ⌂  ←  →    │
├──────────────────────────────┤
│ 🔍 gpui component        ⏎   │
│ [ Best match ▾ ]             │
├──────────────────────────────┤
│ longbridge/gpui-component    │
│ Rust GUI components…  ★ 9.1k │
│ zed-industries/zed           │
│ Code at the speed…   ★ 62k   │
│ …                            │
├──────────────────────────────┤
│ Page 1 of 100         ‹  ›   │
├──────────────────────────────┤
│ Search: 28 of 30 this minute │
└──────────────────────────────┘
```

Reached from the new **Search GitHub** entry in the main menu. The same list screen as Repositories, with a search that sends on `enter` and a sort menu. A result opens the panel's repository screen. The foot shows the search allowance while this screen is open.

### Code search, in the tab

```text
│ Code │ 🔍 load_section_page                  ⏎ │  Searching main (the default branch)     │
│      │─────────────────────────────────────────│                                          │
│      │ crates/helm_panel/src/section.rs      3 │   (the file, at the selected match)      │
│      │   168  pub(super) fn load_section_page… │                                          │
│      │   196  /// [`Self::load_section_page`]… │                                          │
│      │ crates/helm_panel/src/issues.rs       1 │                                          │
│      │    17  self.load_section_page(          │                                          │
│      │ 4 results                               │                                          │
```

The tree's place is taken by results while a search is active: files, each with its matching lines. Selecting a line opens the file there. `esc` returns to the tree. The line beside the box always says which branch was searched, because it is the default branch whatever is in view (section 3a).

## 3e. Keyboard

Everything in the Workspace can be done without the mouse. The keys below are proposed defaults, checked on 7 October 2026 against the app's Windows keymap (`assets/keymaps/default-windows.json`) and against what `gpui_component`'s widgets already handle.

### Rules

- **Every command is an action**, named `helm_workspace::…`. So each one appears in the command palette, can be rebound in the user's keymap, and shows its key in its button's tooltip.
- **Default keys go in the app's keymap files**, where `ctrl-k h` for the panel already is, for Windows, Linux and macOS (`cmd` where Windows has `ctrl`). None are bound in code.
- **All of them are scoped to the tab** (key context `HelmWorkspace`, and narrower ones for the tree, lists and viewer). Nothing here changes a key anywhere else in the app.
- **A key means in the tab what it means in the app.** Where the app has a key for the same idea, the tab uses it: back and forward, go to file, search, copy path, move to the next part.
- **The widgets' own keys are left alone.** Lists, the tree, menus and inputs already handle theirs.

### What the widgets already do

| Widget | Keys it handles |
|---|---|
| `List` (commits, pull requests, issues, changed files, results) | `up`, `down`, `enter`, `ctrl-enter` (second action), `escape` |
| `tree` | `up`, `down`; `right` opens a folder or steps into it; `left` closes it or steps out to its parent |
| Menus and the ref selector's list | `up`, `down`, `enter`, `escape` |
| The viewer, if it is the app's editor (trial 2) | Everything a read-only editor has: find (`ctrl-f`), go to line (`ctrl-g`), selection, copy |

One gap: the `tree` acts on "confirm" but binds no key to it. The Workspace binds `enter` for it.

### Opening

| Key | Does |
|---|---|
| `ctrl-k h` | The Helm panel (exists today). |
| `ctrl-k shift-h` | The Workspace for the repository of the open folder, when it is on GitHub. Otherwise the panel's repository list. Free in the keymap today. |
| `enter` on a repository in the panel | The panel's repository screen, as now. Its new **Open Workspace** button is the first thing focused. |

### Moving between the parts of the tab

| Key | Does |
|---|---|
| `f6` / `shift-f6` | Next and previous part: sections, tree, viewer, context pane. The app uses these same keys to move between its own parts; inside the tab they move within it. |
| `alt-left` / `alt-right` | Back and forward through what this tab has shown: files, commits, sections. The app's own back and forward keys. |
| `escape` | One step out: closes a menu, then leaves search for the tree, then leaves the viewer for the tree. Never closes the tab. |

Where focus lands:

- **The tab opens:** the tree, on the first entry. On Overview when Code has not been opened yet.
- **A file is opened with `enter`:** focus stays in the tree, so `down`, `enter` walks through files. `right` on a file moves into the viewer.
- **A section is chosen:** its list, on the row last selected there.
- **Update is pressed, or the ref changes:** focus stays where it was, on the same path when it still exists.

### In the tab, anywhere

| Key | Does | Same as the app's |
|---|---|---|
| `ctrl-p` | Go to file: focuses the tree's filter box. | Go to file |
| `ctrl-shift-f` | Search code: focuses the code search box. | Search in project |
| `ctrl-shift-b` | The ref selector. Clashes with nothing inside the tab; elsewhere it is the outline panel. | |
| `f5` | Refresh: checks where the branch points. | |
| `alt-u` | Update, when the line offering it is showing. Free today. | |
| `alt-h` | History of the file or folder in view. Free today. | |
| `shift-alt-c` | Copy path. | Copy path in the project panel |
| `ctrl-k p` | Copy permalink. In an editor this key copies the path; here it copies the link to the exact commit. | |
| `ctrl-enter` | Open in editor (W4). The "second action" key the lists already use. | |

### In the tree and in lists only

Single letters, as on github.com, for people who know them. They work only where there is nothing to type into, never in the viewer or a search box.

| Key | Does | On github.com |
|---|---|---|
| `t` | Go to file | `t` |
| `w` | The ref selector | `w` |
| `/` | Search code | `/` |
| `y` | Copy permalink | `y` (makes the URL permanent) |

The letters `h`, `j`, `k` and `l` are kept free on purpose, for the next point.

### Vim mode

When the app's Vim mode is on, the tree and the lists also take `j` and `k` for down and up, and the tree `h` and `l` for left and right, as the project panel does. The viewer, being the app's editor, follows Vim mode by itself.

### On a commit and in a comparison

| Key | Does |
|---|---|
| `up` / `down` in the file list | Selects a file and shows its diff. |
| `right` | Moves into the diff. `escape` or `left` at its edge returns to the list. |
| `alt-]` / `alt-[` | Next and previous changed file, from the list or from inside the diff. In an editor these keys step through edit predictions, which a read-only diff has none of. |

### What is deliberately not bound

- **`alt-1` to `alt-9`** for the sections. The app uses them to switch between tabs in a pane. Sections are reached with `f6` and the arrows, or from the command palette.
- **Anything that writes.** There is nothing in this plan to bind.

### By phase

- **W2:** the actions exist and are in the command palette; opening, `f6`, `f5`, `alt-u`, the ref selector.
- **W3:** the tree and viewer keys, go to file, copy path and permalink, back and forward, the single letters, Vim keys.
- **W4:** `ctrl-enter`.
- **W5:** history, and the keys on a commit.
- **W6:** search code.

## 4. Phase W0 — Decisions and two short trials

Small, and before any real code. Settles the open questions in section 12, and tries the two things the rest of the plan leans on. The trials are throwaway code on a scratch branch; nothing from them goes to `main`. **They wait until this document is agreed.**

### Trial 1: does a `Settings` page hold the Code screen?

A tree, a viewer and a context pane inside one page is more than the managers put there.

First look, from reading `gpui_component`'s `setting` code (not tried): a page can hold any content through `SettingItem::render`, but a page is a scrolling column of groups. A file tree and a viewer each need the full height and their own scrolling, so a poor fit is expected.

Likely outcome: `Settings` pages for the sections that are lists or forms (Overview, Commits, Pull requests, Issues, Actions), and Code as its own full-height `h_resizable` split beside them. The trial builds that shell with placeholder content to see whether the two sit together.

### Trial 2: can a remote file be shown in the app's read-only editor?

If text fetched from GitHub can be put in a read-only editor buffer, the viewer gets highlighting, search and selection for free. How the app builds such a buffer from text that is not a file on disk has not been confirmed. The fallback is `gpui_component`'s highlighter.

### Noted for W5

The app's diff engine (`buffer_diff`) is built from a base text and a buffer, not from two files on disk. That makes drawing diffs with the app's own diff view look possible. It still gets its own trial at the start of W5.

### Done when

- Both trials have an answer, written into this file.
- Every row of section 12 has a decision.

## 4a. Phase W0.5 — A shared `helm_ui` crate

Decided in section 12, row 1: the Workspace is its own crate, `helm_workspace`. The panel and the Workspace both need the same pieces, and today those are private to `helm_panel`. Before the tab is built they move to a `helm_ui` crate that both depend on.

```text
helm_backend   GitHub: requests, answers, cache, limits (no UI)
     ▲
helm_ui        shared pieces: sections, list view, rows, small widgets
     ▲    ▲
helm_panel     helm_workspace
(dock panel)   (the tab)
```

This section is from reading the code as it stands on 7 October 2026 (`helm_panel/src`, 7,465 lines in 24 files). Nothing has been moved or tried.

### How the shared pieces are tied to the panel today

Less than feared. There are exactly three ties:

| Piece | Tie to `HelmPanel` |
|---|---|
| The loaders in `section.rs` (`load_section_with`, `load_section`, `load_section_page`, `load_repo_page`) | Methods on `HelmPanel`. They use two of its fields: `gh_state` and `selected_repo`. |
| `list_view.rs` (`RowsDelegate`, `ListView`, `Pager`, `list_screen`) | Names the type in its signatures: `WeakEntity<HelmPanel>`, `Fn(&HelmPanel, …)`, `Context<HelmPanel>`. It reads no field of the panel; everything it needs comes through the closures a screen hands it. |
| Everything, through `use super::*` | Each file takes its imports from the panel's root file. In a new crate each needs its own. |

`Section<T>` itself, the small widgets and every row function have no tie at all: they take data and a theme and return an element.

### The one generalisation

A small trait says what a view must provide, and the loaders and the list screen become methods any such view gets:

```rust
/// A view that shows GitHub data: the panel, the Workspace tab.
pub trait HelmView: Sized + 'static {
    fn gh_state(&self) -> &Arc<GhState>;
    /// The repository the view is about, when it is about one.
    fn repo(&self) -> Option<&Repo>;
}

/// Loading sections and drawing list screens, for any `HelmView`.
pub trait HelmViewExt: HelmView {
    fn load_section_with<T, E, Fut>(&mut self, cx: &mut Context<Self>, /* as today */);
    fn load_section<T, E, Fut>(&mut self, /* as today */);
    fn load_section_page<T, E, Fut>(&mut self, /* as today */);
    fn load_repo_page<T>(&mut self, /* as today */);
    fn list_screen(&self, /* as today */) -> AnyElement;
}
impl<V: HelmView> HelmViewExt for V {}
```

`ListView`, `RowsDelegate`, `ListStatus` and `Pager` take the view as a type parameter (`ListView<V>`) in place of the name `HelmPanel`.

What this buys: **the panel's screens do not change.** `self.load_repo_page(cx, |this| &mut this.tags, page, …)` and `self.list_screen(…)` read exactly as they do now, because they are still method calls with the same arguments. The panel adds an eight-line `impl HelmView for HelmPanel`, and one line, `type ListView = helm_ui::ListView<HelmPanel>;`, so its sixteen list fields keep their type name.

The bodies of the moved functions change in two places only: `self.gh_state.clone()` becomes `self.gh_state().clone()`, and `self.selected_repo.clone()` becomes `self.repo().cloned()`.

### What moves, exactly

| From `helm_panel/src` | What | Lines, about | To `helm_ui/src` |
|---|---|---|---|
| `section.rs` | All of it: `Section`, `PAGE_SIZE`, `page_slice`, the four loaders, 9 tests | 470 | `section.rs` |
| `list_view.rs` | All of it | 350 | `list_view.rs` |
| `state.rs` | `LoadState` only | 6 | `section.rs` |
| `widgets.rs` | `loading_screen`, `note_screen`, `failed_screen`, `chip`, `fmt_num`, `short_date`, `repo_vis_label`, `hex` | 100 | `widgets.rs` |
| `issues.rs` | `issue_row` | 60 | `rows.rs` |
| `pulls.rs` | `pull_row` | 55 | `rows.rs` |
| `activity.rs` | `commit_row`, `workflow_run_row`, `workflow_status_color` (from `workflow_run_tab.rs`) | 90 | `rows.rs` |
| `branches.rs` | `branch_row` | 25 | `rows.rs` |
| `releases_packages.rs` | `tag_row`, `release_row`, `release_summary`, and their 2 tests | 110 | `rows.rs` |

About 1,270 lines, a sixth of the panel. `helm_panel` drops to about 6,200.

Everything moved goes from `pub(super)` to `pub`. `helm_ui` exports it all from its root, and the panel's root file gains `use helm_ui::*;`, so the panel's other files, which import through the root, need no edit for names.

### What stays in the panel, and why

| Stays | Why |
|---|---|
| `HelmScreen`, `AuthOutcome`, `MenuItem`, `NavRow` | The panel's own navigation and sign-in. |
| `lists.rs` | Each function wires one panel screen to one panel field. |
| `loading.rs` (`load_with`, `load_for_repo`) | Tied to the panel's single shared loading flag and error message, which five of its screens still use. The Workspace must not copy that pattern (see below). |
| `repo_row`, `org_row`, `menu_row`, `profile_row`, `repo_section_row`, `package_row`, `deployment_row`, the two alert rows | No screen in this plan's Workspace shows them. They move when something else needs them, not before. |
| `collaborator_row`, the two invitation rows | They hold a handle to the panel to run its actions. Not shareable as they are, and not needed. |
| Sign-in, the scope gate, `changes.rs` (the single write path), the dialogs, clone | The Workspace does not write. It opens the panel's dialogs and clone when it needs them, in W4. |

### Left for W7, on purpose

Three things the Workspace will share with the panel are **not** moved in W0.5, because moving them means reshaping them, and that belongs with the phase that first uses them:

- **Issue and pull request detail, and the comment thread.** Today they are methods that read four panel fields (`selected_issue`, `selected_pr`, `detail_comments`, `detail_comments_state`). To be shared they become functions that are handed the issue and its comments. The comments become a `Section<Comment>`, which is what they already are in everything but name.
- **The Open / Closed / All switch.** It calls the panel's own filter function.
- **The workflow-run tab.** It already stands by itself (it takes a `GhState`, not the panel) and would move as it is, but nothing needs it before W7.

### New in `helm_ui`, not moved: one value that loads

The Workspace loads many things that are one value, not a list: a tree, a file, a commit, a comparison. The panel's way of doing that is one loading flag and one error message for the whole panel, which the refactor plan already named as a weakness and fixed for lists only.

`helm_ui` gets the single-value sibling of `Section<T>`, written new in W2:

```rust
pub struct Loaded<T> {
    pub value: Option<T>,
    pub state: LoadState,
    pub error: String,
    load: u64,   // which load is current, as in Section
}
```

with `begin`, `finish`, `is_current` and `clear`, and a loader on `HelmViewExt`. The panel's five single-value screens (profile, organisation, user profile, Traffic, comments) could move onto it later. That is not part of this plan.

### Housekeeping the new crates need

- `helm_ui` and `helm_workspace` added to the workspace `Cargo.toml`, each with `README.md` and the `LICENSE-APACHE` link, as `PORTING.md` sets out.
- Both added to `script/test-fork-crates.ps1` and so to the `fork_tests` workflow on GitHub.
- `helm_ui` depends on `gpui`, `gpui_component`, `helm_backend` and `serde`. It does not depend on `workspace` or `editor`, so it stays quick to build and test.
- `helm_workspace` is created empty in this phase: a `Cargo.toml`, a root file with a doc comment, nothing else.

### Order of work

Each step builds and passes the panel's tests before the next, and each is its own commit:

1. Create `helm_ui`, empty. Add the `HelmView` trait.
2. Move `LoadState`, `Section` and its tests. No generalisation needed.
3. Move the widgets and the row functions. No generalisation needed.
4. Move the loaders, changing the two field reads to trait calls. Add `impl HelmView for HelmPanel`.
5. Move `list_view.rs`, replacing the name `HelmPanel` with the type parameter.
6. Create `helm_workspace`, empty. Add both crates to the test script.

Steps 2 and 3 are pure moves. Steps 4 and 5 are the generalisation, and are the only ones where behaviour could change by mistake.

### Done when

- `helm_panel` builds on `helm_ui`. Its 5 remaining tests and the 11 that moved all pass. No panel screen's code changed beyond imports.
- The manual checklist in `Helm_Refactor_Plan.md` passes unchanged.
- `helm_workspace` exists, depends on `helm_ui` and `helm_backend`, and not on `helm_panel`.
- `helm_ui` has no mention of `HelmPanel`.

### What could go wrong

- **Type inference at the call sites.** The closures a screen passes (`|this| &mut this.issues`) are checked today against a function that names `HelmPanel`. Against a trait method on `Self` they should resolve the same way, but "should" is from reading, not from compiling. If some need a type written out, that is an edit per call site (about 25), not a redesign.
- **`impl HelmViewExt for V` and method names.** If the panel ever defines a method with the same name as a trait method, the panel's own wins silently. The moved methods must be removed from the panel in the same commit they are added to the trait.
- **Imports.** Files that lean on `use super::*` hide what they use. The first build of `helm_ui` will list everything missing; tedious, not risky.

## 5. Phase W1 — Backend for code

No UI. Adds to `helm_backend`:

- Request builders and types for: the tree of a commit, a blob, resolving a branch or tag to a commit, a single commit with its files, comparing two refs, the README.
- Typed permissions, the single sign-on error, and "empty repository" as an outcome (section 3b).
- Working out, from a comparison, whether a branch moved forward or was rewritten (section 3c).
- Per-kind rate limits, the byte limit on the cache, and keeping hash-addressed answers.
- Answers carried as bytes, a request's wanted format, and the shared limit on requests in flight (section 3a).
- Holding all requests for the time GitHub gives after a secondary limit.
- Turning a flat tree listing into a nested tree, and handling the truncated case.
- Deciding what a tree entry is from its mode: file, folder, symlink, submodule.
- Deciding what a file is from its path, size and first bytes: text, image, binary, Git LFS pointer, too large to show.

### Done when

- Tests cover every new request, the tree building (including truncated and empty trees, symlinks and submodules), the file classification, the cache's size limit, and the limit on requests in flight.
- A terminal example, like `whoami`, prints the top of a real repository's tree and the first lines of its README.

## 6. Phase W2 — The tab

The shell, with nothing in it yet.

- A Workspace tab that opens for a repository, from a button on the panel's repository screen and from the command palette.
- Header: owner and name, the private, archived and fork tags, the ref selector (branches and tags, searchable), Refresh, Open on GitHub.
- The ref in view as section 3c describes it: resolved to a commit, checked when the tab comes to the front, with the line that offers Update.
- The whole-tab messages of section 3b for a repository that cannot be opened.
- The section pages, each showing an empty state.
- `Loaded<T>`, the single-value sibling of `Section<T>`, added to `helm_ui` (section 4a).
- Overview page: description, topics, default branch, visibility, last commit, counts of open pull requests and issues. All of this is data the panel already fetches.
- The rate-limit line, as in the panel.

### Done when

- The tab opens, is reused when opened again for the same repository, and survives switching branch.
- Pushing to the branch in view from elsewhere brings up the Update line the next time the tab comes forward, and not before.
- Two repositories can be open in two tabs without sharing state.

## 7. Phase W3 — Code

The tree and the viewer. This is the phase that makes the Workspace worth opening.

- File tree for the ref in view, on `gpui_component`'s `tree`, with keyboard navigation and a filter box that narrows by file name (done locally, so it costs no requests).
- File viewer: read-only, highlighted, with the path as a breadcrumb.
- The README is shown when the tab opens and nothing is selected.
- Images are shown as images. Binary and oversized files show what they are, their size, and Open on GitHub.
- Copy path, copy permalink (a link to the file at the exact commit).
- Switching ref keeps the same path open when it exists there.

### Done when

- A repository of ordinary size loads its tree in one request and opens a file in one more.
- Going back to a file already seen sends nothing.
- A very large repository still works, a directory at a time.

## 8. Phase W4 — The editor and a local clone

Reading is remote. Editing is local. This phase joins the two.

- Work out whether the repository is already on disk: one of the open folders has it as a Git remote. Compare its checked-out commit with the commit in view (section 3c).
- **Open in editor** on a file: opens the local file when there is a clone on the same branch. When the local branch differs from the ref in view, say so before opening, because the file may not be the same.
- When there is no clone: offer Clone, using the panel's existing clone flow, then open the file.
- Show in the header whether a local clone exists and which branch it is on.

### Done when

- From a file in the Workspace, one action lands in the editor on the same file and line.
- A mismatch between the local branch and the ref in view is never silent.

## 9. Phase W5 — History and diffs

- Commits page: the history of the ref in view, paged, and of a single file or folder when one is selected.
- Commit detail: message, author, the files it changed, and the diff of each.
- Compare: pick two refs, see the commits and the changed files between them.
- Tags in the ref selector open the tree at that tag (already possible from W2); the Commits page shows which commits are tagged.

How diffs are drawn is the main open question of this phase (section 12, row 5).

### Done when

- From any file, its history is one step away, and from any commit, its diff is.
- A commit with hundreds of changed files stays responsive: files are listed at once and each diff is drawn when opened.

## 10. Phase W6 — Search, and W7 — Context

**W6. Search.** Three kinds, in rising cost:

1. **File names** in the open repository: already there from W3.
2. **Code** in the open repository: GitHub's code search. Results open the file at the matching line.
3. **Repositories across GitHub:** in the dock panel, not the tab, since it is how you reach a repository that is not yours. It needs a new entry in the panel's main menu, **Search GitHub**, between Repositories and Create Repository, leading to a screen with a search box and a paged list of results. A result opens the panel's existing repository screen, from which the Workspace opens; a repository that is not yours shows there without the actions you have no permission for. This is the gap left in the roadmap's phase 1 and does not depend on W3 to W5; it can be moved earlier if wanted.

Search has a small allowance of its own, so the search box sends on `enter`, not on every keystroke, and shows that allowance when it runs low.

**W7. Context pane.** For the ref in view:

- The pull request whose head is this branch, with its state and checks.
- The latest Actions run for the branch, opening the existing run tab.
- Open issues and pull requests, as lists that open the detail in the tab.
- The Pull requests, Issues and Actions pages get their content here. They show the same data as the panel's screens; the row drawing is shared, not copied.
- The three pieces section 4a left for this phase move to `helm_ui` here, reshaped so that both the panel and the tab can use them: issue and pull request detail with the comment thread, the Open / Closed / All switch, and the workflow-run tab.

### Done when

- Looking at a branch, you can tell without leaving the tab whether it has a pull request and whether its checks pass.
- No screen exists twice: the panel and the tab draw a pull request row with the same code.

## 11. Manual checklist

Added to the one in `Helm_Refactor_Plan.md`. Run after every phase from W2 on.

| # | Do | Expect |
|---|---|---|
| 1 | Open a repository's Workspace from the panel | A tab with the repository's name and its default branch selected |
| 2 | Open it again | The same tab comes forward; no second one |
| 3 | Open a second repository | A second tab; the first is unchanged |
| 4 | Expand folders, open a source file | Highlighted, read-only; the tree is usable from the keyboard |
| 5 | Open an image and a binary file | The image is shown; the binary says what it is |
| 6 | Switch branch with a file open | The same path at the new branch, or a clear "not on this branch" |
| 7 | Revisit files and folders already seen | Instant, and the count of requests left does not fall |
| 8 | Open in editor, with a clone and without | The local file opens; without a clone, Clone is offered |
| 9 | Open a file's history, then a commit | The commit's files and diffs |
| 10 | Search code, open a result | The file at the matching line |
| 11 | Open this fork, then `torvalds/linux` | This fork's tree loads in one request. Linux's is too large for that and loads a folder at a time; nothing freezes |
| 11a | In this fork, open a `LICENSE-GPL` symlink; in a repository that has them, a submodule and a Git LFS file | Each says what it is, not a blank or garbled viewer |
| 12 | Open a repository you cannot see, and one that is empty | A clear message, not a blank tab |
| 13 | With a tab open on a scratch branch, push a commit to it from a terminal, then bring the tab forward | "has 1 new commit. Update"; the tree does not change until Update |
| 14 | Force-push that branch, then delete it | "was rewritten", then "was deleted", and the tab still shows the last commit |
| 15 | Open someone else's public repository, and an archived one | Both open; the archived one says so |
| 16 | Put the mouse aside. Open the Workspace, find a file with go to file, open it, see its history, open a commit, come back | Every step has a key, and focus is always visible |
| 17 | Open the command palette and type "helm workspace" | Every command is listed, with its key |

## 12. Decisions

| # | Question | Decision |
|---|---|---|
| 1 | Where does the code live: inside `helm_panel`, or a new crate? | **Decided:** a new `helm_workspace` crate. The pieces both need move to a shared `helm_ui` crate first (phase W0.5). |
| 2 | Sections as `Settings` pages or a tab bar? | Open until trial 1. Expected: `Settings` pages for the list and form sections, Code as its own split. |
| 3 | File viewer: the app's editor, read-only, or `gpui_component`'s highlighter? | Open until trial 2. The editor if it works. |
| 4 | How large a file is shown inline? | **Decided:** text up to 1 MB and images up to 10 MB; above that, the file's size and Open on GitHub. The endpoint allows 100 MB, so this is our limit, chosen to keep the viewer quick, and can be raised later. |
| 5 | W5: draw diffs with the app's own diff view, or from GitHub's patch text? | Open until a trial at the start of W5. The app's diff view looks possible (see section 4). |
| 6 | Is the tab restored when the app restarts? | **Decided:** not at first. Added after W3, once there is something worth restoring. |
| 7 | W6: is repository search in the panel or the tab? | **Decided:** the panel, with a new **Search GitHub** entry in its main menu. |
| 8 | W4: when the local clone is not on the commit in view, open anyway? | **Decided:** ask each time; never switch the local branch for the user. No question when the commits are the same. |
| 10 | Does the tab follow its branch, or stay on the commit it loaded? | **Decided:** stay, and offer Update when the branch moves (section 3c). |
| 11 | How often is the branch checked? | **Decided:** when the tab comes to the front and a minute has passed, and on Refresh. Never in the background. |
| 12 | Single-letter keys as on github.com (`t`, `w`, `/`, `y`) in the tree and lists? | **Decided** (8 October 2026): yes (section 3e). |
| 13 | `j`, `k`, `h`, `l` in the tree and lists when Vim mode is on? | **Decided** (8 October 2026): yes, in W3. Check first that the project panel binds the same letters, and match it. |
| 14 | The three keys in section 3e that reuse one bound elsewhere in the app: `ctrl-shift-b`, `ctrl-k p`, `alt-]` / `alt-[`. | **Decided** (8 October 2026): as proposed. |
| 9 | Which phase follows W3? | **Decided:** W4 (the editor and a local clone). The order of W5, W6 and W7 after it is still open; suggested W7, W6, W5. |

## 13. Size of the work

Rough, and only that.

| Phase | Size | Risk |
|---|---|---|
| W0. Decisions and trials | Small | Low, and it lowers the risk of everything after |
| W0.5. Shared `helm_ui` crate | Small to medium: about 1,270 lines moved, six commits | Low to medium: two of the six steps change signatures; the panel's screens are not edited |
| W1. Backend for code | Medium | Low: no UI, fully testable |
| W2. The tab | Small to medium | Low |
| W3. Code | Large | Medium: the viewer and very large trees |
| W4. Editor and clone | Medium | Medium: matching a local clone to a repository has edge cases (forks, renamed remotes, several clones) |
| W5. History and diffs | Large | High: diff drawing is the least known part |
| W6. Search | Medium | Low to medium: the separate allowance |
| W7. Context | Medium | Low: mostly existing data in a new place |

Order: W0, then W0.5 and W1 in either order, then W2, W3, W4. W2 needs W0.5; W3 needs W1 and W2. W5, W6 and W7 each need W3 and are independent of one another. Repository search in W6 depends on nothing but W1's rate-limit change.

## 13a. Still soft in this document

To settle before code starts:

- **Three facts to confirm by trying them in W1:** the answer for an organisation that needs single sign-on, the answer for an empty repository's tree, and how long a commit that is on no branch stays available.

## 14. After this plan

With W0 to W7 done, the roadmap's phase 2 is in place and the reading half of phase 3. Next, each with its own short plan:

1. **Writing:** create and delete branches, commit and push through local Git, create pull requests from the Workspace.
2. **Review:** comment on a diff, approve, request changes, merge.
3. **Issues:** create, label, assign, close.
