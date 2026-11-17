import './computer-use-settings.css';
import { escape, icon } from './dom';
import { trackSegment } from './motion';
import { APPROVAL_CHOICES, BYPASS_MODE_DESCRIPTION, MODE_DESCRIPTION, MODES, SCREEN_CHOICES, SCREEN_RECORDING_GUIDANCE, ago, available, describeStatus, outcomeLabel, type ComputerUseMode, type ComputerUsePatch, type ComputerUseState } from './computer-use-model';

/* Settings → Computer use (loaded with Settings). What this computer can do (the status row says the helper and
 * Accessibility state, with the pane to open when it is off; Screen Recording is only known when a screenshot is
 * refused, so its row is guidance), the mode as a three-way segment, when input asks, whether agents may take
 * the screen, the recent actions, and,
 * apart from all of it, Bypass all permissions: turning it on goes through a confirmation and sends
 * `acknowledgeRisk: true`; turning it off needs nothing. While bypass is on a standing warning heads the page. */

export interface ComputerUseSettingsHost {
  call<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  /** `computer-use-changed`; returns its cleanup. */
  listen(callback: () => void): Promise<() => void>;
  /** The bypass confirmation: true only on an explicit confirm. */
  confirmBypass(): Promise<boolean>;
  /** A thread's title for the recent actions, if it is known. */
  threadTitle(id: string): string | undefined;
}

const RECENT_ROWS = 12;

export function mountComputerUseSettings(root: HTMLElement, host: ComputerUseSettingsHost): () => void {
  let state: ComputerUseState | null = null;
  let error = '', loadError = '', saving = false, checking = false, disposed = false, drawn = '';
  let unlisten: (() => void) | undefined;

  const statusCard = (current: ComputerUseState) => {
    const status = describeStatus(current.status);
    const tone = status.tone ? ` data-state-tone="${status.tone}"` : '';
    const mac = current.status.driverAvailable && current.status.mode !== 'off';
    const permission = (name: string, value: string, detail: string, tone: string) => `<div class="ds-row"><span class="ds-row-icon">${icon('shield').replace('<svg ', `<svg${tone ? ` data-state-tone="${tone}"` : ''} `)}</span><span class="ds-row-main"><span class="ds-row-title">${name}</span><span class="ds-row-sub cu-wrap">${detail}</span></span><span class="ds-row-meta">${value}</span></div>`;
    return `<section class="ds-section" aria-labelledby="cu-status-head"><div class="ds-section-head"><h3 id="cu-status-head">This computer</h3></div>
      <div class="ds-card"><div class="ds-card-inner">
        <div class="ds-row cu-status" data-available="${available(current.status)}"><span class="ds-row-icon">${icon('monitor').replace('<svg ', `<svg${tone} `)}</span><span class="ds-row-main"><span class="ds-row-title">${escape(status.title)}</span><span class="ds-row-sub cu-wrap">${escape(status.detail)}</span></span><span class="ds-row-trail"><button type="button" class="ds-btn" data-cu="check" ${checking ? 'disabled' : ''}>${checking ? 'Checking…' : 'Check again'}</button></span></div>
        ${mac ? permission('Screen Recording', 'Asked at first screenshot', escape(SCREEN_RECORDING_GUIDANCE), '') : ''}
      </div></div></section>`;
  };

  const modeSection = (current: ComputerUseState) => {
    const can = available(current.status), mode = current.settings.mode;
    return `<section class="ds-section" aria-labelledby="cu-mode-head"><div class="ds-section-head"><h3 id="cu-mode-head">Mode</h3></div>
      <div class="ds-seg cu-mode" role="group" aria-labelledby="cu-mode-head">${MODES.map(item => `<button type="button" data-cu="mode" data-value="${item.id}" aria-pressed="${mode === item.id}" ${!can && item.id !== 'off' && mode !== item.id ? 'disabled' : ''} ${saving ? 'disabled' : ''}>${item.label}</button>`).join('')}</div>
      <p class="cu-note" id="cu-mode-note">${escape(current.bypass && mode !== 'off' ? BYPASS_MODE_DESCRIPTION : MODE_DESCRIPTION[mode])}</p></section>`;
  };

  const choices = <T extends string>(id: string, title: string, items: { id: T; label: string; detail: string }[], value: T | null) => `<section class="ds-section" aria-labelledby="cu-${id}-head"><div class="ds-section-head"><h3 id="cu-${id}-head">${title}</h3></div>
    <div class="ds-card"><div class="ds-card-inner" role="radiogroup" aria-labelledby="cu-${id}-head">${items.map(item => `<button type="button" class="ds-row cu-choice" role="radio" data-cu="${id}" data-value="${item.id}" aria-checked="${value === item.id}" ${saving ? 'disabled' : ''}><span class="ds-row-main"><span class="ds-row-title">${item.label}</span><span class="ds-row-sub cu-wrap">${item.detail}</span></span><span class="ds-row-trail">${value === item.id ? icon('check', 'cu-check') : ''}</span></button>`).join('')}</div></div></section>`;

  const recent = (current: ComputerUseState) => {
    const rows = current.activity.slice(0, RECENT_ROWS), now = Date.now();
    return `<section class="ds-section" aria-labelledby="cu-recent-head"><div class="ds-section-head"><h3 id="cu-recent-head">Recent actions</h3></div>
      <div class="ds-card"><div class="ds-card-inner cu-recent">${rows.length ? rows.map(entry => {
        const title = host.threadTitle(entry.threadId);
        return `<div class="ds-row cu-recent-row"><span class="ds-row-main"><span class="ds-row-title"><code>${escape(entry.command)}</code></span><span class="ds-row-sub${entry.outcome === 'ok' ? '' : ' ds-danger'}">${escape(outcomeLabel(entry))}${title ? ` · ${escape(title)}` : ''}</span></span><span class="ds-row-meta">${ago(entry.at, now)}</span></div>`;
      }).join('') : '<div class="ds-row"><span class="ds-row-main"><span class="ds-row-sub">No computer actions yet.</span></span></div>'}</div>
      <div class="ds-card-foot"><span class="cu-note">Newest first. Typed text and targets are never recorded.</span></div></div></section>`;
  };

  const bypassSection = (current: ComputerUseState) => {
    const on = current.bypass, can = available(current.status);
    return `<section class="ds-section cu-danger-zone" aria-labelledby="cu-bypass-head"><div class="ds-section-head"><h3 id="cu-bypass-head">Danger zone</h3></div>
      <div class="ds-card"><div class="ds-card-inner"><div class="ds-row"><span class="ds-row-icon">${icon('alert').replace('<svg ', '<svg data-state-tone="danger" ')}</span><span class="ds-row-main"><label class="ds-row-title" for="cu-bypass">Bypass all permissions</label><span class="ds-row-sub cu-wrap" id="cu-bypass-sub">Removes every approval and safeguard, protected apps included. At your own risk.</span></span><input type="checkbox" class="ds-toggle" role="switch" id="cu-bypass" data-cu="bypass" aria-describedby="cu-bypass-sub" ${on ? 'checked' : ''} ${saving || (!on && !can) ? 'disabled' : ''}></div></div></div></section>`;
  };

  const warning = () => `<div class="cu-bypass-warning ds-card" role="status"><div class="ds-row"><span class="ds-row-icon">${icon('alert').replace('<svg ', '<svg data-state-tone="danger" ')}</span><span class="ds-row-main"><span class="ds-row-title">Bypass all permissions is on</span><span class="ds-row-sub cu-wrap">Agents operate this computer without asking, including password managers, system security dialogs and this app’s approval prompts.</span></span><span class="ds-row-trail"><button type="button" class="ds-btn" data-cu="bypass-off" ${saving ? 'disabled' : ''}>Turn off</button></span></div></div>`;

  function draw(): void {
    if (disposed) return;
    let markup: string;
    if (!state) markup = loadError ? `<p class="settings-embed-error" role="alert">${escape(loadError)}</p>` : '<p class="dialog-loading">Loading…</p>';
    else {
      const control = state.settings.mode === 'control' && !state.bypass;
      markup = `${state.bypass ? warning() : ''}${statusCard(state)}${modeSection(state)}${control ? choices('approvals', 'Ask before input', APPROVAL_CHOICES, state.settings.approvals === 'bypass' ? null : state.settings.approvals) + choices('screen', 'Show on screen', SCREEN_CHOICES, state.settings.screen) : ''}${error ? `<p class="settings-embed-error cu-error" role="alert">${escape(error)}</p>` : ''}${recent(state)}${bypassSection(state)}`;
    }
    if (markup === drawn) return;
    drawn = markup;
    const active = document.activeElement instanceof HTMLElement && root.contains(document.activeElement) ? document.activeElement : null;
    const key = active?.dataset.cu ? `[data-cu="${active.dataset.cu}"]${active.dataset.value ? `[data-value="${active.dataset.value}"]` : ''}` : '';
    root.innerHTML = `<div class="cu-settings">${markup}</div>`;
    const seg = root.querySelector<HTMLElement>('.cu-mode');
    if (seg) trackSegment(seg, seg.querySelector<HTMLElement>('[aria-pressed="true"]'));
    if (key) (root.querySelector<HTMLElement>(key) ?? root.querySelector<HTMLElement>('[data-cu="bypass"]'))?.focus({ preventScroll: true });
  }

  async function load(): Promise<void> {
    try { state = await host.call<ComputerUseState>('computer_use_state'); loadError = ''; }
    catch (failure) { if (!state) loadError = `Couldn’t read computer use: ${failure instanceof Error ? failure.message : String(failure)}`; }
    draw();
  }

  async function save(patch: ComputerUsePatch): Promise<void> {
    if (saving) return;
    saving = true; error = ''; draw();
    try { await host.call('computer_use_set_settings', { settings: patch }); }
    catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
    saving = false;
    await load();
  }

  const click = (event: MouseEvent) => {
    const target = (event.target as Element | null)?.closest<HTMLElement>('[data-cu]');
    if (!target || !root.contains(target) || target instanceof HTMLInputElement || (target as HTMLButtonElement).disabled) return;
    const value = target.dataset.value ?? '';
    switch (target.dataset.cu) {
      case 'check': checking = true; draw(); void load().finally(() => { checking = false; draw(); }); break;
      case 'mode': if (value !== state?.settings.mode) void save({ mode: value as ComputerUseMode }); break;
      case 'approvals': if (value !== state?.settings.approvals) void save({ approvals: value as ComputerUsePatch['approvals'] }); break;
      case 'screen': if (value !== state?.settings.screen) void save({ screen: value as ComputerUsePatch['screen'] }); break;
      case 'bypass-off': void save({ approvals: 'thread' }); break;
    }
  };
  const change = async (event: Event) => {
    const input = event.target instanceof HTMLInputElement && event.target.dataset.cu === 'bypass' ? event.target : null;
    if (!input || !state) return;
    if (!input.checked) { await save({ approvals: 'thread' }); return; }
    // Nothing turns on until the confirmation says so; the switch stays off meanwhile.
    input.checked = false;
    const confirmed = await host.confirmBypass();
    if (disposed) return;
    if (confirmed) await save({ approvals: 'bypass', acknowledgeRisk: true });
    else { drawn = ''; draw(); }
  };

  root.addEventListener('click', click);
  root.addEventListener('change', change);
  void host.listen(() => { if (!saving) void load(); }).then(cleanup => { if (disposed) cleanup(); else unlisten = cleanup; }).catch(() => undefined);
  draw();
  void load();
  return () => {
    disposed = true; unlisten?.();
    root.removeEventListener('click', click); root.removeEventListener('change', change);
    root.replaceChildren();
  };
}
