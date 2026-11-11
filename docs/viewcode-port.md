# ViewCode port: baseline and parity

This fork ports [ViewCode](https://github.com/CreatorGhost/ViewCode) (branch `dev`) onto
PandaCode's Tauri and Rust base. This page records where the port started and what each
ViewCode feature needs here. Statuses come from reading this repository's code, not its
docs. Update a row in the pull request that changes it.

## Branches

- `main`: a clean copy of upstream PandaCode.
- `dev`: the port. One feature per branch, pull request into `dev`, merged when CI is green.

## Baseline (upstream `9437b3a`, November 2026)

| Check | Result |
| --- | --- |
| `npm ci`, `npm run build` | pass |
| `npm test` | 525 pass, 0 fail |
| `npm run test:release` | 59 pass, 0 fail |
| `cargo test --locked --lib` | 2594 pass, 16 fail, 97 ignored (Linux container) |
| `cargo clippy --all-targets -- -D warnings` | pass |

The Rust failures come from the container the baseline ran in, not from the code: no
OpenSSH client (2 `cloud::ssh` tests), no IPv6 (1 `run_script` test), running as root,
which ignores the file permissions 12 `skill_manager` tests rely on, and one
`integrations` process-reaping test that depends on timing (it passes on some runs).
GitHub Actions is the reference for these.

## Parity

Status: **Same** as ViewCode, **Partly** there (what is missing), or **Missing**.

### A. Build

| # | Feature | Status | What is here, what is missing |
| --- | --- | --- | --- |
| 1 | Code review | Partly (in progress) | Was missing: only a free-text "Review" prompt, Jev advice and team council scoring. Landed: findings, ratings, review depth playbooks, local and incremental reviews, review skill with the learning-thread rule (`src-tauri/src/review`). Missing: pull request reviews, posting to the host, Fix with agent and undo (needs checkpoints, row 28), learned review styles, the review CLI, all UI. |
| 3 | Phone through own Cloudflare (Quick connect) | Partly (backend done) | Landed ([docs](remote-access.md#quick-connect)): ViewCode's setup flow as IPC (`src-tauri/src/quick_connect/`: tools → Cloudflare sign-in → deploy → secret → verify, account choice, redeploy, remove), a bounded `npx wrangler@<exact>` runner, Node 22+ gating with a pinned checksummed download, the relay Worker's self-hosted mode with a host secret (`relay/src/self.ts`), one-time 5-minute pairing links with per-device revocation, connection choice as a setting, and the GitHub-based shared relay hidden by default (not removed). Kept from PandaCode: end-to-end encryption and the frame protocol. Missing: the Connect phone UI, the mobile app's side of pairing, auto-creating a workers.dev subdomain, and the local listener that Same Wi-Fi and Tailscale `serve` need. |
| 2 | Computer use | Partly (in progress) | Landed: the native helper crate (`native/computer-driver`), the gate's rules (`src-tauri/src/computer_use`), the client that runs the helper as a child process (restart on crash, timeouts), the service (per-thread state, approvals as backend state, the pause while any approval waits, stale-target refusal, screenshots), the settings (off by default, schema 39; an opt-in "bypass all permissions" that removes every safeguard at the user's own risk), the `pandacode_computer_*` agent tools and the `computer_use_*` commands with the `computer-use-changed` event. See [computer-use.md](computer-use.md). Missing: all UI (settings, approval cards, recent actions), so approvals cannot be answered from the app yet; bundling the helper into the macOS build; the `viewcode-computer` launcher; directory grants for screenshots; background input and the agent cursor. |
| 4 | Project colours | Partly (backend done) | `projects.color` (schema 38), one of ViewCode's 18 names, `set_project_color`, `project-changed` event; every project list carries it. Missing: the rail tile, folder badge and composer glow, and the two appearance settings (colour the app by the active project, composer glow), which are webview preferences here. |
| 5 | Cursor agent | Missing | Not among the 19 harnesses. The CLI runtime fits `providers/acp.rs`; the SDK runtime is Node-only. |
| 6 | Managed-Mac mode | Missing | No policy reader, no launch gate, no first-run choice. `discover` probes every installed CLI and Claude always gets `--strict-mcp-config`. |
| 7 | CLI tools where MCP is blocked | Missing | Tools reach agents only through the per-run loopback MCP host. No shims, no HTTP tool route. |
| 8 | Chat import | Partly | Claude Code and Codex only, text only. Missing: Cursor (CLI and editor), T3 Code, the one-time ViewCode import (order, attachments, parent links, side chats, goals, colours), nested Codex sub-agents. |
| 9 | PR card | Partly | PR detail, checks and mergeable via `gh` (`github_pull.rs`). Missing: commit graph, conflict files via `git merge-tree`, running command, card placement. |
| 10 | GitHub account routing | Missing | Every call uses the active `gh` account. |
| 11 | RockBot | Missing | Agent tools are scoped to one project; no orchestrator thread kind, no confirmed sends. |
| 12 | Debug mode | Missing | No `/debug`, no per-turn guidance. |
| 13 | Droid agent | Missing | No `droid` flavour in `acp.rs`. |
| 14 | Read aloud | Missing | `voice/` is dictation only. |
| 15 | Mermaid and stack diagrams | Missing | Mermaid shows as labelled source; no diagram skill. |
| 16 | GitLab, Forgejo, Gitea, Bitbucket, Azure DevOps | Missing | GitHub only, through `gh`. |
| 17 | Cookie import, default browser, SnapShot, prompt links, question attachments | Missing | The browser pane is incognito; no URL handler; no window capture; answers to agent questions take JSON only. |
| 18 | Merged-PR rule | Missing | No automatic settling at all; merges only update `task_links.state`. |
| 19 | ViewCode look | Partly | Data model lacks a generic parent link and thread kind on sessions, and project colour. The shell restyle is in progress on its own branch. |
| 20 | Outside agents | Missing | The MCP host is per run, loopback, with a per-run token. No long-lived endpoint, sign-in, scopes or stdio bridge. |

### B. Compare and close gaps

| # | Feature | Status | Gaps |
| --- | --- | --- | --- |
| 21 | Mid-chat switch with handoff | Partly | Fixed 64 KiB / 200 messages instead of a budget from the model's window; long messages are cut; no references to omitted history; no error when it cannot fit; no native Codex history; resume is not kept per account; a provider switch deletes the goal. |
| 22 | Fork from a user message | Partly | No refusal for unfinished turns; tool activity is copied; the fork shares the source checkout. |
| 23 | Child agents and messaging | Partly (different model) | Children exist only inside a configured team. No spawn tool for ordinary chats, no per-child worktree, no reply routing by run, no hop limit, no held mail or discard, pause and resume act on the whole team. |
| 24 | Side chats and expiry | Partly | `/btw` lives in the webview (`localStorage`). No thread kind, no 24-hour expiry, not hidden from lists, live context instead of a snapshot. |
| 25 | Goals and Run goal | Partly, close to missing | Codex-only and Codex-driven. No provider-neutral goal, no per-turn injection, no agent goal tools, no Run goal message, erased on provider switch. |
| 26 | Usage, rings, auto resume | Partly | Resume is opt-in per task and its timer runs in the webview. No throttle versus limit split, no once-per-failure rule, no keep-awake, no pooling across accounts, no banked resets. |
| 27 | Queue and steering | Same | On desktop the queue is drained by the webview, so a hold is not backend state; steering carries no attachments. |
| 28 | Worktrees, terminal, diffs, checkpoints, rewind | Partly | No checkpoints, no file restore, no in-place rewind (rewind forks). Per-turn diffs cover declared writes only. No terminal scrollback replay. |
| 29 | Browser panel with agent access | Partly | One visible webview. Missing: tabs, background use, press, hover, drag, scroll, select, upload, dialog, wait, resize, evaluate, recording, console and network logs, take-over, an off switch. |
| 30 | Device panel | Partly (strong core) | Agent tools are macOS-only even for Android; only appearance among typed device actions; no agent-access setting; frame polling instead of video. |
| 31 | Dictation | Partly | More models and services than ViewCode, but no live words while speaking. |
| 32 | Phone app and notifications | Partly | Only "waiting" and "finished", sent through the shared relay with generic text. A stopped, failed or limited run still says finished. No limit, gave-up or resumed kinds, no per-kind settings, no thread title. |
| 33 | Activity and thinking log | Partly | Frontend only: no summary ranking, no message counting, no lead-derived child status. |
| 34 | HTML renders | Partly | Static fences render without scripts. No publish tool, page store, image inlining, headless preview, lifecycle on revert, or phone delivery. |
| 35 | Composer | Partly | 4 images, 5 documents, 3 folders. No structured context references, thread references, PR picker, quote a reply, or large files. |
| 36 | Permission modes, project settings, keybindings, palette, themes | Partly | No default or per-project mode (new tasks are hard-coded to full). Per-project settings and keybindings are missing. Palette is the same. Themes lack Follow System and an editor. |
| 37 | Welcome wizard, updates, keep-awake | Partly | Updates are the same or better. Welcome wizard and keep-awake are missing; no auto-continue after a restart. |
| 38 | Scheduled tasks | Partly | More schedule kinds than ViewCode. Missing: webhooks, a new thread per run, skipping a fixed-time run missed by more than 10 minutes. |
| 39 | Agents | Partly | Claude, Codex, OpenCode, Command Code, Devin, Pi, Muse, Antigravity and Grok are here. The ACP registry (search, install, checksum) is missing. |
| 40 | Analytics off by default | Same | No analytics code at all. No opt-in diagnostics export. |

### C. Not needed

| Feature | Finding |
| --- | --- |
| No-port desktop mode | The window uses Tauri IPC and there is no app server, so there is nothing to hide for the UI. The app does open loopback ports in normal use: one MCP host per agent run (random port, token), a preview server per previewed folder, the bot web channel when enabled, and private-address listeners while Connect direct routes are on. Matching ViewCode fully would move the MCP host to stdio or a Unix socket. |

## Found on the way

- The updater in `src-tauri/tauri.conf.json` still points at upstream's releases and
  signing key. A build of this fork would update itself to upstream PandaCode. It must
  change before the first release of the fork (row 19, brand and name).
