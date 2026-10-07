# Helm Workspace — Phased Plan

**Written:** 7 October 2026. **Status:** not started. Nothing here has been built or tried; sizes and API limits are estimates to confirm in phase W0.
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
| A file's contents | Contents / Blobs | Blobs are addressed by hash and never change, so they can be kept without checking. |
| Commit with its changed files | Commits (single) | Carries the patch per file. |
| Two refs compared | Compare | |
| Code and repository search | Search | Has its own, much smaller allowance than the main one. |

Three things in `helm_backend` have to change to carry this safely:

1. **Rate limits per kind of request.** The backend tracks only the main allowance and ignores the others. Search needs its own count, shown and respected separately.
2. **A size limit on remembered answers.** The cache holds up to 200 answers whatever their size. Trees and files can be megabytes each, so it needs a limit in bytes.
3. **Answers that never change.** A tree or blob fetched by hash is the same forever. These should be kept and served without asking GitHub again.

## 4. Phase W0 — Decisions and two short trials

Small, and before any real code. Settles the open questions in section 12, and tries the two things the rest of the plan leans on:

- **Does a `Settings` page hold the Code screen?** A tree, a viewer and a context pane inside one page is more than the managers put there. If it does not fit, the sections become a tab bar and `Settings` is used for Overview only.
- **Can a remote file be shown in a read-only editor?** The app already makes local, read-only buffers elsewhere (the agent panel does). If that works for a file fetched from GitHub, the viewer gets highlighting, search and selection for free. If not, the fallback is `gpui_component`'s highlighter.

### Done when

- Both trials have an answer, written into this file.
- Every row of section 12 has a decision.

## 5. Phase W1 — Backend for code

No UI. Adds to `helm_backend`:

- Request builders and types for: the tree of a ref, a blob, a file by path, resolving a branch or tag to a commit, a single commit with its files, comparing two refs.
- Per-kind rate limits, the byte limit on the cache, and keeping hash-addressed answers.
- Turning a flat tree listing into a nested tree, and handling the truncated case.
- Deciding what a file is from its path and first bytes: text, image, binary, too large to show.

### Done when

- Tests cover every new request, the tree building (including truncated and empty trees), the file classification, and the cache's size limit.
- A terminal example, like `whoami`, prints the top of a real repository's tree and the first lines of its README.

## 6. Phase W2 — The tab

The shell, with nothing in it yet.

- A Workspace tab that opens for a repository, from a button on the panel's repository screen and from the command palette.
- Header: owner and name, the ref selector (branches and tags, searchable), Open on GitHub.
- The section pages, each showing an empty state.
- Overview page: description, topics, default branch, visibility, last commit, counts of open pull requests and issues. All of this is data the panel already fetches.
- The rate-limit line, as in the panel.

### Done when

- The tab opens, is reused when opened again for the same repository, and survives switching branch.
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

- Work out whether the repository is already on disk: one of the open folders has it as a Git remote.
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
3. **Repositories across GitHub:** in the dock panel, not the tab, since it is how you reach a repository that is not yours. Results open in a Workspace tab. This is the gap left in the roadmap's phase 1 and does not depend on W3 to W5; it can be moved earlier if wanted.

Search has a small allowance of its own, so the search box sends on `enter`, not on every keystroke, and shows that allowance when it runs low.

**W7. Context pane.** For the ref in view:

- The pull request whose head is this branch, with its state and checks.
- The latest Actions run for the branch, opening the existing run tab.
- Open issues and pull requests, as lists that open the detail in the tab.
- The Pull requests, Issues and Actions pages get their content here. They show the same data as the panel's screens; the row drawing is shared, not copied.

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
| 11 | Open a very large repository (this fork is one) | The tree still loads; nothing freezes |
| 12 | Open a repository you cannot see, and one that is empty | A clear message, not a blank tab |

## 12. Decisions

Needed in phase W0 unless another is named.

| # | Question | Suggested default |
|---|---|---|
| 1 | Where does the code live: inside `helm_panel`, or a new crate? | A new `helm_workspace` crate. The pieces both need (`Section`, the list view, row drawing, small widgets) move to a shared `helm_ui` crate first, as a move-only step. |
| 2 | Sections as `Settings` pages or a tab bar? | `Settings` pages, to match the managers, if the first W0 trial passes. |
| 3 | File viewer: the app's editor, read-only, or `gpui_component`'s highlighter? | The editor, if the second W0 trial passes. |
| 4 | How large a file is shown inline? | 1 MB, the limit of the simplest endpoint. Above that, offer Open on GitHub. Confirm the limit in W0. |
| 5 | W5: draw diffs with the app's own diff view, or from GitHub's patch text? | The app's diff view if it can take two texts that are not files on disk; otherwise the patch text, highlighted. Needs a short trial at the start of W5. |
| 6 | Is the tab restored when the app restarts? | Not at first. Add it after W3, once there is something worth restoring. |
| 7 | W6: is repository search in the panel or the tab? | The panel. |
| 8 | W4: when the local branch differs from the ref in view, open anyway? | Ask each time; never switch the local branch for the user. |

## 13. Size of the work

Rough, and only that.

| Phase | Size | Risk |
|---|---|---|
| W0. Decisions and trials | Small | Low, and it lowers the risk of everything after |
| W1. Backend for code | Medium | Low: no UI, fully testable |
| W2. The tab | Small to medium | Low |
| W3. Code | Large | Medium: the viewer and very large trees |
| W4. Editor and clone | Medium | Medium: matching a local clone to a repository has edge cases (forks, renamed remotes, several clones) |
| W5. History and diffs | Large | High: diff drawing is the least known part |
| W6. Search | Medium | Low to medium: the separate allowance |
| W7. Context | Medium | Low: mostly existing data in a new place |

W1 and W2 do not depend on each other. W3 needs both. W4, W5, W6 and W7 each need W3 and are independent of one another, so their order can follow what is wanted first. Repository search in W6 depends on nothing but W1's rate-limit change.

## 14. After this plan

With W0 to W7 done, the roadmap's phase 2 is in place and the reading half of phase 3. Next, each with its own short plan:

1. **Writing:** create and delete branches, commit and push through local Git, create pull requests from the Workspace.
2. **Review:** comment on a diff, approve, request changes, merge.
3. **Issues:** create, label, assign, close.
