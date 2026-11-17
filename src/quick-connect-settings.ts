/* Settings → Connections → Remote access: the Phone section (ViewCode's ConnectPhoneSettingsSection) and the
 * phones paired through Quick connect (its Authorized clients), then the older GitHub-based PandaCode Connect,
 * mounted only while `status.legacyConnect.visible` (always true for a computer still signed in to it). */
import './quick-connect.css';
import { escape, icon } from './dom';
import { describeQuickConnectStatus, expiryText, lastSeenText, resolveView } from './quick-connect-logic';
import { mountRemoteSettings } from './remote-settings';
import type { QuickConnectDevice, QuickConnectMode, QuickConnectStatus } from './quick-connect-types';
import type { RemoteHost } from './remote-types';

export function mountQuickConnectSettings(root: HTMLElement, host: RemoteHost): () => void {
  let status: QuickConnectStatus | undefined, error = '', loadError = '', busy = false, disposed = false, revoking: string | null = null;
  let legacyCleanup: (() => void) | undefined;
  const message = (e: unknown) => e instanceof Error ? e.message : String(e);
  root.innerHTML = '<div class="qc-settings"></div><div class="qc-legacy" hidden><h3 class="settings-heading">PandaCode Connect (GitHub)</h3><div class="qc-legacy-body"></div></div>';
  const main = root.querySelector<HTMLElement>('.qc-settings')!, legacy = root.querySelector<HTMLElement>('.qc-legacy')!;
  const legacyBody = legacy.querySelector<HTMLElement>('.qc-legacy-body')!;
  const connectPhone = (mode?: QuickConnectMode) => host.connectPhone?.(mode);

  function syncLegacy(): void {
    const visible = !!status?.legacyConnect.visible;
    legacy.hidden = !visible;
    if (visible && !legacyCleanup) legacyCleanup = mountRemoteSettings(legacyBody, host);
    else if (!visible && legacyCleanup) { legacyCleanup(); legacyCleanup = undefined; legacyBody.replaceChildren(); }
  }
  function quickControl(state: QuickConnectStatus): string {
    const view = resolveView(state);
    if (state.setup.status === 'running') return `<button type="button" class="ds-btn" data-qcs="show">Show</button>`;
    if (!state.relay) return `<button type="button" class="ds-btn" data-qcs="set-up">Set up</button>`;
    const on = view !== 'off';
    return `<input type="checkbox" role="switch" class="ds-toggle" data-qcs="toggle" aria-label="Quick connect"${on ? ' checked' : ''}${busy ? ' disabled' : ''}>`;
  }
  function deviceRow(device: QuickConnectDevice, now: number): string {
    const name = escape(device.name);
    if (revoking === device.deviceId) {
      return `<div class="ds-row qc-device qc-device-confirm" role="group" aria-label="Revoke ${name}?"><span class="ds-row-icon">${icon('phone')}</span><span class="ds-row-main"><span class="ds-row-title">Revoke ${name}?</span><span class="ds-row-sub">It stops reaching this computer at once and must pair again with a new code.</span></span><span class="ds-row-trail"><button type="button" class="ds-btn" data-qcs="revoke-cancel">Cancel</button><button type="button" class="ds-btn qc-danger" data-qcs="revoke-confirm" data-id="${escape(device.deviceId)}"${busy ? ' disabled' : ''}>Revoke</button></span></div>`;
    }
    const seen = lastSeenText(device, now), ends = expiryText(device.expiresAt, now);
    const mark = icon('phone').replace('<svg', device.online ? '<svg data-state-tone="done"' : '<svg');
    return `<div class="ds-row qc-device" data-device="${escape(device.deviceId)}"><span class="ds-row-icon" title="${device.online ? 'Online' : 'Offline'}">${mark}</span><span class="ds-row-main"><span class="ds-row-title">${name}</span><span class="ds-row-sub">${escape(seen)} · ${escape(ends)}</span></span><span class="ds-row-trail"><button type="button" class="ds-btn" data-qcs="revoke" data-id="${escape(device.deviceId)}" aria-label="Revoke ${name}"${busy ? ' disabled' : ''}>Revoke</button></span></div>`;
  }
  function render(): void {
    if (disposed) return;
    const focused = document.activeElement instanceof HTMLElement && main.contains(document.activeElement) ? document.activeElement : undefined;
    const keep = focused?.dataset.qcs ? `[data-qcs="${CSS.escape(focused.dataset.qcs)}"]${focused.dataset.id ? `[data-id="${CSS.escape(focused.dataset.id)}"]` : ''}` : '';
    if (!status) {
      main.innerHTML = loadError ? `<div class="qc-alert" role="alert">${icon('alert').replace('<svg', '<svg data-state-tone="danger"')}<p>Could not load Quick connect. ${escape(loadError)}</p></div><div class="qc-actions"><button type="button" class="ds-btn" data-qcs="reload">Try again</button></div>` : '<p class="dialog-loading" role="status">Loading…</p>';
      return;
    }
    const now = Date.now(), view = resolveView(status), setup = status.setup;
    const tone = view === 'ready' ? ' data-state-tone="done"' : view === 'unreachable' || setup.status === 'failed' || setup.status === 'remove-failed' ? ' data-state-tone="warn"' : '';
    const accounts = setup.status === 'needs-account' ? `<div class="qc-settings-extra"><p class="qc-lead">Which Cloudflare account should hold the relay?</p><div class="qc-actions">${setup.accounts.map(account => `<button type="button" class="ds-btn" data-qcs="account" data-id="${escape(account.id)}"${busy ? ' disabled' : ''}>${escape(account.name)}</button>`).join('')}<button type="button" class="ds-btn qc-ghost" data-qcs="cancel">Cancel</button></div></div>` : '';
    const devices = status.devices;
    main.innerHTML = `<section class="ds-section" aria-labelledby="qc-phone-heading"><div class="ds-section-head"><h3 id="qc-phone-heading">Phone</h3></div>
      <div class="ds-card"><div class="ds-card-inner">
        <div class="ds-row"><span class="ds-row-icon">${icon('phone')}</span><span class="ds-row-main"><span class="ds-row-title">Connect phone</span><span class="ds-row-sub">Scan a pairing code with the PandaCode phone app. No account needed on the phone.</span></span><span class="ds-row-trail"><button type="button" class="ds-btn-primary" data-qcs="connect">Connect phone</button></span></div>
        <div class="ds-row"><span class="ds-row-icon">${icon('globe').replace('<svg', `<svg${tone}`)}</span><span class="ds-row-main"><span class="ds-row-title">Quick connect</span><span class="ds-row-sub qc-status-line">${escape(describeQuickConnectStatus(status))}</span></span><span class="ds-row-trail">${quickControl(status)}</span></div>
        ${accounts}
      </div></div></section>
      <section class="ds-section" aria-labelledby="qc-devices-heading"><div class="ds-section-head"><h3 id="qc-devices-heading">Paired phones</h3></div>
      <div class="ds-card"><div class="ds-card-inner qc-devices">${devices.length ? devices.map(device => deviceRow(device, now)).join('') : '<p class="qc-quiet qc-empty">No phones yet. Choose Connect phone to pair one.</p>'}</div></div></section>
      ${error ? `<div class="qc-alert" role="alert">${icon('alert').replace('<svg', '<svg data-state-tone="danger"')}<p>${escape(error)}</p></div>` : ''}`;
    if (keep) main.querySelector<HTMLElement>(keep)?.focus({ preventScroll: true });
  }
  function apply(next: QuickConnectStatus): void {
    status = next;
    if (revoking && !next.devices.some(device => device.deviceId === revoking)) revoking = null;
    syncLegacy(); render();
  }
  async function act(command: string, args?: Record<string, unknown>): Promise<void> {
    busy = true; error = ''; render();
    try { const next = await host.call<QuickConnectStatus>(command, args); if (!disposed) apply(next); }
    catch (e) { error = message(e); }
    finally { busy = false; render(); }
  }
  async function load(): Promise<void> {
    loadError = '';
    try { apply(await host.call<QuickConnectStatus>('quick_connect_status')); }
    catch (e) { loadError = message(e); render(); syncLegacy(); }
  }
  main.addEventListener('click', event => {
    const target = (event.target as HTMLElement).closest<HTMLElement>('button[data-qcs]');
    if (!target || (target as HTMLButtonElement).disabled) return;
    const id = target.dataset.id;
    switch (target.dataset.qcs) {
      case 'reload': void load(); break;
      case 'connect': connectPhone(); break;
      case 'set-up': case 'show': connectPhone('quickConnect'); break;
      case 'account': void act('quick_connect_choose_account', { accountId: id }); break;
      case 'cancel': void act('quick_connect_cancel'); break;
      case 'revoke': revoking = id ?? null; render(); main.querySelector<HTMLElement>('[data-qcs="revoke-cancel"]')?.focus(); break;
      case 'revoke-cancel': { const back = revoking; revoking = null; render(); if (back) main.querySelector<HTMLElement>(`[data-qcs="revoke"][data-id="${CSS.escape(back)}"]`)?.focus(); break; }
      case 'revoke-confirm': revoking = null; void act('quick_connect_revoke_device', { deviceId: id }); break;
    }
  });
  main.addEventListener('change', event => {
    const input = event.target as HTMLInputElement;
    if (input.dataset.qcs !== 'toggle') return;
    void act('quick_connect_set_enabled', { enabled: input.checked });
  });
  const unlisten = [host.listen('quick-connect://status', (next: QuickConnectStatus) => { if (!disposed && next) apply(next); })];
  // Last seen and the pairing's end read in minutes and days: one repaint a minute is enough.
  const clock = window.setInterval(() => { if (!revoking) render(); }, 60_000);
  render();
  void load();
  return () => {
    disposed = true; window.clearInterval(clock);
    legacyCleanup?.(); legacyCleanup = undefined;
    for (const u of unlisten) void u.then(fn => fn(), () => undefined);
  };
}
