import { expect, test, type Locator, type Page } from '@playwright/test';
import { runCommand } from './rail-helper';

/* Connect phone and Quick connect (src/connect-phone.ts, src/quick-connect-settings.ts) on the preview mock
 * (src/preview-quick-connect.ts): `__quickConnect` steers it, `__quickConnectAdvance()` moves a stepped setup on,
 * `__quickConnectPair()` plays the phone. The mock's link is a recognisably fake one (`PREVIEW-TOKEN-n`). */
type Hooks = Record<string, unknown>;

async function open(page: Page, hooks: Hooks = {}) {
  await page.addInitScript(h => {
    const w = window as any;
    w.__quickConnect = { ...h };
    // Links the app opens in the browser are recorded instead of opening tabs.
    w.__opened = [];
    w.open = (url: string) => { w.__opened.push(String(url)); return null; };
  }, hooks);
  await page.goto('/');
  await page.locator('#navigation .task-item').first().waitFor();
}
async function openDialog(page: Page): Promise<Locator> {
  await page.locator('.sidebar-bottom [data-action="connect-phone"]').click();
  const dialog = page.getByRole('dialog', { name: 'Connect phone' });
  await expect(dialog).toBeVisible();
  return dialog;
}
const advance = (page: Page) => page.evaluate(() => (window as any).__quickConnectAdvance());
const setHook = (page: Page, key: string, value: unknown) => page.evaluate(([k, v]) => { (window as any).__quickConnect[k as string] = v; }, [key, value] as const);
const calls = (page: Page) => page.evaluate(() => ((window as any).__quickConnectCalls ?? []).map((call: any) => ({ ...call })));
const commands = async (page: Page) => (await calls(page)).map((call: any) => call.command);
const opened = (page: Page) => page.evaluate(() => (window as any).__opened as string[]);
const qr = (dialog: Locator) => dialog.getByRole('img', { name: 'Pairing code for the PandaCode phone app' });
/** Starts a stepped setup and waits for its first step, so `advance` never races the click. */
async function startStepped(dialog: Locator) {
  await dialog.getByRole('button', { name: 'Connect with Cloudflare' }).click();
  await expect(dialog.getByRole('list', { name: 'Setup steps' })).toBeVisible();
}
const step = (dialog: Locator, label: string) => dialog.getByRole('list', { name: 'Setup steps' }).locator('li', { hasText: label });

async function openRemoteSettings(page: Page): Promise<Locator> {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('tab', { name: 'Connections', exact: true }).click();
  await page.getByRole('tab', { name: 'Remote access' }).click();
  return page.getByRole('tabpanel', { name: 'Remote access' });
}
/** Every attribute value and the page's text: the pairing link must appear in neither. */
const leaks = (page: Page) => page.evaluate(() => {
  const hits: string[] = [];
  const secret = /PREVIEW-TOKEN|pandacode:\/\/quick-connect/;
  for (const element of document.querySelectorAll('*')) for (const attribute of element.attributes) if (secret.test(attribute.value)) hits.push(`${element.tagName}[${attribute.name}]`);
  if (secret.test(document.documentElement.outerHTML)) hits.push('markup');
  for (const input of document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('input,textarea')) if (secret.test(input.value)) hits.push('field value');
  return hits;
});

test.describe('quick connect', () => {
  test('the whole setup: each step in ViewCode\'s words, the sign-in link and code, then the pairing code', async ({ page }) => {
    await open(page, { steps: true });
    const dialog = await openDialog(page);
    await expect(dialog.getByRole('group', { name: 'Phone connection' }).getByRole('button', { name: 'Anywhere' })).toHaveAttribute('aria-pressed', 'true');
    await expect(dialog.getByText('PandaCode sets up a small relay on your own free Cloudflare account.', { exact: false })).toBeVisible();
    await startStepped(dialog);
    await expect(step(dialog, 'Signing in to Cloudflare')).toHaveAttribute('aria-current', 'step');
    await expect(step(dialog, 'Signing in to Cloudflare')).toContainText('Getting ready…');
    await expect(step(dialog, 'Setting up your relay')).toHaveAttribute('data-state', 'pending');

    await advance(page);
    await expect(dialog.getByLabel('Sign-in code', { exact: true })).toHaveText('WDJB-MJHT');
    await expect(dialog.getByText('Approve PandaCode in the browser window that opened.', { exact: false })).toBeVisible();
    await expect(dialog.getByText('https://dash.cloudflare.com/oauth2/device', { exact: true })).toBeVisible();
    // The app opens Cloudflare's page once by itself, then on request through its external-link opener.
    await expect.poll(() => opened(page)).toEqual(['https://dash.cloudflare.com/oauth2/device?user_code=WDJB-MJHT']);
    await dialog.getByRole('button', { name: 'Open link again' }).click();
    await expect.poll(() => opened(page)).toHaveLength(2);
    await expect(dialog.getByRole('button', { name: 'Create one free' })).toBeVisible();

    await advance(page);
    await expect(step(dialog, 'Setting up your relay')).toHaveAttribute('aria-current', 'step');
    await expect(step(dialog, 'Signing in to Cloudflare')).toHaveAttribute('data-state', 'done');
    await advance(page);
    await expect(step(dialog, 'Securing it')).toHaveAttribute('aria-current', 'step');
    await advance(page);
    await expect(step(dialog, 'Checking it works')).toHaveAttribute('aria-current', 'step');
    await advance(page);

    await expect(qr(dialog)).toBeVisible();
    await expect(qr(dialog).locator('svg path')).toHaveCount(1);
    await expect(dialog.getByText(/This code works once\. Expires in [45]:\d\d\./)).toBeVisible();
    await expect(dialog.getByText('Uses pandacode-quick-connect.preview.workers.dev')).toBeVisible();
    await expect(dialog.getByText('Pair once; it reconnects on its own.')).toBeVisible();
    expect(await commands(page)).toEqual(expect.arrayContaining(['quick_connect_start_setup', 'quick_connect_pairing_create']));
    expect(await leaks(page)).toEqual([]);
  });

  test('cancel stops setup mid-way and returns to the start', async ({ page }) => {
    await open(page, { steps: true });
    const dialog = await openDialog(page);
    await startStepped(dialog);
    await advance(page);
    await expect(dialog.getByLabel('Sign-in code', { exact: true })).toBeVisible();
    await advance(page);
    await expect(step(dialog, 'Setting up your relay')).toHaveAttribute('aria-current', 'step');
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Connect with Cloudflare' })).toBeVisible();
    await expect(dialog.getByRole('list', { name: 'Setup steps' })).toHaveCount(0);
    expect(await commands(page)).toContain('quick_connect_cancel');
  });

  test('several Cloudflare accounts: the app asks which one holds the relay', async ({ page }) => {
    await open(page, { accounts: 2 });
    const dialog = await openDialog(page);
    await dialog.getByRole('button', { name: 'Connect with Cloudflare' }).click();
    await expect(dialog.getByText('Which Cloudflare account should hold the relay?')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Personal' })).toBeVisible();
    await dialog.getByRole('button', { name: 'Work' }).click();
    await expect(qr(dialog)).toBeVisible();
    expect(await calls(page)).toContainEqual({ command: 'quick_connect_choose_account', accountId: 'bbbb2222' });
  });

  test('a missing workers.dev address: open Cloudflare, then Continue', async ({ page }) => {
    await open(page, { subdomain: true });
    const dialog = await openDialog(page);
    await dialog.getByRole('button', { name: 'Connect with Cloudflare' }).click();
    await expect(dialog.getByText('Your Cloudflare account needs a free workers.dev address first.', { exact: false })).toBeVisible();
    await dialog.getByRole('button', { name: 'Open Cloudflare' }).click();
    await expect.poll(() => opened(page)).toEqual(['https://dash.cloudflare.com/?to=/:account/workers-and-pages']);
    await setHook(page, 'subdomain', false);
    await dialog.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect(qr(dialog)).toBeVisible();
    expect(await commands(page)).toContain('quick_connect_continue');
  });

  test('a denied sign-in fails plainly, with Try again and Back', async ({ page }) => {
    await open(page, { steps: true, signInDenied: true });
    const dialog = await openDialog(page);
    await startStepped(dialog);
    await advance(page);
    await expect(dialog.getByLabel('Sign-in code', { exact: true })).toBeVisible();
    await advance(page);
    await expect(dialog.getByRole('alert')).toHaveText('Cloudflare sign-in did not finish. Try again.');
    await dialog.locator('summary', { hasText: 'Details' }).click();
    await expect(dialog.getByText('Login denied.')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Try again' })).toBeVisible();
    await dialog.getByRole('button', { name: 'Back' }).click();
    await expect(dialog.getByRole('button', { name: 'Connect with Cloudflare' })).toBeVisible();
  });

  test('Node.js missing: the failure offers Install Node.js', async ({ page }) => {
    await open(page, { fail: 'node' });
    const dialog = await openDialog(page);
    await dialog.getByRole('button', { name: 'Connect with Cloudflare' }).click();
    await expect(dialog.getByRole('alert')).toContainText('needs Node.js 22 or newer');
    await dialog.getByRole('button', { name: 'Install Node.js' }).click();
    await expect.poll(() => opened(page)).toEqual(['https://nodejs.org/']);
  });

  test('an unreachable relay says so, and Try again connects once the network allows it', async ({ page }) => {
    await open(page, { unreachable: 'network-refused' });
    const dialog = await openDialog(page);
    await dialog.getByRole('button', { name: 'Connect with Cloudflare' }).click();
    await expect(dialog.getByRole('alert')).toHaveText('Your relay is set up, but its address is not answering yet. Try again later from Settings.');
    await expect(qr(dialog)).toHaveCount(0);
    await setHook(page, 'unreachable', undefined);
    await dialog.getByRole('button', { name: 'Try again' }).click();
    await expect(qr(dialog)).toBeVisible();
    expect(await calls(page)).toContainEqual({ command: 'quick_connect_set_enabled', enabled: true });
  });

  test('a rejected secret offers Redeploy with a new secret', async ({ page }) => {
    await open(page, { unreachable: 'credential-rejected' });
    const dialog = await openDialog(page);
    await dialog.getByRole('button', { name: 'Connect with Cloudflare' }).click();
    await expect(dialog.getByRole('alert')).toContainText('Redeploy with a new secret to fix it.');
    await setHook(page, 'unreachable', undefined);
    await dialog.getByRole('button', { name: 'Redeploy with a new secret' }).click();
    await expect(qr(dialog)).toBeVisible();
    expect(await calls(page)).toContainEqual({ command: 'quick_connect_start_setup', rotateSecret: true });
  });

  test('pairing: the QR counts down, expires, New code makes another, and a phone pairs', async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await open(page, { preset: 'ready', pairingTtlMs: 4_000 });
    const dialog = await openDialog(page);
    await expect(qr(dialog)).toBeVisible();
    const countdown = dialog.locator('.qc-countdown');
    await expect(countdown).toHaveText(/^0:0[2-4]$/);
    const first = await countdown.textContent();
    await expect.poll(() => countdown.textContent(), { timeout: 3_000 }).not.toBe(first);
    await dialog.getByRole('button', { name: 'Copy link' }).click();
    // The clipboard holds the link (the mock's fake one); it is checked by shape, never printed.
    expect(await page.evaluate(async () => /^pandacode:\/\/quick-connect\?v=1&relay=https%3A%2F%2F/.test(await navigator.clipboard.readText()))).toBe(true);

    await expect(dialog.getByText('This code expired', { exact: true })).toBeVisible({ timeout: 8_000 });
    await expect(qr(dialog)).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: 'Copy link' })).toHaveCount(0);
    expect(await leaks(page)).toEqual([]);

    await setHook(page, 'pairingTtlMs', 300_000);
    await dialog.getByRole('button', { name: 'New code' }).click();
    await expect(qr(dialog)).toBeVisible();
    await expect(countdown).toHaveText(/^[45]:\d\d$/);
    await page.evaluate(() => (window as any).__quickConnectPair('Test phone'));
    await expect(dialog.getByText('Phone connected')).toBeVisible();
    await expect(dialog.getByText('Each code works once. Make a new one for another device.')).toBeVisible();
    await expect(qr(dialog)).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: 'New code' })).toBeVisible();
    expect((await commands(page)).filter(command => command === 'quick_connect_pairing_create')).toHaveLength(2);
  });

  test('the pairing link is in no attribute, and is gone from the page once the dialog closes', async ({ page }) => {
    await open(page, { preset: 'ready' });
    const dialog = await openDialog(page);
    await expect(qr(dialog)).toBeVisible();
    expect(await leaks(page)).toEqual([]);
    await dialog.getByRole('button', { name: 'Close dialog' }).click();
    await expect(dialog).toBeHidden();
    expect(await leaks(page)).toEqual([]);
    await expect(page.locator('[data-connect-phone]')).toHaveCount(0);
    // The unused code ends with the dialog.
    await expect.poll(() => commands(page)).toContain('quick_connect_pairing_cancel');
  });

  test('connection choices: Same Wi-Fi and Tailscale say why they are not available', async ({ page }) => {
    await open(page, { tailscaleInstalled: true });
    const dialog = await openDialog(page);
    const anywhere = dialog.getByRole('group', { name: 'Anywhere method' });
    await expect(anywhere.getByRole('button', { name: 'Quick connect' })).toHaveAttribute('aria-pressed', 'true');
    await anywhere.getByRole('button', { name: 'Tailscale' }).click();
    await expect(dialog.getByText('Tailscale is not available yet')).toBeVisible();
    await expect(dialog.getByText('this needs the app\'s local network listener', { exact: false })).toBeVisible();
    await dialog.getByRole('group', { name: 'Phone connection' }).getByRole('button', { name: 'Same Wi-Fi' }).click();
    await expect(dialog.getByText('Same Wi-Fi is not available yet')).toBeVisible();
    await expect(dialog.getByRole('group', { name: 'Anywhere method' })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Use Quick connect instead' }).click();
    await expect(dialog.getByRole('button', { name: 'Connect with Cloudflare' })).toBeVisible();
    await expect.poll(async () => (await calls(page)).filter((call: any) => call.command === 'quick_connect_set_mode').map((call: any) => call.mode)).toEqual(['tailscale', 'sameWifi', 'quickConnect']);
  });

  test('Tailscale is not offered when it is not installed', async ({ page }) => {
    await open(page);
    const dialog = await openDialog(page);
    await expect(dialog.getByRole('group', { name: 'Phone connection' })).toBeVisible();
    await expect(dialog.getByRole('group', { name: 'Anywhere method' })).toHaveCount(0);
  });

  test('the ⋯ menu: turn off and on, redeploy, redeploy with a new secret (phones stay paired), remove', async ({ page }) => {
    await open(page, { preset: 'ready' });
    const dialog = await openDialog(page);
    const menu = async (item: string) => {
      await dialog.getByRole('button', { name: 'Quick connect options' }).click();
      await page.getByRole('menuitem', { name: item }).click();
    };
    await expect(qr(dialog)).toBeVisible();
    await menu('Turn off Quick connect');
    await expect(dialog.getByText('Your relay is set up but turned off. Turn it on to show a code for your phone.')).toBeVisible();
    await expect(qr(dialog)).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Use existing relay' }).click();
    await expect(qr(dialog)).toBeVisible();

    await menu('Redeploy relay');
    await expect(qr(dialog)).toBeVisible();
    await dialog.getByRole('button', { name: 'Quick connect options' }).click();
    await expect(page.getByRole('menuitem', { name: 'Redeploy with a new secret' })).toContainText('Phones stay paired.');
    await page.getByRole('menuitem', { name: 'Redeploy with a new secret' }).click();
    await expect(page.locator('#toast')).toContainText('Paired phones stay paired.');
    await expect(qr(dialog)).toBeVisible();
    const deploys = (await calls(page)).filter((call: any) => call.command === 'quick_connect_start_setup');
    expect(deploys).toEqual([{ command: 'quick_connect_start_setup', rotateSecret: false }, { command: 'quick_connect_start_setup', rotateSecret: true }]);

    await menu('Remove Quick connect');
    const confirm = dialog.getByRole('group', { name: 'Remove Quick connect?' });
    await expect(confirm).toContainText('deletes the relay from your Cloudflare account');
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    await expect(qr(dialog)).toBeVisible();
    await menu('Remove Quick connect');
    await confirm.getByRole('button', { name: 'Remove Quick connect' }).click();
    await expect(dialog.getByText('Quick connect is removed.')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Connect with Cloudflare' })).toBeVisible();

    await dialog.getByRole('button', { name: 'Connect with Cloudflare' }).click();
    await expect(qr(dialog)).toBeVisible();
    await menu('Remove from this computer only');
    await dialog.getByRole('group', { name: 'Remove Quick connect from this computer only?' }).getByRole('button', { name: 'Remove from this computer' }).click();
    await expect(dialog.getByText('Quick connect is removed from this computer. The relay is still on your Cloudflare account.')).toBeVisible();
    expect((await calls(page)).filter((call: any) => call.command === 'quick_connect_remove')).toEqual([{ command: 'quick_connect_remove', localOnly: false }, { command: 'quick_connect_remove', localOnly: true }]);
  });

  test('when Cloudflare cannot remove the relay, it can be removed from this computer only', async ({ page }) => {
    await open(page, { preset: 'ready', removeFails: true });
    const dialog = await openDialog(page);
    await dialog.getByRole('button', { name: 'Quick connect options' }).click();
    await page.getByRole('menuitem', { name: 'Remove Quick connect' }).click();
    await dialog.getByRole('group', { name: 'Remove Quick connect?' }).getByRole('button', { name: 'Remove Quick connect' }).click();
    await expect(dialog.getByRole('alert')).toContainText('Could not delete the relay from Cloudflare.');
    await dialog.getByRole('button', { name: 'Remove from this computer only' }).click();
    await expect(dialog.getByText('Quick connect is removed from this computer.', { exact: false })).toBeVisible();
  });

  test('Settings lists paired phones with last seen and when each pairs again, and revokes after a confirm', async ({ page }) => {
    await open(page, { preset: 'ready' });
    const panel = await openRemoteSettings(page);
    const phones = panel.getByRole('region', { name: 'Paired phones', exact: true });
    await expect(phones.locator('.qc-device')).toHaveCount(2);
    await expect(phones.locator('.qc-device', { hasText: 'Preview iPhone' })).toContainText(/Online now · Pairs again after \w+ \d+/);
    await expect(phones.locator('.qc-device', { hasText: 'Preview Pixel' })).toContainText(/Last seen 3 days ago · Pairs again after \w+ \d+/);
    await phones.getByRole('button', { name: 'Revoke Preview Pixel' }).click();
    const confirm = phones.getByRole('group', { name: 'Revoke Preview Pixel?' });
    await expect(confirm).toContainText('It stops reaching this computer at once');
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    await expect(phones.locator('.qc-device')).toHaveCount(2);
    await phones.getByRole('button', { name: 'Revoke Preview Pixel' }).click();
    await confirm.getByRole('button', { name: 'Revoke' }).click();
    await expect(phones.locator('.qc-device')).toHaveCount(1);
    await expect(phones).not.toContainText('Preview Pixel');
    expect(await calls(page)).toContainEqual({ command: 'quick_connect_revoke_device', deviceId: 'phone-b' });
  });

  test('Settings: the Quick connect row sets up through the dialog and turns it off and on', async ({ page }) => {
    await open(page);
    const panel = await openRemoteSettings(page);
    const phone = panel.getByRole('region', { name: 'Phone', exact: true });
    await expect(phone).toContainText('Not set up. Connect it to your own free Cloudflare account.');
    await phone.getByRole('button', { name: 'Set up' }).click();
    const dialog = page.getByRole('dialog', { name: 'Connect phone' });
    await dialog.getByRole('button', { name: 'Connect with Cloudflare' }).click();
    await expect(qr(dialog)).toBeVisible();
    await dialog.getByRole('button', { name: 'Close dialog' }).click();
    await expect(phone).toContainText('Connected through pandacode-quick-connect.preview.workers.dev.');
    const toggle = phone.getByRole('switch', { name: 'Quick connect' });
    await expect(toggle).toBeChecked();
    await toggle.click();
    await expect(phone).toContainText('Off.');
    await expect(phone.getByRole('switch', { name: 'Quick connect' })).not.toBeChecked();
    await phone.getByRole('button', { name: 'Connect phone' }).click();
    await expect(page.getByRole('dialog', { name: 'Connect phone' }).getByRole('button', { name: 'Use existing relay' })).toBeVisible();
  });

  test('the older GitHub relay is hidden until asked for, then shown in Settings', async ({ page }) => {
    await open(page);
    const panel = await openRemoteSettings(page);
    await expect(panel.getByRole('region', { name: 'Phone', exact: true })).toBeVisible();
    await expect(panel.getByRole('button', { name: 'Sign in with GitHub' })).toHaveCount(0);
    await runCommand(page, 'Show or hide PandaCode Connect (GitHub)');
    await expect(panel.getByRole('button', { name: 'Sign in with GitHub' })).toBeVisible();
    await runCommand(page, 'Show or hide PandaCode Connect (GitHub)');
    await expect(panel.getByRole('button', { name: 'Sign in with GitHub' })).toHaveCount(0);
  });

  test('a legacy relay already shown stays reachable', async ({ page }) => {
    await open(page, { legacyVisible: true });
    const panel = await openRemoteSettings(page);
    await expect(panel.getByRole('button', { name: 'Sign in with GitHub' })).toBeVisible();
    await expect(panel.getByRole('region', { name: 'Phone', exact: true })).toBeVisible();
  });
});

// Dark-theme screenshots for review: `QC_SHOTS=<folder>` writes them; a normal run skips this.
test.describe('quick connect screenshots', () => {
  test.skip(!process.env.QC_SHOTS, 'set QC_SHOTS to a folder to write screenshots');
  test.use({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
  const shot = (name: string) => `${process.env.QC_SHOTS}/${name}`;

  test('screenshots', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await open(page, { steps: true, tailscaleInstalled: true });
    const dialog = await openDialog(page);
    await expect(dialog.getByRole('button', { name: 'Connect with Cloudflare' })).toBeVisible();
    await page.screenshot({ path: shot('qc-choice.png') });
    await startStepped(dialog);
    await advance(page);
    await expect(dialog.getByLabel('Sign-in code', { exact: true })).toBeVisible();
    await page.screenshot({ path: shot('qc-setup-signin.png') });
    for (const label of ['Setting up your relay', 'Securing it', 'Checking it works']) { await advance(page); await expect(step(dialog, label)).toHaveAttribute('aria-current', 'step'); }
    await advance(page);
    await expect(qr(dialog)).toBeVisible();
    await expect(qr(dialog).locator('svg')).toBeVisible();
    await page.screenshot({ path: shot('qc-pairing.png') });
  });

  test('devices screenshot', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await open(page, { preset: 'ready' });
    const panel = await openRemoteSettings(page);
    await expect(panel.locator('.qc-device')).toHaveCount(2);
    await page.screenshot({ path: shot('qc-devices.png') });
  });
});
