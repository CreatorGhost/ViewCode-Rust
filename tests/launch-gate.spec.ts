import { expect, test, type Page } from '@playwright/test';

/** Opens Settings → Agents → Providers with every card expanded. */
async function providers(page: Page, query: string) {
  await page.goto(`/${query}`);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('region', { name: 'Settings', exact: true });
  await settings.getByRole('tab', { name: 'Agents', exact: true }).click();
  await settings.getByRole('tab', { name: 'Providers', exact: true }).click();
  const panel = page.locator('#settings-panel-providers');
  await panel.locator('details.connection-card').first().waitFor();
  await expect(panel.locator('[data-launch-gate="claude"]')).toBeAttached();
  for (const summary of await panel.locator('details.connection-card>summary').all()) await summary.click();
  return panel;
}

const card = (panel: ReturnType<Page['locator']>, id: string) => panel.locator('details.connection-card', { has: panel.page().locator(`[data-launch-gate="${id}"]`) });
const gateCalls = (page: Page) => page.evaluate(() => (window as unknown as { __launchGateCalls?: unknown[] }).__launchGateCalls ?? []);
const gate = (page: Page) => page.evaluate(async () => {
  const { invoke } = await import('/src/preview.ts');
  return invoke<{ firstRunPending: boolean; enabled: string[]; managed: boolean; strict: boolean; pinned: string[] | null; blocked: string[] }>('launch_gate_state', {});
});

test('a fresh profile shows every harness as not enabled, and enabling one calls the gate', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const panel = await providers(page, '?gate=fresh');

  // The first-run choice is pending and exposed for the welcome step that will ask.
  expect(await gate(page)).toMatchObject({ firstRunPending: true, enabled: [], managed: false });
  // Discovery offers nothing: "not enabled" is not "not installed".
  const discovered = await page.evaluate(async () => (await import('/src/preview.ts')).invoke<{ id: string; installed: boolean; present: boolean; enabled: boolean }[]>('list_providers', {}));
  expect(discovered.find(provider => provider.id === 'codex')).toMatchObject({ installed: false, present: true, enabled: false });

  const codex = card(panel, 'codex');
  const enabled = codex.getByRole('switch', { name: 'Enabled', exact: true });
  await expect(enabled).not.toBeChecked();
  await expect(codex.locator('.connection-state').first()).toHaveText('Not enabled');
  await expect(card(panel, 'claude').locator('.connection-state').first()).toHaveText('Not enabled');

  await enabled.check();
  await expect(page.locator('.connection-feedback')).toContainText('Enabled.');
  expect(await gateCalls(page)).toEqual([{ command: 'launch_gate_set_enabled', harness: 'codex', enabled: true }]);
  await expect(enabled).toBeChecked();
  await expect(codex.locator('.connection-state').first()).not.toHaveText('Not enabled');
  // Turning one on in Settings is the first-run choice: the rest stay off.
  expect(await gate(page)).toMatchObject({ firstRunPending: false, enabled: ['codex'] });
  await expect(card(panel, 'claude').getByRole('switch', { name: 'Enabled', exact: true })).not.toBeChecked();

  await enabled.uncheck();
  await expect(page.locator('.connection-feedback')).toContainText('Turned off.');
  expect((await gateCalls(page)).at(-1)).toEqual({ command: 'launch_gate_set_enabled', harness: 'codex', enabled: false });
  await expect(codex.locator('.connection-state').first()).toHaveText('Not enabled');
  expect(errors).toEqual([]);
});

test('an organization policy locks what it blocks and what it does not allow', async ({ page }) => {
  const panel = await providers(page, '?gate=managed');
  expect(await gate(page)).toMatchObject({ firstRunPending: true, managed: true, strict: true, blocked: ['codex'], pinned: ['claude', 'opencode'] });

  const codex = card(panel, 'codex');
  await expect(codex.locator('.connection-state').first()).toHaveText('Blocked by your organization');
  await expect(codex.getByRole('switch', { name: 'Enabled', exact: true })).toBeDisabled();
  const kimi = card(panel, 'kimi');
  await expect(kimi.locator('.connection-state').first()).toHaveText('Not allowed by your organization');
  await expect(kimi.getByRole('switch', { name: 'Enabled', exact: true })).toBeDisabled();

  // What the policy allows is still off until the user turns it on.
  const claude = card(panel, 'claude').getByRole('switch', { name: 'Enabled', exact: true });
  await expect(claude).toBeEnabled();
  await expect(claude).not.toBeChecked();
  await claude.check();
  await expect(claude).toBeChecked();
  expect(await gate(page)).toMatchObject({ firstRunPending: false, enabled: ['claude'] });
  // The first-run choice was made by that switch: a stale welcome screen is refused.
  const stale = await page.evaluate(async () => (await import('/src/preview.ts')).invoke('launch_gate_complete_first_run', { enabled: ['opencode'] }).then(() => 'accepted', error => String(error)));
  expect(stale).toContain('already chosen');
});

test('the first-run choice turns on exactly what was picked', async ({ page }) => {
  await page.goto('/?gate=fresh');
  await page.getByRole('button', { name: 'Settings', exact: true }).waitFor();
  const next = await page.evaluate(async () => (await import('/src/preview.ts')).invoke<{ firstRunPending: boolean; enabled: string[] }>('launch_gate_complete_first_run', { enabled: ['claude', 'opencode'] }));
  expect(next).toMatchObject({ firstRunPending: false, enabled: ['claude', 'opencode'] });
  const discovered = await page.evaluate(async () => (await import('/src/preview.ts')).invoke<{ id: string; installed: boolean }[]>('list_providers', {}));
  expect(discovered.filter(provider => provider.installed).map(provider => provider.id)).toEqual(['claude', 'opencode']);
});

test('without a scenario every sample harness is enabled, as before the gate', async ({ page }) => {
  const panel = await providers(page, '');
  expect(await gate(page)).toMatchObject({ firstRunPending: false, managed: false });
  await expect(card(panel, 'codex').getByRole('switch', { name: 'Enabled', exact: true })).toBeChecked();
  await expect(card(panel, 'codex').locator('.connection-state').first()).not.toHaveText('Not enabled');
});

// Settings → Agents → Advanced → Power: keep the computer awake while a turn runs.
async function power(page: Page, query: string) {
  await page.goto(`/${query}`);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('region', { name: 'Settings', exact: true });
  await settings.getByRole('tab', { name: 'Agents', exact: true }).click();
  await settings.locator('#settings-panel-agents details.settings-fold>summary').click();
  const embed = settings.locator('#settings-panel-agents [data-embed="keepAwake"]');
  return { toggle: embed.getByRole('checkbox', { name: 'Keep this computer awake while an agent works', exact: true }), status: embed.locator('#keep-awake-status') };
}

test('keep awake is on by default and can be turned off', async ({ page }) => {
  const { toggle } = await power(page, '');
  await expect(toggle).toBeEnabled();
  await expect(toggle).toBeChecked();
  await toggle.uncheck();
  await expect(toggle).not.toBeChecked();
  expect(await page.evaluate(() => (window as unknown as { __keepAwakeCalls?: boolean[] }).__keepAwakeCalls)).toEqual([false]);
  await toggle.check();
  await expect(toggle).toBeChecked();
});

test('a policy turns keep awake off, and a platform without it says so', async ({ page }) => {
  const managed = await power(page, '?gate=managed');
  await expect(managed.toggle).toBeDisabled();
  await expect(managed.toggle).not.toBeChecked();
  await expect(managed.status).toHaveText('Turned off by your organization.');
  const unsupported = await power(page, '?keepawake=unsupported');
  await expect(unsupported.toggle).toBeDisabled();
  await expect(unsupported.status).toHaveText('Available on macOS.');
});
