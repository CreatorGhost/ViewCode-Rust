// Browser preview mocks for code review (docs/code-review.md): every `review_*` command and the `review-changed`
// event, over sample data. `?review` adds three review threads to the orbit project: a finished Deep review with
// every part of a findings card, a Quick review still in progress, and a re-review that says what was fixed.
// Ratings and skill edits stay with the browser tab (sessionStorage), as native keeps them, so a reload shows them.
// Hooks: `__reviewStarts` (each review_start_local input), `__reviewSubmit(sessionId, final = true)` (the
// reviewer sends findings: the in-progress sample's final set, or a set for a review started here),
// `__reviewSkillFinish(projectId)` (the learning thread hands its lessons over), `__reviewFail[command]` (that
// command fails once with the given text).
import type { Message, Project, Session } from './types';
import type { Finding, FindingsPayload, Lesson, LocalScope, Rating, ReviewMode, ReviewState, Skill, SkillEdit, StartedPayload } from './review-model';

type Deps = {
  sessions(): Session[];
  projects(): Project[];
  messages: Record<string, Message[]>;
  invoke<T>(command: string, args: Record<string, unknown>): Promise<T>;
  emit(name: string, event: unknown): void;
};
type Thread = { started: StartedPayload; findings?: { id: string; payload: FindingsPayload } };

const STORE_KEY = 'pandacode.preview.review';
export const REVIEW_FULL = 'review-full', REVIEW_LIVE = 'review-live', REVIEW_AGAIN = 'review-again';
const LESSON_DEBOUNCE = 'search-input-is-debounced-before-it-queries';

const FULL: FindingsPayload = {
  verdict: 'request-changes', mergeConfidence: 2, reviewEffort: 3, target: { kind: 'local' }, mode: 'fix', complete: true,
  findings: [
    { severity: 'blocker', confidence: 92, path: 'src/lib/search-index.ts', line: 5, endLine: 7, title: 'Search lowercases the query once per item',
      detail: '`query.toLowerCase()` runs inside the `filter` callback, so a 10,000-item index lowercases the same query **10,000 times on every keystroke**. Hoist it above `items.filter`.',
      suggestedFix: '    search(query) {\n      const needle = query.toLowerCase();\n      return items.filter(item => item.title.toLowerCase().includes(needle));\n    }',
      rule: 'Hot paths allocate nothing per item (CONTRIBUTING.md).',
      lesson: { id: LESSON_DEBOUNCE, text: 'Search input is debounced before it queries', occurrences: 3, evidence: [{ label: '#412', url: 'https://github.com/example/orbit/pull/412' }, { label: '#437', url: 'https://github.com/example/orbit/pull/437' }] } },
    { severity: 'important', confidence: 81, path: 'src/components/search.ts', line: 3, title: 'The index is rebuilt on every render', preExisting: true,
      detail: 'The component calls `createSearchIndex(items)` in its body, so each render throws the old index away. The change makes renders more frequent, which makes this reachable on every keystroke.' },
    { severity: 'important', confidence: 74, path: 'src/components/search.ts', line: 9, title: 'An empty query returns every item',
      detail: 'Clearing the field shows the whole catalogue at once. The previous version showed recent searches instead.' },
    { severity: 'nit', confidence: 95, path: 'src/lib/search-index.ts', line: 1, title: 'The header comment names the old sample', detail: 'The first line still says “sample source”.' },
    { severity: 'question', confidence: 50, path: 'README.md', title: 'Is the debounce delay documented?', detail: 'The README describes search but not how long it waits after typing.' },
  ],
  needsChecking: [
    { severity: 'important', confidence: 45, path: 'src/lib/search-index.ts', line: 4, title: 'A missing title would throw',
      detail: 'If an item arrives without a title, `item.title.toLowerCase()` throws. The type says a title is required, but the API client does not check it.' },
  ],
};
const LIVE: FindingsPayload = {
  verdict: 'approve-with-nits', mergeConfidence: 4, reviewEffort: 2, target: { kind: 'local' }, mode: 'fix', complete: false,
  findings: [
    { severity: 'important', confidence: 77, path: 'src/components/search.ts', line: 6, title: 'Arrow keys skip the first result', detail: 'The highlighted index starts at 1 after a new query.' },
    { severity: 'nit', confidence: 90, path: 'src/components/search.ts', line: 2, title: 'Unused import', detail: '`Item` is imported and never used.' },
  ],
  needsChecking: [],
};
const LIVE_FINAL: Finding = { severity: 'question', confidence: 55, path: 'src/components/search.ts', line: 8, title: 'Should Escape clear the query?', detail: 'Escape closes the list but keeps the text; the old version cleared it.' };
const AGAIN: FindingsPayload = {
  verdict: 'approve-with-nits', mergeConfidence: 4, reviewEffort: 2, target: { kind: 'local' }, mode: 'fix', complete: true,
  incremental: true, basedOn: { activityId: 'review-act-full' }, previous: { fixed: 2, stillOpen: 1 },
  findings: [FULL.findings[1], FULL.findings[3]], needsChecking: [],
};
const SKILL: Skill = {
  projectId: 'project-1', status: 'learned', learningThreadId: null, learnedAt: '2026-11-03T10:00:00Z',
  summary: 'Reviews of orbit care most about search staying fast as the catalogue grows, and about routes shipping with tests. Style comments are rare.',
  lessons: [
    { id: 'migrations-must-be-reversible', text: 'Migrations must be reversible', where: 'db/migrations/**', source: 'yours', evidence: [], enabled: true, helpful: 0, unhelpful: 0 },
    { id: LESSON_DEBOUNCE, text: 'Search input is debounced before it queries', where: 'src/components/search*', source: 'kept-breaking', occurrences: 3, evidence: [{ label: '#412', url: 'https://github.com/example/orbit/pull/412' }, { label: '#437', url: 'https://github.com/example/orbit/pull/437' }], enabled: true, helpful: 0, unhelpful: 0 },
    { id: 'new-api-routes-have-an-integration-test-beside-them', text: 'New API routes have an integration test beside them', where: 'server/routes/**', source: 'reviewers-ask', evidence: [{ label: '#398', url: 'https://github.com/example/orbit/pull/398' }], enabled: true, helpful: 1, unhelpful: 1 },
    { id: 'shared-modules-use-named-exports', text: 'Shared modules use named exports', source: 'reviewers-ask', evidence: [], enabled: false, disabledByRatings: true, helpful: 0, unhelpful: 3 },
  ],
};
const LEARNED = { summary: 'Reviews of this project ask for small, tested changes and flag slow paths in search and lists.', lessons: [
  { text: 'Lists over 200 rows are virtualised', where: 'src/components/**', source: 'kept-breaking' as const, occurrences: 2 },
  { text: 'Every user-facing string goes through the copy file', source: 'reviewers-ask' as const },
] };

const titleFor = (mode: ReviewMode, scope?: LocalScope, branch?: string | null) => {
  const prefix = mode === 'quick' ? 'Review' : mode === 'deep' ? 'Deep review' : 'Security audit';
  if (!scope) return `${prefix}: local changes${branch ? ` on ${branch}` : ''}`;
  if (scope.kind === 'uncommitted') return `${prefix}: uncommitted changes`;
  if (scope.kind === 'branch') return `${prefix}: branch vs ${scope.base ?? 'its base'}`;
  return `${prefix}: commit ${scope.sha.slice(0, 7)}`;
};

/** The sample review threads `?review` adds, newest first. */
export function reviewSessions(now: number): Session[] {
  const base = { projectId: 'project-1', provider: 'claude' as const, model: 'sonnet', mode: 'full' as const, archived: false, worktreePath: null, resumeId: null };
  return [
    { ...base, id: REVIEW_LIVE, title: 'Review: local changes on main', status: 'running', createdAt: now - 20000, updatedAt: now + 9000, lastResponseAt: null },
    { ...base, id: REVIEW_AGAIN, title: 'Deep review: local changes on search-speed', status: 'idle', createdAt: now - 60000, updatedAt: now + 8000, lastResponseAt: now - 30000 },
    { ...base, id: REVIEW_FULL, title: 'Deep review: uncommitted changes', status: 'idle', createdAt: now - 120000, updatedAt: now + 7000, lastResponseAt: now - 90000 },
  ] as Session[];
}
/** Their messages: the short first message the app sends, and the reviewer's report. */
export function reviewMessages(now: number): Record<string, Message[]> {
  const ask = (sessionId: string, id: number, mode: string, at: number): Message => ({ id, sessionId, role: 'user', content: `${mode} of the local changes in /Users/you/Projects/orbit.\n\nRead the review playbook and follow it for these changes. Do not change my working tree, index or branch, and post nothing to the host.`, createdAt: at });
  return {
    [REVIEW_FULL]: [ask(REVIEW_FULL, 900001, 'Deep review', now - 120000), { id: 900002, sessionId: REVIEW_FULL, role: 'assistant', content: 'I read the five changed files with three lenses and checked each finding against the code. One blocker in the search loop; the rest are below.', createdAt: now - 90000 }],
    [REVIEW_AGAIN]: [ask(REVIEW_AGAIN, 900003, 'Deep review', now - 60000), { id: 900004, sessionId: REVIEW_AGAIN, role: 'assistant', content: 'Two of the three earlier findings are fixed. The index is still rebuilt on every render.', createdAt: now - 30000 }],
    [REVIEW_LIVE]: [ask(REVIEW_LIVE, 900005, 'Quick review', now - 20000)],
  };
}

type Saved = { ratings: Record<string, Record<string, Rating>>; skills: Record<string, Skill> };
function load(): Saved {
  try { const saved = JSON.parse(sessionStorage.getItem(STORE_KEY) ?? 'null'); if (saved && typeof saved === 'object') return { ratings: saved.ratings ?? {}, skills: saved.skills ?? {} }; } catch { /* a fresh tab */ }
  return { ratings: {}, skills: { 'project-1': structuredClone(SKILL) } };
}

export function createPreviewReview(deps: Deps) {
  const saved = load();
  const save = () => { try { sessionStorage.setItem(STORE_KEY, JSON.stringify(saved)); } catch { /* per page only */ } };
  const threads = new Map<string, Thread>([
    [REVIEW_FULL, { started: { mode: 'deep', scope: { kind: 'uncommitted' }, target: { kind: 'local' } }, findings: { id: 'review-act-full', payload: structuredClone(FULL) } }],
    [REVIEW_LIVE, { started: { mode: 'quick', target: { kind: 'local' } }, findings: { id: 'review-act-live', payload: structuredClone(LIVE) } }],
    [REVIEW_AGAIN, { started: { mode: 'deep', target: { kind: 'local' }, incremental: true, basedOn: { activityId: 'review-act-full' } }, findings: { id: 'review-act-again', payload: structuredClone(AGAIN) } }],
  ]);
  let message = 990000;
  const w = window as any;
  w.__reviewStarts ??= [];
  const session = (id: string) => deps.sessions().find(task => task.id === id);
  const state = (sessionId: string): ReviewState => {
    const thread = threads.get(sessionId);
    return { started: thread?.started ?? null, findingsActivityId: thread?.findings?.id ?? null, findings: thread?.findings?.payload ?? null, ratings: thread?.findings ? { ...saved.ratings[thread.findings.id] } : {}, posted: {}, fixStarted: null };
  };
  /** Counts every rating of findings that cited a lesson into it, as `skill::apply_ratings` does. */
  const countRatings = (projectId: string) => {
    const skill = saved.skills[projectId];
    if (!skill) return;
    const counts = new Map<string, { up: number; down: number }>();
    for (const [sessionId, thread] of threads) {
      if (session(sessionId)?.projectId !== projectId || !thread.findings) continue;
      for (const [key, rating] of Object.entries(saved.ratings[thread.findings.id] ?? {})) {
        const [list, index] = key.split(':');
        const lessonId = (list === 'check' ? thread.findings.payload.needsChecking : thread.findings.payload.findings)[Number(index)]?.lesson?.id;
        if (!lessonId) continue;
        const count = counts.get(lessonId) ?? { up: 0, down: 0 };
        if (rating === 'up') count.up++; else count.down++;
        counts.set(lessonId, count);
      }
    }
    for (const lesson of skill.lessons) {
      if (lesson.id === 'shared-modules-use-named-exports' || lesson.id === 'new-api-routes-have-an-integration-test-beside-them') continue; // sample counts from older reviews
      const count = counts.get(lesson.id) ?? { up: 0, down: 0 };
      lesson.helpful = count.up; lesson.unhelpful = count.down;
    }
  };
  const changed = (sessionId: string | null, projectId: string | null) => deps.emit('review-changed', { sessionId, projectId });
  const finishRun = (sessionId: string, text: string) => {
    const task = session(sessionId);
    if (task) { task.status = 'idle'; task.updatedAt = Date.now(); task.lastResponseAt = Date.now(); }
    (deps.messages[sessionId] ??= []).push({ id: ++message, sessionId, role: 'assistant', content: text, createdAt: Date.now() });
    deps.emit('sessions-changed', {});
  };

  w.__reviewSubmit = (sessionId: string, final = true) => {
    const thread = threads.get(sessionId);
    if (!thread) throw new Error('Not a review thread.');
    if (sessionId === REVIEW_LIVE) {
      const payload = thread.findings!.payload;
      if (final) { payload.findings = [...LIVE.findings, LIVE_FINAL]; payload.complete = true; }
      thread.findings = { id: final ? 'review-act-live-final' : thread.findings!.id, payload };
    } else {
      const findings = final ? [FULL.findings[2], FULL.findings[3]] : [FULL.findings[2]];
      const payload: FindingsPayload = { ...structuredClone(AGAIN), incremental: thread.started.incremental, basedOn: thread.started.basedOn, previous: thread.started.basedOn ? { fixed: 3, stillOpen: 1 } : undefined, findings, complete: final };
      if (!thread.started.basedOn) { delete payload.previous; delete payload.incremental; delete payload.basedOn; }
      thread.findings = { id: `review-act-${crypto.randomUUID()}`, payload };
    }
    if (final) finishRun(sessionId, 'The review is done; the findings are under this report.');
    changed(sessionId, null);
  };
  w.__reviewSkillFinish = (projectId: string) => {
    const skill = saved.skills[projectId];
    if (!skill || skill.status !== 'learning' || !skill.learningThreadId) throw new Error('Nothing is learning.');
    const yours = skill.lessons.filter(lesson => lesson.source === 'yours');
    const learned: Lesson[] = LEARNED.lessons.map(lesson => ({ id: lesson.text.toLowerCase().replace(/[^a-z0-9]+/g, '-'), evidence: [], enabled: true, helpful: 0, unhelpful: 0, ...lesson }));
    saved.skills[projectId] = { ...skill, status: 'learned', learnedAt: new Date().toISOString(), summary: LEARNED.summary, lessons: [...yours, ...learned] };
    save();
    finishRun(skill.learningThreadId, 'I read the project’s history and handed over what its reviews look for.');
    changed(skill.learningThreadId, null);
  };

  async function invoke(command: string, args: Record<string, any>): Promise<unknown> {
    const fail = w.__reviewFail?.[command];
    if (fail) { delete w.__reviewFail[command]; throw new Error(fail); }
    switch (command) {
      case 'review_state': return state(String(args.sessionId));
      case 'review_rate_finding': {
        const sessionId = String(args.sessionId), thread = threads.get(sessionId), task = session(sessionId);
        if (!task) throw new Error('The review thread was not found.');
        if (!thread?.findings || thread.findings.id !== args.findingsActivityId) throw new Error('That finding is not part of this review.');
        const [list, index] = String(args.findingKey).split(':');
        const finding = (list === 'check' ? thread.findings.payload.needsChecking : list === 'finding' ? thread.findings.payload.findings : [])[Number(index)];
        if (!finding) throw new Error('That finding is not part of this review.');
        const ratings = saved.ratings[thread.findings.id] ??= {};
        if (args.rating === 'up' || args.rating === 'down') ratings[args.findingKey] = args.rating; else delete ratings[args.findingKey];
        ((w.__reviewRatings ??= []) as unknown[]).push({ key: args.findingKey, rating: args.rating ?? null, reason: args.reason ?? null });
        countRatings(task.projectId); save();
        changed(sessionId, task.projectId);
        return state(sessionId);
      }
      case 'review_skill': {
        if (!deps.projects().some(project => project.id === args.projectId)) throw new Error('The project was not found.');
        return saved.skills[args.projectId] ?? null;
      }
      case 'review_edit_skill': {
        if (!deps.projects().some(project => project.id === args.projectId)) throw new Error('The project was not found.');
        const skill: Skill = saved.skills[args.projectId] ?? { projectId: args.projectId, status: 'learned', learningThreadId: null, learnedAt: null, summary: '', lessons: [] };
        const edit = args.edit as SkillEdit;
        if (edit.kind === 'add') {
          const text = String(edit.text ?? '').trim();
          if (!text || text.length > 200) throw new Error('A lesson is 1 to 200 characters.');
          if (skill.lessons.length >= 40) throw new Error('A review skill holds at most 40 lessons.');
          let id = text.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 56) || 'lesson';
          while (skill.lessons.some(lesson => lesson.id === id)) id += '-2';
          skill.lessons.unshift({ id, text, ...(edit.where ? { where: edit.where } : {}), source: 'yours', evidence: [], enabled: true, helpful: 0, unhelpful: 0 });
        } else {
          const lesson = skill.lessons.find(item => item.id === edit.lessonId);
          if (!lesson) throw new Error('That lesson is not part of this project’s review skill.');
          if (edit.kind === 'remove') skill.lessons = skill.lessons.filter(item => item !== lesson);
          else { lesson.enabled = edit.enabled; delete lesson.disabledByRatings; }
        }
        saved.skills[args.projectId] = skill; save();
        changed(null, args.projectId);
        return structuredClone(skill);
      }
      case 'review_learn_skill': {
        const input = args.input ?? {}, project = deps.projects().find(item => item.id === input.projectId);
        if (!project) throw new Error('The project was not found.');
        const current = saved.skills[project.id];
        if (current?.status === 'learning') return { sessionId: current.learningThreadId };
        const task = await deps.invoke<Session>('create_session', { projectId: project.id, provider: input.provider, model: input.model, mode: 'full' });
        Object.assign(session(task.id)!, { title: `Learn: how ${project.name} is reviewed`, draft: false, status: 'running', updatedAt: Date.now() });
        deps.messages[task.id] = [{ id: ++message, sessionId: task.id, role: 'user', content: 'Learn how this project is reviewed: read the playbook at learn-review-skill.md and hand over its lessons.', createdAt: Date.now() }];
        saved.skills[project.id] = { ...(current ?? { projectId: project.id, learnedAt: null, summary: '', lessons: [] }), status: 'learning', learningThreadId: task.id };
        save();
        deps.emit('sessions-changed', {});
        changed(task.id, project.id);
        return { sessionId: task.id };
      }
      case 'review_start_local': {
        const input = args.input ?? {}, project = deps.projects().find(item => item.id === input.projectId);
        if (!project) throw new Error('The review’s project was not found.');
        if (!input.provider) throw new Error('Choose an agent for the review.');
        if (typeof input.focus === 'string' && input.focus.length > 2000) throw new Error('The focus is too long.');
        const source = input.sourceSessionId ? session(input.sourceSessionId) : undefined;
        if (input.sourceSessionId && source?.projectId !== project.id) throw new Error('The thread whose changes were to be reviewed was not found in this project.');
        const basedOn = input.basedOn ? threads.get(input.basedOn.threadId)?.findings : undefined;
        if (input.basedOn && basedOn?.id !== input.basedOn.activityId) throw new Error('The earlier review to start from was not found, or reviewed something else.');
        w.__reviewStarts.push(structuredClone(input));
        const mode: ReviewMode = input.mode ?? 'quick';
        const task = await deps.invoke<Session>('create_session', { projectId: project.id, provider: input.provider, model: input.model, mode: source?.mode ?? 'full' });
        const title = titleFor(mode, input.scope, source?.worktreeBranch ?? null);
        Object.assign(session(task.id)!, { title, draft: false, status: 'running', updatedAt: Date.now(), worktreePath: source?.worktreePath ?? null, worktreeBranch: source?.worktreeBranch ?? null });
        const started: StartedPayload = { mode, target: { kind: 'local' }, ...(input.scope ? { scope: input.scope } : {}), ...(input.focus ? { focus: input.focus } : {}), ...(basedOn ? { incremental: true, basedOn: { activityId: basedOn.id } } : {}) };
        threads.set(task.id, { started });
        deps.messages[task.id] = [{ id: ++message, sessionId: task.id, role: 'user', content: `${mode === 'quick' ? 'Quick review' : mode === 'deep' ? 'Deep review' : 'Security audit'} of the local changes in ${project.path}.\n\nRead the review playbook and follow it for these changes. Do not change my working tree, index or branch, and post nothing to the host.${input.focus ? `\n\nFocus: ${input.focus}` : ''}`, createdAt: Date.now() }];
        deps.emit('sessions-changed', {});
        changed(task.id, project.id);
        return { sessionId: task.id, title };
      }
    }
    throw new Error(`Unknown preview command: ${command}`);
  }
  return { invoke };
}
