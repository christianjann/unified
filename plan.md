# Implementation Plan

## Overview

`un` (unified) is a Rust CLI tool that manages multi-repo workspaces, artifact downloads, and tool execution via a declarative `unified.toml` config and deterministic `unified.lock` lock file.

See [README.md](README.md) for user-facing documentation and [doc/architecture.md](doc/architecture.md) for internal architecture.

## Research Summary

### Existing tools — none fill the gap

| Tool | Type | Git repos | Artifacts | Lock file | Language | Status |
|------|------|-----------|-----------|-----------|----------|--------|
| git submodules | Built-in | Yes | No | Sort-of | C | Painful UX |
| Google repo | Multi-repo | Yes (XML) | No | No | Python | Gerrit-focused |
| tsrc | Multi-repo | Yes (YAML) | No | No | Python | Unmaintained |
| metarepo | Multi-repo | Yes (JSON) | No | No | Rust | Incomplete |
| git-subrepo | Multi-repo | Yes (squashes) | No | No | Bash | Niche |
| foreman | Tool mgmt | No | Yes (GH/Artifactory) | No | Rust | Roblox-specific |
| rustup | Tool mgmt | No | Yes (Rust toolchains) | No | Rust | Rust-specific |
| cargo | Build tool | Packages only | No | Yes | Rust | Rust-specific |
| uv | Package mgmt | No | Yes (PyPI) | Yes | Rust | Python-specific |
| Jujutsu (jj) | VCS | Single-repo | No | No | Rust | Different goal |
| GitButler | VCS GUI | Single-repo | No | No | Rust/TS | Different goal |

**Conclusion:** No existing tool combines multi-repo git management + artifact downloads + tool execution + lock files + CI optimization. We build `un`.

### Inspiration mapping

| Feature | Primary inspiration | What we take |
|---------|-------------------|--------------|
| Git caching (db/checkout) | cargo `sources/git/` | Three-tier GitRemote→GitDatabase→GitCheckout architecture |
| TOML config schema | cargo `TomlDetailedDependency` | Named dependency sections with branch/tag/rev |
| Lock file format | cargo `Cargo.lock` | TOML lock with pinned revisions and checksums |
| CLI structure | uv `uv-cli` | clap derive with argument composition via flatten |
| Sync/lock workflow | uv `sync.rs` | `sync`, `sync --locked`, `sync --frozen` modes |
| GitHub/Artifactory download | foreman `tool_provider/` | Provider trait, platform detection, token auth |
| HTTP resume downloads | rustup `download/mod.rs` | Range headers, streaming hash, progress bars |
| Tool execution | foreman `main.rs` | Download-if-needed, exec with passthrough |
| Multi-repo orchestration | metarepo `exec/` | Parallel command execution across repos |
| Worktree management | metarepo `worktree/` | Git worktree add/list/remove across managed repos |

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Git backend | gix (primary) + CLI fallback | Pure Rust default, corporate compatibility via `git-fetch-with-cli` |
| Workspace checkout | Configurable per-repo | Worktrees for editable deps, copies for read-only deps |
| Partial checkout | Two orthogonal axes in Phase 2 | `include`/`exclude` → sparse worktree or filtered copy; `shallow` → depth-1 clone. Compose freely. |
| License | MIT + Apache-2.0 (dual) | Standard Rust ecosystem convention |
| Tool execution | `un run <tool>` subcommand | Simpler than separate binary, discoverable |
| Config format | Named sections `[repos.<name>]` | Familiar to Cargo users, easy to reference |
| Scope | Workspace-first | Per-project `unified.toml`, global tools via `un tool install` |

## Implementation Phases

### Phase 1: Foundation (crate scaffold, config, cache)

**Goal:** Compilable workspace with config parsing and cache paths. No operations yet.

- [ ] Convert `Cargo.toml` to workspace manifest
- [ ] Create `crates/un-cli/` with clap CLI skeleton and command stubs
- [ ] Create `crates/un-core/` with `Config` and `LockFile` serde structs
- [ ] Create `crates/un-cache/` with cache directory layout
- [ ] Create `crates/un-git/` with `GitReference` enum and stubs
- [ ] Create `crates/un-download/` with provider trait and stubs
- [ ] `un init` writes a scaffold `unified.toml`
- [ ] Unit tests: config parsing round-trip, lock file serialization, cache path generation

### Phase 2: Git operations

**Goal:** `un sync` can clone and check out git repos.

- [ ] Implement `GitRemote` — URL normalization, refspec construction
- [ ] Implement `GitDatabase` — bare clone via gix, resolve refs to OIDs
- [ ] Implement CLI fallback — shell out to `git` for fetch
- [ ] Implement `GitCheckout` — worktree mode (git worktree add)
- [ ] Implement `GitCheckout` — copy mode (hardlink/copy)
- [ ] Implement `GitCheckout` — sparse worktree (include/exclude → blobless clone + sparse-checkout)
- [ ] Implement `GitCheckout` — filtered copy (checkout=copy + include/exclude → walk+copy subset)
- [ ] Implement shallow clone (`--depth 1`, orthogonal to checkout mode)
- [ ] Implement `--shallow` CLI flag and `UN_SHALLOW` env var
- [ ] Atomic operations: `.unified-ok` markers, temp dirs
- [ ] Integration test: `un sync` with file:// git repo

### Phase 3: Core commands

**Goal:** Full sync/status/update workflow.

- [ ] `un sync` — orchestrate git fetches, populate workspace, write lock file
- [ ] `un sync` — auto-update `.gitignore` (managed block with sentinel comments)
- [ ] `un sync` — auto-update `.vscode/settings.json` (`git.ignoredRepositories`)
- [ ] `settings.manage-gitignore` / `settings.manage-vscode` opt-out flags
- [ ] `un sync --locked` — fail if config changed since lock
- [ ] `un sync --frozen` — no network, cache-only
- [ ] `un status` — report clean/modified/ahead-behind per repo
- [ ] `un update` — fetch latest for branch-tracking repos, update lock
- [ ] `un add <url>` — add repo to config, sync
- [ ] `un remove <name>` — remove from config, lock, workspace
- [ ] Parallel git fetches with semaphore
- [ ] Progress bars (indicatif MultiProgress)

### Phase 4: Git workflow commands

**Goal:** Developers can make changes, commit, and push from worktree-mode workspace repos. These are convenience wrappers around git, run inside the repo's workspace path.

- [ ] `un branch <repo> <name>` — `git checkout -b <name>` in worktree
- [ ] `un commit <repo> [-m msg]` — `git commit -a` in worktree (opens $EDITOR if no -m)
- [ ] `un push <repo>` — `git push` in worktree
- [ ] `un diff [<repo>]` — `git diff` in one or all worktree repos
- [ ] `un log <repo> [-n N]` — `git log --oneline` in worktree
- [ ] Error on copy-mode repos with clear message: "repo X is in copy mode, switch to worktree for git operations"

### Phase 5: Artifact & tool management

**Goal:** Download artifacts and execute tools.

- [ ] Download engine — reqwest, resume, SHA-256, progress
- [ ] GitHub Releases provider — API, semver, platform detection
- [ ] Artifactory provider — storage API, bearer auth
- [ ] Generic HTTP provider — direct URL, checksum
- [ ] Artifact sync integrated into `un sync`
- [ ] `un run <tool> [args...]` — download + execute
- [ ] `un tool install` — global install to `~/.unified/bin/`

### Phase 6: Polish & CI

**Goal:** Production-ready for daily developer use and CI pipelines.

- [ ] `un exec <cmd>` — run across all repos
- [ ] `un clean` — garbage collect cache
- [ ] Shell completions (bash, zsh, fish, powershell)
- [ ] `un import-submodules` — migrate from `.gitmodules`
- [ ] Error recovery and retry logic
- [ ] Windows support (junction points, path handling)
- [ ] Man pages / mdbook documentation
- [ ] CI release pipeline (cross-compile, publish)

## Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `clap` | 4.x | CLI parsing with derive macros |
| `serde` + `toml` | latest | Config/lock file serialization |
| `gix` | latest | Pure-Rust git operations |
| `reqwest` | latest | HTTP client (with rustls) |
| `tokio` | 1.x | Async runtime |
| `sha2` | latest | SHA-256 checksums |
| `semver` | 1.x | Version requirement matching |
| `glob` | latest | File path pattern matching (include/exclude) |
| `indicatif` | 0.17+ | Progress bars |
| `console` | latest | Terminal colors |
| `tracing` | latest | Structured logging |
| `home` | latest | Home directory detection |
| `thiserror` / `anyhow` | latest | Error handling |
| `tempfile` | latest | Atomic file operations |

## Verification Criteria

1. `cargo build` — All crates compile without errors
2. `cargo test` — Unit tests pass for config, lock, cache paths
3. `un init` — Creates valid `unified.toml` in empty directory
4. `un sync` — Clones a public GitHub repo into workspace via worktree
5. `un sync --locked` — Fails when config changed since lock
6. `un status` — Reports dirty/clean correctly
7. `un update` — Advances branch-tracking repos, updates lock
8. `un run <tool>` — Downloads and executes a tool from GitHub Releases
9. Corporate proxy — Works with `git-fetch-with-cli = true`
10. CI — `un sync` in clean Docker container (no prior cache)
