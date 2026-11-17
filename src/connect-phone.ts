/* The Connect phone dialog (ViewCode's ConnectPhoneDialog + QuickConnectPhone, step for step): how the phone
 * reaches this computer (Same Wi-Fi, or Anywhere: Quick connect / Tailscale), Quick connect's setup on the
 * user's own Cloudflare account, and the pairing code. Lazy: main.ts imports it when the dialog opens.
 *
 * The pairing link is a secret. It lives in this module's closure only: the QR code is drawn from it (the SVG
 * holds modules, not the text) and Copy link writes it to the clipboard. It is never put in the DOM, an
 * attribute, a log or an error, and it is dropped when the code expires, a phone uses it, or the dialog closes
 * (which also ends the unused code). */
import './quick-connect.css';
import { escape, icon } from './dom';
import { trackSegment } from './motion';
import { openChoiceMenu } from './choice-menu';
import {
  CLOUDFLARE_SIGN_UP_URL, CLOUDFLARE_WORKERS_URL, NODE_DOWNLOAD_URL, QUICK_CONNECT_STABLE_NOTE, QUICK_CONNECT_TRAFFIC_NOTE,
  anywhereModes, countdownText, describeSlowSetupStep, formatMinutes, minutesSince, nextTick, relayHost, resolveView, setupChecklist, shownMode, tabOf,
} from './quick-connect-logic';
import type { QuickConnectMode, QuickConnectPairing, QuickConnectSetup, QuickConnectStatus } from './quick-connect-types';
import type { RemoteHost } from './remote-types';

const MODE_LABELS: Record<QuickConnectMode, string> = { sameWifi: 'Same Wi-Fi', quickConnect: 'Quick connect', tailscale: 'Tailscale' };
/** Sign-in pages already opened for the person, so reopening the dialog does not open them again (as ViewCode). */
const openedSignIns = new Set<string>();

type CodeState = 'idle' | 'creating' | 'active' | 'expired' | 'paired' | 'error';
type Confirm = 'remove' | 'remove-local' | null;

export function mountConnectPhone(root: HTMLElement, host: RemoteHost, options: { mode?: QuickConnectMode } = {}): () => void {
  let status: QuickConnectStatus | undefined, loadError = '', actionError = '', busy = false, disposed = false;
  let mode: QuickConnectMode | undefined = options.mode;
  // The secret: only here, never in markup.
  let link: string | null = null, expiresAt = 0, codeState: CodeState = 'idle', codeError = '', codeTicket = 0;
  let qrModule: typeof import('./quick-connect-qr') | undefined, qrFor = '', qrMarkup = '';
  let confirm: Confirm = null, subdomainArmed = false;
  let countdownTimer = 0, minuteTimer = 0;
  const message = (e: unknown) => e instanceof Error ? e.message : String(e);

  root.innerHTML = '<div class="qc" data-connect-phone><div class="qc-body"></div><p class="sr-only qc-live" role="status" aria-live="polite"></p></div>';
  const body = root.querySelector<HTMLElement>('.qc-body')!, live = root.querySelector<HTMLElement>('.qc-live')!;
  const announce = (text: string) => { live.textContent = ''; window.setTimeout(() => { if (!disposed) live.textContent = text; }, 30); };

  // ---- Pairing code ----
  function dropCode(next: CodeState): void {
    link = null; expiresAt = 0; codeState = next; qrFor = ''; qrMarkup = '';
    window.clearTimeout(countdownTimer);
  }
  async function createCode(): Promise<void> {
    const ticket = ++codeTicket;
    dropCode('creating'); codeError = ''; render();
    try {
      const pairing = await host.call<QuickConnectPairing>('quick_connect_pairing_create', { scope: 'full' });
      if (disposed || ticket !== codeTicket) return;
      link = pairing.link; expiresAt = pairing.expiresAt; codeState = 'active';
      qrModule ??= await import('./quick-connect-qr');
      if (disposed || ticket !== codeTicket) return;
      tick();
    } catch (e) {
      if (disposed || ticket !== codeTicket) return;
      dropCode('error'); codeError = message(e);
    }
    render();
  }
  function tick(): void {
    window.clearTimeout(countdownTimer);
    if (codeState !== 'active') return;
    const now = Date.now();
    if (now >= expiresAt) { dropCode('expired'); render(); announce('This code expired.'); return; }
    const text = body.querySelector<HTMLElement>('.qc-countdown');
    if (text) text.textContent = countdownText(expiresAt, now);
    countdownTimer = window.setTimeout(tick, nextTick(expiresAt, now) + 20);
  }
  function syncCode(): void {
    const ready = !!status && mode === 'quickConnect' && resolveView(status) === 'ready' && !confirm;
    if (!ready) {
      // Leaving the code's screen (turned off, a redeploy, another tab) ends an unused code.
      if (codeState === 'active') void host.call('quick_connect_pairing_cancel').catch(() => undefined);
      if (codeState !== 'idle') { codeTicket++; dropCode('idle'); }
      return;
    }
    if (codeState === 'idle') void createCode();
  }

  // ---- Status ----
  function apply(next: QuickConnectStatus): void {
    const before = status ? resolveView(status) : undefined;
    status = next;
    mode ??= shownMode(next.mode, next);
    const now = resolveView(next);
    if (before && before !== 'ready' && now === 'ready' && next.setup.status === 'succeeded') announce(next.setup.message);
    syncCode();
    scheduleMinute();
  }
  async function act(command: string, args?: Record<string, unknown>): Promise<void> {
    if (busy) return;
    busy = true; actionError = ''; render();
    try { const next = await host.call<QuickConnectStatus>(command, args); if (!disposed) apply(next); }
    catch (e) { actionError = message(e); }
    finally { busy = false; if (!disposed) render(); }
  }
  async function load(): Promise<void> {
    loadError = '';
    try { apply(await host.call<QuickConnectStatus>('quick_connect_status')); }
    catch (e) { loadError = message(e); }
    render();
  }
  /** A running step shows its age once a minute, never per second. */
  function scheduleMinute(): void {
    window.clearTimeout(minuteTimer);
    if (status?.setup.status !== 'running') return;
    const since = status.setup.stepStartedAt, minutes = minutesSince(since, Date.now());
    minuteTimer = window.setTimeout(() => { if (!disposed) { render(); scheduleMinute(); } }, Math.max(1000, since + (minutes + 1) * 60_000 - Date.now()));
  }

  // ---- Markup ----
  const btn = (action: string, label: string, cls = 'ds-btn', extra = '') => `<button type="button" class="${cls}" data-qc="${action}"${busy ? ' disabled' : ''}${extra}>${label}</button>`;
  const alert = (tone: 'danger' | 'warn', text: string) => `<div class="qc-alert" role="alert">${icon('alert').replace('<svg', `<svg data-state-tone="${tone}"`)}<p>${escape(text)}</p></div>`;
  const details = (text?: string) => text ? `<details class="qc-details"><summary>Details</summary><pre>${escape(text)}</pre></details>` : '';
  const quiet = (text: string) => `<p class="qc-quiet">${escape(text)}</p>`;
  const signUp = () => `<p class="qc-quiet">No Cloudflare account? <button type="button" class="qc-link" data-qc="sign-up">Create one free</button>, then come back here.</p>`;
  const header = () => `<div class="qc-head"><h3>Quick connect</h3><button type="button" class="ds-btn ds-btn-icon qc-menu" id="qc-menu-button" data-qc="menu" aria-haspopup="menu" aria-label="Quick connect options"${busy ? ' disabled' : ''}>${icon('more')}</button></div>`;
  const setupNote = (setup: QuickConnectSetup) => setup.status === 'idle' && setup.message ? `<p class="qc-quiet" role="status">${escape(setup.message)}</p>` : '';

  function segments(current: QuickConnectMode, state: QuickConnectStatus): string {
    const tab = tabOf(current), methods = anywhereModes(state);
    const seg = (label: string, pressed: boolean, data: string) => `<button type="button" data-qc="mode" data-mode="${data}" aria-pressed="${pressed}">${label}</button>`;
    const top = `<div class="ds-seg qc-seg" role="group" aria-label="Phone connection">${seg('Same Wi-Fi', tab === 'local', 'sameWifi')}${seg('Anywhere', tab === 'anywhere', methods.includes(current) ? current : 'quickConnect')}</div>`;
    const sub = tab === 'anywhere' && methods.length > 1 ? `<div class="ds-seg qc-seg qc-seg-sub" role="group" aria-label="Anywhere method">${methods.map(method => seg(MODE_LABELS[method], method === current, method)).join('')}</div>` : '';
    return top + sub;
  }
  function unavailable(current: QuickConnectMode, state: QuickConnectStatus): string {
    const reason = current === 'sameWifi' ? state.modes.sameWifi.reason : state.modes.tailscale.reason;
    const lead = current === 'sameWifi' ? 'Same Wi-Fi works only on the same network as this computer.' : 'Works wherever your phone is on your tailnet; phones still have to pair.';
    return `<div class="ds-card qc-unavailable"><div class="ds-card-inner"><div class="ds-row"><span class="ds-row-icon">${icon('info').replace('<svg', '<svg data-state-tone="warn"')}</span><span class="ds-row-main"><span class="ds-row-title">${escape(MODE_LABELS[current])} is not available yet</span><span class="qc-reason">${escape(reason ?? 'Not available on this computer.')}</span></span></div></div></div>
      ${quiet(lead)}<div class="qc-actions">${btn('use-quick', 'Use Quick connect instead')}</div>`;
  }
  function checklist(setup: Extract<QuickConnectSetup, { status: 'running' }>): string {
    if (setup.step === 'removing') return `<p class="qc-progress" role="status"><span class="support-spinner" aria-hidden="true"></span>${escape(setup.message || 'Removing Quick connect…')}${elapsed(setup.stepStartedAt)}</p>`;
    const minutes = minutesSince(setup.stepStartedAt, Date.now()), slow = describeSlowSetupStep(setup.step, minutes);
    return `<ol class="qc-checklist" aria-label="Setup steps">${setupChecklist(setup.step, setup.message).map(row => {
      const mark = row.state === 'done' ? icon('check').replace('<svg', '<svg data-state-tone="done"') : row.state === 'current' ? '<span class="support-spinner" aria-hidden="true"></span>' : icon('circle');
      const state = row.state === 'done' ? 'Done: ' : row.state === 'current' ? 'Now: ' : '';
      return `<li data-state="${row.state}"${row.state === 'current' ? ' aria-current="step"' : ''}><span class="qc-check-mark">${mark}</span><span class="qc-check-text"><span class="sr-only">${state}</span>${escape(row.label)}${row.state === 'current' ? elapsed(setup.stepStartedAt) : ''}${row.note ? `<small role="status">${escape(row.note)}</small>` : ''}${row.state === 'current' && slow ? `<small>${escape(slow)}</small>` : ''}</span></li>`;
    }).join('')}</ol>`;
  }
  const elapsed = (since: number) => { const label = formatMinutes(minutesSince(since, Date.now())); return label ? `<span class="qc-elapsed"> · ${label}</span>` : ''; };
  function signInPrompt(setup: Extract<QuickConnectSetup, { status: 'running' }>): string {
    const signIn = setup.signIn;
    if (!signIn?.url && !signIn?.code) return '';
    const openUrl = signIn.openUrl ?? signIn.url;
    return `<div class="ds-card qc-signin"><div class="ds-card-inner">
      <p class="qc-signin-lead">Approve PandaCode in the browser window that opened. If it did not open, open the link and enter this code.</p>
      ${signIn.code ? `<div class="qc-signin-code"><output class="qc-code" aria-label="Sign-in code">${escape(signIn.code)}</output>${btn('copy-sign-in-code', icon('copy'), 'ds-btn ds-btn-icon', ' aria-label="Copy sign-in code"')}</div>` : ''}
      ${signIn.url ? `<p class="qc-signin-url">${escape(signIn.url)}</p>` : ''}
      ${openUrl ? `<div class="qc-actions">${btn('open-sign-in', `${icon('external')}Open link again`)}${btn('copy-sign-in-link', `${icon('copy')}Copy link`)}</div>` : ''}
    </div></div>`;
  }
  function pairingArea(state: QuickConnectStatus): string {
    const steps = `<ol class="qc-steps"><li><span>1</span>Install PandaCode on your phone.</li><li><span>2</span>Open it and tap Add environment.</li><li><span>3</span>Scan this code.</li></ol>`;
    let code: string;
    if (codeState === 'active') code = `<div class="qc-qr" role="img" aria-label="Pairing code for the PandaCode phone app">${qrMarkup || '<span class="support-spinner" aria-hidden="true"></span>'}</div>`;
    else if (codeState === 'paired') code = `<div class="qc-qr-state">${icon('check').replace('<svg', '<svg data-state-tone="done"')}<strong>Phone connected</strong><small>Each code works once. Make a new one for another device.</small></div>`;
    else if (codeState === 'expired') code = `<div class="qc-qr-state"><strong>This code expired</strong></div>`;
    else if (codeState === 'error') code = `<div class="qc-qr-state">${alert('danger', codeError || 'Could not create a pairing code.')}</div>`;
    else code = `<div class="qc-qr-state" role="status"><span class="support-spinner" aria-hidden="true"></span><span class="sr-only">Making a code…</span></div>`;
    const after = codeState === 'active'
      ? `<p class="qc-expiry">This code works once. Expires in <span class="qc-countdown">${countdownText(expiresAt, Date.now())}</span>.</p><div class="qc-actions">${btn('copy-link', `${icon('copy')}Copy link`)}</div>`
      : codeState === 'paired' || codeState === 'expired' || codeState === 'error' ? `<div class="qc-actions">${btn('new-code', 'New code', 'ds-btn-primary')}</div>` : '';
    const host = relayHost(state);
    return `${steps}<div class="qc-code-area">${code}${after}</div><div class="qc-uses">${icon('phone')}<span>Uses ${escape(host ?? 'your relay')}</span></div>${quiet(QUICK_CONNECT_STABLE_NOTE)}`;
  }
  function confirmBox(kind: Exclude<Confirm, null>): string {
    const [title, text, action] = kind === 'remove'
      ? ['Remove Quick connect?', 'This deletes the relay from your Cloudflare account and forgets it on this computer. Phones paired through it stop reaching this computer.', 'Remove Quick connect']
      : ['Remove Quick connect from this computer only?', 'This computer forgets the relay without contacting Cloudflare; the relay stays on your Cloudflare account until you delete it there. Phones paired through it stop reaching this computer.', 'Remove from this computer'];
    return `<div class="ds-card qc-confirm" role="group" aria-label="${escape(title)}"><div class="ds-card-inner"><strong>${escape(title)}</strong><p>${escape(text)}</p><div class="qc-actions">${btn('confirm-cancel', 'Cancel')}${btn(`confirm-${kind}`, escape(action), 'ds-btn qc-danger')}</div></div></div>`;
  }
  function quickConnect(state: QuickConnectStatus): string {
    const setup = state.setup, view = resolveView(state);
    if (setup.status === 'running') return `${checklist(setup)}${setup.step === 'signing-in' ? signInPrompt(setup) + signUp() : ''}<div class="qc-actions">${btn('cancel', 'Cancel')}</div>${setup.problem ? quiet(waiting(setup.problem)) : ''}`;
    if (setup.status === 'needs-account') return `<p class="qc-lead">Which Cloudflare account should hold the relay?</p><div class="qc-actions qc-accounts">${setup.accounts.map(account => `<button type="button" class="ds-btn" data-qc="account" data-id="${escape(account.id)}"${busy ? ' disabled' : ''}>${escape(account.name)}</button>`).join('')}${btn('cancel', 'Cancel', 'ds-btn qc-ghost')}</div>`;
    if (setup.status === 'needs-subdomain') return `<p class="qc-lead">${escape(setup.message || 'Your Cloudflare account needs a free workers.dev address first. Open Cloudflare, choose one under Workers & Pages, then come back.')}</p><div class="qc-actions">${btn('open-workers', `${icon('external')}Open Cloudflare`, 'ds-btn-primary')}${btn('continue', 'Continue')}${btn('cancel', 'Back', 'ds-btn qc-ghost')}</div>`;
    if (setup.status === 'failed') return `${alert('danger', setup.message || 'Setup did not finish.')}<div class="qc-actions">${setup.action === 'install-node' ? btn('install-node', `${icon('external')}Install Node.js`, 'ds-btn-primary') : ''}${btn('retry', 'Try again', setup.action === 'install-node' ? 'ds-btn' : 'ds-btn-primary')}${btn('cancel', 'Back', 'ds-btn qc-ghost')}</div>${details(setup.details)}`;
    if (setup.status === 'remove-failed') return `${alert('danger', setup.message)}<div class="qc-actions">${btn('remove-again', 'Try again', 'ds-btn-primary')}${btn('remove-local', 'Remove from this computer only')}${btn('cancel', 'Back', 'ds-btn qc-ghost')}</div>${details(setup.details)}`;
    if (view === 'not-set-up') return `<div class="qc-intro"><span class="qc-intro-icon">${icon('globe')}</span><div><h3>Quick connect</h3><p>Reach this computer from your phone over the internet, including on networks that block tunnels. PandaCode sets up a small relay on your own free Cloudflare account.</p></div></div>${setupNote(setup)}<div class="qc-actions">${btn('start', 'Connect with Cloudflare', 'ds-btn-primary')}</div>${signUp()}`;
    const top = header() + (confirm ? confirmBox(confirm) : '');
    if (confirm) return top;
    if (view === 'off') return `${top}<p class="qc-lead">Your relay is set up but turned off. Turn it on to show a code for your phone.</p>${setupNote(setup)}<div class="qc-actions">${btn('turn-on', 'Use existing relay', 'ds-btn-primary')}</div>`;
    if (view === 'unreachable' && setup.status === 'unreachable') return `${top}${alert('warn', setup.message)}<div class="qc-actions">${btn('turn-on', 'Try again', 'ds-btn-primary')}${setup.problem === 'credential-rejected' ? btn('rotate', 'Redeploy with a new secret') : ''}</div>${details(setup.details)}`;
    if (view === 'connecting') return `${top}<p class="qc-progress" role="status"><span class="support-spinner" aria-hidden="true"></span>Connecting to your relay…</p><div class="qc-actions">${btn('turn-on', 'Check again')}</div>`;
    return top + pairingArea(state);
  }
  const waiting = (problem: string) => problem === 'network-refused'
    ? 'Waiting for your network to allow the new address. This can take a few minutes. You can close this; the app keeps trying.'
    : problem === 'credential-rejected' ? 'The relay has not accepted this computer\'s secret yet. Checking again…' : 'Waiting for the new address to come online. You can close this; the app keeps trying.';

  function render(): void {
    if (disposed) return;
    const focused = document.activeElement instanceof HTMLElement && body.contains(document.activeElement) ? document.activeElement : undefined;
    const keep = focused?.dataset.qc ? `[data-qc="${CSS.escape(focused.dataset.qc)}"]${focused.dataset.mode ? `[data-mode="${CSS.escape(focused.dataset.mode)}"]` : ''}${focused.dataset.id ? `[data-id="${CSS.escape(focused.dataset.id)}"]` : ''}` : '';
    if (!status) {
      body.innerHTML = loadError ? `${alert('danger', `Could not load Quick connect. ${loadError}`)}<div class="qc-actions">${btn('reload', 'Try again')}</div>` : '<p class="dialog-loading" role="status">Loading…</p>';
    } else {
      const current = mode ?? 'quickConnect';
      if (codeState === 'active' && link && qrModule && qrFor !== link) { qrMarkup = qrModule.qrSvg(link); qrFor = link; }
      const available = current === 'quickConnect' || (current === 'sameWifi' ? status.modes.sameWifi.available : status.modes.tailscale.available);
      const panel = current === 'quickConnect' ? `${quickConnect(status)}${quiet(QUICK_CONNECT_TRAFFIC_NOTE)}` : available ? '' : unavailable(current, status);
      body.innerHTML = `${segments(current, status)}<div class="qc-panel">${panel}</div>${actionError ? alert('danger', actionError) : ''}`;
      for (const group of body.querySelectorAll<HTMLElement>('.qc-seg')) trackSegment(group, group.querySelector<HTMLElement>(':scope>[aria-pressed="true"]'));
    }
    if (keep) body.querySelector<HTMLElement>(keep)?.focus({ preventScroll: true });
  }

  // ---- Actions ----
  async function copy(text: string, done: string): Promise<void> {
    try { await navigator.clipboard.writeText(text); host.notify(done); }
    catch { host.notify('Couldn’t copy. Try again.'); }
  }
  function open(url: string): void { void host.openUrl(url).catch(e => { actionError = message(e); render(); }); }
  function showMenu(anchor: HTMLElement): void {
    if (!status) return;
    const enabled = status.enabled;
    openChoiceMenu({
      anchor, title: 'Quick connect options', actions: true, width: 300,
      items: [
        enabled ? { id: 'off', label: 'Turn off Quick connect' } : { id: 'on', label: 'Turn on Quick connect' },
        { id: 'redeploy', label: 'Redeploy relay' },
        { id: 'rotate', label: 'Redeploy with a new secret', description: 'Phones stay paired.' },
        { id: 'remove', label: 'Remove Quick connect', danger: true, separator: true },
        { id: 'remove-local', label: 'Remove from this computer only', description: 'The relay stays on your Cloudflare account.', danger: true },
      ],
      onSelect: id => { void menuAction(id); },
      onClose: () => undefined,
    });
  }
  async function menuAction(id: string): Promise<void> {
    if (id === 'off') await act('quick_connect_set_enabled', { enabled: false });
    else if (id === 'on') await act('quick_connect_set_enabled', { enabled: true });
    else if (id === 'redeploy') await act('quick_connect_start_setup', { rotateSecret: false });
    else if (id === 'rotate') await rotate();
    else if (id === 'remove' || id === 'remove-local') { confirm = id; syncCode(); render(); body.querySelector<HTMLElement>('[data-qc="confirm-cancel"]')?.focus(); }
  }
  async function rotate(): Promise<void> {
    host.notify('Redeploying with a new secret. Paired phones stay paired.');
    await act('quick_connect_start_setup', { rotateSecret: true });
  }
  async function setMode(next: QuickConnectMode): Promise<void> {
    if (next === mode) return;
    mode = next; syncCode(); render();
    // Only stores the choice: nothing starts Tailscale or a listener.
    try { const updated = await host.call<QuickConnectStatus>('quick_connect_set_mode', { mode: next }); if (!disposed) { status = updated; render(); } }
    catch (e) { actionError = message(e); render(); }
  }
  body.addEventListener('click', event => {
    const target = (event.target as HTMLElement).closest<HTMLElement>('[data-qc]');
    if (!target || (target as HTMLButtonElement).disabled || !status && target.dataset.qc !== 'reload') return;
    const signIn = status?.setup.status === 'running' ? status.setup.signIn : undefined;
    switch (target.dataset.qc) {
      case 'reload': void load(); break;
      case 'mode': void setMode(target.dataset.mode as QuickConnectMode); break;
      case 'use-quick': void setMode('quickConnect'); break;
      case 'start': case 'retry': void act('quick_connect_start_setup', { rotateSecret: false }); break;
      case 'cancel': subdomainArmed = false; void act('quick_connect_cancel'); break;
      case 'continue': subdomainArmed = false; void act('quick_connect_continue'); break;
      case 'account': void act('quick_connect_choose_account', { accountId: target.dataset.id }); break;
      case 'open-workers': subdomainArmed = true; open(CLOUDFLARE_WORKERS_URL); break;
      case 'install-node': open(NODE_DOWNLOAD_URL); break;
      case 'sign-up': open(CLOUDFLARE_SIGN_UP_URL); break;
      case 'open-sign-in': { const url = signIn?.openUrl ?? signIn?.url; if (url) open(url); break; }
      case 'copy-sign-in-link': { const url = signIn?.openUrl ?? signIn?.url; if (url) void copy(url, 'Link copied'); break; }
      case 'copy-sign-in-code': if (signIn?.code) void copy(signIn.code, 'Code copied'); break;
      case 'turn-on': void act('quick_connect_set_enabled', { enabled: true }); break;
      case 'rotate': void rotate(); break;
      case 'menu': showMenu(target); break;
      case 'remove-again': void act('quick_connect_remove', { localOnly: false }); break;
      case 'remove-local': void act('quick_connect_remove', { localOnly: true }); break;
      case 'confirm-cancel': confirm = null; syncCode(); render(); body.querySelector<HTMLElement>('[data-qc="menu"]')?.focus(); break;
      case 'confirm-remove': confirm = null; void act('quick_connect_remove', { localOnly: false }); break;
      case 'confirm-remove-local': confirm = null; void act('quick_connect_remove', { localOnly: true }); break;
      case 'copy-link': if (link) void copy(link, 'Pairing link copied'); break;
      case 'new-code': void createCode(); break;
    }
  });
  // After "Open Cloudflare" the person picks a subdomain in the browser; coming back to this window continues once.
  const onFocus = () => { if (!subdomainArmed || status?.setup.status !== 'needs-subdomain') return; subdomainArmed = false; void act('quick_connect_continue'); };
  window.addEventListener('focus', onFocus);

  const unlisten = [
    host.listen('quick-connect://status', (next: QuickConnectStatus) => { if (disposed || !next) return; apply(next); maybeOpenSignIn(); render(); }),
    host.listen('quick-connect://paired', () => {
      if (disposed || codeState !== 'active') return;
      codeTicket++; dropCode('paired'); render(); announce('Phone connected.');
    }),
  ];
  /** The desktop opens Cloudflare's page itself (ViewCode does the same), once per sign-in link. */
  function maybeOpenSignIn(): void {
    const setup = status?.setup;
    if (setup?.status !== 'running' || setup.step !== 'signing-in') return;
    const url = setup.signIn?.openUrl ?? setup.signIn?.url;
    if (!url || openedSignIns.has(url)) return;
    openedSignIns.add(url); open(url);
  }
  void load();

  return () => {
    if (disposed) return;
    // An unused one-time code should not outlive the dialog that showed it.
    if (codeState === 'active' && expiresAt > Date.now()) void host.call('quick_connect_pairing_cancel').catch(() => undefined);
    disposed = true; codeTicket++; dropCode('idle');
    window.clearTimeout(minuteTimer);
    window.removeEventListener('focus', onFocus);
    root.replaceChildren();
    for (const u of unlisten) void u.then(fn => fn(), () => undefined);
  };
}
