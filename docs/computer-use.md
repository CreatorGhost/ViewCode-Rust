# Computer use

An agent sees the apps open on this machine and, if the user allows it, operates them.
Ported from ViewCode. Port status: [viewcode-port.md](viewcode-port.md).

Only the rules are here so far (`src-tauri/src/computer_use/`): pure functions and
tables with tests, called by nothing yet. The native helper they are written for is
`native/computer-driver`.

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

## Ported and not ported

Ported: the modes, approvals and show-on-screen settings; request validation; the
protected-apps rule and the screenshot overlap check; the destructive heuristic; the
decisions (`admit`, `begin_input`, `decide_input`, `final_check`); the pending-approval
tracker; approval wording and options; the target tables; mapping of helper failures;
the launcher-command and screenshot-read recognisers; the protocol types.

Not ported yet: the service that starts the helper and holds the dispatch lock, the
settings and their UI, approval cards, the agent tools (or the `viewcode-computer`
launcher the refusals name), the walk of the app's process ancestry beyond its parent,
the screenshot files and their pruning, the recent-actions list and the on-screen agent
cursor. The helper drives macOS only.

## Differences from ViewCode

- The app's own windows are matched under `APP_NAME` as well as ViewCode's and T3 Code's
  names, which stay refused.
- Lengths in approval wording count characters, not UTF-16 units, so a clipped label
  never ends in half a character.
- A reply's `diagnostics` (windows that could not be enumerated) is read past, not kept.
