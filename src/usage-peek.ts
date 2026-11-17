/** The last plan usage each agent account reported, kept for the rail's usage ring.
 *
 * This module never asks a provider anything: it only remembers what a check the user started
 * (the Usage dialog, Settings → Providers) returned, so the ring adds no automatic request.
 * With nothing remembered the ring is an empty track ("not checked yet"). */
import type { ProviderId } from './types';

export interface UsagePeek { percent: number | null; checkedAt: number; }
/** The fields of an `agent_usage` result the ring reads. */
export interface PeekSource { status: string; windows: { usedPercent: number }[]; checkedAt?: number; }

const KEY = 'pandacode.usage.peek';
/** Fired on `window` when a remembered value changes. */
export const USAGE_PEEK_EVENT = 'pandacode-usage-peek';
/** A value older than this no longer says anything about the current window. */
const MAX_AGE = 24 * 60 * 60 * 1000;

let cache: Record<string, UsagePeek> | undefined;
function read(): Record<string, UsagePeek> {
  if (cache) return cache;
  cache = {};
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    if (saved && typeof saved === 'object') for (const [id, value] of Object.entries(saved as Record<string, Partial<UsagePeek>>)) {
      const percent = value?.percent, checkedAt = Number(value?.checkedAt);
      if ((percent === null || Number.isFinite(percent)) && Number.isFinite(checkedAt)) cache[id] = { percent: percent ?? null, checkedAt };
    }
  } catch { /* The ring starts unknown. */ }
  return cache;
}

/** The highest window of a result, 0–100; null when the account reports none. */
export function peakPercent(result: PeekSource | null | undefined): number | null {
  if (!result || result.status !== 'ready') return null;
  const used = result.windows.map(window => window.usedPercent).filter(Number.isFinite);
  return used.length ? Math.round(Math.min(100, Math.max(0, ...used))) : null;
}

/** Remembers the default account's result for a harness. */
export function recordUsage(provider: ProviderId, result: PeekSource | null | undefined): void {
  const all = read(), percent = peakPercent(result), previous = all[provider];
  all[provider] = { percent, checkedAt: Date.now() };
  try { localStorage.setItem(KEY, JSON.stringify(all)); } catch { /* Kept for this launch. */ }
  if (previous?.percent !== percent && typeof window !== 'undefined') window.dispatchEvent(new Event(USAGE_PEEK_EVENT));
}

/** What is remembered for a harness; null percent when unknown or too old. */
export function usagePeek(provider: ProviderId, now = Date.now()): UsagePeek | undefined {
  const value = read()[provider];
  return value && now - value.checkedAt <= MAX_AGE ? value : undefined;
}

/** From here a plan is near its limit (amber); at 100 it is out (red). */
export const USAGE_NEAR = 80;
/** The state tone (`data-state-tone`) for a figure: none below `USAGE_NEAR` or while unknown. */
export function usageTone(percent: number | null): '' | 'near' | 'limited' {
  return percent === null ? '' : percent >= 100 ? 'limited' : percent >= USAGE_NEAR ? 'near' : '';
}

/** The agent the rail's one usage ring shows: the open thread's; with no thread open, the installed agent
 * closest to its limit as last checked, or the first installed one while nothing has been checked. */
export function usageRingAgent(providers: readonly { id: ProviderId; installed: boolean }[], openProvider: ProviderId | undefined, now = Date.now()): ProviderId | undefined {
  if (openProvider) return openProvider;
  const installed = providers.filter(provider => provider.installed);
  let closest: ProviderId | undefined, most = -1;
  for (const provider of installed) {
    const percent = usagePeek(provider.id, now)?.percent ?? null;
    if (percent !== null && percent > most) { closest = provider.id; most = percent; }
  }
  return closest ?? installed[0]?.id;
}
