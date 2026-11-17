// The findings card of a code review thread (docs/code-review.md): the verdict, merge confidence and review
// effort, the findings grouped by severity, the items that need checking, and thumbs to rate each finding.
// One card element per thread is kept and moved into the transcript's `#review-host` on every redraw, so a
// rating, a chosen reason and the rendered Markdown survive the transcript's innerHTML. Styles: review.css
// (always loaded, since the transcript draws this).
import { escape, icon, markdown } from './dom';
import {
  MODE_LABELS, RATING_REASONS, VERDICT_LABELS, checkingEntries, fencedCode, findingLocation, groupBySeverity, issueCount, nextRating,
  rereviewSummary, reviewPhase, scopeLabel, scoreDots, severityHeading, soFarText, withRating,
  type FindingEntry, type Rating, type ReviewPhase, type ReviewState, type Severity, type SkillEdit,
} from './review-model';

type Deps = {
  call<T>(command: string, args: Record<string, unknown>): Promise<T>;
  /** Whether the thread's agent is working now. */
  running(sessionId: string): boolean;
  /** The thread's project, for its review skill. */
  projectOf(sessionId: string): string | undefined;
  /** Opens a file of the thread's checkout at a line, in the file tabs. */
  openFile(sessionId: string, path: string, line?: number): void;
  /** Opens the start dialog to review the thread's changes again from these findings. */
  rereview(sessionId: string, activityId: string, anchor: HTMLElement): void;
  /** Opens the project's review skill. */
  openSkill(projectId: string, anchor: HTMLElement): void;
  notify(text: string): void;
  report(error: unknown): void;
};
type Entry = { state: ReviewState | null; card?: HTMLElement; painted?: string; loading?: Promise<void>; reasons: Map<string, string> };

const SEVERITY_ICONS: Record<Severity, string> = { blocker: 'blocker', important: 'alert', nit: 'info', question: 'help' };
const VERDICT_ICONS = { approve: 'check', 'approve-with-nits': 'check', 'request-changes': 'alert' } as const;
/** The findings card's markup for one phase of a review; `reasons` are the reasons chosen this session. */
export function reviewCardHtml(state: ReviewState, phase: ReviewPhase, reasons: ReadonlyMap<string, string>, hasSkill: boolean): string {
  const findings = state.findings, mode = state.started?.mode ?? 'quick';
  const scope = findings?.target.kind === 'pull-request' ? `pull request #${findings.target.number}` : scopeLabel(state.started?.scope);
  const status = phase === 'starting' ? 'Reviewing…'
    : phase === 'live' ? `Still reviewing · ${soFarText(findings)}`
    : phase === 'ended' ? (findings ? `Stopped early · ${soFarText(findings).replace(' so far', ' sent')}` : 'Stopped before sending findings')
    : findings ? issueCount(findings) : '';
  const working = phase === 'starting' || phase === 'live';
  const head = `<header class="review-card-head"><span class="review-card-mark" aria-hidden="true">${icon(mode === 'security' ? 'shield' : 'search')}</span><h3 class="review-card-title">${escape(MODE_LABELS[mode])}<span class="review-card-scope"> · ${escape(scope)}</span></h3><span class="review-card-status" role="status">${working ? '<span class="review-live-dot" aria-hidden="true"></span>' : ''}${escape(status)}</span></header>`;
  if (!findings) {
    const body = working ? '<p class="review-card-note">The reviewer is reading the change. Findings appear here as it verifies them.</p>' : '<p class="review-card-note">The reviewer ended without sending its findings. Review again to start over.</p>';
    return head + body + footHtml(state, phase, hasSkill);
  }
  const rereview = rereviewSummary(findings);
  const score = (label: string, value: number) => `<span class="review-score"><span class="review-score-label">${label}</span><span class="ds-dots" role="img" aria-label="${label} ${value} of 5">${scoreDots(value).map(on => `<span${on ? ' class="on"' : ''}></span>`).join('')}</span><span class="review-score-value" aria-hidden="true">${value}/5</span></span>`;
  const summary = `<div class="review-summary ds-card-inner"><div class="review-summary-row"><span class="review-verdict" data-verdict="${findings.verdict}"><span class="review-verdict-icon" aria-hidden="true">${icon(VERDICT_ICONS[findings.verdict])}</span>${escape(VERDICT_LABELS[findings.verdict])}</span>${score('Merge confidence', findings.mergeConfidence)}${score('Review effort', findings.reviewEffort)}</div>${rereview ? `<p class="review-rereview">${icon('refresh')}<span>${escape(rereview)}</span></p>` : ''}</div>`;
  const groups = groupBySeverity(findings.findings).map(group => groupHtml(severityHeading(group.severity, group.items.length), group.items, state, reasons, group.severity)).join('');
  const checking = findings.needsChecking.length ? groupHtml('Needs checking', checkingEntries(findings.needsChecking), state, reasons, 'checking') : '';
  const empty = !findings.findings.length && !findings.needsChecking.length ? `<p class="review-card-note">${phase === 'done' ? 'No issues found in this change.' : 'Nothing verified yet.'}</p>` : '';
  return head + summary + groups + checking + empty + footHtml(state, phase, hasSkill);
}

function footHtml(state: ReviewState, phase: ReviewPhase, hasSkill: boolean): string {
  const again = (phase === 'done' || phase === 'ended') && state.findingsActivityId && state.findings?.target.kind !== 'pull-request'
    ? `<button type="button" class="ds-btn review-again" data-review-action="rereview" aria-haspopup="dialog">${icon('refresh')}Review again</button>` : '';
  const skill = hasSkill ? `<button type="button" class="ds-btn" data-review-action="skill" aria-haspopup="dialog">${icon('spark')}Review skill</button>` : '';
  return again || skill ? `<footer class="review-card-foot">${again}${skill}</footer>` : '';
}

function groupHtml(heading: string, items: FindingEntry[], state: ReviewState, reasons: ReadonlyMap<string, string>, kind: Severity | 'checking'): string {
  const id = `review-group-${kind}`;
  return `<section class="review-group" data-group="${kind}" aria-labelledby="${id}"><h4 class="review-group-head" id="${id}">${escape(heading)}<span class="review-group-count">${items.length}</span></h4><ol class="review-findings ds-card-inner">${items.map(item => findingHtml(item, state, reasons)).join('')}</ol></section>`;
}

function findingHtml({ key, finding }: FindingEntry, state: ReviewState, reasons: ReadonlyMap<string, string>): string {
  const rating = state.ratings[key], reason = reasons.get(key);
  const location = findingLocation(finding);
  const chips = [finding.preExisting ? 'pre-existing' : '', finding.lesson?.occurrences ? `broke ${finding.lesson.occurrences}× here` : ''].filter(Boolean).map(text => `<span class="ds-chip ds-chip-text">${escape(text)}</span>`).join('');
  const rule = finding.rule ? `<blockquote class="review-rule"><span class="review-label">Rule</span>${escape(finding.rule)}</blockquote>` : '';
  const evidence = (finding.lesson?.evidence ?? []).map(item => `<a href="${escape(item.url)}" target="_blank" rel="noreferrer">${escape(item.label)}</a>`).join('');
  const lesson = finding.lesson ? `<p class="review-lesson">${icon('spark')}<span><span class="review-label">Lesson</span>${escape(finding.lesson.text)}</span>${evidence}</p>` : '';
  const fix = finding.suggestedFix ? `<div class="review-fix"><span class="review-label">Suggested fix</span><div class="markdown review-md" data-review-md="fix" data-md-pending data-source="${escape(fencedCode(finding.suggestedFix, finding.path))}"></div></div>` : '';
  const rate = (value: Rating, label: string) => `<button type="button" class="review-rate-button" data-review-action="rate" data-rating="${value}" data-focus-id="rate:${key}:${value}" aria-pressed="${rating === value}" aria-label="${label}">${icon(value === 'up' ? 'thumbUp' : 'thumbDown')}</button>`;
  const why = rating === 'down' ? reason
    ? `<p class="review-reason" role="status">${icon('check')}<span>${escape(reason)} · noted for this project</span><button type="button" class="review-link" data-review-action="reason-undo" data-focus-id="undo:${key}">Undo</button></p>`
    : `<div class="review-reasons" role="group" aria-label="Why not?"><span class="review-reasons-label" aria-hidden="true">Why not?</span>${RATING_REASONS.map(text => `<button type="button" class="ds-chip ds-chip-text review-reason-chip" data-review-action="reason" data-reason="${escape(text)}" data-focus-id="reason:${key}:${escape(text)}">${escape(text)}</button>`).join('')}${finding.lesson?.id ? `<button type="button" class="review-link" data-review-action="stop-lesson" data-lesson="${escape(finding.lesson.id)}">Stop checking this here</button>` : ''}</div>` : '';
  return `<li class="review-finding" data-key="${key}" data-severity="${finding.severity}"><div class="review-finding-head"><span class="review-sev-icon" data-severity="${finding.severity}" aria-hidden="true">${icon(SEVERITY_ICONS[finding.severity])}</span><span class="sr-only">${escape(finding.severity)}: </span><h5 class="review-finding-title">${escape(finding.title)}</h5><span class="review-confidence" aria-label="Confidence ${finding.confidence}%">${finding.confidence}%</span></div><div class="review-finding-body"><p class="review-where"><button type="button" class="review-location" data-review-action="open" data-path="${escape(finding.path)}"${finding.line ? ` data-line="${finding.line}"` : ''} data-focus-id="open:${key}" aria-label="Open ${escape(location)}">${icon('file')}<span>${escape(location)}</span></button>${chips}</p><div class="markdown review-md review-detail" data-review-md="detail" data-md-pending data-source="${escape(finding.detail)}"></div>${rule}${lesson}${fix}<div class="review-finding-foot"><span class="review-rate" role="group" aria-label="Rate “${escape(finding.title)}”">${rate('up', 'Helpful')}${rate('down', 'Not helpful')}</span></div>${why}</div></li>`;
}

/** Keeps one card per review thread and moves it into the transcript on each redraw. */
export function createReviewCards(deps: Deps) {
  const entries = new Map<string, Entry>();
  let shown = '';

  function entry(sessionId: string): Entry {
    let found = entries.get(sessionId);
    if (!found) { found = { state: null, reasons: new Map() }; entries.set(sessionId, found); }
    return found;
  }

  async function load(sessionId: string): Promise<void> {
    const found = entry(sessionId);
    try { found.state = await deps.call<ReviewState>('review_state', { sessionId }); }
    catch (error) { found.state = null; deps.report(error); }
    paint(sessionId);
  }

  function paint(sessionId: string, force = false): void {
    const found = entries.get(sessionId);
    if (!found) return;
    const phase = reviewPhase(found.state, deps.running(sessionId));
    const host = sessionId === shown ? document.querySelector<HTMLElement>('#review-host') : null;
    if (phase === 'none' || !found.state) { found.card?.remove(); if (host) host.hidden = true; return; }
    const projectId = deps.projectOf(sessionId);
    const signature = JSON.stringify([phase, found.state, [...found.reasons], Boolean(projectId)]);
    if (!found.card) found.card = createCard(sessionId);
    if (force || found.painted !== signature) {
      const card = found.card, focused = card.contains(document.activeElement) ? (document.activeElement as HTMLElement).dataset.focusId : undefined;
      card.dataset.phase = phase;
      card.setAttribute('aria-label', `${MODE_LABELS[found.state.started?.mode ?? 'quick']} findings`);
      card.innerHTML = reviewCardHtml(found.state, phase, found.reasons, Boolean(projectId));
      found.painted = signature;
      if (focused) card.querySelector<HTMLElement>(`[data-focus-id="${CSS.escape(focused)}"]`)?.focus({ preventScroll: true });
    }
    if (host && found.card.parentElement !== host) host.append(found.card);
    if (!host) return;
    host.hidden = false;
    // Markdown is drawn once the card is in the page: markdown() draws text it has seen at once and waits for the rest.
    for (const node of found.card.querySelectorAll<HTMLElement>('[data-md-pending]')) {
      node.removeAttribute('data-md-pending');
      void markdown(node, node.dataset.source ?? '').catch(deps.report);
    }
  }

  async function rate(sessionId: string, key: string, rating: Rating | null, reason?: string): Promise<void> {
    const found = entries.get(sessionId), state = found?.state;
    if (!found || !state?.findingsActivityId) return;
    const before = state;
    found.state = withRating(state, key, rating);
    if (rating !== 'down') found.reasons.delete(key);
    paint(sessionId);
    try {
      found.state = await deps.call<ReviewState>('review_rate_finding', { sessionId, findingsActivityId: state.findingsActivityId, findingKey: key, rating, ...(reason ? { reason } : {}) });
    } catch (error) {
      found.state = before;
      deps.report(error);
    }
    paint(sessionId);
  }

  function createCard(sessionId: string): HTMLElement {
    const card = document.createElement('section');
    card.className = 'ds-card review-card';
    card.dataset.reviewCard = sessionId;
    card.addEventListener('click', event => {
      const control = (event.target as Element).closest<HTMLElement>('[data-review-action]');
      if (!control || !card.contains(control)) return;
      const found = entries.get(sessionId), key = control.closest<HTMLElement>('[data-key]')?.dataset.key ?? '';
      switch (control.dataset.reviewAction) {
        case 'open': deps.openFile(sessionId, control.dataset.path!, control.dataset.line ? Number(control.dataset.line) : undefined); break;
        case 'rate': void rate(sessionId, key, nextRating(found?.state?.ratings[key], control.dataset.rating as Rating)); break;
        case 'reason': {
          const reason = control.dataset.reason!;
          found?.reasons.set(key, reason);
          void rate(sessionId, key, 'down', reason);
          break;
        }
        case 'reason-undo': found?.reasons.delete(key); paint(sessionId); card.querySelector<HTMLElement>(`[data-focus-id="rate:${CSS.escape(key)}:down"]`)?.focus(); break;
        case 'stop-lesson': {
          const projectId = deps.projectOf(sessionId), lessonId = control.dataset.lesson!;
          if (!projectId) break;
          found?.reasons.set(key, 'Stop checking this here');
          const edit: SkillEdit = { kind: 'set-enabled', lessonId, enabled: false };
          void rate(sessionId, key, 'down', 'Stop checking this here');
          void deps.call('review_edit_skill', { projectId, edit }).then(() => deps.notify('This lesson is off for later reviews of the project.'), deps.report);
          break;
        }
        case 'rereview': if (found?.state?.findingsActivityId) deps.rereview(sessionId, found.state.findingsActivityId, control); break;
        case 'skill': { const projectId = deps.projectOf(sessionId); if (projectId) deps.openSkill(projectId, control); break; }
      }
    });
    return card;
  }

  return {
    /** Shows the thread's card in the transcript's host, reading its review state the first time. */
    attach(host: HTMLElement | null, sessionId: string): void {
      shown = sessionId;
      if (!host) return;
      const found = entries.get(sessionId);
      if (!found) { entry(sessionId).loading = load(sessionId); host.hidden = true; return; }
      paint(sessionId);
    },
    /** `review-changed`: the thread's records (or none named) changed; read them again. */
    changed(sessionId: string | null | undefined): void {
      const ids = sessionId ? [sessionId] : [...entries.keys()];
      for (const id of ids) if (entries.has(id)) entry(id).loading = load(id);
    },
    /** The thread's run started or ended: its phase may differ. */
    refresh(sessionId: string): void { if (entries.has(sessionId)) paint(sessionId); },
    forget(sessionId: string): void { entries.get(sessionId)?.card?.remove(); entries.delete(sessionId); },
    /** The review state last read for a thread, if any. */
    state(sessionId: string): ReviewState | null { return entries.get(sessionId)?.state ?? null; },
  };
}
