# Managed mode and the launch gate

On some company-managed computers, security software reacts to an agent CLI merely being
run: a version check is enough. So this app starts an agent's binary only when you turned
that agent on, and never when your organization forbids it.

**The rule.** For an agent that is not enabled, the app does not execute its binary at all:
not for discovery, not `--version`, not a model list, not a warm process, not an update or
sign-in check, not a console, and not through a shell or `npx`. It only looks at files: is
the binary there, where is it, is there a config file.

This is about not launching things. Nothing here hides, renames or delays a launch, and
nothing is excluded from security tools.

## What you see

- **A new profile** starts with every agent off. The first-run choice (backend state today,
  the screen comes with the welcome step) asks which agents to turn on. "None" is a valid
  answer.
- **Settings → Agents → Providers**: each agent's card has an **Enabled** switch. A card
  reads *Not enabled*, *Blocked by your organization* or *Not allowed by your organization*
  when the agent cannot be started.
- **An existing profile** keeps working after the update: see [the upgrade rule](#the-upgrade-rule).
- **Turning an agent off** stops it now: its warm process and consoles are killed and its
  running turns are stopped. See [turning an agent off](#turning-an-agent-off).
- An agent that is off is not offered in pickers. Anything that would still start it (a
  schedule, a bot, a team role, a turn started from another computer) is refused with
  `<Agent> is not enabled. Turn it on in Settings → Providers before using it.`

## What still runs at startup

On a fresh install, before any agent is enabled, the app executes nothing that belongs to
an agent. At startup it reads files only: its database, its settings, the policy, and
directory listings to find installed binaries.

What can still run without an agent being enabled, each only for the feature that needs it:

| Program | When |
| --- | --- |
| `git` | A project is open: status, branches, worktrees, diffs. |
| `gh` | GitHub features, once you use them. |
| This app's own helper process and `/usr/bin/security` (macOS) | Reading a saved sign-in for the usage check of an *enabled* agent. |
| `ps`, `open`, `xdg-open` | Listing a terminal's processes; opening a file or link you clicked. |
| Your shell | A terminal you open, a project's run script, a project check. |
| `node`, `npx`, `brew`, `uv` | Only for a feature you start: skills, deploys, an agent update you click. |

`tailscale` is never run by this app: it only checks whether the file exists.

A terminal is your own shell. If you type an agent's command there, you started it; the
gate covers what the app starts.

## The upgrade rule

A profile that has no saved choice gets one the first time this version opens it:

1. An agent counts as **used** when the profile has a thread with at least one message in
   it, anything saved for it in provider settings (an executable path, environment
   variables, OpenCode's model selection, MiMo's endpoint, Claude's connection mode), or a
   named connection. An empty draft is not a choice. Installed is not used.
2. **Not a strict machine:** every used agent starts enabled, everything else disabled. The
   first-run choice is pending only when nothing was used, which is a fresh profile.
3. **A strict machine** (managed, or a policy is installed): nothing starts enabled, the
   first-run choice is pending, and the used agents are only remembered as suggestions for
   the choice. The update must not start, on a managed computer, the very binary that
   causes the incident.
4. The policy is applied first: an agent it forbids is never enabled or suggested.

The decision is saved once to `launch-gate.json` in the app data folder and not made again:
a thread created later in another agent does not turn that agent on. If the file cannot be
read or written, every agent stays off for that run and the unreadable file is left as it is.

## Managed computers

**Detection (macOS only, read-only).** A Mac counts as managed when any of these exists:

- an entry in `/Library/Managed Preferences` (MDM delivers managed preferences),
- Claude Code's administrator files, `managed-settings.json` or `managed-mcp.json`, in
  `/Library/Application Support/ClaudeCode`,
- a policy file for this app (below).

The app reads directory entries and files. It runs no `defaults`, `plutil` or `profiles`:
a spawn at startup is what endpoint security reacts to. On Linux and Windows nothing is
detected; the enabled set and a policy file still apply.

**Strict mode** is the default on a managed computer: nothing is turned on for the user, as
described in the upgrade rule. A policy can set `strict` either way.

## Policy

An administrator can install a policy. The user cannot change or lift it from Settings, a
project or the environment.

| Platform | Policy file |
| --- | --- |
| macOS | `/Library/Application Support/ViewCode/policy.json` |
| Linux | `/etc/viewcode/policy.json` |
| Windows | `%ProgramData%\ViewCode\policy.json` |

These are system folders that only an administrator can write. The file is read, never
executed.

On macOS a second source is read as well: the MDM managed preferences for
`ai.pandacode.desktop` (`/Library/Managed Preferences/ai.pandacode.desktop.plist`, then
`/Library/Managed Preferences/<user>/ai.pandacode.desktop.plist`), an XML or binary plist
with the same keys.

**The stricter source wins.** When more than one exists they are combined: an agent any
source blocks is blocked; with several allow lists only what all of them name is allowed;
strict if any says so; keep-awake only if none forbids it.

**When it is read.** At launch, and again while the app runs: every 30 seconds the app
compares the files' size and modification time and re-reads them when one appeared,
changed or went away. A new block then stops that agent's running turns, warm process and
consoles, as turning it off does.

```json
{
  "strict": true,
  "providers": { "codex": { "allowed": false } },
  "allowedProviders": ["claude"],
  "keepAwake": { "allowed": false }
}
```

| Key | Meaning |
| --- | --- |
| `strict` | `true`: nothing is enabled for the user. `false`: a managed computer follows the normal upgrade rule. Absent: strict when managed or when a policy is installed. |
| `providers.<id>.allowed: false` | Blocks an agent. The id is matched against the agent id and against the executable name it runs, so `claude` also blocks GLM Coding Plan and MiMo Token Plan, which run the `claude` binary. |
| `allowedProviders` | Pins the allowed set: only these can be enabled. An agent enabled before the pin stops being started. |
| `keepAwake.allowed: false` | The app never holds the computer awake during a turn. |

Agent ids are the ones in the roster (`claude`, `codex`, `opencode`, …; `provider_configuration`
lists them). `claudeAgent` and `commandCode` are accepted as `claude` and `commandcode`, so a
profile written for ViewCode's TypeScript app keeps its meaning. Unknown keys are ignored.

**A policy that cannot be read blocks every agent.** If any source exists but cannot be
applied (not JSON or a plist, a value of the wrong type, too large), no agent starts until
it is fixed, and the state says why (`policyProblem`). The file is there, so an
administrator meant a policy; guessing which agents it named could allow what they forbade.
A file that does not exist is simply no policy.

A blocked agent cannot be turned on by the user; its switch is disabled and its card reads
exactly **Blocked by your organization**.

## Turning an agent off

Turning an agent off stops what is already running for it:

- its warm process is killed (the process group this app started and recorded),
- its consoles are closed, which ends their process,
- its running turns get Stop, the same as pressing Stop.

Stopping is the safer of "finish or stop": a turn left running would keep starting the
binary that was just turned off, and every new start would be refused mid-turn. Cloud runs
on another machine are not stopped; they start no process on this computer.

Processes are only ever ended by the PID this app started. Nothing is killed by name.

## Claude Code's own administrator files

Claude Code has its own managed files, and they change how its CLI may be started. They are
read from disk; the CLI is not asked.

| File | What the CLI does | What this app does |
| --- | --- | --- |
| `managed-mcp.json` | Loads only the MCP servers the organization lists; refuses `--strict-mcp-config` and any server passed with `--mcp-config`, and exits. | Leaves both flags out of the model-list probe. Starts turns and warm agents without its own tool server, and tells the agent that the app's tools are unavailable so it does not pretend to call them. Chat, edits, search, the terminal and git work as usual. |
| `managed-settings.json` with `disableBypassPermissionsMode: "disable"` (top level or under `permissions`) | Refuses `bypassPermissions`. | Full access runs as accept-edits, in task runs and in the console, and `--allow-dangerously-skip-permissions` is not passed. |

Locations: `/Library/Application Support/ClaudeCode` (macOS), `/etc/claude-code` (Linux),
`C:\Program Files\ClaudeCode` and `C:\ProgramData\ClaudeCode` (Windows).

The app respects the managed MCP list and never routes around it: no project `.mcp.json`,
no settings file, no environment variable. To get the app's tools back in Claude, the
organization adds its MCP server to the managed list.

These apply to GLM Coding Plan and MiMo Token Plan too, because they run the same binary.

## Keep awake

A Mac that sleeps when its screen locks cuts an agent's connection mid-turn (a Cursor turn
ends with `RetriableError: [unavailable] PING timed out`). While at least one agent turn is
running, and only then, the app holds one macOS power assertion that prevents idle sleep
(`PreventUserIdleSystemSleep`, the same one `caffeinate -i` holds). It is released when the
last turn ends, however it ends: finished, stopped, failed or crashed. It is also released
at exit, and by macOS itself if the app dies.

- **Setting:** Settings → Agents → Advanced → Power, *Keep this computer awake while an
  agent works*. On by default. Turning it off releases the assertion at once.
- **Policy:** `keepAwake.allowed: false` turns it off for everyone on the computer; the
  switch is then disabled and says so.
- **No helper process.** The assertion is taken through IOKit inside the app, so nothing is
  started for it, not even `/usr/bin/caffeinate`. It is listed under the app's name by
  `pmset -g assertions`, where you and IT can see it.
- **The limit.** This stops *idle* sleep. Closing the lid on battery power still puts the
  Mac to sleep, as do a low battery and Apple menu → Sleep; the display can still turn off
  and lock. A turn interrupted that way fails as before.
- **Other platforms:** nothing is held on Linux or Windows; the setting is shown as
  available on macOS.

IPC: `keep_awake_state` → `{enabled, permitted, supported, active, turns}`;
`keep_awake_set_enabled {enabled}`, which emits `keep-awake-changed`. The setting is saved
in `keep-awake.json` in the app data folder.

## How it is enforced

`src-tauri/src/launch_gate.rs` holds the decision: `launch_gate::allow(harness)`. It is
keyed by the harness id and knows no harness by name, so an agent added later is covered,
and off, without a change there.

- **One enforcement point: `providers::executable(id)`.** It is how every caller finds an
  agent's binary, and it asks the gate first, before a saved executable override is even
  read. For an agent that is off or blocked it returns nothing, so every caller skips it,
  and an override cannot turn it back on.
- **A second check for paths that come from elsewhere.** A named connection can carry its
  own executable. So the three builders that turn a path into a command
  (`providers/launch.rs`: `command`, `std_command`, `pty_command`, behind
  `providers::provider_command`) ask the gate again, by harness id.
- **Fail closed.** Before the gate has loaded, and whenever its state cannot be read,
  nothing starts.
- **Asked early too**, for a clear message and no side effects: turn admission
  (`prepare_task_run_in_project`, which the composer, schedules, bots, team roles and
  remote starts all pass), `run_with_tools`, discovery, the model catalog, warm agents, the
  update check and the usage check.
- **A source scan** (`launch_gate/scan_tests.rs`) lists every place the app builds a
  process directly and what it runs. A new one fails the test until it goes through the
  gated builders or is listed with its reason. It also fails if production code names a
  harness binary as a program, if `executable` stops asking the gate first, if a new caller
  uses the ungated file lookup (`providers::locate`, for showing where a binary is), or if
  `executable` is asked for something that is not a harness.
- **A fake-process test** (`providers/launch_gate_tests.rs`) puts a stand-in for every
  harness binary first on `PATH`, each writing a marker file when executed. With every
  agent off, then with Codex off beside enabled agents, then with Codex and Claude Code
  blocked by policy while their switches are on, it runs app launch and `serve` discovery,
  the update check, usage, the model list, skills and integrations, warm-up, a turn, the
  admission that schedules, bots, team roles and remote starts pass, and the remote start
  check, and finds no marker for the agent. It also shows that enabled agents still run
  and that turning one off kills its warm process. It uses no sleeps.

### IPC

| Command | |
| --- | --- |
| `launch_gate_state` | The enabled set, `firstRunPending`, `managed`, `managedEvidence`, `strict`, `policySource`, `policyProblem`, `pinned`, `blocked`, and every harness with `installed`, `path`, `enabled`, `changeable`, `locked`, `suggested`. |
| `launch_gate_set_enabled {harness, enabled}` | Turns one harness on or off. While the first-run choice is pending this is the choice: the rest stay off. |
| `launch_gate_complete_first_run {enabled}` | The first-run choice: exactly these on. Refused once a choice exists. |

Both changes emit `launch-gate-changed` with the new state. `provider_configuration` carries
the same state as `launchGate`. `list_providers` reports each harness with `installed`
(usable now: found and enabled), `present` (found on disk) and `enabled`, so "not enabled"
is distinct from "not installed" and older clients simply do not offer the harness.

Preview: `?gate=fresh` (nothing enabled, first run pending) and `?gate=managed` (a managed
computer whose policy blocks Codex and allows only Claude Code and OpenCode);
`window.__launchGateCalls` records the changes. Spec: `tests/launch-gate.spec.ts`.
