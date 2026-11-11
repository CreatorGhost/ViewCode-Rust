import { expect, type Locator, type Page } from '@playwright/test';

/** The composer's `+`: what can be attached, then every composer control that is not in the row. */
export const plus = (page: Page) => page.getByRole('button', { name: 'Add files and tools', exact: true });
export const plusMenu = (page: Page) => page.getByRole('dialog', { name: 'Add files and tools', exact: true });

/** Opens the `+` menu and returns it. */
export async function openPlus(page: Page): Promise<Locator> {
  await plus(page).click();
  await expect(plusMenu(page)).toBeVisible();
  return plusMenu(page);
}

/**
 * Presses a parked composer control (Team, Token saver, Council, the computer, the branch, a linked PR)
 * the way a person does: from the `+` menu, where its row carries the control's own name. The control
 * itself stays in the DOM, invisible, so its state can still be read from it.
 */
export async function pickFromPlus(page: Page, control: Locator): Promise<void> {
  await expect(control).toHaveCount(1);
  // A control renames itself as its state loads, so its row is looked up by the name it has once the menu is open.
  await expect(async () => {
    if (!await plusMenu(page).isVisible()) await plus(page).click();
    await expect(plusMenu(page)).toBeVisible();
    const name = (await control.getAttribute('aria-label'))!;
    try { await plusMenu(page).getByRole('menuitem', { name, exact: true }).click({ timeout: 4000 }); }
    catch (error) { await page.keyboard.press('Escape'); throw error; }
  }).toPass();
}
