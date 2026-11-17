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

## Screens

- **Starting a review** (`src/review-start.ts`, a dialog loaded on first use): depth as a segment
  (Quick, Deep, Security), what to read (Auto, Uncommitted, Branch: `scope`), an optional focus, and
  the agent it runs with (the source thread's, else the default for new tasks). It calls
  `review_start_local` with the open thread as `sourceSessionId` (its worktree, else the project
  folder) and opens the new thread. Entry points: the **Review** button in the Changes tab's
  header, **Review my changes…** in the command palette, **Review changes…** in a project's menu
  (rail tile or folder heading), **Review my changes** on an empty thread, and **Review again** on a
  finished findings card, which adds a Re-review segment (Check again sends `basedOn`; Full review is
  a fresh one) and keeps the earlier depth.
- **Findings card** (`src/review-card.ts`, styles in the always-loaded `src/review.css`): drawn after
  the turns of any thread whose `review_state` has a started record or findings. One card element per
  thread is kept and moved into `#review-host` on every transcript redraw, so ratings, chosen reasons
  and rendered Markdown survive. Header: depth, scope, and the phase (`Reviewing…`, `Still reviewing ·
  N findings so far` with a pulsing dot, `N issues`, or `Stopped early`). Then the verdict, merge
  confidence and review effort as five-dot meters, the re-review line (`Re-review · 2 fixed · 1 still
  open`), the findings grouped by severity (Blocker, Important, Nit, Question; the colour is on the
  leading icon only), and **Needs checking**. A finding shows its title, confidence, `path:line` (opens
  the file in a file tab with that line marked and scrolled to), chips (pre-existing, `broke N× here`),
  the detail and the suggested fix through the shared Markdown renderer, the quoted rule and the lesson
  it came from. `review-changed` re-reads the thread's state, so findings arrive live.
- **Rating**: Helpful / Not helpful (`aria-pressed`, pressing again takes it back with `rating: null`),
  shown at once and saved by `review_rate_finding`. Not helpful offers ViewCode's reasons, sent as
  `reason`; a finding that cites a lesson also offers **Stop checking this here**, which switches that
  lesson off (`review_edit_skill`). The chosen reason shows for the session only (`review_state` does
  not return reasons).
- **Review skill** (`src/review-skill.ts`, a dialog): from a project's menu, the command palette
  (**Review skill**) and a findings card. Shows the status (not learned, learning with a pulsing dot,
  learned on a date), the summary, each lesson with its source, where it applies, evidence links and
  its ratings (`2 helpful, 1 not`, `off after your ratings`), a switch per lesson, remove for your own,
  and a field to add one. **Learn now / Learn again** calls `review_learn_skill` (never `auto`);
  **Watch / How it was learned** opens the learning thread.

Different from ViewCode: no Review side panel with the diff (the card sits in the transcript and the
Changes tab already shows the diff), no Dismiss, no Previous/Next, no Fix with agent, posting or pull
request reviews (no backend), and no automatic learning of new projects (it spends the user's plan
and has no setting here).

Preview: `?review` (`src/preview-review.ts`) adds a finished Deep review, a Quick review in progress
and a re-review to orbit. Spec: `tests/review.spec.ts`; helpers: `tests/review-model.test.mjs`.

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
