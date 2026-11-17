// Starting a code review of local changes (docs/code-review.md): depth (Quick, Deep, Security), what to read,
// an optional focus, and, from a review's findings, a re-review that says which of them were fixed. The
// reviewer runs in a new thread on the source thread's checkout (or the project's folder) and changes
// nothing; `review_start_local` returns that thread. Loaded on first use; styles in review.css.
import { escape, icon } from './dom';
import { trackSegment } from './motion';
import { DEPTHS, FOCUS_MAX_CHARS, MODE_LABELS, type LocalScope, type ReviewMode } from './review-model';

export type ReviewStartOptions = {
  projectId: string;
  projectName: string;
  /** The thread whose checkout to review; absent, the project's folder. */
  sourceSessionId?: string;
  /** Where the review reads, in words: "this thread's worktree", "orbit". */
  where: string;
  provider: string;
  model: string;
  /** The agent and model, as the dialog names them. */
  agentLabel: string;
  /** Findings to re-review: the review thread and its findings record. */
  rereview?: { threadId: string; activityId: string };
  mode?: ReviewMode;
};
type Deps = {
  modal(title: string, body: string, cls?: string): HTMLDialogElement;
  closeModal(): void;
  call<T>(command: string, args: Record<string, unknown>): Promise<T>;
  /** The review thread exists: show it. */
  started(sessionId: string): Promise<void> | void;
};

type ScopeChoice = 'auto' | 'uncommitted' | 'branch';
const SCOPES: { id: ScopeChoice; label: string; detail: string }[] = [
  { id: 'auto', label: 'Auto', detail: 'Uncommitted changes, or the branch when everything is committed.' },
  { id: 'uncommitted', label: 'Uncommitted', detail: 'Only what is not committed yet.' },
  { id: 'branch', label: 'Branch', detail: 'Every commit on this branch, against its base.' },
];
const scopeOf = (choice: ScopeChoice): LocalScope | undefined => choice === 'uncommitted' ? { kind: 'uncommitted' } : choice === 'branch' ? { kind: 'branch' } : undefined;

const segment = (name: string, label: string, items: { id: string; label: string }[], selected: string) =>
  `<div class="ds-seg review-seg" role="group" aria-label="${escape(label)}" data-seg="${name}">${items.map(item => `<button type="button" data-value="${item.id}" aria-pressed="${item.id === selected}">${escape(item.label)}</button>`).join('')}</div>`;

/** The start dialog's markup. */
export function reviewStartHtml(options: ReviewStartOptions): string {
  const mode = options.mode ?? 'quick', depth = DEPTHS.find(item => item.mode === mode)!;
  const rereview = options.rereview ? `<div class="review-start-field" data-field="again"><span class="field-label" id="review-again-label">Re-review</span>${segment('again', 'Re-review', [{ id: 'again', label: 'Check again' }, { id: 'full', label: 'Full review' }], 'again')}<p class="review-start-hint" data-hint="again">Reads the change again and says which of the earlier findings were fixed.</p></div>` : '';
  return `<form id="review-start-form" class="review-start"><p class="dialog-description">A new thread reviews ${escape(options.where)} and reports what it finds. It changes nothing.</p>${rereview}<div class="review-start-field"><span class="field-label" id="review-depth-label">Depth</span>${segment('depth', 'Review depth', DEPTHS.map(item => ({ id: item.mode, label: item.label })), mode)}<p class="review-start-hint" data-hint="depth" aria-live="polite">${escape(depth.detail)}</p></div><div class="review-start-field" data-field="scope"${options.rereview ? ' hidden' : ''}><span class="field-label" id="review-scope-label">What to review</span>${segment('scope', 'What to review', SCOPES, 'auto')}<p class="review-start-hint" data-hint="scope" aria-live="polite">${escape(SCOPES[0].detail)}</p></div><div class="review-start-field"><label class="field-label" for="review-focus">Focus <span class="review-optional">optional</span></label><textarea id="review-focus" name="focus" rows="2" maxlength="${FOCUS_MAX_CHARS}" placeholder="What to look at hardest, for example the retry logic"></textarea></div><p class="review-start-agent">${icon('bot')}<span>Runs with ${escape(options.agentLabel)}</span></p><p class="review-start-error" role="alert" hidden></p><div class="dialog-footer review-start-foot"><button type="button" class="ds-btn" data-action="close-dialog">Cancel</button><button type="submit" class="ds-btn-primary review-start-submit">Start ${escape(MODE_LABELS[mode].toLowerCase())}</button></div></form>`;
}

/** Opens the start dialog. */
export function openReviewStart(deps: Deps, options: ReviewStartOptions): void {
  const dialog = deps.modal(options.rereview ? 'Review again' : 'Review changes', reviewStartHtml(options), 'compact-dialog review-start-dialog');
  const form = dialog.querySelector<HTMLFormElement>('#review-start-form')!;
  const value = (name: string) => form.querySelector<HTMLElement>(`[data-seg="${name}"] [aria-pressed="true"]`)?.dataset.value ?? '';
  const submit = form.querySelector<HTMLButtonElement>('.review-start-submit')!, error = form.querySelector<HTMLElement>('.review-start-error')!;
  for (const group of form.querySelectorAll<HTMLElement>('[data-seg]')) trackSegment(group, group.querySelector('[aria-pressed="true"]'));
  // The depth is the first choice, so it takes focus rather than the focus field.
  form.querySelector<HTMLElement>('[data-seg="depth"] [aria-pressed="true"]')?.focus();
  const sync = () => {
    const mode = value('depth') as ReviewMode, again = value('again') === 'again';
    submit.textContent = `Start ${MODE_LABELS[mode].toLowerCase()}`;
    form.querySelector<HTMLElement>('[data-hint="depth"]')!.textContent = DEPTHS.find(item => item.mode === mode)!.detail;
    form.querySelector<HTMLElement>('[data-hint="scope"]')!.textContent = SCOPES.find(item => item.id === value('scope'))?.detail ?? '';
    const againHint = form.querySelector<HTMLElement>('[data-hint="again"]');
    if (againHint) againHint.textContent = again ? 'Reads the change again and says which of the earlier findings were fixed.' : 'A fresh review of the whole change.';
    form.querySelector<HTMLElement>('[data-field="scope"]')!.hidden = Boolean(options.rereview) && again;
  };
  form.addEventListener('click', event => {
    const option = (event.target as Element).closest<HTMLButtonElement>('[data-seg] > button');
    if (!option) return;
    const group = option.parentElement!;
    for (const button of group.querySelectorAll('button')) button.setAttribute('aria-pressed', String(button === option));
    trackSegment(group, option);
    sync();
  });
  form.addEventListener('keydown', event => {
    const option = (event.target as Element).closest<HTMLButtonElement>('[data-seg] > button');
    if (!option || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
    event.preventDefault();
    const buttons = [...option.parentElement!.querySelectorAll<HTMLButtonElement>('button')];
    const next = buttons[(buttons.indexOf(option) + (event.key === 'ArrowRight' ? 1 : -1) + buttons.length) % buttons.length];
    next.focus(); next.click();
  });
  form.addEventListener('submit', event => {
    event.preventDefault();
    const mode = value('depth') as ReviewMode, again = Boolean(options.rereview) && value('again') === 'again';
    const focus = form.querySelector<HTMLTextAreaElement>('#review-focus')!.value.trim();
    const scope = again ? undefined : scopeOf(value('scope') as ScopeChoice);
    const input = {
      projectId: options.projectId, provider: options.provider, model: options.model, mode,
      ...(options.sourceSessionId ? { sourceSessionId: options.sourceSessionId } : {}),
      ...(scope ? { scope } : {}), ...(focus ? { focus } : {}),
      ...(again && options.rereview ? { basedOn: options.rereview } : {}),
    };
    submit.disabled = true; submit.textContent = 'Starting…'; error.hidden = true;
    void deps.call<{ sessionId: string }>('review_start_local', { input }).then(async result => {
      if (form.isConnected) deps.closeModal();
      await deps.started(result.sessionId);
    }).catch(failure => {
      if (!form.isConnected) return;
      submit.disabled = false; sync();
      error.textContent = failure instanceof Error ? failure.message : String(failure);
      error.hidden = false;
    });
  });
}
