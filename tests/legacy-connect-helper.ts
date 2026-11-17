import type { Page } from '@playwright/test';

/* The older shared relay (PandaCode Connect, signed in with GitHub) is hidden in Settings → Remote access unless
 * `legacyConnect.visible` (src/quick-connect-settings.ts). Specs that drive it start with it shown, through the
 * Quick connect preview mock's `legacyVisible` hook (src/preview-quick-connect.ts). */
export async function showLegacyConnect(page: Page): Promise<void> {
  await page.addInitScript(() => { const w = window as any; w.__quickConnect = { ...(w.__quickConnect ?? {}), legacyVisible: true }; });
}
