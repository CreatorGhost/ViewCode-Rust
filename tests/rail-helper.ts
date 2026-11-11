import { expect, type Page } from '@playwright/test';

// The rail's foot holds only Remote access, Pull requests and Settings. What used to sit beside them
// (Bots, Help, Agent usage, the star, the theme toggle) is a command in Go to anything (⌘K).

/** Runs a command from Go to anything by its name. */
export async function runCommand(page: Page, name: string | RegExp): Promise<void> {
  const query = page.getByRole('textbox', { name: 'Search tasks and commands', exact: true });
  if (!await query.isVisible()) await page.keyboard.press('ControlOrMeta+k');
  await expect(query).toBeVisible();
  if (typeof name === 'string') await query.fill(name);
  await page.locator('#command-results').getByRole('button', { name }).first().click();
}

/** Light ↔ dark, as the rail's toggle did. */
export const toggleTheme = (page: Page) => runCommand(page, /^Switch to (light|dark) mode/);
/** The Agent usage dialog. */
export const openAgentUsage = (page: Page) => runCommand(page, 'Agent usage');

const inBotsView = (page: Page) => page.locator('#workspace-sidebar').evaluate(sidebar => sidebar.classList.contains('bots-view'));
/** The sidebar's Bots view (bots and channels in place of the thread list). */
export async function showBotsView(page: Page): Promise<void> {
  await page.locator('#workspace-sidebar').waitFor();
  if (!await inBotsView(page)) await runCommand(page, 'Bots in the sidebar');
  await expect(page.getByRole('region', { name: 'Bots', exact: true })).toBeVisible();
}
/** Back to the thread list, by the Bots view's own back button. */
export async function showThreadsView(page: Page): Promise<void> {
  if (await inBotsView(page)) await page.getByRole('region', { name: 'Bots', exact: true }).getByRole('button', { name: 'Back to threads', exact: true }).click();
  await expect(page.locator('#navigation')).toBeVisible();
}
