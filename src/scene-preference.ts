/** Whether a live scene (the sky and its siblings, `src/scenes/`) is drawn behind the thread while
 * Wallpapers is off. Opt-in: with nothing saved the thread is the plain panel surface. */
const KEY = 'pandacode.threadScenes';
export const SCENES_EVENT = 'pandacode-scenes-change';

export function scenesEnabled(): boolean {
  try { return localStorage.getItem(KEY) === 'on'; } catch { return false; }
}
export function setScenesEnabled(on: boolean): void {
  try { localStorage.setItem(KEY, on ? 'on' : 'off'); } catch { /* A choice that cannot be saved leaves the scenes off. */ }
  window.dispatchEvent(new Event(SCENES_EVENT));
}
