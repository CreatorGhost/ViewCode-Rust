import type { Page } from '@playwright/test';

/** Live scenes (the sky and its siblings behind the thread) are opt-in: a fresh profile shows a plain
 * panel. Specs about the scenes turn them on first, as Settings → Appearance → Live scenes does; a
 * choice the test itself saves later (and reloads with) is left alone. */
export const enableScenes = (page: Page) => page.addInitScript(() => {
  if (localStorage.getItem('pandacode.threadScenes') === null) localStorage.setItem('pandacode.threadScenes', 'on');
});
