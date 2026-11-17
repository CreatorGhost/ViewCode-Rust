# Side chats (`/btw`)

Ask a quick question about a thread while its agent keeps working. Type `/btw <question>` at the start of the prompt, or write in the inspector's **By the way** tab. The side chat starts with what the thread has said so far, answers in the panel, and never interrupts the thread: nothing is queued for it, steered into it or written to it.

## What a side chat is

- **A real session.** `sessions.kind = 'side'` with `sessions.parent_session_id` naming its thread (schema 40, `src-tauri/src/side_chat.rs`). It runs on its own agent process, in the thread's project, harness, model, permissions and checkout (a worktree thread shares its worktree path; nothing is forked).
- **A snapshot, not a feed.** Opening one copies the thread's newest saved user and assistant messages into it: at most 60 rows, 8 user turns, 24,000 characters in all and the last 8,000 of any one message. Tool rows, coordination turns and an unfinished reply are not copied. The copy is queued as the ordinary context handoff (`context_recovery`), so the side agent's first turn opens with a notice that this is a parent snapshot, that the parent may still be working and does not see the side chat. Later work in the thread is never added; ask in a new side chat for a fresh view.
- **Its own messages only.** The copied rows stay in the database for the agent's context but `get_messages` (`Store::message_page`) leaves them out for a side chat, so the panel shows only the exchange the user had there.
- **Hidden.** `Store::sessions()` (the sidebar, boards, tiles, the tray, the agents dock, phones and paired computers through `remote/host.rs`), `search_tasks` and the schedule tools' task list all skip `kind = 'side'`. Only `side_chat_list` returns them, to their own thread.
- **Expires.** A side chat with no activity for 24 hours is archived: `expire(store, now, busy)` takes the clock as an argument, compares it with the newest of `updated_at`, `last_prompt_at` and `last_response_at`, and keeps any chat that is running, waiting on the user, holding queued messages or still owning an agent process. A 10-minute loop (`side_chat::start`) and every `side_chat_list` call run it. An archived side chat is gone from its thread; its rows remain.
- **Deleted with its thread.** `task_deletion` removes a thread's side chats with it (a running one blocks the deletion like any running task).

`kind` and `parent_session_id` are general columns: `kind` is `thread` for everything else today, and a side chat that becomes a full thread keeps its parent link as lineage. `side_chats(session_id, inherited_through, created_at)` holds what only a side chat needs: the id up to which its rows are its parent's.

## IPC

| Command | Does |
| --- | --- |
| `side_chat_open { parentId, question? }` → `Session` | Opens a side chat with the snapshot. `question` only names it (one line, 48 characters). Refused beside a side chat, a team worker, or a task that runs on another computer or a cloud machine. |
| `side_chat_list { parentId }` → `Session[]` | The thread's live side chats, most recently active first, after expiring idle ones. |
| `side_chat_send { sessionId, text }` | Sends on the side chat's own agent process (the composer's `send_prompt` path). Refused for an expired chat. |
| `side_chat_stop { sessionId }` | Stops the side chat's run only. |
| `side_chat_close { sessionId }` | Archives an idle side chat now, as expiry would. |
| `side_chat_promote { sessionId }` → `Session` | "Open as full thread": `kind` becomes `thread`, so it is listed and shows the history it inherited. |

Streaming uses the same `agent-event` stream as every thread, keyed by the side chat's session id. `side-chats-changed { parentId, sessionId, reason: 'expired' | 'closed' | 'promoted' }` tells an open panel to read its list again.

## The panel

`src/btw-panel.ts` (styles in `src/btw.css`, always loaded, tokens only), mounted by `main.ts` as the inspector's `btw` view.

- **One renderer.** The panel has no renderer of its own. `main.ts` hands it the main transcript's functions through `BtwPanelHost`, each taking a session: `turnRowsHtml()` (one turn card per turn, tool traces, change cards, user messages), `paintSavedMarkdown()`, and `paintStream()` for the live turn (the live card, thinking, the step trace, streaming Markdown, errors, questions). The agent's events reach it through the one `onAgent` handler and the same `state.streams` entry the main transcript would use for that session.
- **Per token, only the live turn.** A `text`, `thinking`, `tool`, `message_start`, `usage` or `subagent` event repaints the live turn alone, once per animation frame (`paintLive`). The header, composer, queue, approvals and saved rows are redrawn only when a run starts or ends, or when one of them changes (`update`).
- **Following without measuring.** The log is registered with `followTranscriptBottom`, and a `ResizeObserver` pins a followed log after layout, as the main transcript does. Streaming Markdown paints therefore read no layout; scrolling up stops the follow.
- **Never in the main chat's way.** Streaming calls `focus()` on nothing and scrolls only the panel's own log. The panel takes focus only for the user's own action (a bare `/btw`, the tab, New side thread).
- **Where you left off.** Side chats live in the backend, so the panel comes back with the thread: the chat it last showed (`pandacode.sidechat.active` in `localStorage`, a per-viewer convenience read once and served from memory), else the thread's newest. An unsent question is kept per side chat (`state.drafts`), and before any exists per thread. The panel re-reads `side_chat_list` each time it opens.
- **Composer.** The main composer's pieces scoped to the side chat: model and effort (`modelTriggerHtml`, the same picker, saved with `configure_session`/`set_reasoning_effort`), permissions, Steer, Stop, the approval bar and the queue; slash commands and @ mentions mount on the side prompt. Picks made before a side chat exists are kept on a stand-in task and applied to the new chat before its first message. Service tier (`/fast`) is among the app commands that run for the side chat.
- **Header.** *New side thread* (an empty composer until the next question; the previous chat is kept until it expires), *Open as full thread*, *Close* (hides the panel; the chat is kept), and the status (*Working*, *Needs your input*, *Ready*, plus *· main running*). Each final answer has *Insert into main prompt*.
- **Refusals.** Attachments in a `/btw` prompt, bot conversations, and tasks that run elsewhere.

## Earlier `/btw` state

Before schema 40, `/btw` made an ordinary task titled `BTW: …` and remembered it in `localStorage['pandacode.btw']`. That key is removed at startup and not migrated: the tasks it pointed at are ordinary threads the user can see in the sidebar, and turning them into side chats would hide them and archive them a day later. They stay as they are.

## Tests

- `src-tauri/src/side_chat/tests.rs`: kind and parent storage, the schema step, the snapshot and its bounds, hidden from lists and search, own messages only, expiry against a given clock, a running parent left untouched, deletion.
- `src-tauri/src/providers/side_chat_stream_tests.rs`: a parent and a side chat as two real overlapping processes on the recorded stream protocol; events carry their own session; the side chat ends and is stopped alone.
- `tests/side-chat.spec.ts` and `tests/btw.spec.ts` (browser), `tests/btw.test.mjs` (picking rules, preferences).

Preview mocks: `side_chat_*` in `src/preview.ts`. `__sideChatHold` keeps a side run open so a test streams it with `__previewEmit('agent-event', …)` and ends it with `__sideChatFinish(sessionId, text)`; `__sideChatClock` (milliseconds) moves the expiry clock; `__sideChatSnapshot(sessionId)` returns the snapshot taken when that chat was opened.
