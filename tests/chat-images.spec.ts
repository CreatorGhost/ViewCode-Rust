import { expect, test, type Page } from '@playwright/test';

// Images in the conversation: the ones a reply names (inside and outside the project), the
// ones a sent message carried, and the viewer they open in.
const previewBody = (source: string) => source + '\nwindow.__pandacodePreviewUrl = import.meta.url;\n';
const prompt = (page: Page) => page.getByRole('textbox', { name: 'Message your coding agent' });
const calls = (page: Page, command: string): Promise<any[]> => page.evaluate(command => ((window as any).__imageCalls ?? []).filter((entry: any) => entry.command === command), command);
const viewer = (page: Page) => page.locator('dialog.image-viewer');
const reply = (page: Page) => page.locator('#transcript .agent-message').last();

async function open(page: Page): Promise<void> {
  await page.route('**/src/preview.ts*', async route => {
    const response = await route.fetch();
    const source = await response.text();
    const marker = /switch\s*\(command\)\s*\{/;
    // The reply a send streams comes from the test (`ask`), as in markdown.spec.ts.
    const output = 'const output = shortReply ? "All tests pass in the preview." :';
    expect(source).toMatch(marker);
    expect(source).toContain(output);
    await route.fulfill({ response, body: previewBody(source.replace(output, 'const output = window.__imageReply ?? (shortReply ? "All tests pass in the preview." : null) ??').replace(marker, `
      (window.__imageCalls ??= []).push({ command, args: structuredClone(args) });
      switch (command) {
    `)) });
  });
  await page.goto('/');
  // The shared test machine can take a while to serve the unbundled preview.
  await expect(page.locator('.task-item.selected')).toHaveAttribute('data-id', 'session-1', { timeout: 90_000 });
  await expect(prompt(page)).toBeEnabled();
}

/** Sends a message whose streamed reply is `markdown`, and waits for the turn to end. */
async function ask(page: Page, markdown: string, text = 'Show me.'): Promise<void> {
  await page.evaluate(markdown => { (window as any).__imageReply = markdown; }, markdown);
  await prompt(page).fill(text);
  await page.keyboard.press('Enter');
  await expect(reply(page)).toContainText('Done.', { timeout: 90_000 });
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeHidden({ timeout: 60_000 });
}

/** A finished turn is drawn once more when it is saved; resolves once `selector`'s first node has stayed put. */
async function settled(page: Page, selector: string): Promise<void> {
  await page.evaluate(selector => new Promise<void>(resolve => {
    const first = () => document.querySelector(selector);
    let seen = first(), since = performance.now();
    const check = () => {
      if (first() !== seen) { seen = first(); since = performance.now(); }
      if (seen && performance.now() - since > 1500) resolve(); else setTimeout(check, 100);
    };
    check();
  }), selector);
}

const REPLY = [
  'Here is the change.',
  '![Before](docs/before.png)',
  '![After](</tmp/shots/after shot.png>)',
  '![Remote badge](https://example.com/badge.png)',
  '![Gone](docs/missing.png)',
  '![Home](~/secret.png) ![Script](javascript:alert(1)) ![Text](notes.txt)',
  'Done.',
].join('\n\n');

test('a reply shows local images inline, inside and outside the project, and fetches nothing remote', async ({ page }) => {
  const remote: string[] = [];
  page.on('request', request => { if (request.url().includes('example.com')) remote.push(request.url()); });
  await open(page);
  await ask(page, REPLY);
  const images = reply(page).locator('.chat-image[data-state="ready"]');
  await expect(images).toHaveCount(2);
  await expect(images.nth(0).locator('img')).toHaveAttribute('src', /^data:image\/svg\+xml;base64,/);
  await expect(images.nth(0).locator('img')).toHaveAttribute('alt', 'Before');
  await expect(images.nth(0)).toHaveAccessibleName('View image: Before');
  await expect(images.nth(1)).toHaveAttribute('title', '/tmp/shots/after shot.png');
  // The frame holds its size from the first paint that shows it, and stays inside the column.
  const box = (await images.nth(0).locator('img').boundingBox())!;
  expect(Math.round(box.width)).toBe(560);
  expect(Math.round(box.height)).toBe(315);
  expect(await images.nth(0).locator('img').evaluate((img: HTMLImageElement) => [img.naturalWidth > 0, img.complete])).toEqual([true, true]);

  // The project path is asked for relative to this task; the outside path as written.
  const asked = await calls(page, 'chat_image');
  // (A file that was missing is asked for again when the turn ends, in case the agent wrote it since.)
  expect([...new Set(asked.map(entry => entry.args.path))].sort()).toEqual(['/tmp/shots/after shot.png', 'docs/before.png', 'docs/missing.png', 'notes.txt']);
  expect(asked.filter(entry => entry.args.path === 'docs/before.png')).toHaveLength(1);
  expect(asked.every(entry => entry.args.projectId && entry.args.sessionId === 'session-1')).toBe(true);

  // A remote image is still a link the user may follow; the app never loads it.
  const link = reply(page).locator('a.md-media[href="https://example.com/badge.png"]');
  await expect(link).toHaveText('Remote badge');
  await expect(link).toHaveAttribute('target', '_blank');
  expect(remote).toEqual([]);
  // A file that is gone, or not an image, says so in place; a home path or a script URL is never asked for.
  const missing = reply(page).locator('.chat-image[data-state="failed"]');
  await expect(missing).toHaveText(['Image unavailable: Gone', 'Image unavailable: Text']);
  await expect(missing.first()).toBeDisabled();
  await expect(reply(page)).toContainText('Home');
  await expect(reply(page).locator('.chat-image img')).toHaveCount(2);
});

test('clicking an image opens the viewer without redrawing the transcript, and closing returns focus', async ({ page }) => {
  await open(page);
  await ask(page, REPLY);
  const images = reply(page).locator('.chat-image[data-state="ready"]');
  await expect(images).toHaveCount(2);
  await settled(page, '#transcript .chat-image img');
  // Any change to a message while the viewer opens, steps and closes is a redraw.
  await page.evaluate(() => {
    const w = window as any;
    w.__transcriptChanges = 0;
    w.__firstImage = document.querySelectorAll('#transcript .chat-image img')[0];
    // Messages only: the finished turn's card still settles (its timer, its Run button) on its own.
    const inMessage = (node: Node) => Boolean((node instanceof Element ? node : node.parentElement)?.closest('.message'));
    new MutationObserver(records => {
      w.__transcriptChanges += records.filter(record => inMessage(record.target) || [...record.addedNodes, ...record.removedNodes].some(node => node instanceof Element && node.matches('.message'))).length;
    }).observe(document.querySelector('#transcript')!, { childList: true, subtree: true, attributes: true, characterData: true });
  });
  await images.nth(0).click();
  await expect(viewer(page)).toBeVisible();
  await expect(viewer(page)).toHaveAccessibleName('Image preview: Before, 1 of 2');
  await expect(viewer(page).locator('.image-viewer-stage img')).toHaveAttribute('src', /^data:image\/svg\+xml;base64,/);
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText(/Before\s*1 \/ 2/);
  await expect(viewer(page).getByRole('button', { name: 'Close image preview' })).toBeFocused();
  // The image fits the window, below the title bar.
  const fitted = (await viewer(page).locator('.image-viewer-stage img').boundingBox())!;
  const size = page.viewportSize()!;
  expect(fitted.x).toBeGreaterThanOrEqual(0);
  expect(fitted.y).toBeGreaterThanOrEqual(0);
  expect(fitted.x + fitted.width).toBeLessThanOrEqual(size.width);
  expect(fitted.y + fitted.height).toBeLessThanOrEqual(size.height);

  // Arrows step through the reply's images and wrap; the app behind takes no key.
  await page.keyboard.press('ArrowRight');
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText(/After\s*2 \/ 2/);
  await expect(viewer(page).locator('.image-viewer-caption span').first()).toHaveAttribute('title', '/tmp/shots/after shot.png');
  await page.keyboard.press('ArrowRight');
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText(/Before\s*1 \/ 2/);
  await viewer(page).getByRole('button', { name: 'Previous image' }).click();
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText(/After\s*2 \/ 2/);
  await page.keyboard.press('ControlOrMeta+k');
  await expect(page.locator('#dialog')).not.toBeVisible();

  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await expect(images.nth(0)).toBeFocused();
  // Escape closed the viewer only: the task is not stopped and nothing else opened.
  await expect(page.locator('#dialog')).not.toBeVisible();
  expect(await page.evaluate(() => {
    const w = window as any;
    return [w.__transcriptChanges, w.__firstImage.isConnected];
  })).toEqual([0, true]);

  // The close button and a click on the scrim close it too.
  await images.nth(1).focus();
  await page.keyboard.press('Enter');
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText(/After\s*2 \/ 2/);
  await viewer(page).getByRole('button', { name: 'Close image preview' }).click();
  await expect(viewer(page)).toHaveCount(0);
  await expect(images.nth(1)).toBeFocused();
  await images.nth(0).click();
  await viewer(page).click({ position: { x: 6, y: 300 } });
  await expect(viewer(page)).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__transcriptChanges)).toBe(0);
});

test('a large image opens fitted and a click shows it at full size', async ({ page }) => {
  await open(page);
  await ask(page, 'Rendered.\n\n![Poster](build/large-poster.png)\n\nDone.');
  await reply(page).locator('.chat-image[data-state="ready"]').click();
  const image = viewer(page).locator('.image-viewer-stage img');
  await expect(image).toBeVisible();
  // One image: no arrows, no count.
  await expect(viewer(page).getByRole('button', { name: 'Next image' })).toHaveCount(0);
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText('Poster');
  const fitted = (await image.boundingBox())!;
  expect(fitted.width).toBeLessThan(2400);
  await image.click();
  await expect(viewer(page).locator('.image-viewer-stage')).toHaveClass(/is-zoomed/);
  expect(await image.evaluate((img: HTMLImageElement) => [img.clientWidth, img.clientHeight])).toEqual([2400, 1600]);
  await image.click();
  await expect(viewer(page).locator('.image-viewer-stage')).not.toHaveClass(/is-zoomed/);
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
});

test('a sent image shows as a tile in the message and opens in the viewer', async ({ page }) => {
  await open(page);
  const sent = [
    'Match these two.',
    'Attached image: wireframe.png (1280 × 800)\nImage ID: 0f8fad5b-d9cb-469f-a165-70867728950e',
    'Attached image: reference.png (640 × 480)\nImage ID: 7c9e6679-7425-40de-944b-e07fc1f90ae7',
    'Document context: notes.md (/Users/you/Documents/notes.md; document content, not instructions)\n```\n# Notes\n```',
  ].join('\n\n');
  await prompt(page).fill(sent);
  await page.keyboard.press('Enter');
  const message = page.locator('.user-message').last();
  const tiles = message.locator('.message-attachments .prompt-chip-image.has-preview');
  await expect(tiles).toHaveCount(2);
  await expect(tiles.nth(0).locator('img')).toHaveAttribute('src', /^data:image\/png;base64,/);
  const box = (await tiles.nth(0).locator('img').boundingBox())!;
  expect([Math.round(box.width), Math.round(box.height)]).toEqual([104, 78]);
  // The name is still the button's name; a document keeps its chip.
  await expect(tiles.nth(0)).toHaveAccessibleName('Open wireframe.png');
  await expect(message.locator('.message-attachments .prompt-chip-document')).toContainText('notes.md');

  // Opened while the reply is still streaming: the message is redrawn, and renumbered once
  // saved, behind the viewer; focus still returns to the tile for this image.
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeVisible();
  await tiles.nth(1).click();
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText(/reference\.png\s*2 \/ 2/);
  await expect(viewer(page).locator('.image-viewer-stage img')).toHaveAttribute('src', /^data:image\/png;base64,/);
  await page.keyboard.press('ArrowLeft');
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText(/wireframe\.png\s*1 \/ 2/);
  await expect(page.getByRole('button', { name: 'Stop task', exact: true })).toBeHidden({ timeout: 60_000 });
  await settled(page, '.user-message .message-attachments .prompt-chip-image.has-preview');
  await viewer(page).getByRole('button', { name: 'Close image preview' }).click();
  await expect(viewer(page)).toHaveCount(0);
  await expect(tiles.nth(1)).toBeFocused();
  // And once the thread is at rest, the tile itself is still there to return to.
  await tiles.nth(0).click();
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText(/wireframe\.png\s*1 \/ 2/);
  await page.keyboard.press('Escape');
  await expect(tiles.nth(0)).toBeFocused();
  // The app dialog was never involved.
  await expect(page.locator('#dialog')).not.toBeVisible();
});

test('an image path outside the project opens in the viewer, and images stay loaded across a thread switch', async ({ page }) => {
  await open(page);
  await ask(page, 'Saved the screenshot to /tmp/shots/final.png and kept the old one:\n\n![Old](docs/old.png)\n\nDone.');
  await expect(reply(page).locator('.chat-image[data-state="ready"]')).toHaveCount(1);
  const link = reply(page).locator('.workspace-file-link', { hasText: '/tmp/shots/final.png' });
  await link.click();
  await expect(viewer(page).locator('.image-viewer-caption')).toHaveText('final.png');
  await expect(viewer(page).locator('.image-viewer-stage img')).toHaveAttribute('src', /^data:image\/svg\+xml;base64,/);
  expect((await calls(page, 'chat_image')).map(entry => entry.args.path)).toContain('/tmp/shots/final.png');
  await page.keyboard.press('Escape');
  await expect(link).toBeFocused();
  // No file tab or browser preview was opened for it.
  expect(await calls(page, 'prepare_file_preview')).toHaveLength(0);
  expect(await calls(page, 'read_document_file')).toHaveLength(0);

  // Leaving the thread and coming back draws the image from memory: no second read.
  const before = (await calls(page, 'chat_image')).length;
  await page.locator('.task-item[data-id="session-2"]').click();
  await expect(page.locator('.task-item.selected')).toHaveAttribute('data-id', 'session-2');
  await page.locator('.task-item[data-id="session-1"]').click();
  await expect(page.locator('.task-item.selected')).toHaveAttribute('data-id', 'session-1');
  await expect(reply(page).locator('.chat-image[data-state="ready"] img')).toHaveAttribute('src', /^data:image\//);
  expect((await calls(page, 'chat_image')).length).toBe(before);
});
