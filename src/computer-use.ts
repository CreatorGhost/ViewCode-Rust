import './computer-use.css';
import { escape, icon } from './dom';
import type { AgentEvent } from './types';
import { BYPASS_KEEPS, BYPASS_REMOVES, approvalsFor, approvalVariant, ago, commandWord, computerCommand, isRefusal, outcomeLabel, VARIANT_TITLE, type ComputerUseApproval, type ComputerUseDecision, type ComputerUseState } from './computer-use-model';

/* Computer use in the app shell (always loaded; the settings page is `computer-use-settings.ts`, loaded with
 * Settings). One store of what `computer_use_state` last said, kept fresh by `computer-use-changed`:
 * an event naming a thread is an approval opening or closing, so only the approvals are read again; one
 * without a thread is a settings change, so everything is. Nothing is polled, and while computer use is off
 * reading the state starts no helper. It draws three things for main.ts to place: the approval card above
 * the prompt of the thread whose agent asked, the "using the computer" strip while a turn calls the gate's
 * tools, and the bypass indicator in the header. */

export interface ComputerUseHost {
  call<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen(callback: (event: { sessionId?: string | null } | null) => void): Promise<unknown>;
  /** Something drawn here changed: the threads whose card or row changed, or every thread (`null`). */
  changed(threads: string[] | null): void;
  /** An approval arrived for a thread that is not open. */
  arrived(approval: ComputerUseApproval): void;
}

export interface ComputerUse {
  start(): Promise<void>;
  /** Reads everything again (`full`) or only the waiting approvals. */
  refresh(full?: boolean): Promise<void>;
  state(): ComputerUseState | null;
  bypass(): boolean;
  /** An approval of the gate waits in this thread. */
  waiting(threadId: string): boolean;
  /** The approval cards for a thread, or ''. */
  cardHtml(threadId: string): string;
  respond(threadId: string, requestId: string, decision: ComputerUseDecision): Promise<void>;
  /** A tool event of any thread: the gate's own tools light the thread's strip. */
  toolEvent(event: AgentEvent): void;
  /** The thread's run ended: its strip goes and its cards are read again. */
  turnEnded(threadId: string): void;
  /** The "using the computer" strip for a thread, or ''. */
  activityHtml(threadId: string): string;
  toggleActivity(threadId: string): void;
}

/** The body of the bypass confirmation (main.ts opens it in the shared dialog): what it removes, what stays,
 * and a checkbox that must be ticked before the confirm button works. */
export function bypassDialogBody(): string {
  return `<form id="cu-bypass-form" class="cu-bypass-form"><p class="dialog-description">Agents will operate this computer with no safeguards, at your own risk.</p>
    <p class="cu-bypass-lead">Turning this on removes:</p><ul class="cu-bypass-list">${BYPASS_REMOVES.map(line => `<li>${escape(line)}</li>`).join('')}</ul>
    <p class="cu-bypass-keeps">${escape(BYPASS_KEEPS)}</p>
    <label class="cu-bypass-ack"><input type="checkbox" id="cu-bypass-ack"><span>I understand that agents can then approve their own prompts and operate any app.</span></label>
    <div class="dialog-footer"><button type="button" class="secondary-button" data-action="close-dialog">Cancel</button><button type="submit" class="secondary-button cu-bypass-confirm" disabled>Turn on bypass</button></div></form>`;
}
/** Wires the confirmation: resolves true only when the box is ticked and the confirm button pressed. */
export function confirmBypassIn(dialog: HTMLDialogElement, close: () => void): Promise<boolean> {
  const form = dialog.querySelector<HTMLFormElement>('#cu-bypass-form')!, ack = form.querySelector<HTMLInputElement>('#cu-bypass-ack')!, confirm = form.querySelector<HTMLButtonElement>('.cu-bypass-confirm')!;
  ack.addEventListener('change', () => { confirm.disabled = !ack.checked; });
  form.querySelector<HTMLButtonElement>('[data-action="close-dialog"]')!.focus();
  return new Promise(resolve => {
    dialog.addEventListener('close', () => resolve(false), { once: true });
    form.addEventListener('submit', event => {
      event.preventDefault(); event.stopPropagation();
      if (!ack.checked) return;
      resolve(true); close();
    });
  });
}

const empty = (): ComputerUseState => ({ settings: { mode: 'off', approvals: 'thread', screen: 'allow' }, bypass: false, status: { mode: 'off', platform: '', driverAvailable: false, accessibility: 'unknown' }, approvals: [], activity: [] });
/** Recent actions shown under the strip, newest first. */
const STRIP_ROWS = 6;

export function createComputerUse(host: ComputerUseHost): ComputerUse {
  let current: ComputerUseState | null = null;
  const busy = new Set<string>();
  const errors = new Map<string, string>();
  const seen = new Set<string>();
  /** Per thread, this turn's calls of the gate's tools: how many, and the newest command. */
  const live = new Map<string, { calls: Set<string>; count: number; command: string; running: boolean }>();
  const expanded = new Set<string>();
  let reading: Promise<void> | null = null, again = false;

  const threadsOf = (approvals: ComputerUseApproval[]) => approvals.map(approval => approval.threadId);
  function applyApprovals(approvals: ComputerUseApproval[]): string[] {
    const before = threadsOf(current?.approvals ?? []);
    const state = current ??= empty();
    state.approvals = approvals;
    for (const approval of approvals) {
      if (seen.has(approval.requestId)) continue;
      seen.add(approval.requestId);
      host.arrived(approval);
    }
    const open = new Set(approvals.map(approval => approval.requestId));
    for (const id of [...errors.keys()]) if (!open.has(id)) errors.delete(id);
    return [...new Set([...before, ...threadsOf(approvals)])];
  }

  async function read(full: boolean): Promise<void> {
    if (full) {
      const next = await host.call<ComputerUseState>('computer_use_state');
      const wasBypass = current?.bypass, threads = applyApprovals(next.approvals ?? []);
      current = { ...next, approvals: current!.approvals };
      host.changed(wasBypass === next.bypass ? threads : null);
    } else host.changed(applyApprovals(await host.call<ComputerUseApproval[]>('computer_use_approvals')));
  }
  /** Reads run one at a time; a full read asked for while one runs follows it. */
  async function refresh(full = true): Promise<void> {
    if (reading) { again ||= full; return reading; }
    reading = read(full).catch(() => undefined).finally(() => { reading = null; });
    await reading;
    if (again) { again = false; await refresh(true); }
  }

  function card(approval: ComputerUseApproval): string {
    const variant = approvalVariant(approval), working = busy.has(approval.requestId), error = errors.get(approval.requestId);
    const warning = approval.options.find(option => option.warning)?.warning;
    let primary = false;
    const buttons = approval.options.map(option => {
      const quiet = isRefusal(option.decision) || primary;
      if (!quiet) primary = true;
      return `<button type="button" class="${quiet ? 'ds-btn' : 'ds-btn-primary'}" data-action="cu-respond" data-thread="${escape(approval.threadId)}" data-request="${escape(approval.requestId)}" data-decision="${escape(option.decision)}" ${working ? 'disabled' : ''}>${escape(option.label)}</button>`;
    }).join('');
    const tone = variant === 'destructive' ? 'danger' : 'near', glyph = variant === 'destructive' ? 'alert' : variant === 'screen' ? 'monitor' : 'select';
    const title = `cu-title-${escape(approval.requestId)}`;
    return `<section class="cu-approval ds-card" data-variant="${variant}" data-request="${escape(approval.requestId)}" role="group" aria-labelledby="${title}">
      <div class="cu-approval-head">${icon(glyph, 'cu-approval-mark').replace('<svg ', `<svg data-state-tone="${tone}" `)}<span class="cu-approval-app">${escape(approval.appName || 'Computer use')}</span></div>
      <div class="ds-card-inner cu-approval-body"><p class="cu-approval-title" id="${title}">${VARIANT_TITLE[variant]}</p><p class="cu-approval-detail">${escape(approval.detail)}</p>${warning ? `<p class="cu-approval-warning">${escape(warning)}</p>` : ''}${error ? `<p class="cu-approval-error" role="alert">${escape(error)}</p>` : ''}<div class="cu-approval-actions">${buttons}</div></div>
    </section>`;
  }

  async function respond(threadId: string, requestId: string, decision: ComputerUseDecision): Promise<void> {
    const approval = current?.approvals.find(item => item.requestId === requestId && item.threadId === threadId);
    // Only a decision the card offered is sent; anything else would be a decline anyway.
    if (!approval || busy.has(requestId) || !approval.options.some(option => option.decision === decision)) return;
    busy.add(requestId); errors.delete(requestId); host.changed([threadId]);
    try {
      await host.call('computer_use_respond', { sessionId: threadId, requestId, decision });
      current!.approvals = current!.approvals.filter(item => item.requestId !== requestId);
    } catch (failure) {
      errors.set(requestId, failure instanceof Error ? failure.message : String(failure));
    } finally {
      busy.delete(requestId); host.changed([threadId]);
      // Answered elsewhere, timed out or ended: the backend's list is the truth.
      void refresh(false);
    }
  }

  function activityHtml(threadId: string): string {
    const run = live.get(threadId);
    if (!run) return '';
    const open = expanded.has(threadId);
    const rows = open ? (current?.activity ?? []).filter(entry => entry.threadId === threadId).slice(0, STRIP_ROWS) : [];
    const now = Date.now();
    const list = open ? `<ol class="cu-activity-list" aria-label="Recent computer actions">${rows.length ? rows.map(entry => `<li class="${entry.outcome === 'ok' ? '' : 'cu-activity-refused'}"><span class="cu-activity-command">${escape(entry.command)}</span><span class="cu-activity-outcome">${escape(outcomeLabel(entry))}</span><time>${ago(entry.at, now)}</time></li>`).join('') : '<li class="cu-activity-empty">No computer actions recorded yet.</li>'}</ol>` : '';
    const word = run.running ? commandWord(run.command) : 'Using the computer';
    return `<div class="cu-activity${open ? ' is-open' : ''}"><button type="button" class="cu-activity-line" data-action="cu-activity" data-thread="${escape(threadId)}" aria-expanded="${open}">${icon('monitor', 'cu-activity-mark').replace('<svg ', `<svg data-state-tone="${current?.bypass ? 'danger' : 'running'}" `)}<span class="cu-activity-word">${escape(word)}</span><span class="cu-activity-count">${run.count} ${run.count === 1 ? 'action' : 'actions'}${current?.bypass ? ' · bypass on' : ''}</span>${icon('chevron', 'cu-activity-chevron')}</button>${list}</div>`;
  }

  return {
    async start() {
      await host.listen(event => void refresh(!event?.sessionId));
      await refresh(true);
    },
    refresh,
    state: () => current,
    bypass: () => Boolean(current?.bypass),
    waiting: threadId => Boolean(current?.approvals.some(approval => approval.threadId === threadId)),
    cardHtml: threadId => {
      const approvals = approvalsFor(current?.approvals ?? [], threadId);
      return approvals.length ? `<div class="cu-approvals" aria-live="polite">${approvals.map(card).join('')}</div>` : '';
    },
    respond,
    toolEvent(event) {
      const name = event.tool?.name ?? '', command = computerCommand(name);
      if (!command) return;
      const run = live.get(event.sessionId) ?? { calls: new Set<string>(), count: 0, command, running: true };
      run.calls.add(event.tool!.id);
      run.count = run.calls.size; run.command = command; run.running = event.tool!.status === 'running';
      live.set(event.sessionId, run);
      host.changed([event.sessionId]);
    },
    turnEnded(threadId) {
      const had = live.delete(threadId) || expanded.delete(threadId);
      if (had) host.changed([threadId]);
      if (current?.approvals.some(approval => approval.threadId === threadId)) void refresh(false);
    },
    activityHtml,
    toggleActivity(threadId) {
      if (expanded.delete(threadId)) { host.changed([threadId]); return; }
      expanded.add(threadId); host.changed([threadId]);
      // The list is read when it opens, never polled.
      void refresh(true);
    },
  };
}
