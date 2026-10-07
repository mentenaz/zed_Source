# Mentenaz Forge — Helm Platform, Workspace & Phased Delivery Roadmap

## 1. Overview

**Mentenaz Forge** is positioned as a developer OS / developer cockpit: an environment where developers can write code, manage projects, run development workflows, interact with automation, and coordinate AI-assisted development.

Within Forge, **Helm** is the dedicated GitHub platform.

Helm is not intended to be a simple GitHub viewer or a replacement website with a different visual style. It is a **full GitHub management and development platform built around the GitHub API**, integrated directly into the Forge desktop environment.

The direction established in this discussion is to give Helm **two complementary experiences**:

1. **Helm Control / Management** — the operational and administrative side of GitHub.
2. **Helm Workspace** — a GPUI-native GitHub development workspace for actively working with repositories and code.

These two experiences should work together while maintaining clear responsibilities.

---

# 2. Helm's Core Identity

The central concept is:

> **Helm is the command center for GitHub inside Mentenaz Forge.**

Rather than reproducing GitHub's website, Helm should use GitHub's APIs as the underlying infrastructure while providing a Forge-native experience optimized for developers.

GitHub supplies the underlying capabilities:

- repositories
- Git data
- branches
- commits
- files
- issues
- pull requests
- reviews
- Actions
- releases
- organizations
- teams
- security information
- users
- permissions
- webhooks
- and other GitHub resources

Helm provides the **developer-oriented control layer and user experience** over those capabilities.

---

# 3. Helm's Two Experiences

```text
                         MENTENAZ FORGE
                      Developer OS / IDE
                              │
                              ▼
                           HELM
                    GitHub Platform Layer
                              │
              ┌───────────────┴───────────────┐
              │                               │
       HELM CONTROL                      WORKSPACE
       Management                        Development
              │                               │
              ▼                               ▼
     GitHub administration             GitHub repository UI
     Repository management             Code
     Organizations                     Files
     Teams                             Branches
     Permissions                       Commits
     Actions management                Diffs
     Security                          Pull Requests
     Releases                          Issues
     Automation                        Actions
                                      Git history
```

### Helm Control

> **Operate GitHub.**

Focus:

- organizations
- repositories
- permissions
- administration
- Actions
- security
- releases
- teams
- bulk operations
- GitHub-wide activity
- automation

### Helm Workspace

> **Work with GitHub.**

Focus:

- repository explorer
- files
- branches
- commits
- diffs
- Git history
- pull requests
- issues
- Actions
- releases
- code
- Forge editor integration

---

# 4. Phased Helm Delivery Roadmap

The roadmap should deliberately avoid attempting to build the entire GitHub platform at once.

The first objective is to establish the **core architecture and daily-use workflow**. Later phases add management depth, automation, intelligence, and eventually broader developer-OS integration.

---

## Phase 0 — Helm Foundation

### Goal

Build the underlying platform that everything else depends on.

This phase should be relatively invisible to the user, but it is arguably the most important technically.

### Core infrastructure

Build:

- GitHub authentication
- GitHub API client
- REST API layer
- GraphQL layer where appropriate
- centralized request manager
- caching
- pagination
- rate-limit handling
- error handling
- permission/capability detection
- API state management
- repository model
- user/account model
- organization model
- event model

Conceptually:

```text
GPUI
 │
 ▼
Helm UI
 │
 ▼
Helm Application Layer
 │
 ▼
Helm Data Layer
 │
 ├── Cache
 ├── Request Queue
 ├── Deduplication
 ├── Permissions
 ├── Rate Limits
 └── Events
 │
 ▼
GitHub REST / GraphQL
```

### Deliverable

A stable internal Helm API/data layer that every subsequent panel uses.

### Success criteria

Helm can reliably:

- authenticate
- discover the current user
- discover organizations
- list repositories
- retrieve repository metadata
- handle GitHub errors
- respect permissions
- avoid redundant requests
- operate within API limits

---

# Phase 1 — Helm Core

### Goal

Deliver the first genuinely useful Helm experience.

This is the **minimum viable Helm**, not merely a prototype.

### Core UI

Build:

- Helm navigation
- repository list
- organization selector
- repository overview
- global search
- command palette
- notifications
- basic activity feed

### Repository overview

Show:

- repository information
- default branch
- visibility
- recent commits
- open PRs
- open issues
- Actions status
- releases
- contributors

### Command palette

Start establishing the Helm interaction model:

```text
Ctrl+K

> Open repository
> Search repository
> Open pull requests
> Open issues
> View Actions
> Create issue
> Create branch
```

### Deliverable

A user can open Forge, authenticate with GitHub, navigate their GitHub environment, and understand the state of their repositories.

### Success criteria

A developer can answer:

> "What is happening with my repositories?"

without opening GitHub in a browser.

---

# Phase 2 — Helm Workspace

### Goal

Introduce the **GPUI-native GitHub development workspace**.

This is the point where Helm becomes deeply integrated with Forge rather than simply being a management console.

### Workspace features

Build:

- repository tree
- branch selector
- file browser
- file viewer
- commit history
- Git history
- diffs
- tags
- repository search

Conceptually:

```text
┌───────────────────────────────────────────────────────────────┐
│ Helm Workspace                             forge / main        │
├──────────────┬───────────────────────────────┬───────────────┤
│ REPOSITORY   │ CODE                          │ CONTEXT       │
│              │                               │               │
│ ▼ src        │ workspace.rs                  │ PR #184       │
│   ├─ helm    │                               │ ✓ CI          │
│   ├─ ui      │ fn load_workspace() {        │ ✓ Review      │
│   └─ git     │     ...                       │               │
│              │ }                             │ Issues: 3     │
│ ▼ crates     │                               │ Actions ✓     │
│              │                               │               │
│ README.md    │                               │               │
│ Cargo.toml   │                               │               │
└──────────────┴───────────────────────────────┴───────────────┘
```

### Forge editor integration

A major milestone is:

```text
GitHub Repository
       ↓
Helm Workspace
       ↓
Select file
       ↓
Forge Editor
```

The developer should not need to leave Forge just to inspect or edit a repository.

### Success criteria

A developer can:

1. Open a GitHub repository.
2. Browse its files.
3. Switch branches.
4. Inspect commits.
5. View diffs.
6. Open files in Forge.
7. Work on the code locally.

At this point Helm becomes a genuine **GitHub development environment**.

---

# Phase 3 — Git & Collaboration

### Goal

Turn the Workspace from a repository explorer into a complete development workflow.

### Git capabilities

Add:

- branch creation
- branch deletion where permitted
- branch switching
- commits
- pushes
- tags
- commit comparison
- history navigation
- diff viewing

Where appropriate, GitHub API operations should be combined with Forge's local Git capabilities rather than forcing every Git operation through the remote API.

### Pull requests

Add:

- PR list
- PR creation
- PR detail
- changed files
- commits
- comments
- reviews
- approval
- request changes
- merge
- merge status
- checks

### Issues

Add:

- issue list
- issue creation
- issue detail
- comments
- labels
- assignees
- milestones
- closing/reopening

### Success criteria

A developer can perform a complete development cycle:

```text
Branch
  ↓
Code
  ↓
Commit
  ↓
Push
  ↓
Pull Request
  ↓
Review
  ↓
Merge
```

without leaving Forge.

---

# Phase 4 — Actions & Release Operations

### Goal

Bring CI/CD into the Helm workflow.

This is the **Engine Room** phase.

### Actions

Build:

- workflow list
- workflow details
- run history
- run status
- job details
- logs
- rerun
- cancel
- trigger workflow
- artifacts

### Live status

A repository should be able to show:

```text
Build       ✓
Tests       ✓
Lint        ✓
Security    ⚠
Deploy      Running
```

### Releases

Add:

- release list
- release detail
- draft release
- publish release
- edit release
- tags
- release assets
- artifact information

### Success criteria

A developer can:

```text
Code
 ↓
Commit
 ↓
PR
 ↓
Merge
 ↓
GitHub Actions
 ↓
Artifact
 ↓
Release
```

from the Forge environment.

---

# Phase 5 — Helm Control / Administration

### Goal

Expand Helm from a development interface into a genuine GitHub management platform.

### Organization management

Add:

- organizations
- members
- teams
- invitations
- collaborators
- repository access
- organization repositories

### Repository administration

Add:

- repository creation
- repository configuration
- visibility
- archive
- transfer
- branch configuration
- repository policies
- webhooks
- variables
- secrets where supported

### Permission-aware UI

Every operation should understand whether it is available to the current user.

For example:

```text
🔒 Delete Repository

Unavailable

Reason:
Requires repository administration permission.
```

### Success criteria

An organization owner or administrator can perform substantial GitHub administration from Helm rather than using GitHub's web interface.

---

# Phase 6 — Security & Repository Intelligence

### Goal

Turn raw GitHub information into useful engineering intelligence.

### Security

Where supported and authorized:

- Dependabot
- code scanning
- secret scanning
- security advisories
- vulnerabilities
- security configuration

### Repository health

Introduce a high-level health view:

```text
Repository Health
────────────────────────
Build             ✓
Tests             ✓
Dependencies      ⚠
Security          ✓
PR backlog        ⚠
Issues            ✓

Health            87%
```

### Intelligence

Surface things such as:

- stale PRs
- stale branches
- failing repositories
- inactive repositories
- long-running PRs
- recurring workflow failures
- dependency problems
- unusual activity

The objective is to answer:

> **"What needs my attention?"**

rather than merely:

> "What data exists?"

---

# Phase 7 — Event-Driven Helm

### Goal

Make Helm feel alive.

Move away from unnecessary polling and toward an event-driven architecture where possible.

Conceptually:

```text
                    GitHub
                       │
                    Webhooks
                       │
                       ▼
                Forge Event Bus
                       │
          ┌────────────┼────────────┐
          ▼            ▼            ▼
        Helm       Workspace      Agents
          │            │            │
          └────────────┼────────────┘
                       ▼
                      GPUI
```

### Events

Potentially react to:

- push events
- PR changes
- issue changes
- Action completion
- workflow failures
- releases
- repository events

### UI

The Helm interface can maintain a live activity stream:

```text
21:31  PR #184 updated
21:32  CI started
21:34  Tests passed
21:35  Review requested
21:37  PR approved
```

### Success criteria

The user should feel that Helm is **monitoring GitHub continuously**, rather than refreshing GitHub manually.

---

# Phase 8 — AI & Agent Integration

### Goal

Connect Forge's AI capabilities directly to Helm's GitHub data and operations.

This is where Helm begins to become substantially more than a GitHub client.

### AI repository understanding

Allow AI to reason over:

- repository structure
- commits
- PRs
- issues
- Actions
- documentation
- code
- repository history

### Agent workflows

For example:

```text
User:
"Investigate why the latest build is failing."

Helm Agent:
  ↓
Find failed workflow
  ↓
Inspect logs
  ↓
Inspect commit
  ↓
Inspect changed files
  ↓
Identify probable cause
  ↓
Create explanation
```

Potentially later:

```text
Investigate
  ↓
Fix
  ↓
Create branch
  ↓
Commit
  ↓
Push
  ↓
Create PR
  ↓
Monitor CI
```

This must remain permission-aware and require appropriate confirmation for consequential actions.

---

# Phase 9 — Cross-Repository Mission Control

### Goal

Move from individual repository management to **engineering fleet management**.

Instead of looking at:

> Repository A

Helm can understand:

> **My entire GitHub environment.**

For example:

```text
                    HELM
                      │
       ┌──────────────┼──────────────┐
       ▼              ▼              ▼
   Repository       Actions       Security
      Fleet           Fleet          Fleet
       │              │              │
       └──────────────┼──────────────┘
                      ▼
                Mission Control
```

Potential views:

- repositories requiring attention
- failing builds
- pending reviews
- stale PRs
- security issues
- releases
- organizational activity
- active agents

This is where the **Mission Control / Radar / Fleet / Engine Room** concepts become particularly valuable.

---

# Phase 10 — Forge Developer OS Integration

### Goal

Make Helm one subsystem of the larger Forge developer operating system.

The long-term architecture could become:

```text
                         MENTENAZ FORGE
                         Developer OS
                              │
          ┌───────────────────┼───────────────────┐
          │                   │                   │
          ▼                   ▼                   ▼
        HELM                 EDITOR             AGENTS
          │                   │                   │
          ▼                   │                   ▼
       GitHub                 │              AI workflows
          │                   │                   │
          └───────────────────┼───────────────────┘
                              │
                              ▼
                       Developer Workspace
```

Eventually Helm can sit alongside other Forge subsystems such as:

- local Git
- containers
- cloud infrastructure
- deployment systems
- package management
- project/task systems
- AI agents
- CI/CD
- observability

The goal is not to make Helm responsible for everything.

The goal is to make Helm the **GitHub control layer that connects naturally to those systems**.

---

# 5. Recommended Priority

The phases should not necessarily be treated as rigid releases. They represent an order of **technical dependency and product maturity**.

The recommended priority is:

```text
PHASE 0
Foundation
   ↓
PHASE 1
Helm Core
   ↓
PHASE 2
Workspace
   ↓
PHASE 3
Git + PR + Issues
   ↓
PHASE 4
Actions + Releases
   ↓
PHASE 5
Administration
   ↓
PHASE 6
Security + Intelligence
   ↓
PHASE 7
Events
   ↓
PHASE 8
AI / Agents
   ↓
PHASE 9
Mission Control
   ↓
PHASE 10
Forge Developer OS
```

This ordering is intentional.

There is little value in building an elaborate Mission Control dashboard before Helm has a reliable underlying repository, Git, PR, Actions, and event model.

---

# 6. MVP Definition

The first meaningful Helm release should **not** attempt to implement every GitHub feature.

A strong MVP would be:

### Helm Core

- GitHub authentication
- account/org discovery
- repository discovery
- centralized API layer
- caching
- permissions
- rate-limit handling

### Workspace

- repository explorer
- branches
- files
- commits
- diffs
- Forge editor integration

### Collaboration

- pull requests
- reviews
- issues

### Actions

- workflow status
- run status
- basic logs

This already gives Forge a powerful workflow:

```text
Login
 ↓
Choose Repository
 ↓
Open Workspace
 ↓
Browse Code
 ↓
Edit
 ↓
Commit
 ↓
Push
 ↓
Create PR
 ↓
Review
 ↓
CI
 ↓
Merge
```

That is a complete and compelling first version.

---

# 7. What Should Not Be Built Too Early

Several things should deliberately wait.

### Don't start with a giant dashboard

A dashboard without deep underlying functionality risks becoming decorative.

### Don't clone GitHub's UI

The value is Forge integration, not pixel-level replication.

### Don't build every GitHub endpoint immediately

Implement capabilities based on real workflows.

### Don't let every GPUI component access GitHub independently

Use the central Helm data layer.

### Don't rely entirely on polling

Design the event model early, even if full webhook support comes later.

### Don't make AI the foundation

AI should become powerful because Helm has excellent structured GitHub data—not because AI is being used to compensate for missing product architecture.

---

# 8. The End State

The final vision is not:

> **"A GitHub client built in GPUI."**

It is:

> **"A GitHub-native development and operations layer inside a developer operating system."**

The developer experience becomes:

```text
                         MENTENAZ FORGE
                              │
                              ▼
                           HELM
                              │
             ┌────────────────┴────────────────┐
             │                                 │
       HELM CONTROL                       WORKSPACE
             │                                 │
       Manage GitHub                     Develop
             │                                 │
       Organizations                    Repository
       Repositories                     Code
       Teams                            Branches
       Permissions                      Commits
       Security                         PRs
       Actions                          Issues
       Releases                         Actions
             │                                 │
             └────────────────┬────────────────┘
                              │
                              ▼
                       FORGE EDITOR
                              │
                              ▼
                         Local Git
                              │
                              ▼
                           GitHub
                              │
                    ┌─────────┴─────────┐
                    ▼                   ▼
                 Actions               PR
                    │                   │
                    └─────────┬─────────┘
                              ▼
                           HELM
```

The distinction remains simple:

**Helm Control:**

> _Operate GitHub._

**Helm Workspace:**

> _Work with GitHub._

**Mentenaz Forge:**

> _The environment where the entire development lifecycle comes together._

That separation gives Helm a clear product identity today while leaving room for it to grow into the larger developer-OS vision without turning the initial implementation into an unmanageable "build all of GitHub" project.
