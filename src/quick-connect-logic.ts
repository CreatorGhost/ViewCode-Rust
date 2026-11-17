/* Pure helpers for the Connect phone dialog and Settings → Remote access (Quick connect):
 * which screen a status shows, the setup checklist, and the time texts. No DOM, no IPC,
 * so `tests/quick-connect-logic.test.mjs` runs them in Node. Wording follows ViewCode's
 * `connectPhone.logic.ts` step for step. */
import type { QuickConnectDevice, QuickConnectMode, QuickConnectStatus, QuickConnectStep } from './quick-connect-types';

/** What the Quick connect tab shows for a status. Setup states come first, as in ViewCode. */
export type QuickConnectView =
  | 'running' | 'needs-account' | 'needs-subdomain' | 'failed' | 'remove-failed'
  /** No relay on this computer: offer "Connect with Cloudflare". */
  | 'not-set-up'
  /** Set up but turned off. */
  | 'off'
  /** On, but the relay has not answered (or this computer has not connected yet). */
  | 'unreachable' | 'connecting'
  /** On and connected: the pairing code. */
  | 'ready';

export function resolveView(status: QuickConnectStatus): QuickConnectView {
  const setup = status.setup.status;
  if (setup === 'running' || setup === 'needs-account' || setup === 'needs-subdomain' || setup === 'failed' || setup === 'remove-failed') return setup;
  if (!status.relay) return 'not-set-up';
  if (!status.enabled) return 'off';
  if (setup === 'unreachable') return 'unreachable';
  return status.attached ? 'ready' : 'connecting';
}

/** One line for Settings → Remote access; a running setup says which step it is on. */
export function describeQuickConnectStatus(status: QuickConnectStatus): string {
  const setup = status.setup;
  if (setup.status === 'running') return setup.message || 'Setting up…';
  if (setup.status === 'needs-account' || setup.status === 'needs-subdomain' || setup.status === 'failed' || setup.status === 'remove-failed') return setup.message || 'Setup did not finish.';
  switch (resolveView(status)) {
    case 'not-set-up': return 'Not set up. Connect it to your own free Cloudflare account.';
    case 'off': return 'Off.';
    case 'unreachable': return setup.message || 'Your relay is set up, but its address is not answering yet.';
    case 'connecting': return 'Connecting to your relay…';
    default: return `Connected through ${relayHost(status) ?? 'your relay'}.`;
  }
}

/** The relay's host name, as "Uses …" shows it. */
export function relayHost(status: Pick<QuickConnectStatus, 'relay'>): string | null {
  if (!status.relay) return null;
  try { return new URL(status.relay.url).host; } catch { return null; }
}

/** The setup checklist as the person sees it; the tool check counts as signing in (ViewCode's `QUICK_CONNECT_SETUP_STEPS`). */
export const SETUP_CHECKLIST: readonly { label: string; steps: readonly QuickConnectStep[] }[] = [
  { label: 'Signing in to Cloudflare', steps: ['checking-tools', 'signing-in'] },
  { label: 'Setting up your relay', steps: ['deploying'] },
  { label: 'Securing it', steps: ['storing-secret'] },
  { label: 'Checking it works', steps: ['verifying'] },
];
export type ChecklistRow = { label: string; state: 'done' | 'current' | 'pending'; note?: string };

/** Each row's state for the step running now. The step's message becomes the current row's note
 *  when it says more than the label ("Getting ready…", waiting for the network). */
export function setupChecklist(step: QuickConnectStep | undefined, message?: string): ChecklistRow[] {
  const current = step ?? 'checking-tools';
  const index = SETUP_CHECKLIST.findIndex(row => row.steps.includes(current));
  return SETUP_CHECKLIST.map((row, n) => {
    const state = n < index ? 'done' : n === index ? 'current' : 'pending';
    const saysMore = state === 'current' && !!message && message.replace(/…$/u, '') !== row.label;
    return { label: row.label, state, ...(saysMore ? { note: message } : {}) };
  });
}

/** Whole minutes from `since` to `now`: 0 when unknown or ahead of this clock. */
export function minutesSince(since: number | undefined, now: number): number {
  if (since === undefined || !Number.isFinite(since)) return 0;
  return Math.max(0, Math.floor((now - since) / 60_000));
}
/** "3 min"; null under a minute, where a count says nothing. */
export function formatMinutes(minutes: number): string | null { return minutes < 1 ? null : `${minutes} min`; }

const TOOL_STEPS: ReadonlySet<QuickConnectStep> = new Set(['checking-tools', 'deploying', 'storing-secret']);
/** Said under a tool step that has run unusually long (two minutes); null before that. */
export function describeSlowSetupStep(step: QuickConnectStep | undefined, minutes: number): string | null {
  if (!step || !TOOL_STEPS.has(step) || minutes < 2) return null;
  return 'Taking longer than usual; the first time can take a few minutes. If it does not finish, setup stops and you can try again.';
}

/** A pairing code's life: valid until `expiresAt` unless a phone used it first. */
export type PairingCodeState = 'active' | 'paired' | 'expired';
export function pairingCodeState(expiresAt: number, now: number, paired = false): PairingCodeState {
  if (paired) return 'paired';
  return now < expiresAt ? 'active' : 'expired';
}

/** The countdown beside a code, "4:59": minutes and seconds left, rounded up so 0:00 only shows once it expired. */
export function countdownText(expiresAt: number, now: number): string {
  const seconds = Math.max(0, Math.ceil((expiresAt - now) / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

/** The milliseconds until the countdown's text next changes, so one timer repaints it exactly then. */
export function nextTick(expiresAt: number, now: number): number {
  const left = expiresAt - now;
  if (left <= 0) return 0;
  return left % 1000 || 1000;
}

type TimeOptions = { locale?: string; timeZone?: string };

/** When a paired phone was last on: "Online now", "Last seen 5 min ago", "Not seen yet". */
export function lastSeenText(device: Pick<QuickConnectDevice, 'online' | 'lastSeen'>, now: number, options: TimeOptions = {}): string {
  if (device.online) return 'Online now';
  if (device.lastSeen === null || !Number.isFinite(device.lastSeen)) return 'Not seen yet';
  const minutes = Math.floor(Math.max(0, now - device.lastSeen) / 60_000);
  if (minutes < 1) return 'Last seen just now';
  if (minutes < 60) return `Last seen ${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `Last seen ${hours} h ago`;
  const days = Math.floor(hours / 24);
  if (days === 1) return 'Last seen yesterday';
  if (days < 7) return `Last seen ${days} days ago`;
  return `Last seen ${new Date(device.lastSeen).toLocaleDateString(options.locale, { month: 'short', day: 'numeric', timeZone: options.timeZone })}`;
}

/** A pairing ends 30 days after it was made; then the phone pairs again with a new code.
 *  "Pairs again after Dec 17", or the time of day when that is within a day. */
export function expiryText(expiresAt: number, now: number, options: TimeOptions = {}): string {
  if (expiresAt <= now) return 'Pairing ended; pair again with a new code';
  const within = expiresAt - now < 86_400_000;
  const when = within
    ? new Date(expiresAt).toLocaleTimeString(options.locale, { hour: 'numeric', minute: '2-digit', timeZone: options.timeZone })
    : new Date(expiresAt).toLocaleDateString(options.locale, { month: 'short', day: 'numeric', timeZone: options.timeZone });
  return `Pairs again after ${when}`;
}

/** The connection choices in the dialog: Same Wi-Fi, then Anywhere (Quick connect, and Tailscale only
 *  when it is installed on this computer), as in ViewCode. */
export type ConnectTab = 'local' | 'anywhere';
export function tabOf(mode: QuickConnectMode): ConnectTab { return mode === 'sameWifi' ? 'local' : 'anywhere'; }
export function anywhereModes(status: Pick<QuickConnectStatus, 'modes'>): QuickConnectMode[] {
  return ['quickConnect', ...(status.modes.tailscale.installed ? ['tailscale' as const] : [])];
}
/** The method shown for a stored mode: a Tailscale choice falls back to Quick connect once Tailscale is gone. */
export function shownMode(mode: QuickConnectMode, status: Pick<QuickConnectStatus, 'modes'>): QuickConnectMode {
  return mode === 'tailscale' && !status.modes.tailscale.installed ? 'quickConnect' : mode;
}

export const CLOUDFLARE_SIGN_UP_URL = 'https://dash.cloudflare.com/sign-up';
/** Workers & Pages of whichever account the person picks, where the workers.dev subdomain is chosen. */
export const CLOUDFLARE_WORKERS_URL = 'https://dash.cloudflare.com/?to=/:account/workers-and-pages';
export const NODE_DOWNLOAD_URL = 'https://nodejs.org';
export const QUICK_CONNECT_STABLE_NOTE = 'Pair once; it reconnects on its own.';
/** Where ViewCode warns that its relay is not end-to-end encrypted, this relay is: it forwards only ciphertext. */
export const QUICK_CONNECT_TRAFFIC_NOTE = 'End-to-end encrypted: your Cloudflare Worker passes on only ciphertext.';
