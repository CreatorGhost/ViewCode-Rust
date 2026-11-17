// A project's review skill (docs/code-review.md): what its history taught reviewers, with a switch per lesson,
// the user's own lessons, and the thread that learns it. Opened from the project's menu, the command palette
// and a findings card. Loaded on first use; styles in review.css.
import { escape, icon, isImeKey } from './dom';
import { LESSON_MAX_CHARS, LESSON_SOURCE_LABELS, lessonStanding, skillSummary, type Lesson, type Skill, type SkillEdit } from './review-model';

type Deps = {
  modal(title: string, body: string, cls?: string): HTMLDialogElement;
  call<T>(command: string, args: Record<string, unknown>): Promise<T>;
  /** The agent and model a learning thread runs with, or why there is none. */
  learner(): { provider: string; model: string };
  /** Shows a thread (the learning one). */
  openThread(sessionId: string): Promise<void> | void;
};
type Open = { projectId: string; root: HTMLElement; skill: Skill | null; busy: boolean; deps: Deps };
let open: Open | undefined;

function lessonHtml(lesson: Lesson, busy: boolean): string {
  const evidence = lesson.evidence.map(item => `<a href="${escape(item.url)}" target="_blank" rel="noreferrer">${escape(item.label)}</a>`).join('');
  const where = lesson.where ? `<span class="review-lesson-where">${escape(lesson.where)}</span>` : '';
  const remove = lesson.source === 'yours' ? `<button type="button" class="icon-button review-lesson-remove" data-skill-action="remove" aria-label="Remove “${escape(lesson.text)}”"${busy ? ' disabled' : ''}>${icon('trash')}</button>` : '';
  return `<li class="ds-row review-lesson${lesson.enabled ? '' : ' ds-row-dim'}" data-lesson="${escape(lesson.id)}"><span class="ds-row-main"><span class="ds-row-title">${escape(lesson.text)}</span><span class="ds-row-sub review-lesson-meta"><span class="ds-chip ds-chip-text" data-source="${lesson.source}">${LESSON_SOURCE_LABELS[lesson.source]}</span>${where}${evidence}<span class="review-lesson-standing">${escape(lessonStanding(lesson))}</span></span></span>${remove}<input type="checkbox" class="ds-toggle" role="switch" data-skill-action="toggle" aria-label="Use “${escape(lesson.text)}”"${lesson.enabled ? ' checked' : ''}${busy ? ' disabled' : ''}></li>`;
}

/** The dialog's body for a skill (or none yet). */
export function reviewSkillHtml(projectName: string, skill: Skill | null, busy = false): string {
  const learning = skill?.status === 'learning';
  const learnLabel = !skill || !skill.learnedAt ? 'Learn now' : 'Learn again';
  const thread = skill?.learningThreadId ? `<button type="button" class="ds-btn" data-skill-action="thread">${learning ? 'Watch' : 'How it was learned'}</button>` : '';
  const status = `<div class="ds-card review-skill-status"><div class="ds-row"><span class="ds-row-icon review-skill-icon"${learning ? ' data-learning' : ''}>${icon('spark')}</span><span class="ds-row-main"><span class="ds-row-title">What reviews of ${escape(projectName)} look for</span><span class="ds-row-sub" role="status">${learning ? '<span class="review-live-dot" aria-hidden="true"></span>' : ''}${escape(skillSummary(skill))}</span></span></div>${skill?.summary ? `<p class="review-skill-summary">${escape(skill.summary)}</p>` : ''}<div class="review-skill-actions">${thread}<button type="button" class="ds-btn-primary" data-skill-action="learn"${busy || learning ? ' disabled' : ''}>${learnLabel}</button></div></div>`;
  const lessons = skill?.lessons ?? [];
  const on = lessons.filter(lesson => lesson.enabled).length;
  const list = lessons.length ? `<section class="ds-card review-skill-lessons" aria-labelledby="review-lessons-head"><div class="ds-section-head"><h3 id="review-lessons-head">Lessons</h3><span class="ds-row-meta">${on} of ${lessons.length} on</span></div><ul class="ds-card-inner">${lessons.map(lesson => lessonHtml(lesson, busy)).join('')}</ul></section>` : '';
  const add = `<form class="review-lesson-add"><input type="text" name="lesson" maxlength="${LESSON_MAX_CHARS}" autocomplete="off" aria-label="A lesson of your own" placeholder="Add a lesson of your own, e.g. migrations must be reversible"${busy ? ' disabled' : ''}><button type="submit" class="ds-btn"${busy ? ' disabled' : ''}>Add</button></form>`;
  return `${status}${list}${add}<p class="review-skill-error" role="alert" hidden></p>`;
}

function paint(state: Open, projectName: string, error = ''): void {
  const focused = state.root.contains(document.activeElement) ? document.activeElement as HTMLElement : null;
  const focusKey = focused ? `${focused.closest<HTMLElement>('[data-lesson]')?.dataset.lesson ?? ''}|${focused.dataset.skillAction ?? (focused instanceof HTMLInputElement && focused.name) ?? ''}` : '';
  const draft = state.root.querySelector<HTMLInputElement>('.review-lesson-add input')?.value ?? '';
  state.root.innerHTML = reviewSkillHtml(projectName, state.skill, state.busy);
  const input = state.root.querySelector<HTMLInputElement>('.review-lesson-add input')!;
  input.value = draft;
  if (error) { const alert = state.root.querySelector<HTMLElement>('.review-skill-error')!; alert.textContent = error; alert.hidden = false; }
  if (focusKey) {
    const [lesson, action] = focusKey.split('|');
    const scope = lesson ? state.root.querySelector<HTMLElement>(`[data-lesson="${CSS.escape(lesson)}"]`) : state.root;
    const target = action === 'lesson' ? input : scope?.querySelector<HTMLElement>(`[data-skill-action="${CSS.escape(action)}"]`);
    (target ?? input).focus({ preventScroll: true });
  }
}

/** Opens a project's review skill. */
export async function openReviewSkill(deps: Deps, project: { id: string; name: string }): Promise<void> {
  const dialog = deps.modal(`Review skill · ${project.name}`, `<div class="review-skill" data-project-id="${escape(project.id)}"><p class="review-skill-loading" role="status">Loading…</p></div>`, 'review-skill-dialog');
  const root = dialog.querySelector<HTMLElement>('.review-skill')!;
  const state: Open = { projectId: project.id, root, skill: null, busy: false, deps };
  open = state;
  const run = async (work: () => Promise<Skill | null | void>) => {
    state.busy = true; paint(state, project.name);
    let error = '';
    try { const next = await work(); if (next !== undefined) state.skill = next; }
    catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
    state.busy = false;
    if (root.isConnected) paint(state, project.name, error);
  };
  const edit = (change: SkillEdit) => run(() => deps.call<Skill>('review_edit_skill', { projectId: project.id, edit: change }));
  root.addEventListener('click', event => {
    const control = (event.target as Element).closest<HTMLElement>('[data-skill-action]');
    if (!control || control.matches('input')) return;
    const lessonId = control.closest<HTMLElement>('[data-lesson]')?.dataset.lesson;
    switch (control.dataset.skillAction) {
      case 'learn': void run(async () => {
        const learner = deps.learner();
        await deps.call('review_learn_skill', { input: { projectId: project.id, auto: false, ...learner } });
        return deps.call<Skill | null>('review_skill', { projectId: project.id });
      }); break;
      case 'thread': if (state.skill?.learningThreadId) void deps.openThread(state.skill.learningThreadId); break;
      case 'remove': if (lessonId) void edit({ kind: 'remove', lessonId }); break;
    }
  });
  root.addEventListener('change', event => {
    const toggle = (event.target as Element).closest<HTMLInputElement>('[data-skill-action="toggle"]');
    const lessonId = toggle?.closest<HTMLElement>('[data-lesson]')?.dataset.lesson;
    if (toggle && lessonId) void edit({ kind: 'set-enabled', lessonId, enabled: toggle.checked });
  });
  root.addEventListener('submit', event => {
    event.preventDefault();
    const input = root.querySelector<HTMLInputElement>('.review-lesson-add input')!, text = input.value.trim();
    if (!text) return;
    void edit({ kind: 'add', text }).then(() => { if (!root.querySelector('.review-skill-error:not([hidden])')) { const field = root.querySelector<HTMLInputElement>('.review-lesson-add input'); if (field) { field.value = ''; field.focus(); } } });
  });
  // The Enter that commits an IME composition must not add the lesson.
  root.addEventListener('keydown', event => { if (event.key === 'Enter' && isImeKey(event)) event.preventDefault(); });
  let error = '';
  try { state.skill = await deps.call<Skill | null>('review_skill', { projectId: project.id }); }
  catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
  if (root.isConnected && open === state) paint(state, project.name, error);
  root.querySelector<HTMLElement>('[data-skill-action="learn"]:not(:disabled), .review-lesson-add input')?.focus();
}

/** `review-changed` for a project: the open dialog reads its skill again (a learning thread finished, a rating counted). */
export async function refreshReviewSkill(projectId: string | null | undefined, projectName: string): Promise<void> {
  const state = open;
  if (!state || !state.root.isConnected || state.busy || (projectId && projectId !== state.projectId)) return;
  try { state.skill = await state.deps.call<Skill | null>('review_skill', { projectId: state.projectId }); }
  catch { return; }
  if (state.root.isConnected && open === state && !state.busy) paint(state, projectName);
}
