# Architecture

This document describes the internal architecture of `un` — how the codebase is organized, how data flows through the system, and the key design decisions behind each component.

## Crate Structure

`un` is organized as a Cargo workspace with focused crates. This mirrors the approach of uv and cargo, keeping compilation units small and responsibilities clear.

```
unified/
├── Cargo.toml              # Workspace manifest
├── crates/
│   ├── un-cli/             # CLI entry point, argument parsing, command dispatch
│   │   └── src/
│   │       ├── main.rs     # Entry point, sets up logging/runtime
│   │       └── commands/   # One module per command (sync.rs, status.rs, ...)
│   ├── un-core/            # Configuration, lock file, workspace logic
│   │   └── src/
│   │       ├── config.rs   # unified.toml schema (serde)
│   │       ├── lock.rs     # unified.lock schema (serde)
│   │       ├── workspace.rs # Workspace discovery and state
│   │       └── resolver.rs  # Resolve config → operations
│   ├── un-git/             # Git operations (fetch, checkout, worktree)
│   │   └── src/
│   │       ├── remote.rs   # GitRemote: URL + reference handling
│   │       ├── database.rs # GitDatabase: bare repo management
│   │       ├── checkout.rs # GitCheckout: worktree/copy to workspace
│   │       ├── reference.rs # GitReference: branch/tag/rev/default
│   │       └── fetch.rs    # Fetch dispatch (gix vs CLI)
│   ├── un-download/        # HTTP download engine + provider abstraction
│   │   └── src/
│   │       ├── client.rs   # HTTP client (reqwest), resume, progress
│   │       ├── provider.rs # Provider trait
│   │       └── providers/
│   │           ├── github.rs      # GitHub Releases API (incl. GitHub Enterprise)
│   │           ├── gitlab.rs      # GitLab Releases API (incl. self-hosted)
│   │           ├── gitea.rs       # Gitea/Forgejo Releases API
│   │           ├── artifactory.rs # Artifactory API (incl. company instances)
│   │           └── http.rs        # Generic URL download
│   └── un-cache/           # Cache directory layout and management
│       └── src/
│           ├── layout.rs   # Path computation for cache entries
│           └── gc.rs       # Garbage collection of stale entries
├── src/
│   └── bin/
│       └── un.rs           # Thin wrapper → un-cli::main()
└── doc/
    └── architecture.md     # This file
```

### Why this split?

| Crate | Responsibility | Key dependencies |
|-------|---------------|-----------------|
| `un-cli` | User-facing CLI, output formatting, command orchestration | `clap`, `indicatif`, `console` |
| `un-core` | Config parsing, lock file, workspace state, resolution | `serde`, `toml`, `semver`, `glob` |
| `un-git` | All git operations, abstracted from CLI details | `gix`, `un-cache` |
| `un-download` | HTTP downloads, provider APIs, checksum verification | `reqwest`, `sha2`, `urlencoding`, `un-cache` |
| `un-cache` | Cache directory paths and lifecycle | `home`, `dirs` |

The crate boundary means `un-git` and `un-download` never depend on each other — they both depend on `un-cache` for path resolution, and `un-core` orchestrates them.

## Data Flow

### `un sync` — The Core Command

This is the most important code path. Everything else is a subset or variation.

```
unified.toml ──→ Config ──→ Resolver ──→ Operations ──→ Lock File
                    │            │            │              │
                    │            │            ▼              │
                    │            │     ┌──────────────┐     │
                    │            │     │  Git Fetch    │     │
                    │            │     │  Git Checkout │     │
                    │            │     │  HTTP Download│     │
                    │            │     └──────────────┘     │
                    │            │            │              │
                    │            │            ▼              │
                    ▼            ▼       Workspace           ▼
              Parse TOML    Compare     populated       Write TOML
                           with lock                   unified.lock
```

**Step by step:**

1. **Read config** — Parse `unified.toml` into typed `Config` struct. Validate fields (mutually exclusive branch/tag/rev, valid URLs, valid semver requirements).

2. **Read lock file** (if exists) — Parse `unified.lock` into `LockFile` struct. Each locked entry contains the exact revision SHA or artifact checksum.

3. **Resolve** — For each repo/artifact in config:
   - Apply active collection filter (if any) — keep only items named in the collection
   - If lock file has a matching entry and config hasn't changed → use locked revision (fast path, no network)
   - If `--locked` flag → fail if config changed since lock was written
   - If `--frozen` flag → fail if entry not in lock (no network allowed)
   - Otherwise → resolve to a concrete revision/version (may require network)

4. **Execute operations** — Run git fetches and HTTP downloads in parallel (up to `settings.parallel` concurrency):
   - **Git repos**: Fetch into `~/.unified/git/db/{name}-{hash}/`, then create checkout
   - **Artifacts**: Download to `~/.unified/artifacts/{name}-{hash}/{version}/`
   - **Tools**: Download to `~/.unified/tools/{name}/{version}/`
   - **Apps**: Download to `~/.unified/apps/{name}/{version}/`

5. **Populate workspace** — Create worktrees or copy files into workspace paths specified in config.

6. **Write lock file** — Serialize all resolved revisions and checksums to `unified.lock`.

7. **Update workspace integration files** — Keep `.gitignore` and `.vscode/settings.json` in sync with managed paths (see Workspace Integration below).

### Sync modes

| Mode | Network | Reads lock | Writes lock | Fails if |
|------|---------|-----------|------------|----------|
| `un sync` | Yes | Yes (fast path) | Yes | Never (unless network error) |
| `un sync --collection X` | Yes | Yes (fast path) | Yes (collection items only) | Collection name not found |
| `un sync --shallow` | Yes | Yes (fast path) | Yes | Never (shallow clone all repos, sparse-checkout if `include` set) |
| `un sync --locked` | Yes | Yes (required) | No | Config changed since lock |
| `un sync --frozen` | No | Yes (required) | No | Entry missing from lock/cache |

## Git Architecture

The git subsystem follows cargo's proven three-tier model, adapted for workspace checkout:

```
                    Remote Repository
                          │
                          │ fetch (gix or git CLI)
                          ▼
                ┌─────────────────────┐
                │   GitDatabase       │    ~/.unified/git/db/{name}-{hash}/
                │   (bare repo)       │    Shared across all workspaces.
                │                     │    Contains all objects and refs.
                └─────────┬───────────┘
                          │
              ┌───────────┴───────────┐
              │                       │
              ▼                       ▼
    ┌──────────────────┐    ┌──────────────────┐
    │  GitCheckout     │    │  GitCheckout     │    ~/.unified/git/checkouts/
    │  (worktree src)  │    │  (another rev)   │    {name}-{hash}/{short_rev}/
    └────────┬─────────┘    └──────────────────┘
             │
             │ worktree or copy
             ▼
    ┌──────────────────┐
    │  Workspace Path  │    ./components/firmware/
    │  (user's files)  │    What the developer sees and edits.
    └──────────────────┘
```

### GitRemote

Represents a remote repository URL. Responsible for:
- URL normalization (strip trailing slashes, convert SCP-style to SSH URLs)
- Constructing fetch refspecs from a `GitReference`
- Initiating the fetch operation

### GitDatabase

A bare git repository in the cache (`~/.unified/git/db/`). Responsible for:
- Storing all git objects (shared across revisions)
- Resolving references to commit OIDs
- Serving as the source for checkouts

Multiple workspaces pointing to the same repo URL share one `GitDatabase`. Only incremental fetches are needed after the initial clone.

### GitCheckout

A checked-out working copy at a specific revision. The workspace path is populated via one of these modes, determined by the combination of `checkout`, `include`/`exclude`, and `shallow` settings:

#### Worktree mode (default)

`git worktree add` creates a linked worktree at the workspace path. The `.git` file in the workspace points back to the database. Developers can create branches, commit, and push directly from the workspace path.

#### Sparse worktree mode (`include`/`exclude` with worktree)

When `include`/`exclude` patterns are set and `checkout` is `"worktree"` (the default), `un` creates a worktree with git sparse-checkout configured. The workspace is still a proper git repo — you can commit and push — but only matching paths are visible.

Under the hood this uses git's partial clone + sparse-checkout:
1. Clone the database with `--filter=blob:none` (blobless — only tree/commit objects fetched)
2. `git worktree add` with sparse-checkout enabled
3. `git sparse-checkout set` with the `include` patterns
4. Git fetches only the blobs for the matching paths on demand

This means **non-matching blobs are never downloaded**, saving both network and disk.

```
Remote ──filter=blob:none──→ GitDatabase (blobless) ──worktree + sparse-checkout──→ Workspace
                              trees + commits only         only matching blobs fetched
```

If `exclude` is set, it's applied as a client-side filter after sparse-checkout (removing additional files from the working tree).

#### Copy mode (`checkout = "copy"`)

Files are hardlinked (or copied on cross-device) from a full checkout to the workspace path. The workspace directory is not a git repo — it's read-only content. Used for dependencies that don't need local modification.

#### Filtered copy mode (`checkout = "copy"` + `include`/`exclude`)

Like copy mode, but only paths matching the glob patterns are copied to the workspace. The full repo is still fetched in the database — filtering happens at the copy step by walking the tree and matching globs. Use this when you want a plain directory (not a git repo) with a subset of files.

#### How modes compose

| `checkout` | `include`/`exclude` | What happens |
|---|---|---|
| `"worktree"` (default) | Not set | Full worktree — all files, proper git repo |
| `"worktree"` (default) | Set | **Sparse worktree** — git sparse-checkout, only matching files visible, blobless clone |
| `"copy"` | Not set | Full copy — all files, plain directory |
| `"copy"` | Set | **Filtered copy** — plain directory with only matching files |

### Path Filtering Patterns

`include` and `exclude` use standard gitignore-style glob patterns (via the `glob` crate):

```toml
[repos.monorepo]
url = "https://github.com/org/platform.git"
tag = "v4.0.0"
path = "libs/platform-api"
include = ["packages/api/**"]
exclude = ["**/test/**", "**/*.test.ts"]
```

**Glob syntax:**
- `*` matches anything except `/`
- `**` matches zero or more directories
- `?` matches a single character
- `[abc]` matches character classes
- Patterns are matched against the repo-relative path (e.g., `src/lib.rs`)

**Evaluation order:**
1. If `include` is set, keep only files matching at least one include pattern. If not set, keep all files.
2. If `exclude` is set, remove files matching any exclude pattern.

**Workspace output shows the file count** so misconfigured globs are easy to spot:
```
  Checkout  monorepo → libs/platform-api (sparse, 48 files, v4.0.0 @ f1a2b3c)
```

### Shallow Mode (`shallow = true` / `--shallow`)

Orthogonal to checkout mode and include/exclude. Controls **clone depth**, not path filtering.

When `shallow = true` is set (per-repo, in `[settings]`, via `--shallow` CLI flag, or `UN_SHALLOW` env var), `un` clones with `--depth 1` — only the target revision, no history.

```
Remote ──depth=1──→ GitDatabase (shallow)
                     single revision, no history
```

**Shallow composes with every other mode:**

| | All files | With `include` (sparse) |
|---|---|---|
| **Full history** (default) | Full clone, full worktree | Blobless clone, sparse worktree |
| **Shallow** (`shallow = true`) | Depth-1 clone, full worktree | Depth-1 + blobless, sparse worktree |

**Trade-offs:**

| | Full history | Shallow |
|---|---|---|
| Network transfer | Full repo | One revision only |
| `un update` | Incremental fetch | Must re-fetch |
| `un branch`/`commit`/`push` | Full workflow | Works, but no history for rebase/log |
| Best for | Developer machines | CI pipelines |

**Config examples:**
```toml
# CI: shallow clone, all files
[repos.firmware]
url = "https://github.com/org/firmware.git"
branch = "main"
path = "components/firmware"
shallow = true

# CI: shallow clone + sparse (minimal possible transfer)
[repos.huge-monorepo]
url = "https://github.com/org/platform.git"
branch = "main"
path = "deps/platform"
include = ["sdk/**", "protos/**"]
shallow = true

# Developer: sparse worktree, full history (can change include later without re-fetch)
[repos.huge-monorepo]
url = "https://github.com/org/platform.git"
branch = "main"
path = "deps/platform"
include = ["sdk/**", "protos/**"]
```

### GitReference

Enum representing what the user specified:

```
GitReference
├── Branch(String)      # Track a branch head (e.g., "main")
├── Tag(String)         # Pin to a tag (e.g., "v1.0.0")
├── Rev(String)         # Pin to a commit (e.g., "a1b2c3d4")
└── DefaultBranch       # HEAD (no branch/tag/rev specified)
```

**Resolution rules:**
- `Branch` → fetch branch, resolve to HEAD commit of that branch. `un update` can advance this.
- `Tag` → fetch tag, resolve to tagged commit. `un update` cannot advance this (tags are immutable).
- `Rev` → exact commit hash. `un update` cannot advance this.
- `DefaultBranch` → fetch HEAD, resolve to default branch's HEAD commit.

### Fetch Dispatch

Two backends, selectable via `settings.git-fetch-with-cli`:

1. **gitoxide (gix)** — Default. Pure Rust, no system dependencies. Supports shallow clones. Uses `gix::remote::connect()` with refspec construction per reference type.

2. **Git CLI fallback** — Shells out to `git fetch`. Required for corporate environments where git is configured with custom proxy settings, credential helpers, or SSH configurations in `~/.gitconfig` that gix can't replicate. Mirrors cargo's `net.git-fetch-with-cli` option.

**Selection logic:**
```
if settings.git-fetch-with-cli || env UN_GIT_FETCH_WITH_CLI {
    use git CLI
} else {
    try gix
    if gix fails with auth/transport error {
        suggest: "Try setting git-fetch-with-cli = true in [settings]"
    }
}
```

## Download Architecture

### Provider Trait

```rust
trait ArtifactProvider {
    /// List available versions for this artifact.
    async fn list_versions(&self, spec: &ArtifactSpec) -> Result<Vec<Version>>;

    /// Resolve a version requirement to a specific downloadable asset.
    async fn resolve(&self, spec: &ArtifactSpec, version_req: &VersionReq)
        -> Result<ResolvedArtifact>;

    /// Download the artifact to a local path.
    async fn download(&self, resolved: &ResolvedArtifact, dest: &Path)
        -> Result<DownloadResult>;
}
```

### Providers

**GitHub Releases** (`github = "owner/repo"`)
- Queries `GET {api_url}/repos/{owner}/{repo}/releases` with pagination
- Parses `tag_name` as semver (strips leading `v`)
- Selects asset matching platform keywords (configurable via `platform` map)
- Auth via `Authorization: token {t}` header
- Default API URL: `https://api.github.com`; overridable via `[providers]` for GitHub Enterprise

**GitLab Releases** (`gitlab = "group/project"`)
- Queries `GET {api_url}/api/v4/projects/{id}/releases` with pagination
- Project identifier can be a numeric ID or a `group/project` path (URL-encoded)
- Parses `tag_name` as semver (strips leading `v`)
- Selects asset from `assets.links[]` matching platform keywords
- Auth via `PRIVATE-TOKEN` header
- Default API URL: `https://gitlab.com`; overridable via `[providers]` for self-hosted GitLab

**Gitea/Forgejo Releases** (`gitea = "owner/repo"`)
- Queries `GET {api_url}/api/v1/repos/{owner}/{repo}/releases` with pagination
- Parses `tag_name` as semver (strips leading `v`)
- Selects asset from `assets[]` using `browser_download_url` field
- Auth via `Authorization: token {t}` header
- Default API URL: `https://gitea.com`; overridable via `[providers]` for self-hosted instances

**Artifactory** (`artifactory = "path/to/artifact"`)
- Queries storage API: `GET {host}/artifactory/api/storage/{path}?list&deep&listFolders=1`
- Extracts version directories from path structure
- Auth via `Authorization: Bearer {t}` header
- Host configured via `[providers]` or `ARTIFACTORY_URL` env var

**Generic HTTP** (`url = "https://..."`)
- Direct URL download, no version resolution
- Requires `sha256` for integrity verification
- Supports resume via `Range` header

### Provider Configuration

Each artifact, tool, or app specifies a source via one of: `github`, `gitlab`, `gitea`, `artifactory`, or `url`. An optional `provider` field references a named entry in `[providers]` to override the default API URL and auth token:

```toml
[providers.company-gh]
provider_type = "github"                        # github | gitlab | gitea | artifactory
api_url = "https://github.example.com/api/v3"  # API base URL
token_env = "GHE_TOKEN"                         # Env var holding auth token

[artifacts.internal-lib]
github = "org/internal-lib"
version = ">=1.0.0"
path = "vendor/internal-lib"
provider = "company-gh"                         # Uses the GHE instance above
```

Without a `provider` field, built-in defaults are used:

| Source type | Default API URL | Default token env |
|---|---|---|
| `github` | `https://api.github.com` | `GITHUB_TOKEN` |
| `gitlab` | `https://gitlab.com` | `GITLAB_TOKEN` |
| `gitea` | `https://gitea.com` | `GITEA_TOKEN` |
| `artifactory` | `ARTIFACTORY_URL` env | `ARTIFACTORY_TOKEN` |

Provider resolution (`Config::resolve_provider()`) merges the named provider config with the source type to produce a `ResolvedProvider { provider_type, api_url, token }` that is passed to the appropriate provider implementation.

### Download Engine

All providers use a shared download engine:
- **HTTP client**: `reqwest` with configurable timeout, proxy support
- **Resume**: Partial downloads via HTTP `Range` header (checks for existing `*.partial` files in `~/.unified/tmp/`)
- **Integrity**: SHA-256 hash computed during streaming download, verified against expected checksum
- **Progress**: `indicatif` progress bars with download speed and ETA
- **Parallelism**: `tokio` task pool, up to `settings.parallel` concurrent downloads

### Platform Detection

For GitHub Releases and Artifactory, asset selection uses platform-specific keywords:

| Platform | Keywords (tried in order) |
|----------|---------------------------|
| Linux x86_64 | `linux-x86_64`, `linux-amd64`, `linux64`, `linux` |
| Linux aarch64 | `linux-aarch64`, `linux-arm64` |
| macOS x86_64 | `darwin-x86_64`, `macos-x86_64`, `macos64` |
| macOS aarch64 | `darwin-arm64`, `darwin-aarch64`, `macos-arm64` |
| Windows x86_64 | `windows-x86_64`, `win64`, `windows-amd64`, `windows` |

Users can override this per-artifact with the `platform` field in `unified.toml`:
```toml
[artifacts.sdk]
github = "org/sdk"
version = "1.*"
platform = { linux-x86_64 = "ubuntu-22.04-x64", macos-aarch64 = "macos-universal" }
```

## Cache Architecture

### Directory Layout

```
$UNIFIED_HOME/                       # Default: ~/.unified
├── git/
│   ├── db/
│   │   └── {name}-{url_hash}/       # Bare git databases
│   └── checkouts/
│       └── {name}-{url_hash}/
│           └── {short_rev}/          # Checked-out trees
├── artifacts/
│   └── {name}-{source_hash}/
│       └── {version}/                # Downloaded and extracted artifacts
├── tools/
│   └── {name}/
│       └── {version}/
│           └── {binary}              # Tool executables
├── apps/
│   └── {name}/
│       └── {version}/                # Downloaded applications
├── bin/                              # Global tool symlinks (un tool install)
└── tmp/                              # In-progress downloads
    └── {uuid}.partial                # Resumable partial downloads
```

### Cache Key Computation

Cache keys prevent collisions between artifacts/repos with the same name but different sources:

```rust
fn cache_key(name: &str, url: &str) -> String {
    let hash = sha256(url);
    let short_hash = &hash[..16];  // First 16 hex chars
    format!("{name}-{short_hash}")
}
```

This mirrors cargo's `ident()` function: `gimli-a0d193bd15a5ed96`.

### Atomic Operations

Cache writes are atomic to prevent corruption from interrupted operations:

1. **Downloads**: Write to `~/.unified/tmp/{uuid}.partial`, then rename to final path
2. **Git checkouts**: Clone to temp dir, create `.unified-ok` marker, then rename
3. **Lock file**: Write to `unified.lock.tmp`, then rename to `unified.lock`

If `un sync` is interrupted, the next run sees incomplete entries (missing `.unified-ok` marker or truncated partial downloads) and retries them.

### Garbage Collection

`un clean` removes:
- Partial downloads in `tmp/`
- Checkout revisions not referenced by any workspace's `unified.lock`
- Old artifact versions superseded by newer ones

The `db/` directory is never cleaned automatically — bare databases are cheap (shared objects) and expensive to re-fetch.

## Workspace State

### Discovery

`un` finds the workspace by searching upward from the current directory for `unified.toml`, similar to how cargo searches for `Cargo.toml`:

```
current_dir → check for unified.toml
    ↑
parent_dir → check for unified.toml
    ↑
root → give up, error: "not a unified workspace"
```

### Members (multi-workspace)

The `[workspace]` section can specify member directories:

```toml
[workspace]
name = "platform"
members = ["services/*", "libraries/*"]
```

Each member directory can contain its own `unified.toml` that inherits settings from the root but adds repos/artifacts specific to that component.

### Workspace ↔ Cache Mapping

The workspace tracks which cache entries are checked out where via internal metadata:

```
.unified/                            # Workspace metadata (in workspace root)
├── state.toml                       # Maps workspace paths → cache entries
├── user.toml                        # User-local preferences (default-collection, etc.)
└── worktrees/                       # Worktree metadata (git internals)
```

This `.unified/` directory in the workspace root (not the cache) stores the mapping between workspace paths and cache entries, plus user-local preferences. It is auto-added to `.gitignore`.

### Workspace Integration

`un sync` automatically manages integration files so that the host repo's git and your editor don't scan or track the managed checkout paths.

#### `.gitignore`

After every sync, `un` updates a managed block in `.gitignore`:

```gitignore
# --- managed by un (do not edit) ---
.unified/
components/firmware/
components/protocol/
libs/shared/
vendor/design-tokens/
libs/platform-api/
deps/huge-repo/
# --- end managed by un ---
```

Only the block between the sentinel comments is touched — user entries above or below are preserved. If the workspace root is not a git repo, this step is skipped.

#### `.vscode/settings.json`

To prevent VS Code from scanning every worktree as a separate git repo (flooding the Source Control panel), `un sync` maintains a managed section in `.vscode/settings.json`:

```json
{
  "git.ignoredRepositories": [
    "components/firmware",
    "components/protocol",
    "vendor/design-tokens",
    "libs/platform-api",
    "deps/huge-repo"
  ],
  "files.exclude": {},
  "search.exclude": {}
}
```

`un` only manages the `git.ignoredRepositories` key — existing user settings are preserved. Copy-mode repos (not git repos) are not added to this list.

Both files are updated on every `un sync` and `un remove`. `un init` creates the initial `.gitignore` block.

#### Disabling

To opt out of automatic integration file management:

```toml
[settings]
manage-gitignore = false      # Don't touch .gitignore
manage-vscode = false          # Don't touch .vscode/settings.json
```

## Tool & App Execution

Tools (`[tools]`) and apps (`[apps]`) share the same download infrastructure as artifacts (Provider trait, platform detection, checksum verification). The difference is lifecycle:

- **Artifacts** are placed at a workspace `path` and treated as static data.
- **Tools** are executables cached in `~/.unified/tools/{name}/{version}/` and run via `un run <tool>`.
- **Apps** are larger applications cached in `~/.unified/apps/{name}/{version}/` and launched via `un app <name>`.

### Tool Execution (`un run`)

```
un run protoc -- --cpp_out=gen/ protos/*.proto

1. Resolve version    →  Find latest matching version in cache or fetch metadata
2. Download (if new)  →  Provider.download() → ~/.unified/tools/protoc/v25.1/protoc
3. Prepare env        →  Merge tool's `env` field into environment
4. Prepend args       →  Tool's `args` + user's args → final argv
5. Exec               →  Replace process (unix: execvp, windows: CreateProcess)
```

Tools support workspace-specific configuration via `env` and `args`:

```toml
[tools.clang-format]
artifactory = "tools/llvm/clang-format"
version = "17.*"
env = { CLANG_FORMAT_STYLE = "file" }      # Set when running
args = ["--style=file"]                     # Prepended to user args
```

### Global Install (`un tool install`)

`un tool install <name>` creates a symlink in `~/.unified/bin/` pointing to the cached tool binary. Users add `~/.unified/bin/` to their `PATH` for direct invocation without `un run`.

### App Launch (`un app`)

Same download mechanism as tools. `un app <name>` downloads (if needed) and launches the application. Apps are expected to be self-contained executables or directories — `un` does not manage installation beyond downloading and extracting.

## Task Runner

`un` provides a minimal task runner via `[tasks]` — named shell commands with optional dependencies. This is intentionally simple; for complex build workflows, use a dedicated task runner like [just](https://github.com/casey/just) and call it from tasks:

```toml
[tasks.format]
cmd = "un run clang-format -- src/**/*.cpp"
description = "Format C++ files"

[tasks.build]
cmd = "just build"                  # Delegate to Justfile
depends = ["format"]
```

### Execution (`un task <name>`)

1. Topological sort on `depends` — detect cycles, error if found
2. Execute each dependency in order (not parallel — keep it simple)
3. Execute the task's `cmd` via the system shell (`sh -c` / `cmd /c`)
4. Exit code propagation — if any step fails, abort

`un task` (no name) lists all tasks with their descriptions.

## Setup Hooks

`[setup]` defines a list of commands run by `un setup`. These handle one-time workspace environment configuration that doesn't belong in sync — IDE extension installation, environment verification, etc.

```toml
[setup]
run = [
    "code --install-extension rust-lang.rust-analyzer",
    "code --install-extension tamasfe.even-better-toml",
    "un run protoc --version",
]
```

`un setup` runs each command sequentially via the system shell. It's idempotent — safe to run repeatedly. It is **not** invoked by `un sync`; developers run it explicitly after initial sync or when setup changes.

This is deliberately generic — no IDE-specific logic in `un` itself. Any editor, tool, or environment setup works as long as it's a shell command.

## Launcher

The launcher generates a platform-specific script (`launch.sh` / `launch.bat`) that presents an interactive numbered menu referencing apps, tasks, and arbitrary commands.

```toml
[launcher]
generate = true

[[launcher.entries]]
name = "Open IDE"
app = "clion"                       # Downloads + launches [apps.clion]
icon = "🔧"

[[launcher.entries]]
name = "Format Code"
task = "format"                     # Runs [tasks.format] with depends
icon = "✨"

[[launcher.entries]]
name = "Open Editor"
cmd = "code ."                      # Arbitrary command
icon = "📝"
```

Generation happens during `un sync`. Each entry resolves to a shell command:
- `app = "..."` → `un app <name>`
- `task = "..."` → `un task <name>`
- `cmd = "..."` → literal command

## Collections

Collections let developers sync a named subset of the workspace. This solves two real problems:

1. **Permissions** — Not every developer has access to every private repo. Without collections, `un sync` fails on the first inaccessible repo. With collections, a developer selects only the repos they can reach.
2. **Speed / disk** — Large workspaces may define dozens of repos. A frontend developer doesn't need the firmware repos, and a CI pipeline may only need two.

### Config Schema

Collections are defined in `unified.toml` alongside the items they reference:

```toml
[collections.firmware-team]
repos = ["firmware", "protocol", "shared-libs"]     # Names from [repos.*]
artifacts = ["test-vectors", "firmware-binary"]       # Names from [artifacts.*]
tools = ["protoc"]                                     # Names from [tools.*]

[collections.frontend]
repos = ["design-system", "monorepo"]
artifacts = ["internal-sdk"]
```

All three arrays are optional — omitting `tools` means no tools are filtered (they're all available). A name can appear in multiple collections.

### Validation

During config parsing, every name in a collection is checked against the corresponding `[repos.*]`, `[artifacts.*]`, or `[tools.*]` section. An unknown name is a hard error:

```
error: collection "firmware-team" references unknown repo "firmwrae"
  → did you mean "firmware"?
```

### Resolution Filtering

The active collection is determined by this precedence (highest first):

```
--collection <name>  CLI flag (per-invocation)
       ↓
UN_COLLECTION        env var (per-shell / CI)
       ↓
user.toml            default-collection (per-machine)
       ↓
(none)               sync everything
```

The resolver applies the active collection as a filter **before** executing operations:

```
unified.toml ──→ Config ──→ Collection Filter ──→ Resolver ──→ Operations
                                  │
                                  │ keep only repos/artifacts/tools
                                  │ named in the active collection
                                  ▼
                           Filtered Config
```

If no collection is active (no flag, no env var, no `user.toml` default), all items are included — the existing behavior.

`--all` explicitly bypasses the collection filter, even if a default is set in `user.toml`.

### Effect on Other Commands

When a collection is active, these commands operate only on the collection's items:

| Command | Behavior with active collection |
|---------|-------------------------------|
| `un sync` | Fetch/checkout only collection repos, download only collection artifacts/tools |
| `un status` | Show status only for collection repos |
| `un diff` | Show diffs only for collection repos |
| `un exec` | Run command only in collection repos |
| `un update` | Update only collection repos/artifacts |

All accept `--all` to override. Commands that operate on a single named item (`un run <tool>`, `un app <name>`, `un task <name>`) are not filtered — they work regardless of the active collection.

### Workspace Integration with Collections

`.gitignore` and `.vscode/settings.json` are updated to reflect only the paths that were actually checked out. When a collection is active, only that collection's repo paths appear in the managed blocks. Switching collections and re-syncing updates these files accordingly.

## User-Local Config

Per-machine preferences live in `.unified/user.toml` inside the workspace root. This file is git-ignored (the entire `.unified/` directory is listed in the managed `.gitignore` block) and is never committed.

```toml
# .unified/user.toml
default-collection = "firmware-team"
```

### Supported Fields

| Field | Type | Description |
|-------|------|-------------|
| `default-collection` | `String` | Active collection name. Omit or remove to sync everything. |

Future user-local preferences (e.g., override `parallel`, local tool paths) can be added here without polluting the shared `unified.toml`.

### File Lifecycle

- **Created by**: `un collection use <name>` (creates `.unified/` dir if needed)
- **Cleared by**: `un collection use --clear` (removes the `default-collection` key)
- **Read by**: Every command that resolves the active collection
- **Never written by**: `un sync`, `un init` (they don't touch user preferences)

The generated script is self-contained — it doesn't require `un` to be installed (entries using `app` or `task` do invoke `un`, but `cmd` entries are plain shell).

## Concurrency Model

`un` uses `tokio` for async I/O with a bounded concurrency model:

```
┌──────────────────────────────────────┐
│         Command Orchestrator         │   un-cli (main thread)
│  (reads config, drives operations)   │
└──────────────┬───────────────────────┘
               │
    ┌──────────┴──────────┐
    ▼                     ▼
┌────────────┐    ┌──────────────┐
│ Git Pool   │    │ Download Pool│      tokio::JoinSet with semaphore
│ (N tasks)  │    │ (M tasks)   │      N + M ≤ settings.parallel
└────────────┘    └──────────────┘
```

- Git fetches and downloads run concurrently
- A shared semaphore limits total parallelism
- Progress bars are updated from multiple tasks via a shared `MultiProgress`
- Lock file writes happen only after all operations complete

### File Locking

When multiple `un` processes might run simultaneously (e.g., CI with concurrent jobs sharing cache):
- Cache database access uses `flock()` advisory locking
- Each operation acquires a shared lock for reads, exclusive lock for writes
- Lock files are per-database-entry: `~/.unified/git/db/{key}/.lock`

## Error Handling

### Error Categories

| Category | Example | Recovery |
|----------|---------|----------|
| Network | DNS failure, timeout | Retry with backoff (3 attempts) |
| Auth | 401/403 from GitHub/Artifactory | Suggest setting token env var |
| Git | Ref not found, corrupt repo | Report, suggest `un clean` + retry |
| Config | Invalid TOML, invalid semver | Report exact line/field, exit 1 |
| Workspace | Dirty worktree on update | Report, refuse to overwrite |
| Cache | Disk full, permission denied | Report, suggest `un clean` |

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error (config, network, git) |
| 2 | Lock file mismatch (`--locked` mode) |
| 3 | Dirty workspace (operation refused) |
| 126 | Tool execution failed (`un run`) |
| 127 | Tool not found (`un run`) |

## Logging & Output

### User-facing output

Structured, colorized output follows uv/cargo conventions:

```
  Fetching  repo-name (url)          # Green prefix, action in progress
  Checkout  repo-name → path (mode)  # Green prefix, action complete
  Download  artifact-name v1.0.0     # Green prefix, downloading
  Cached    artifact-name v1.0.0     # Blue prefix, using cache
  Warning   message                  # Yellow prefix
  Error     message                  # Red prefix
```

### Diagnostic logging

Controlled via `UN_LOG` env var. Uses `tracing` crate for structured logs:
- `error` — Always shown
- `warn` — Shown by default
- `info` — Verbose mode (`-v`)
- `debug` — Debug mode (`-vv`)
- `trace` — Trace mode (`-vvv`, shows git commands)

## Security Model

### Supply Chain

- **Git repos**: Locked to exact commit SHA in `unified.lock`. Even if a branch is force-pushed, `--locked` will fetch the original commit (or fail).
- **Artifacts**: Locked to SHA-256 checksum. Tampered downloads are rejected.
- **Tools**: Same checksum verification as artifacts.

### Authentication

| Source | Auth method | Storage |
|--------|------------|---------|
| GitHub | `GITHUB_TOKEN` env var, or `gh auth token` | Not stored by un |
| Artifactory | `ARTIFACTORY_TOKEN` env var | Not stored by un |
| Generic HTTP | URL-embedded credentials (basic auth) | In `unified.toml` (user's responsibility) |
| Git SSH | SSH agent, `~/.ssh/config` | System SSH |
| Git HTTPS | Git credential helpers | System git config |

`un` never stores credentials. It delegates to environment variables and system-level credential management.

### Workspace Isolation

- `un sync` never modifies files outside the workspace root and `~/.unified/`
- Worktrees are linked via `.git` files, not symlinks to arbitrary paths
- The `path` field in config is validated to be within the workspace root (no `../` escape)

## Testing Strategy

### Unit Tests

Each crate has unit tests for its core logic:
- `un-core`: Config parsing, lock file serialization, version resolution
- `un-git`: Reference parsing, cache key computation, refspec construction
- `un-download`: Provider URL construction, platform detection, checksum verification
- `un-cache`: Path layout computation

### Integration Tests

End-to-end tests using temporary directories and local git repos:
- `un init` creates valid config
- `un sync` with a local file:// git repo
- `un sync --locked` with stale lock file
- `un status` reports dirty/clean correctly
- `un update` advances branch-tracking repos

### CI Matrix

- Linux x86_64, macOS aarch64, Windows x86_64
- Latest stable Rust
- With and without system git (to test gix vs CLI fallback)
