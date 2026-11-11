import { modelOption, openPicker, pickerSearch, pickerTrigger, showHarness } from './model-picker-helper';
import { expect, test, type Page } from '@playwright/test';

async function newCursorTask(page: Page): Promise<void> {
  await page.addInitScript(() => localStorage.setItem('pandacode.provider', 'cursor'));
  await page.goto('/');
  await expect(page.locator('.task-item.selected .task-title')).toHaveText('Make search feel instant');
  await page.locator('.new-task-button').click();
  await expect(page.locator('#model-picker-trigger .provider-logo[data-provider="cursor"]')).toBeVisible();
}

test('Cursor is in the model picker with its models, their options and efforts, and its own modes', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await newCursorTask(page);

  const modes = page.getByRole('dialog', { name: 'Permissions', exact: true }).getByRole('menuitemradio');
  // A composer redraw while the app starts cancels a menu just opened, so try again until one stays.
  await expect(async () => {
    if (!await modes.count()) await page.locator('#mode-picker-trigger').click();
    await expect(modes).toHaveCount(4, { timeout: 1000 });
  }).toPass();
  await expect(modes.filter({ hasText: 'Standard' })).toContainText('Agent mode');
  await expect(modes.filter({ hasText: 'Plan mode' })).toContainText('Cursor’s Plan mode');
  await expect(modes.filter({ hasText: 'Full access' })).toContainText('--force');
  await page.keyboard.press('Escape');

  const picker = await openPicker(page);
  await showHarness(page, 'cursor');
  await expect(modelOption(page, 'cursor', 'default')).toContainText('Auto');
  // A model with options of its own is listed once per combination, each naming every option.
  const search = pickerSearch(page);
  await search.fill('GPT-5.4');
  await expect(picker.locator('.model-menu-option[data-provider="cursor"][data-model^="gpt-5.4["]')).toHaveCount(4);
  await expect(modelOption(page, 'cursor', 'gpt-5.4[context=272k,fast=false]')).toContainText('GPT-5.4');
  await expect(modelOption(page, 'cursor', 'gpt-5.4[context=1m,fast=true]')).toContainText('GPT-5.4 · 1M · Fast');
  await search.fill('Thinking off');
  await expect(modelOption(page, 'cursor', 'claude-opus-4-6[thinking=false]')).toContainText('Opus 4.6 · Thinking off');
  await search.fill('1M · Fast');
  await modelOption(page, 'cursor', 'gpt-5.4[context=1m,fast=true]').click();
  await expect(pickerTrigger(page)).toContainText('GPT-5.4 · 1M · Fast');

  expect(errors).toEqual([]);
});

test('each Cursor model carries the efforts of its own reasoning option', async ({ page }) => {
  await newCursorTask(page);
  const picker = await openPicker(page);
  const search = pickerSearch(page);
  await showHarness(page, 'cursor');
  await search.fill('1M · Fast');
  await modelOption(page, 'cursor', 'gpt-5.4[context=1m,fast=true]').click();
  // Five for GPT-5.4 and three for Opus, each after the harness default.
  await openPicker(page);
  const effort = picker.getByRole('radiogroup', { name: 'Effort' }).getByRole('radio');
  await expect(effort).toHaveCount(6);
  await expect(effort.filter({ hasText: 'Extra High' })).toHaveCount(1);
  await search.fill('Thinking off');
  await modelOption(page, 'cursor', 'claude-opus-4-6[thinking=false]').click();
  await openPicker(page);
  await expect(effort).toHaveCount(4);
  await page.keyboard.press('Escape');
});

test('Settings lists Cursor with its install and sign-in steps and an executable location', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('region', { name: 'Settings', exact: true });
  await settings.getByRole('tab', { name: 'Agents', exact: true }).click();
  await settings.getByRole('tab', { name: 'Providers', exact: true }).click();
  await page.locator('#settings-panel-providers details.connection-card').first().waitFor();
  const card = page.locator('.connection-card').filter({ has: page.locator('form[data-env-provider="cursor"]') });
  await expect(card.locator('h3')).toContainText('Cursor');
  await expect(card.locator('h3 .provider-logo[data-provider="cursor"]')).toHaveCount(1);
  await expect(card.locator('.connection-state')).toHaveText(/Installed/);
  await card.locator('summary').first().click();
  await expect(card).toContainText('cursor-agent login');
  await expect(card).toContainText('cursor-agent acp');
  await expect(card).toContainText('cursor-agent update');
  await expect(card.getByRole('link', { name: 'Install the Cursor CLI' })).toHaveAttribute('href', 'https://cursor.com/docs/cli/installation');
  await card.locator('details.connection-path > summary').click();
  await expect(card.getByLabel('Cursor CLI executable', { exact: true })).toBeVisible();
  // One card only: the hand-written one stands in for the generated one.
  await expect(page.locator('form[data-env-provider="cursor"]')).toHaveCount(1);
});

test('a Cursor thread streams its tool call and answer in the preview', async ({ page }) => {
  await newCursorTask(page);
  await page.getByRole('textbox', { name: 'Message your coding agent' }).fill('Explain how search is indexed');
  await page.getByRole('button', { name: 'Send message', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeVisible();
  await expect(page.locator('.task-item.selected .task-title')).toHaveText('Explain how search is indexed');
  // The answer arrives in chunks and the run settles.
  await expect(page.getByText('This is the browser preview.', { exact: false }).first()).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toHaveCount(0);
  await expect(page.getByText('no agent or shell is running here', { exact: false }).first()).toBeVisible();
  // The task stays a Cursor task.
  await expect(page.locator('#model-picker-trigger .provider-logo[data-provider="cursor"]')).toBeVisible();
});
