# Code review

An agent reviews a change in an ordinary thread and hands its findings back to the app.
Ported from ViewCode; the JSON shapes and record kinds are ViewCode's, so its chats
import without a translation. Port status: [viewcode-port.md](viewcode-port.md).

## How a review runs

1. The app creates a thread, records a `viewcode.review.started` record on it (depth,
   focus, scope, target, and for a re-review the findings it starts from) and sends the
   first message. That message is short: the depth, what to read, and the path of a
   playbook written to `<app data>/review-playbooks/`.
2. The reviewer follows the playbook (`quick-review.md`, `deep-review.md` or
   `security-audit.md`) and calls `pandacode_submit_review_findings`: with
   `complete: false` while it works, then once more with the final set. Every call
   carries the whole set and replaces the one before it.
3. The reader rates findings up or down. Later reviews of the project are pointed at a
   file of the last 30 rated findings.

Review depth: **Quick** is one agent and one pass, **Deep** is several lenses with
adversarial verification, **Security** is an audit by attack class.

## Rules worth knowing

- A reviewer cannot choose what its findings are about. The target, the head commit and
  the re-review link come from the started record; findings without one may only be
  about local changes.
- Local changes have nowhere to post to, so their findings are always for fixing.
- `submit_review_findings` is offered only on a thread the app started a review in.
- Ratings outlive their review thread; deleting a thread drops its other records.

## The project's review skill

What a project's history teaches a reviewer: one JSON file per project in
`<app data>/review-skills/`. It is the user's, not the repository's. A learning thread
reads the history (playbook `learn-review-skill.md`) and hands lessons over with
`pandacode_submit_review_skill`.

Every later review trusts that file, so only the thread the app asked to learn may
submit, once. Do not loosen this to "any thread of the project": an agent that reads a
hostile comment could then poison every review.

The user can add, remove and switch lessons. A learned lesson rated down three times
and never up is switched off once; the user's own choice is left alone after that.

## Data

`review_activities(id, session_id, project_id, kind, payload, created_at)` (schema 37),
append-only. Kinds: `viewcode.review.started`, `.findings`, `.rating`, `.posted`,
`.fix-started`.

## IPC

`review_state {sessionId}`, `review_rate_finding {sessionId, findingsActivityId, findingKey, rating, reason?}`,
`review_start_local {input}`, `review_skill {projectId}`, `review_edit_skill {projectId, edit}`,
`review_learn_skill {input}`. Changes emit `review-changed {sessionId, projectId}`.

## Code

`src-tauri/src/review.rs` (IPC), `review/findings.rs` (shapes and rules, no I/O),
`review/skill.rs`, `review/playbook.rs` and `review/playbooks/*.md`, `review/store.rs`,
`review/tools.rs`, `review/tests.rs`.
