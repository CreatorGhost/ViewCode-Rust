import { expect, test, type Page } from '@playwright/test';

// Computer use screens (src/computer-use.ts, src/computer-use-settings.ts) against the preview mocks in
// src/preview-computer-use.ts. `?cu=` picks a scenario; `__cuCalls` records every computer_use_* call.

const settingsRegion = (page: Page) => page.getByRole('region', { name: 'Settings', exact: true });
const cuPanel = (page: Page) => settingsRegion(page).getByRole('tabpanel', { name: 'Computer use', exact: true });
const card = (page: Page) => page.locator('#cu-approval .cu-approval');
const indicator = (page: Page) => page.locator('#cu-bypass-indicator');
type Call = { command: string; args: Record<string, any> };
const calls = (page: Page, command: string): Promise<Call[]> => page.evaluate(name => ((window as any).__cuCalls as Call[]).filter(call => call.command === name), command);
const settingsSent = async (page: Page) => (await calls(page, 'computer_use_set_settings')).map(call => call.args.settings);

async function open(page: Page, scenario = ''): Promise<void> {
  await page.goto(scenario ? `/?cu=${scenario}` : '/');
  await expect(page.locator('#prompt')).toHaveAttribute('data-session-id', 'session-1');
  await expect.poll(() => page.evaluate(() => ((window as any).__cuCalls ?? []).length)).toBeGreaterThan(0);
}
async function openSettings(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await settingsRegion(page).getByRole('tab', { name: 'Computer use', exact: true }).click();
  await expect(cuPanel(page).locator('.cu-settings')).toBeVisible();
}
const mode = (page: Page, name: string) => cuPanel(page).getByRole('group', { name: 'Mode', exact: true }).getByRole('button', { name, exact: true });

test('off by default: the mode is a three-way segment and turning it on sends only the mode', async ({ page }) => {
  await open(page);
  await expect(indicator(page)).toBeHidden();
  await openSettings(page);
  await expect(mode(page, 'Off')).toHaveAttribute('aria-pressed', 'true');
  await expect(cuPanel(page).locator('.cu-status')).toContainText('Ready to turn on');
  await expect(cuPanel(page).getByRole('radiogroup')).toHaveCount(0);
  await expect(cuPanel(page).locator('.cu-bypass-warning')).toHaveCount(0);
  await mode(page, 'Look only').click();
  await expect(mode(page, 'Look only')).toHaveAttribute('aria-pressed', 'true');
  expect(await settingsSent(page)).toEqual([{ mode: 'observe' }]);
  await expect(cuPanel(page).locator('.cu-status')).toContainText('Accessibility granted');
  await expect(cuPanel(page).getByText('Screen Recording', { exact: true })).toBeVisible();
  // Approvals and the screen only matter once input is allowed.
  await expect(cuPanel(page).getByRole('radiogroup')).toHaveCount(0);
  await mode(page, 'Control').click();
  const approvals = cuPanel(page).getByRole('radiogroup', { name: 'Ask before input' });
  await expect(approvals.getByRole('radio', { name: /Follow thread permissions/ })).toHaveAttribute('aria-checked', 'true');
  await approvals.getByRole('radio', { name: /Only for risky actions/ }).click();
  await expect(approvals.getByRole('radio', { name: /Only for risky actions/ })).toHaveAttribute('aria-checked', 'true');
  await cuPanel(page).getByRole('radiogroup', { name: 'Show on screen' }).getByRole('radio', { name: /Ask once per task/ }).click();
  expect(await settingsSent(page)).toEqual([{ mode: 'observe' }, { mode: 'control' }, { approvals: 'risky' }, { screen: 'ask' }]);
  await expect(cuPanel(page).locator('.cu-recent')).toContainText('No computer actions yet.');
});

test('look only with Accessibility off says which privacy pane to open', async ({ page }) => {
  await open(page, 'denied');
  await openSettings(page);
  await expect(mode(page, 'Look only')).toHaveAttribute('aria-pressed', 'true');
  await expect(cuPanel(page).locator('.cu-status')).toContainText('Accessibility is off for PandaCode');
  await expect(cuPanel(page).locator('.cu-status')).toContainText('Privacy & Security → Accessibility');
  await expect(cuPanel(page).getByText(/Privacy & Security → Screen Recording/)).toBeVisible();
});

for (const scenario of ['unavailable', 'missing']) test(`not available on this computer (${scenario})`, async ({ page }) => {
  await open(page, scenario);
  await openSettings(page);
  await expect(cuPanel(page).locator('.cu-status')).toContainText('Not available on this computer');
  await expect(cuPanel(page).locator('.cu-status')).toContainText(scenario === 'unavailable' ? 'not supported on Linux' : 'helper is not installed');
  await expect(mode(page, 'Off')).toHaveAttribute('aria-pressed', 'true');
  await expect(mode(page, 'Look only')).toBeDisabled();
  await expect(mode(page, 'Control')).toBeDisabled();
  await expect(cuPanel(page).getByRole('switch', { name: 'Bypass all permissions' })).toBeDisabled();
});

test('bypass needs the confirmation, sends acknowledgeRisk, shows a standing warning and the shell indicator', async ({ page }) => {
  await open(page, 'observe');
  await openSettings(page);
  const toggle = cuPanel(page).getByRole('switch', { name: 'Bypass all permissions' });
  await expect(toggle).not.toBeChecked();
  // Cancelled: nothing is sent and the switch stays off.
  await toggle.click();
  const dialog = page.getByRole('dialog', { name: 'Bypass all computer-use permissions?' });
  await expect(dialog).toBeVisible();
  for (const line of ['Every approval', 'The Full access requirement', 'The show-on-screen question', 'password managers', 'The pause while another approval waits']) await expect(dialog.locator('.cu-bypass-list')).toContainText(line);
  await expect(toggle).not.toBeChecked();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(toggle).not.toBeChecked();
  expect(await settingsSent(page)).toEqual([]);
  // Escape cancels too.
  await toggle.click();
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  expect(await settingsSent(page)).toEqual([]);
  // Confirming needs the box ticked first.
  await toggle.click();
  const confirm = dialog.getByRole('button', { name: 'Turn on bypass', exact: true });
  await expect(confirm).toBeDisabled();
  await dialog.getByRole('checkbox').check();
  await confirm.click();
  await expect(dialog).toBeHidden();
  await expect.poll(() => settingsSent(page)).toEqual([{ approvals: 'bypass', acknowledgeRisk: true }]);
  await expect(toggle).toBeChecked();
  const warning = cuPanel(page).locator('.cu-bypass-warning');
  await expect(warning).toContainText('Bypass all permissions is on');
  // The shell indicator is there once back in the workspace.
  await settingsRegion(page).getByRole('button', { name: 'Back to the workspace', exact: true }).click();
  await expect(indicator(page)).toBeVisible();
  await expect(indicator(page).locator('svg')).toHaveAttribute('data-state-tone', 'danger');
  // It opens the setting; turning it off needs no confirmation.
  await indicator(page).click();
  await expect(cuPanel(page)).toBeVisible();
  await cuPanel(page).getByRole('switch', { name: 'Bypass all permissions' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect.poll(() => settingsSent(page)).toEqual([{ approvals: 'bypass', acknowledgeRisk: true }, { approvals: 'thread' }]);
  await expect(warning).toHaveCount(0);
  await settingsRegion(page).getByRole('button', { name: 'Back to the workspace', exact: true }).click();
  await expect(indicator(page)).toBeHidden();
});

test('bypass already on: indicator at launch, warning on the page, Turn off from the warning', async ({ page }) => {
  await open(page, 'bypass');
  await expect(indicator(page)).toBeVisible();
  await indicator(page).click();
  const warning = cuPanel(page).locator('.cu-bypass-warning');
  await expect(warning).toBeVisible();
  // Approvals and the screen are moot while bypass is on.
  await expect(cuPanel(page).getByRole('radiogroup')).toHaveCount(0);
  await warning.getByRole('button', { name: 'Turn off', exact: true }).click();
  await expect(warning).toHaveCount(0);
  expect(await settingsSent(page)).toEqual([{ approvals: 'thread' }]);
  await expect(cuPanel(page).getByRole('radiogroup', { name: 'Ask before input' })).toBeVisible();
});

const variants = [
  { scenario: 'control', title: 'Allow this computer action?', detail: 'Type 13 characters into text field "Search" in Safari', options: ['Allow once', 'Allow for the rest of this turn', 'Decline'], pick: 'Allow for the rest of this turn', decision: 'acceptForSession', warning: false },
  { scenario: 'destructive', title: 'It looks destructive', detail: 'Press button "Delete" in Notes', options: ['Allow once', 'Decline'], pick: 'Allow once', decision: 'accept', warning: true },
  { scenario: 'screen', title: 'Show this task on screen?', detail: 'Show this on your screen? To work in Safari', options: ['Show on screen for this task', 'Keep it in the background'], pick: 'Keep it in the background', decision: 'decline', warning: false },
];
for (const variant of variants) test(`approval card (${variant.scenario}) shows exactly the options sent and answers with the chosen decision`, async ({ page }) => {
  await open(page, variant.scenario);
  await expect(card(page)).toHaveCount(1);
  await expect(card(page)).toContainText(variant.title);
  await expect(card(page).locator('.cu-approval-detail')).toContainText(variant.detail);
  await expect(card(page).getByRole('button')).toHaveText(variant.options);
  await expect(card(page).locator('.cu-approval-warning')).toHaveCount(variant.warning ? 1 : 0);
  // The thread row says it needs you, as other approvals do.
  await expect(page.locator('.task-item[data-id="session-1"] .task-status')).toHaveText('Approval');
  await card(page).getByRole('button', { name: variant.pick, exact: true }).click();
  await expect(card(page)).toHaveCount(0);
  const responses = await calls(page, 'computer_use_respond');
  expect(responses.map(call => call.args)).toEqual([{ sessionId: 'session-1', requestId: 'cu-1', decision: variant.decision }]);
  await expect(page.locator('.task-item[data-id="session-1"]')).not.toContainText('Approval');
});

test('the card closes when the approval ends elsewhere, and one for another thread waits on its row', async ({ page }) => {
  await open(page, 'control');
  await expect(card(page)).toHaveCount(1);
  await page.evaluate(() => (window as any).__cuResolve());
  await expect(card(page)).toHaveCount(0);
  await page.evaluate(() => (window as any).__cuApprove('destructive', 'session-2'));
  await expect(page.locator('.task-item[data-id="session-2"] .task-status')).toHaveText('Approval');
  await expect(card(page)).toHaveCount(0);
  await page.locator('.task-item[data-id="session-2"]').click();
  await expect(page.locator('#prompt')).toHaveAttribute('data-session-id', 'session-2');
  await expect(card(page)).toContainText('Press button "Delete" in Notes');
  // Answered elsewhere (or timed out): the event closes it.
  await page.evaluate(() => (window as any).__cuResolve());
  await expect(card(page)).toHaveCount(0);
  await expect(page.locator('.task-item[data-id="session-2"]')).not.toContainText('Approval');
});

test('a thread using the computer shows a quiet strip that opens its recent actions and goes when the turn ends', async ({ page }) => {
  await open(page, 'control');
  const strip = page.locator('#cu-activity .cu-activity');
  await expect(strip).toHaveCount(0);
  await page.evaluate(() => (window as any).__cuTool('session-1', 'click', 'running', 'call-1'));
  await expect(strip).toContainText('Clicking');
  await expect(strip).toContainText('1 action');
  await page.evaluate(() => (window as any).__cuTool('session-1', 'click', 'completed', 'call-1'));
  await page.evaluate(() => (window as any).__cuTool('session-1', 'screenshot', 'running', 'call-2'));
  await expect(strip).toContainText('Looking at the screen');
  await expect(strip).toContainText('2 actions');
  await strip.getByRole('button').click();
  await expect(strip.getByRole('list', { name: 'Recent computer actions' }).locator('li')).toHaveCount(3);
  await expect(strip.locator('.cu-activity-refused')).toContainText('CU-CON-008');
  // The run ended.
  await page.evaluate(() => (window as any).__previewEmit('agent-event', { sessionId: 'session-1', kind: 'done', text: '' }));
  await expect(strip).toHaveCount(0);
});
