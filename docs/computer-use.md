# Computer use

An agent sees the apps open on this machine and, if the user allows it, operates them.
Ported from ViewCode. Port status: [viewcode-port.md](viewcode-port.md).

The backend is here; there is no screen for it yet. `src-tauri/src/computer_use/` holds
the rules (pure functions and tables), the client that runs the native helper
(`native/computer-driver`, macOS only), the service that applies the rules to each
request, the settings and the agent tools. It is **off** until the setting is changed,
and until a settings screen exists that takes the `computer_use_set_settings` command.

## The rules

The app is the only gate. It assumes the agent ignores every instruction and sends raw
requests itself, so every decision is made from what is true at that moment and is made
again at each step: when the request arrives, before the helper prepares a target, and
immediately before input.

- **Modes** (`policy::Mode`): off, observe (look only: list windows, read controls, take
  screenshots) and control (also input). An unreadable setting counts as off.
- **Approvals** (`policy::Approvals`): follow thread permissions (Full access skips
  routine approvals, every other mode asks for each input), risky only (only
  destructive-looking input asks), or never. "Allow for the rest of this turn" covers
  routine input until the turn ends and never a destructive-looking action. An
  unreadable setting counts as follow thread permissions.
- **Destructive-looking** is a label or role matching words such as delete, send, submit,
  purchase, sign out or format, and the chords that quit, close or delete. It reads
  names, so it is a safety net, not a guarantee.
- **Protected apps** are never observed or controlled: password managers, Keychain
  Access, System Settings, the login and security prompts, PandaCode itself (by name, by
  a `.app` bundle on the executable path, or as a browser tab titled like it) and any
  window owned by the app's own process or an ancestor of it. A screenshot is refused
  while a password manager or system security window overlaps the window; unknown bounds
  count as overlapping.
- **No input while any approval is pending anywhere**: the gate's own cards in any thread,
  provider approvals tracked before storage shows them, and whatever storage shows.
  Unreadable storage counts as pending. This, not window names, is what stops an agent
  answering an approval for itself.
- **Typed text is counted, not shown.** An approval reads `Type 13 characters into …`;
  the text never reaches a card, a refusal or a debug line.
- **Show on screen**: with the setting on ask, the first input in a turn that would bring
  a window to the front asks once; kept in the background, only actions by reference run.
- **Stale targets** (`targets.rs`): window ids, refs and shots are per-thread numbers
  that start at a random base per run and are never reused. A newer observation retires
  a window's older refs, only a window's newest screenshot accepts coordinates, and a
  window missing from a listing takes its refs and shot with it. The helper re-reads the
  control's role and label, or the window's bounds, at dispatch and refuses on change.
- **A plain launcher call** (`command.rs`): a provider's own prompt for a shell command
  may be answered for the user only when the command is exactly one call to this
  session's launcher path, bare or wholly quoted, with nothing a shell would expand,
  chain or redirect. Reading a screenshot is allowed the same way only for an existing
  file inside the session's own screenshot folder.

Refusals carry ViewCode's codes (`CU-VAL-*`, `CU-NOT-*`, `CU-CON-*`, `CU-EXT-*`,
`CU-INT-*`) and its wording, with the app named from `policy::APP_NAME`.

## The helper's protocol

`protocol.rs` has the JSON lines of `native/computer-driver`: `{id, nonce, request}` in,
`{id, ok, result | error}` out. For input the helper asks `{id, authorize, phase}` about
the target it just read, once as `prepare` before it activates anything and once as
`dispatch` immediately before input, and waits for `{id, nonce, decision}` (`null`
allows). A lost, mismatched or unreadable line is never permission, and a helper that
dies after an allowed `dispatch` is reported as "may have been delivered".

## The service

`service.rs` is the gate. Each request is bound to the thread whose tools it came from
and decided from what is true at that moment, read through `service::Host`:

- **The setting** is read for every request and again at every check. Off refuses
  everything; observe refuses input.
- **A running turn** is the thread's registered agent that has not been stopped; its run
  id is the turn id. Input outside a turn is refused, "allow for the rest of this turn"
  and the show-on-screen answer end with it, and when a run ends its open approvals are
  cancelled. Full access counts only when the thread has it now *and* had it when the run
  started, so loosening a thread never upgrades a turn that started stricter.
- **Provider approvals** are read from the running agents: any waiting request that is
  not a question for the user pauses all input; so does a list that cannot be read.
- **Approvals of the gate** are backend state. `ToolHost.approval_tools` was not used:
  it asks "Allow tool?" with the arguments, which would show typed text, and it cannot
  offer "the rest of this turn" or decide per target. A card is opened under the
  dispatch lock (never two at once), listed by `computer_use_approvals`, answered by
  `computer_use_respond`, and ends as a decline after 10 minutes, when its turn ends,
  when the tool call is dropped, when the thread is deleted or when computer use is
  turned off. An answer the card did not offer is a decline.
- **Input** runs `begin_input`, the show-on-screen question if needed, the best-effort
  read of what a coordinate hits, `decide_input`, the approval if needed, then
  `final_check` three times: once before the helper is called, and for the helper's own
  target before it prepares and immediately before input. That last check takes the
  dispatch lock and keeps it until the helper's next line. Each input returns a fresh
  screenshot of the window (after 350 ms), which becomes its newest shot.
- **Screenshots** are numbered PNGs under `computer-use/screenshots/<thread>` in the
  app's data folder; the newest 8 per thread are kept and a deleted thread's folder is
  removed. A capture is refused while a protected window overlaps, checked before and
  after it.
- **Recent actions**: the last 100 requests in memory, as command, outcome, effect and
  duration only.

## The helper process

`driver.rs` starts the helper on first use and speaks the protocol over its stdin and
stdout, one call at a time. A helper that exits, times out (per-op limits, 5 to 30 s) or
answers out of step is killed and replaced on the next call; its handles are then
`stale` with reason `restarted`, and the agent is told to list windows again. A helper
that died after it was allowed to click or drag is first told to release the mouse. A
helper is also replaced after 500 requests and after a status that does not report
Accessibility granted. A call whose caller went away before it was sent is never sent;
one already sent runs to its reply, with any check still to come refused.

The helper is looked for at `VIEWCODE_COMPUTER_DRIVER_BINARY`, then at
`computer-driver/viewcode-computer-driver` in the app's bundled resources, then beside
the app's executable. The release build does not bundle it yet, so today computer use
reports itself unavailable unless that variable points at a helper built from
`native/computer-driver`. Nothing is started on Windows or Linux.

## Settings, commands and tools

Settings are one JSON row (`computer_use_settings`, schema 39): `mode` (`off`,
`observe`, `control`; default off), `approvals` (`thread`, `risky`, `never`; default
thread) and `screen` (`allow`, `ask`; default allow). A stored value that cannot be read
counts as off, thread and ask.

| Command | Does |
| --- | --- |
| `computer_use_state` | `{settings, status, approvals, activity}`; `status` is `{mode, platform, driverAvailable, accessibility, reason?}` |
| `computer_use_set_settings` | Takes `{mode?, approvals?, screen?}`; turning it off closes every approval |
| `computer_use_approvals` | The waiting cards: `{requestId, threadId, turnId, appName, detail, options, createdAt}` |
| `computer_use_respond` | `sessionId`, `requestId`, `decision` (`accept`, `acceptForSession`, `decline`, `cancel`) |

`computer-use-changed` (`{sessionId}`) is emitted when settings change and when a card
opens or closes.

Agents get the tools from `tools::with_computer_use` when the setting is not off, the
platform is macOS and the helper is installed: `pandacode_computer_status`,
`_list_windows`, `_observe` and `_screenshot` under observe, and under control also
`_press`, `_set_value`, `_type`, `_key`, `_scroll`, `_click`, `_drag`, `_move`,
`_scroll_at`, `_type_focused` and `_focus`. Each is one of ViewCode's `viewcode-computer`
commands with the same arguments. A refusal is the tool's error, as
`CU-CON-008: <message> No input was sent.`. In Plan mode the input tools are refused.
Claude-family harnesses and Cline do not show their own permission prompt for these
tools (`providers::self_scoped_host_tool`): the gate asks, and a harness prompt would
show the arguments, typed text included. Other ACP harnesses may still show theirs.
A turn does not fail when the tools' transport cannot start.

## Ported and not ported

Ported: the modes, approvals and show-on-screen settings; request validation; the
protected-apps rule and the screenshot overlap check; the destructive heuristic; the
decisions (`admit`, `begin_input`, `decide_input`, `final_check`); the pending-approval
tracker; approval wording and options; the target tables; mapping of helper failures;
the launcher-command and screenshot-read recognisers; the protocol types; the helper
client; the service with its dispatch lock, approvals, screenshots and recent actions;
the walk of the app's process ancestry; the settings; the agent tools.

Not ported yet: every screen (settings, approval cards, recent actions), so nothing
shows a waiting approval to the user; bundling the helper into the macOS build; the
`viewcode-computer` launcher that refusals name, with its session credential; granting
a thread's screenshot folder to harnesses that take directory grants (an agent without
Full access may be asked before it reads a screenshot); input to windows that are not
in front, and the on-screen agent cursor that goes with it. The helper drives macOS
only.

## Differences from ViewCode

- The app's own windows are matched under `APP_NAME` as well as ViewCode's and T3 Code's
  names, which stay refused.
- Lengths in approval wording count characters, not UTF-16 units, so a clipped label
  never ends in half a character.
- A reply's `diagnostics` (windows that could not be enumerated) is read past, not kept.
- Agents call tools, not a command-line launcher, so there is no local HTTP route and no
  session credential: a tool is bound to its thread when the app hands it over.
  Refusals keep ViewCode's wording and still say `viewcode-computer <command>`.
- Under observe only the four looking tools are offered; ViewCode's launcher has every
  command and refuses input.
- A turn is a run of the thread's agent. ViewCode keeps a thread's windows and refs
  until its provider session stops; here they are kept until the thread is deleted or
  computer use is turned off, since each run starts a new harness process.
- A waiting approval notices that its turn ended by looking every half second, as well
  as by being told.
- Any helper line that is not a valid reply to the request in flight replaces the
  helper, including a reply with the right id and the wrong shape (ViewCode keeps the
  helper in that one case).
- The helper's lifecycle is not logged (the app has no log to write it to).
- Plan mode refuses input tools, as it refuses the app's other acting tools.
