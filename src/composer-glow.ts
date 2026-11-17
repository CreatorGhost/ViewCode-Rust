/**
 * The composer glow: while a turn runs in the open thread, a light circles the composer's edge over
 * a soft static glow, in the thread's project colour when the project has one, else the accent.
 * Drawn by `viewcode-shell.css` (`.composer-live`, `.composer-live-ring`); `main.ts` adds the markup
 * to the composer it draws. Settings → Appearance → Composer glow turns it off (default on).
 */
import { PROJECT_COLORS } from './palette-core';

const KEY = 'pandacode.composerGlow';
/** Fired on `window` when the setting changes. */
export const GLOW_EVENT = 'pandacode-composer-glow-change';
/** One turn of the light, as `composer-live-ring` in viewcode-shell.css. */
export const GLOW_PERIOD = 3200;

export function composerGlowEnabled(): boolean {
  try { return localStorage.getItem(KEY) !== 'off'; } catch { return true; }
}
export function setComposerGlowEnabled(on: boolean): void {
  try { localStorage.setItem(KEY, on ? 'on' : 'off'); } catch { /* A choice that cannot be saved leaves the glow on. */ }
  window.dispatchEvent(new Event(GLOW_EVENT));
}

/** What the composer wears for a thread: nothing unless a turn runs and the setting is on. `color` is the
 * project's colour name; anything else (none chosen, a name this build does not know) glows in the accent. */
export function composerGlow(live: boolean, color?: string | null, now = Date.now()): { className: string; attributes: string; ring: string } {
  if (!live || !composerGlowEnabled()) return { className: '', attributes: '', ring: '' };
  const known = color && PROJECT_COLORS.includes(color) ? color : '';
  // The composer is redrawn while a turn runs (a queued prompt, an approval): the light keeps its place on a
  // shared clock instead of starting over at the top with every redraw.
  const phase = Math.round(now % GLOW_PERIOD);
  return {
    className: 'composer-live',
    attributes: known ? ` data-glow="${known}" style="--composer-glow:var(--project-${known})"` : ' data-glow="accent"',
    ring: `<span class="composer-live-ring" aria-hidden="true"><span style="animation-delay:-${phase}ms"></span></span>`,
  };
}
