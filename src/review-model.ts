// Code review (docs/code-review.md): the shapes `review_state` and `review_skill` return, and the
// pure rules the findings card, the start dialog and the review skill dialog draw from. No DOM here,
// so `tests/review-model.test.mjs` runs it in Node. The shapes mirror `src-tauri/src/review/findings.rs`
// and `review/skill.rs` (ViewCode's JSON).

export type Severity = 'blocker' | 'important' | 'nit' | 'question';
export type Verdict = 'approve' | 'approve-with-nits' | 'request-changes';
export type ReviewMode = 'quick' | 'deep' | 'security';
export type Rating = 'up' | 'down';
export type Evidence = { label: string; url: string };
export type FindingLesson = { id?: string; text: string; occurrences?: number; evidence?: Evidence[] };
export type Finding = {
  severity: Severity;
  /** 0-100: 100 is proven from the code, 50 plausible but unverified. */
  confidence: number;
  path: string;
  line?: number;
  endLine?: number;
  title: string;
  detail: string;
  suggestedFix?: string;
  rule?: string;
  preExisting?: boolean;
  lesson?: FindingLesson;
};
export type Target = { kind: 'local' } | { kind: 'pull-request'; host: string; repository: string; number: number; url: string };
export type LocalScope = { kind: 'uncommitted' } | { kind: 'branch'; base?: string } | { kind: 'commit'; sha: string };
export type BasedOn = { activityId: string; headSha?: string };
export type FindingsPayload = {
  verdict: Verdict;
  /** 0-5: 5 is safe to merge as is. */
  mergeConfidence: number;
  /** 1-5: how long a careful human review takes. */
  reviewEffort: number;
  findings: Finding[];
  needsChecking: Finding[];
  target: Target;
  mode?: 'fix' | 'comment';
  headSha?: string;
  incremental?: boolean;
  basedOn?: BasedOn;
  previous?: { fixed: number; stillOpen: number };
  /** false: the findings verified so far, sent while the review goes on. */
  complete?: boolean;
};
export type StartedPayload = { mode?: ReviewMode; focus?: string; scope?: LocalScope; headSha?: string; incremental?: boolean; basedOn?: BasedOn; target?: Target };
/** What `review_state {sessionId}` returns. */
export type ReviewState = {
  started: StartedPayload | null;
  findingsActivityId: string | null;
  findings: FindingsPayload | null;
  ratings: Record<string, Rating>;
  posted: Record<string, unknown>;
  fixStarted: unknown;
};
export type LessonSource = 'kept-breaking' | 'reviewers-ask' | 'yours';
export type Lesson = {
  id: string; text: string; where?: string; source: LessonSource; occurrences?: number; evidence: Evidence[];
  enabled: boolean; disabledByRatings?: boolean; helpful: number; unhelpful: number;
};
export type Skill = {
  projectId: string; status: 'learning' | 'learned'; learningThreadId: string | null; learnedAt: string | null;
  summary: string; lessons: Lesson[];
};
export type SkillEdit = { kind: 'set-enabled'; lessonId: string; enabled: boolean } | { kind: 'add'; text: string; where?: string } | { kind: 'remove'; lessonId: string };
export type FindingEntry = { key: string; finding: Finding };

export const SEVERITIES: readonly Severity[] = ['blocker', 'important', 'nit', 'question'];
export const SEVERITY_LABELS: Record<Severity, string> = { blocker: 'Blocker', important: 'Important', nit: 'Nit', question: 'Question' };
/** A severity's group heading, by how many it holds. */
export function severityHeading(severity: Severity, count: number): string {
  if (severity === 'important') return 'Important';
  return count === 1 ? SEVERITY_LABELS[severity] : `${SEVERITY_LABELS[severity]}s`;
}
export const VERDICT_LABELS: Record<Verdict, string> = { approve: 'Approve', 'approve-with-nits': 'Approve with nits', 'request-changes': 'Request changes' };
export const MODE_LABELS: Record<ReviewMode, string> = { quick: 'Quick review', deep: 'Deep review', security: 'Security audit' };
/** The depth choice's names and what each does, as the start dialog says it. */
export const DEPTHS: readonly { mode: ReviewMode; label: string; detail: string }[] = [
  { mode: 'quick', label: 'Quick', detail: 'One agent reads the change once and reports what it can prove.' },
  { mode: 'deep', label: 'Deep', detail: 'Several lenses read the change, and each finding is checked again before it is reported.' },
  { mode: 'security', label: 'Security', detail: 'An audit by attack class: injection, secrets, access, unsafe input and the like.' },
];
export const LESSON_SOURCE_LABELS: Record<LessonSource, string> = { 'kept-breaking': 'kept breaking', 'reviewers-ask': 'reviewers ask', yours: 'yours' };
/** Why a finding was not wanted, as the row under a thumbs-down offers it (ViewCode's words). */
export const RATING_REASONS: readonly string[] = ['Not a real problem', 'Intended here', 'Too minor', 'Handled elsewhere'];
export const LESSON_MAX_CHARS = 200;
export const FOCUS_MAX_CHARS = 2000;

/** `path:line` or `path:line-endLine`, as the report cites a location. */
export function findingLocation(finding: Pick<Finding, 'path' | 'line' | 'endLine'>): string {
  if (finding.line === undefined) return finding.path;
  if (finding.endLine !== undefined && finding.endLine !== finding.line) return `${finding.path}:${finding.line}-${finding.endLine}`;
  return `${finding.path}:${finding.line}`;
}

/** A Markdown code block holding `code` as it stands: the fence outgrows any backtick run inside it, and the
 * file's extension names the language when it looks like one (`findings::fenced_code`). */
export function fencedCode(code: string, path: string): string {
  const longest = Math.max(0, ...[...code.matchAll(/`+/g)].map(run => run[0].length));
  const fence = '`'.repeat(Math.max(3, longest + 1));
  const dot = path.lastIndexOf('.');
  const extension = dot >= 0 ? path.slice(dot + 1) : '';
  const language = /^[A-Za-z0-9]{1,10}$/.test(extension) && !extension.includes('/') ? extension : '';
  const lines = code.split('\n');
  while (lines.length > 1 && !lines[0].replace(/[ \t\r]/g, '')) lines.shift();
  return `${fence}${language}\n${lines.join('\n').trimEnd()}\n${fence}`;
}

/** The findings in runs of one severity, most severe first; each keeps its key (`finding:N`), which names its
 * place in the set the backend rates by. */
export function groupBySeverity(findings: readonly Finding[]): { severity: Severity; items: FindingEntry[] }[] {
  const entries = findings.map((finding, index) => ({ key: `finding:${index}`, finding }));
  return SEVERITIES.map(severity => ({ severity, items: entries.filter(entry => entry.finding.severity === severity) })).filter(group => group.items.length > 0);
}
/** The findings the reviewer could not settle, keyed `check:N`. */
export function checkingEntries(needsChecking: readonly Finding[]): FindingEntry[] {
  return needsChecking.map((finding, index) => ({ key: `check:${index}`, finding }));
}

/** "Re-review · 2 fixed · 1 still open" for findings of a re-review; null for a first review. */
export function rereviewSummary(findings: Pick<FindingsPayload, 'incremental' | 'previous'>): string | null {
  if (findings.previous) return `Re-review · ${findings.previous.fixed} fixed · ${findings.previous.stillOpen} still open`;
  return findings.incremental ? 'Re-review' : null;
}

export type ReviewPhase = 'none' | 'starting' | 'live' | 'done' | 'ended';
/** Where a review thread stands: no review; started and nothing sent yet; findings so far while it works;
 * its final set; or the reviewer stopped without sending a final set. */
export function reviewPhase(state: ReviewState | null, running: boolean): ReviewPhase {
  if (!state || (!state.started && !state.findings)) return 'none';
  if (!state.findings) return running ? 'starting' : 'ended';
  if (state.findings.complete === false) return running ? 'live' : 'ended';
  return 'done';
}

/** "3 issues" over findings and the items that need checking. */
export function issueCount(findings: Pick<FindingsPayload, 'findings' | 'needsChecking'>): string {
  const count = findings.findings.length + findings.needsChecking.length;
  return count === 0 ? 'No issues' : `${count} ${count === 1 ? 'issue' : 'issues'}`;
}
/** "Sent 2 findings so far", or that none have been sent yet. */
export function soFarText(findings: Pick<FindingsPayload, 'findings' | 'needsChecking'> | null): string {
  const count = findings ? findings.findings.length + findings.needsChecking.length : 0;
  return count === 0 ? 'No findings yet' : `${count} ${count === 1 ? 'finding' : 'findings'} so far`;
}

/** The rating a press leaves: pressing the rating a finding already has takes it back. */
export function nextRating(current: Rating | undefined, pressed: Rating): Rating | null {
  return current === pressed ? null : pressed;
}
/** The state with one finding's rating changed at once, before the backend answers. */
export function withRating(state: ReviewState, key: string, rating: Rating | null): ReviewState {
  const ratings = { ...state.ratings };
  if (rating) ratings[key] = rating; else delete ratings[key];
  return { ...state, ratings };
}

/** How findings that cited a lesson were rated, or why it is off. */
export function lessonStanding(lesson: Pick<Lesson, 'disabledByRatings' | 'helpful' | 'unhelpful'>): string {
  if (lesson.disabledByRatings) return 'off after your ratings';
  if (!lesson.helpful && !lesson.unhelpful) return 'not rated yet';
  return `${lesson.helpful} helpful, ${lesson.unhelpful} not`;
}
/** The skill's status line. */
export function skillSummary(skill: Skill | null, formatDate: (iso: string) => string = iso => new Date(iso).toLocaleDateString()): string {
  if (!skill) return 'Not learned yet. An agent reads how this project’s changes were reviewed and what had to be fixed after merging.';
  if (skill.status === 'learning') return 'Learning from this project’s history now.';
  const count = `${skill.lessons.length} ${skill.lessons.length === 1 ? 'lesson' : 'lessons'}`;
  if (!skill.learnedAt) return `Your own lessons · ${count}. Nothing was learned from the project’s history yet.`;
  return `Learned ${formatDate(skill.learnedAt)} · ${count}`;
}
/** A line of the scope a review read, from what its start recorded. */
export function scopeLabel(scope: LocalScope | undefined): string {
  if (!scope) return 'local changes';
  if (scope.kind === 'uncommitted') return 'uncommitted changes';
  if (scope.kind === 'branch') return scope.base ? `the branch against ${scope.base}` : 'the branch';
  return `commit ${scope.sha.slice(0, 7)}`;
}
/** Five dots of a 0-5 or 1-5 score, filled up to the score. */
export function scoreDots(score: number): boolean[] {
  const value = Math.max(0, Math.min(5, Math.round(score)));
  return Array.from({ length: 5 }, (_, index) => index < value);
}
