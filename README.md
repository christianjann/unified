# un — Unified Repo & Artifact Manager

> *Like cargo for your entire workspace.* Manage multi-repo projects, download artifacts, and run tools — all from a single `unified.toml`.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)]()

## Why un?

Modern software projects span multiple repositories, binary artifacts, and external tools. Existing solutions each solve a piece of the puzzle, but none solve it all:

| Problem | Existing tools | What goes wrong |
|---------|---------------|-----------------|
| Multi-repo checkout | git submodules | Constantly break, confusing UX, CI nightmare |
| Multi-repo checkout | git subtrees | Wrong abstraction, pollutes history |
| Multi-repo orchestration | Google repo, tsrc | No lock files, no artifact support, Python-only |
| Large file management | git LFS | Opaque, breaks often, requires server setup |
| Binary artifacts | Manual scripts | No caching, no versioning, no reproducibility |
| Tool management | foreman, asdf | No integration with repo workspace |

**un** combines all of these into a single, fast Rust binary with a familiar workflow inspired by cargo and uv:

- **Declarative config** — Define repos, artifacts, and tools in `unified.toml`
- **Deterministic lock file** — `unified.lock` pins exact revisions and checksums for reproducible builds
- **Smart caching** — Bare git databases and artifact cache in `~/.unified/`, shared across workspaces
- **Workspace checkout** — Repos appear at specified paths via git worktrees or file copies
- **Artifact downloads** — From GitHub Releases, GitLab Releases, Gitea/Forgejo, Artifactory, or any HTTP/HTTPS URL
- **Multi-provider** — Built-in support for GitHub, GitLab, Gitea/Forgejo, and Artifactory. Configure company instances (GitHub Enterprise, self-hosted GitLab, etc.) via `[providers]`
- **Tool execution** — `un run <tool>` downloads and runs tools on demand
- **Setup hooks** — `un setup` runs workspace setup commands (IDE extensions, environment config)
- **Selective checkout** — `include`/`exclude` globs per repo — sparse worktree or filtered copy
- **CI-optimized** — `--shallow` for depth-1 clones, composes with sparse for minimal transfer
- **Collections** — Group repos and artifacts into named collections; sync only what you need or have access to
- **Corporate-friendly** — `git-fetch-with-cli` option for proxy/SSH/credential helper configs

## Installation
> **Caution:** This tool is pre-alpha, and many features are not yet implemented or tested. `cargo install` is not available yet.

```bash
# From source (requires Rust 1.85+)
cargo install un-cli

# Or build from this repo
git clone https://github.com/christianjann/unified.git
cd unified
cargo install --path crates/un-cli
```

This gives you the `un` command.

## Quick Start

### 1. Initialize a workspace

```bash
mkdir my-workspace && cd my-workspace
un init
```

This creates a `unified.toml`:

```toml
[workspace]
name = "my-workspace"

[settings]
# git-fetch-with-cli = true  # Uncomment for corporate proxies/SSH
```

### 2. Add repositories

```toml
[repos.firmware]
url = "https://github.com/org/firmware.git"
branch = "main"
path = "components/firmware"

[repos.protocol]
url = "https://github.com/org/protocol.git"
tag = "v2.1.0"
path = "components/protocol"

[repos.shared-libs]
url = "git@github.com:org/shared-libs.git"
rev = "a1b2c3d4"
path = "libs/shared"
checkout = "copy"  # Read-only file copy instead of worktree

[repos.design-system]
url = "https://github.com/org/design-system.git"
branch = "main"
path = "vendor/design-tokens"
include = ["tokens/*.json", "README.md"]  # Only these files appear in workspace

[repos.monorepo]
url = "https://github.com/org/platform.git"
tag = "v4.0.0"
path = "libs/platform-api"
include = ["packages/api/**"]              # Single subdirectory
exclude = ["**/test/**", "**/*.test.ts"]   # Skip test files

[repos.ci-minimal]
url = "https://github.com/org/huge-repo.git"
branch = "main"
path = "deps/huge-repo"
include = ["sdk/**", "protos/**"]
shallow = true                             # Shallow clone (depth 1) + sparse-checkout of include paths
```

### 3. Add artifacts

```toml
[artifacts.test-vectors]
github = "org/test-vectors"
version = ">=1.0.0"
path = "test-data/vectors"

[artifacts.firmware-binary]
url = "https://releases.example.com/firmware/v3.2.1/firmware.bin"
sha256 = "abc123..."
path = "binaries/firmware.bin"
extract = false                     # Keep the raw downloaded file as-is (don't extract)

[artifacts.internal-sdk]
artifactory = "libs-release/sdk/toolchain"
version = "2.0.*"
path = "vendor/sdk"

[artifacts.models]
gitlab = "ml-team/models"              # GitLab Releases (group/project or numeric ID)
version = ">=2.0.0"
path = "vendor/models"
provider = "company-gitlab"             # Use a custom provider instance (see [providers])

[artifacts.assets]
gitea = "org/game-assets"               # Gitea/Forgejo Releases (owner/repo)
version = "1.*"
path = "vendor/assets"
```

### 4. Add tools, tasks, and apps

```toml
# ─── Providers (custom instances of GitHub, GitLab, etc.) ────────

[providers.company-gh]
provider_type = "github"                    # github | gitlab | gitea | artifactory
api_url = "https://github.example.com/api/v3"  # GitHub Enterprise API URL
token_env = "GHE_TOKEN"                     # Env var holding the auth token

[providers.company-gitlab]
provider_type = "gitlab"
api_url = "https://gitlab.example.com"      # Self-hosted GitLab
token_env = "GITLAB_CORP_TOKEN"

[providers.company-gitea]
provider_type = "gitea"
api_url = "https://gitea.example.com"       # Self-hosted Gitea/Forgejo
token_env = "GITEA_CORP_TOKEN"

[providers.company-artifactory]
provider_type = "artifactory"
api_url = "https://artifactory.example.com" # Company Artifactory instance
token_env = "ARTIFACTORY_CORP_TOKEN"

# ─── Tools (downloaded on demand, cached per-version) ─────────────

[tools.protoc]
github = "protocolbuffers/protobuf"
version = ">=25.0"

[tools.buf]
github = "bufbuild/buf"
version = "1.*"

[tools.clang-format]
artifactory = "tools/llvm/clang-format"
version = "17.*"
provider = "company-artifactory"            # Use company Artifactory instance
env = { CLANG_FORMAT_STYLE = "file" }      # Set env vars when running via `un run`
args = ["--style=file"]                     # Default args prepended to `un run` invocations

# ─── Tasks (workspace commands, like npm scripts) ─────────────────

[tasks.format]
cmd = "un run clang-format -- src/**/*.cpp"
description = "Format all C++ source files"

[tasks.gen-protos]
cmd = "un run protoc -- --cpp_out=gen/ protos/*.proto"
description = "Generate C++ from proto files"
depends = ["format"]                        # Run these tasks first

[tasks.check]
cmd = "cargo clippy --workspace"
description = "Run lints"

# For complex task workflows, use a Justfile and call it from tasks:
# [tasks.build]
# cmd = "just build"

# ─── Setup (commands run by `un setup`, e.g. IDE config) ─────────

[setup]
run = [
    "code --install-extension rust-lang.rust-analyzer",
    "code --install-extension tamasfe.even-better-toml",
    "un run protoc --version",              # Verify tools work
]

# ─── Apps (downloadable applications) ────────────────────────────

[apps.clion]
artifactory = "tools/jetbrains/clion"
version = "2025.*"
description = "CLion IDE"
icon = "🔧"

[apps.custom-debugger]
github = "org/debugger-gui"
version = ">=2.0"
description = "Internal Debugger"
icon = "🐛"

# ─── Launcher (generated click-to-run entry point) ───────────────

[launcher]
generate = true                             # un sync generates launch.sh / launch.bat

[[launcher.entries]]
name = "Open in CLion"
app = "clion"                               # References [apps.clion]
icon = "🔧"

[[launcher.entries]]
name = "Open in VS Code"
cmd = "code ."
icon = "📝"

[[launcher.entries]]
name = "Format Code"
task = "format"                             # References [tasks.format]
icon = "✨"
```

### 5. Define collections (optional)

When a workspace has many repos and not every developer needs (or has access to) all of them, group items into named collections:

```toml
# ─── Collections (named subsets of the workspace) ─────────────────

[collections.firmware-team]
repos = ["firmware", "protocol", "shared-libs"]
artifacts = ["test-vectors", "firmware-binary"]
tools = ["protoc"]

[collections.frontend]
repos = ["design-system", "monorepo"]
artifacts = ["internal-sdk"]

[collections.ci-minimal]
repos = ["firmware", "protocol"]
artifacts = ["test-vectors"]
```

Each collection lists names from `[repos.*]`, `[artifacts.*]`, and `[tools.*]`. A repo/artifact can appear in multiple collections.

### 6. Sync the workspace

```bash
un sync
```

This will:
1. Clone/fetch all git repos into `~/.unified/git/db/`
2. Check out the specified revisions into your workspace paths
3. Download artifacts to `~/.unified/artifacts/` (cached as raw archives) and extract them to workspace paths
4. Download tools and apps to `~/.unified/tools/` and `~/.unified/apps/` (cached as raw archives, extracted on demand)
5. Write `unified.lock` with pinned revisions and checksums
6. Update `.gitignore` and `.vscode/settings.json` to exclude managed paths

Then run `un setup` to execute workspace setup commands (install IDE extensions, etc.).

```
$ un sync
  Fetching  firmware (https://github.com/org/firmware.git)
  Fetching  protocol (https://github.com/org/protocol.git)
  Fetching  shared-libs (git@github.com:org/shared-libs.git)
  Checkout  firmware → components/firmware (worktree, main @ a1b2c3d)
  Checkout  protocol → components/protocol (worktree, v2.1.0 @ e5f6a7b)
  Checkout  shared-libs → libs/shared (copy, a1b2c3d4)
  Checkout  design-system → vendor/design-tokens (sparse, 12 files, main @ b3c4d5e)
  Checkout  monorepo → libs/platform-api (sparse, 48 files, v4.0.0 @ f1a2b3c)
  Download  test-vectors v1.2.0 (GitHub: org/test-vectors)
  Download  firmware-binary (https://releases.example.com/...)
  Cached    internal-sdk v2.0.3 (already downloaded)
  Tool      protoc v25.1 (GitHub: protocolbuffers/protobuf)
  Tool      buf v1.28.0 (GitHub: bufbuild/buf)
  Cached    clang-format v17.0.6 (already downloaded)
  App       clion v2025.1 (Artifactory: tools/jetbrains/clion)
  Locked    unified.lock (3 repos, 3 artifacts, 3 tools, 1 app)
  Updated   .gitignore (6 paths)
  Updated   .vscode/settings.json (6 repos excluded from git scanning)
     Done   in 4.2s
```

To sync only a specific collection:

```bash
# Sync only the firmware-team collection
un sync --collection firmware-team

# Set a default collection for this machine (saved in .unified/user.toml, git-ignored)
un collection use firmware-team

# Now `un sync` only syncs the firmware-team collection
un sync

# Sync everything regardless of default collection
un sync --all

# Clear the default collection (back to syncing everything)
un collection use --clear
```

### 7. Check status

```bash
$ un status
  firmware      components/firmware      ✓ clean (main @ a1b2c3d)
  protocol      components/protocol      ✗ modified (v2.1.0 @ e5f6a7b)
  shared-libs   libs/shared              ✓ clean (copy @ a1b2c3d4)
```

## Commands

### Workspace Management

| Command | Description |
|---------|-------------|
| `un init` | Create a new `unified.toml` in the current directory |
| `un sync` | Fetch repos, download artifacts, check out workspace |
| `un sync --shallow` | Shallow-clone all repos (depth 1). Combined with `include`, also sparse-checkouts. Ideal for CI. |
| `un sync --locked` | Sync using exact versions from `unified.lock` (fails if stale) |
| `un update` | Update all repos/artifacts to latest allowed versions, rewrite lock file |
| `un update <name>` | Update a specific repo or artifact |
| `un status` | Show workspace state — clean, modified, ahead/behind per repo |
| `un add <url>` | Add a git repo to `unified.toml` |
| `un add --artifact <url>` | Add an artifact to `unified.toml` |
| `un remove <name>` | Remove a repo or artifact from config and workspace |

### Git Workflow

These commands are convenience wrappers around `git` operations, run inside the worktree checkout at the repo's workspace path. They only work on repos checked out in worktree mode (including sparse worktrees) — not copy-mode repos.

For fine-grained git operations (interactive staging, rebase, etc.), `cd` into the workspace path and use `git` directly — it's a real git worktree.

| Command | Description |
|---------|-------------|
| `un branch <repo> <name>` | Create and switch to a new branch in the repo's worktree (`git checkout -b`) |
| `un commit <repo> [-m msg]` | Stage all tracked changes and commit (`git commit -a`). Opens `$EDITOR` if no `-m`. |
| `un push <repo>` | Push the current branch to its upstream remote (`git push`) |
| `un diff [<repo>]` | Show uncommitted diffs. Without `<repo>`, shows diffs across all worktree repos |
| `un log <repo> [-n N]` | Show recent commits (`git log --oneline`) |

### Tools & Tasks

| Command | Description |
|---------|-------------|
| `un run <tool> [args...]` | Download (if needed) and execute a tool. Prepends tool's default `args` and sets `env`. |
| `un task <name>` | Run a named task from `[tasks]`. Resolves `depends` first. |
| `un task` | List all available tasks with descriptions |
| `un tool install <name>` | Install a tool globally to `~/.unified/bin/` |
| `un tool list` | List installed tools and their cached versions |
| `un app <name>` | Download (if needed) and launch an application from `[apps]` |
| `un setup` | Run setup commands from `[setup]` (e.g. install IDE extensions) |
| `un launch` | Show interactive launcher menu (same as running `./launch.sh`) |

### Collections

| Command | Description |
|---------|-------------|
| `un collection list` | List all collections defined in `unified.toml` |
| `un collection show <name>` | Show repos, artifacts, and tools in a collection |
| `un collection use <name>` | Set the default collection for this machine (persisted in `.unified/user.toml`) |
| `un collection use --clear` | Clear the default collection (sync everything) |
| `un sync --collection <name>` | Sync only a specific collection (overrides default) |
| `un sync --all` | Sync everything, ignoring the default collection |

Most commands respect the active collection: `un status`, `un diff`, `un exec`, `un update` all filter to the active collection's repos. Use `--all` to override.

### Utility

| Command | Description |
|---------|-------------|
| `un clean` | Remove stale cache entries |
| `un exec <cmd>` | Run a command in all workspace repos (or active collection) |
| `un exec --filter <pat> <cmd>` | Run a command in matching repos |

## Configuration Reference

### `unified.toml`

```toml
[workspace]
name = "my-project"               # Workspace name
members = ["components/*"]         # Glob patterns for sub-workspaces (optional)
exclude = ["components/legacy"]    # Exclusion patterns (optional)

# ─── Git Repositories ─────────────────────────────────────────────

[repos.mylib]
url = "https://github.com/org/mylib.git"   # Repository URL (required)
path = "libs/mylib"                          # Workspace checkout path (required)
branch = "main"                              # Track a branch (mutually exclusive with tag/rev)
# tag = "v1.0.0"                             # Pin to a tag
# rev = "abc1234"                            # Pin to a commit
checkout = "worktree"                        # "worktree" (default) or "copy"
include = ["src/**", "include/**"]           # Sparse worktree: only matching paths visible (blobless clone)
exclude = ["**/test/**"]                     # Exclude matching paths (applied after include)
shallow = false                              # true = --depth 1 (no history). Orthogonal to include/exclude.
# checkout="worktree" + include → sparse worktree (git repo, only matching files, blobs on demand)
# checkout="copy" + include → filtered copy (plain directory, not a git repo)

# ─── Artifacts ────────────────────────────────────────────────────

[artifacts.my-artifact]
github = "org/repo"                # GitHub Releases (owner/repo)
# gitlab = "group/project"         # GitLab Releases (group/project or numeric ID)
# gitea = "owner/repo"             # Gitea/Forgejo Releases (owner/repo)
# artifactory = "path/to/artifact" # Artifactory path
# url = "https://..."              # Direct URL
version = ">=1.0.0, <2.0.0"       # Semver requirement (for github/gitlab/gitea/artifactory)
path = "vendor/artifact"           # Local path to place artifact
sha256 = "..."                     # Expected checksum (optional for github, required for url)
provider = "my-provider"           # Use a custom provider from [providers] (optional)
platform = { linux-x86_64 = "linux-amd64", macos-aarch64 = "darwin-arm64" }  # Platform mappings (optional)
extract = true                     # Extract archive into path (default). Set false to keep the raw download.

# ─── Tools ────────────────────────────────────────────────────────

[tools.mytool]
github = "org/tool-repo"          # GitHub Releases source
# gitlab = "group/project"        # GitLab Releases source
# gitea = "owner/repo"            # Gitea/Forgejo Releases source
# artifactory = "tools/mytool"    # Artifactory source
# url = "https://..."             # Direct URL
version = "1.*"                   # Semver requirement
# provider = "my-provider"        # Custom provider from [providers] (optional)
env = { KEY = "value" }            # Environment variables set during `un run` (optional)
args = ["--flag"]                  # Default args prepended to `un run` invocations (optional)

# ─── Tasks ────────────────────────────────────────────────────────

[tasks.example]
cmd = "un run mytool -- src/"      # Shell command to execute
description = "Run mytool on src"  # Shown by `un task` (optional)
depends = ["other-task"]           # Run these tasks first (optional)

# ─── Setup ────────────────────────────────────────────────────────

[setup]
run = [                            # Commands executed by `un setup`
    "code --install-extension org.my-ext",
    "un run mytool --version",
]

# ─── Apps ─────────────────────────────────────────────────────────

[apps.myapp]
github = "org/app"                 # Same providers as tools/artifacts
# gitlab = "group/app"
# gitea = "owner/app"
# artifactory = "tools/myapp"
version = "2025.*"                 # Semver requirement
description = "My Application"     # Shown by `un app` (optional)
icon = "🔧"                        # Launcher menu icon (optional)

# ─── Launcher ─────────────────────────────────────────────────────

[launcher]
generate = true                    # `un sync` generates launch.sh / launch.bat

[[launcher.entries]]
name = "Open App"                  # Menu entry label
app = "myapp"                      # References [apps.myapp]
icon = "🔧"

[[launcher.entries]]
name = "Run Task"
task = "example"                   # References [tasks.example]
icon = "✨"

[[launcher.entries]]
name = "Custom Command"
cmd = "code ."                     # Arbitrary shell command
icon = "📝"

# ─── Collections (named subsets for partial sync) ────────────────

[collections.team-a]
repos = ["mylib"]                  # Names from [repos.*] (optional)
artifacts = ["my-artifact"]        # Names from [artifacts.*] (optional)
tools = ["mytool"]                 # Names from [tools.*] (optional)

# ─── Providers (custom instances of release APIs) ───────────────

[providers.my-provider]
provider_type = "github"           # github | gitlab | gitea | artifactory
api_url = "https://github.example.com/api/v3"  # API base URL
token_env = "GHE_TOKEN"            # Env var holding the auth token

# Built-in defaults (no config needed for public instances):
#   "github"      → https://api.github.com      / GITHUB_TOKEN
#   "gitlab"      → https://gitlab.com           / GITLAB_TOKEN
#   "gitea"       → https://gitea.com            / GITEA_TOKEN
#   "artifactory" → ARTIFACTORY_URL env          / ARTIFACTORY_TOKEN

# ─── Settings ─────────────────────────────────────────────────────

[settings]
git-fetch-with-cli = false         # Use system git for fetch (for proxies/SSH)
parallel = 4                       # Maximum parallel operations
cache-dir = "~/.unified"           # Cache directory (default: ~/.unified)
shallow = false                     # Shallow-clone all repos (like --shallow)
manage-gitignore = true             # Auto-update .gitignore with managed paths
manage-vscode = true                # Auto-update .vscode/settings.json (git.ignoredRepositories)
```

### `unified.lock`

The lock file is auto-generated by `un sync` and should be committed to version control. It ensures reproducible workspace state across machines and CI.

```toml
version = 1

[[repo]]
name = "mylib"
url = "https://github.com/org/mylib.git"
branch = "main"
rev = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"

[[artifact]]
name = "my-artifact"
source = "github:org/repo"
version = "1.2.0"
url = "https://github.com/org/repo/releases/download/v1.2.0/artifact-linux-x64.tar.gz"
sha256 = "abc123def456..."
size = 12345678
```

### `--locked` vs `--frozen`

- **`un sync`** — Resolves latest matching versions, updates `unified.lock`
- **`un sync --shallow`** — Shallow-clones all repos (`--depth 1`, no history). Composes with `include` patterns (sparse-checkout). Can combine with `--locked`. Ideal for CI.
- **`un sync --locked`** — Uses exact versions from `unified.lock`, fails if config changed since last lock
- **`un sync --frozen`** — Like `--locked`, but also skips network access entirely (uses only cache)
- **`un sync --collection <name>`** — Sync only repos/artifacts/tools in the named collection
- **`un sync --all`** — Sync everything, ignoring the active default collection

Flags compose: `un sync --collection ci-minimal --shallow --locked` syncs only the `ci-minimal` collection, using shallow clones and locked versions.

### User-local config (`.unified/user.toml`)

Per-machine preferences that should not be committed (the `.unified/` directory is git-ignored):

```toml
# .unified/user.toml — written by `un collection use`, not committed
default-collection = "firmware-team"
```

When `default-collection` is set, `un sync`, `un status`, `un diff`, `un exec`, and `un update` all operate on only that collection's items. Use `--all` to override, or `un collection use --clear` to remove the default.

## Cache Layout

All data is cached at `~/.unified/` (overridable via `UNIFIED_HOME` env var or `settings.cache-dir`):

```
~/.unified/
├── git/
│   ├── db/                        # Bare git databases (shared across workspaces)
│   │   └── firmware-a1b2c3d4/     # {name}-{url_hash}
│   └── checkouts/                 # Working copies for worktree creation
│       └── firmware-a1b2c3d4/
│           └── e5f6a7b/           # Short commit hash
├── artifacts/
│   └── test-vectors/
│       └── v1.2.0/
│           └── vectors-linux-x64.tar.gz   # Raw downloaded archive (cache)
├── tools/
│   └── protoc/
│       └── v25.1/
│           ├── protoc-linux-x64.zip       # Raw downloaded archive (cache)
│           └── content/                   # Extracted content (for un run)
│               └── protoc                 # Executable
├── apps/
│   └── clion/
│       └── v2025.1/
│           └── clion-linux.tar.gz         # Raw downloaded archive (cache)
├── bin/                           # Globally installed tool symlinks
│   ├── protoc -> ../tools/protoc/v25.1/content/protoc
│   └── buf -> ../tools/buf/v1.28.0/content/buf
└── tmp/                           # In-progress downloads (cleaned on next sync)
```

Multiple workspaces on the same machine share the git database cache. Fetching a repo that's already in `~/.unified/git/db/` only requires an incremental fetch, not a full clone.

## Design Principles

1. **Familiar workflow** — `un sync` and `un update` work like `uv sync` and `cargo update`. If you know cargo, you know un.
2. **Reproducible** — `unified.lock` pins every revision and checksum. CI gets exactly what the developer locked.
3. **Fast** — Parallel fetches, incremental git updates, hardlink copies, artifact caching. Written in Rust.
4. **Non-destructive** — `un sync` never discards local changes. Dirty worktrees are reported, not overwritten.
5. **Corporate-ready** — `git-fetch-with-cli` shells out to your system git, respecting proxy configs, SSH keys, credential helpers, and `.gitconfig`. Configurable `[providers]` for GitHub Enterprise, self-hosted GitLab/Gitea, and company Artifactory instances.
6. **Composable** — Each workspace has its own `unified.toml`. Workspaces can be nested via `members`.

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `UNIFIED_HOME` | Cache directory location | `~/.unified` |
| `GITHUB_TOKEN` | GitHub API authentication (public github.com) | — |
| `GITLAB_TOKEN` | GitLab API authentication (public gitlab.com) | — |
| `GITEA_TOKEN` | Gitea/Forgejo API authentication (public gitea.com) | — |
| `ARTIFACTORY_TOKEN` | Artifactory authentication | — |
| `UN_LOG` | Log level (`error`, `warn`, `info`, `debug`, `trace`) | `info` |
| `UN_PARALLEL` | Max parallel operations | `4` |
| `UN_GIT_FETCH_WITH_CLI` | Force CLI git for all fetches | `false` |
| `UN_SHALLOW` | Shallow-clone all repos (like `--shallow`) | `false` |
| `UN_COLLECTION` | Override default collection for this invocation | — |

## Comparison with Alternatives

### vs git submodules
Submodules track a commit pointer inside your repo. They break on branch switches, confuse new developers, require manual `git submodule update --init --recursive`, and make CI scripts fragile. **un** manages the same dependency with a simple TOML entry and `un sync` — no footguns.

### vs Google repo (Android)
Repo uses XML manifests, is Python-based, tightly coupled to Gerrit, and doesn't support artifacts or lock files. **un** is faster (Rust), uses TOML, has lock files for reproducibility, and downloads artifacts too.

### vs tsrc
tsrc is a Python tool for multi-repo YAML manifests. No lock files, no artifact support, no caching strategy. **un** adds deterministic locking, artifact management, tool execution, and is written in Rust for speed.

### vs cargo (for non-Rust deps)
Cargo handles Rust dependencies brilliantly but can't manage non-Rust repos, binary artifacts, or test data. **un** fills this gap — use cargo for your Rust crates, un for everything else in your workspace.

### vs git LFS
LFS requires server-side setup, is opaque about what's tracked, breaks in surprising ways, and doesn't version artifacts independently. **un** downloads versioned artifacts from standard sources (GitHub Releases, Artifactory, HTTP) with explicit checksums.

### vs Bazel / Buck2
Build systems that can fetch dependencies, but require buying into an entirely different build paradigm. **un** is build-system-agnostic — it just prepares your workspace. Use it with Make, CMake, Bazel, cargo, or anything else.

## Migrating from git submodules

```bash
# Coming soon: automatic migration
un import-submodules    # Reads .gitmodules, generates unified.toml
un sync                 # Sets up workspace from unified.toml
git rm .gitmodules      # Remove submodule config
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
