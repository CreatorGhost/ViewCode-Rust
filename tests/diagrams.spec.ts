import { expect, test, type Page } from '@playwright/test';

/* ```mermaid diagrams in replies (docs/diagrams.md): rendered once the fence closes, themed from the
 * app's tokens, with a viewer, and the renderer loaded only when a diagram appears. */

const FLOW = '```mermaid\nflowchart LR\n  A[Composer] -->|send| B{{Core}}\n  B --> C[(Transcript)]\n```';

/** Renders Markdown through the app's own pipeline into a probe; `morph` updates it in place like the streaming painter. */
async function render(page: Page, markdown: string, morph = false): Promise<void> {
  await page.evaluate(async ({ source, morph }) => {
    const dom = await import('/src/dom.ts');
    let host = document.querySelector<HTMLElement>('#diagram-probe');
    if (!host) {
      host = document.createElement('div');
      host.id = 'diagram-probe';
      host.className = 'markdown';
      host.style.cssText = 'position:fixed;inset:40px auto auto 40px;width:640px;max-height:80vh;overflow:auto;z-index:999;padding:20px;background:var(--bg)';
      document.body.append(host);
    }
    const html = await dom.markdownHtml(source, false);
    if (morph) dom.morph(host, html); else host.innerHTML = html;
  }, { source: markdown, morph });
}
const diagram = (page: Page, index = 0) => page.locator('#diagram-probe pandacode-ui').nth(index);
const libraryRequests = (page: Page) => {
  const seen: string[] = [];
  page.on('request', request => { if (/vendor\/mermaid|mermaid\.min/.test(request.url())) seen.push(request.url()); });
  return seen;
};

const errors = new WeakMap<Page, string[]>();
test.beforeEach(async ({ page }) => {
  const list: string[] = []; errors.set(page, list);
  page.on('pageerror', error => list.push(error.message));
  page.on('dialog', dialog => { list.push(`dialog: ${dialog.message()}`); void dialog.dismiss(); });
});
test.afterEach(async ({ page }) => { expect(errors.get(page)).toEqual([]); });

test('a thread without diagrams never requests the diagram library', async ({ page }) => {
  const seen = libraryRequests(page);
  await page.goto('/');
  await expect(page.locator('#prompt')).toBeVisible();
  await render(page, 'Plain text, `code`, and a chart:\n\n```ts\nconst a = 1;\n```\n\n```ui\n{"type":"callout","body":"Hi"}\n```');
  await expect(page.locator('#diagram-probe pandacode-ui')).toHaveCount(1);
  await page.waitForTimeout(2500);
  expect(seen).toEqual([]);
  // The first diagram loads it.
  await render(page, FLOW);
  await expect(diagram(page).locator('.gu-diagram-canvas svg')).toBeVisible();
  expect(seen.length).toBeGreaterThan(0);
});

test('a streaming block shows its source until the fence closes, then renders an svg', async ({ page }) => {
  const seen = libraryRequests(page);
  await page.goto('/');
  await expect(page.locator('#prompt')).toBeVisible();
  const open = 'Here it is:\n\n```mermaid\nflowchart LR\n  A[Composer] -->|send| B{{Core';
  await render(page, open);
  const view = diagram(page);
  await expect(view).toHaveClass(/pending/);
  await expect(view.locator('.gu-diagram')).toHaveAttribute('data-phase', 'source');
  // The source is the block itself, slotted: visible, never parsed, no error note.
  await expect(view.locator('pre code')).toBeVisible();
  await render(page, `${open}}}\n  B --> C[(Trans`, true);
  await expect(view.locator('pre code')).toContainText('C[(Trans');
  await expect(view.locator('.gu-diagram-note')).toHaveCount(0);
  await expect(view.locator('svg[id^="pandacode-diagram"]')).toHaveCount(0);
  expect(seen).toEqual([]);
  // The fence closes: the same element draws the diagram.
  await render(page, `${open}}}\n  B --> C[(Transcript)]\n\`\`\`\n\nDone.`, true);
  await expect(view).not.toHaveClass(/pending/);
  await expect(view.locator('.gu-diagram')).toHaveAttribute('data-phase', 'diagram');
  const svg = view.locator('.gu-diagram-canvas svg');
  await expect(svg).toBeVisible();
  await expect(svg.locator('text, tspan').filter({ hasText: 'Transcript' }).first()).toBeAttached();
  await expect(view.locator('pre code')).toBeHidden();
});

test('a reply streamed by the preview agent renders its diagram once the turn completes', async ({ page }) => {
  await page.goto('/');
  const prompt = page.getByRole('textbox', { name: 'Message your coding agent', exact: true });
  await expect(prompt).toBeVisible();
  await prompt.fill('Draw a diagram of how a prompt reaches the agent');
  await prompt.press('Enter');
  const transcript = page.getByRole('log', { name: 'Conversation', exact: true });
  const svg = transcript.locator('pandacode-ui .gu-diagram-canvas svg').last();
  await expect(svg).toBeVisible({ timeout: 20_000 });
  await expect(transcript.locator('pandacode-ui .gu-diagram-note')).toHaveCount(0);
  await expect(svg.locator('text, tspan').filter({ hasText: 'Rust core' }).first()).toBeAttached();
});

test('a broken block keeps its source with a quiet note, and its source copies', async ({ page }) => {
  await page.addInitScript(() => Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { (window as any).__copied = text; } } }));
  await page.goto('/');
  await expect(page.locator('#prompt')).toBeVisible();
  await render(page, '```mermaid\nflowchart LR\n  A --> (\n```');
  const view = diagram(page);
  await expect(view.locator('.gu-diagram')).toHaveAttribute('data-phase', 'failed');
  await expect(view.locator('pre code')).toBeVisible();
  const note = view.locator('.gu-diagram-note');
  await expect(note).toBeVisible();
  await expect(note.locator('.gu-diagram-note-text')).toHaveAttribute('data-label', 'Couldn’t render this diagram');
  expect((await note.getAttribute('title'))?.length).toBeGreaterThan(5);
  // Chrome text stays out of the message's own text.
  expect(await view.evaluate(host => host.textContent)).toBe('flowchart LR\n  A --> (\n');
  await view.getByRole('button', { name: 'Copy diagram source' }).click();
  expect(await page.evaluate(() => (window as any).__copied)).toBe('flowchart LR\n  A --> (\n');
  await expect(view.getByRole('button', { name: 'View larger' })).toHaveCount(0);
});

test('diagrams follow light and dark, and re-theme in place', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('pandacode.theme', 'dark'));
  await page.goto('/');
  await expect(page.locator('#prompt')).toBeVisible();
  await render(page, FLOW);
  const svg = diagram(page).locator('.gu-diagram-canvas svg');
  await expect(svg).toBeVisible();
  const look = () => svg.evaluate(node => {
    const shape = node.querySelector('.node rect, .node polygon, .node path')!;
    const label = node.querySelector('.nodeLabel, .node text, .node tspan')!;
    const lum = (color: string) => { const [r, g, b] = color.match(/[\d.]+/g)!.map(Number); return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255; };
    return { fill: lum(getComputedStyle(shape).fill), text: lum(getComputedStyle(label).fill), id: node.id };
  });
  const dark = await look();
  expect(dark.fill).toBeLessThan(0.35);
  expect(dark.text).toBeGreaterThan(0.75);
  await page.evaluate(() => import('/src/theme.ts').then(theme => theme.setTheme('light')));
  await expect.poll(async () => (await look()).fill).toBeGreaterThan(0.7);
  const light = await look();
  expect(light.text).toBeLessThan(0.3);
  expect(light.id).not.toBe(dark.id);
});

test('the viewer opens larger, zooms, saves and closes with focus returned', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#prompt')).toBeVisible();
  await render(page, FLOW);
  const view = diagram(page);
  const canvas = view.getByRole('button', { name: 'View diagram larger' });
  await expect(canvas.locator('svg')).toBeVisible();
  await canvas.click();
  const viewer = page.getByRole('dialog', { name: 'Diagram' });
  await expect(viewer).toBeVisible();
  await expect(viewer.locator('.diagram-viewer-content svg')).toBeVisible();
  await expect(viewer.locator('.diagram-viewer-stage')).toBeFocused();
  const zoom = viewer.locator('.diagram-viewer-zoom');
  const before = await zoom.textContent();
  await viewer.getByRole('button', { name: 'Zoom in' }).click();
  await expect(zoom).not.toHaveText(before!);
  await viewer.getByRole('button', { name: 'Fit to view' }).click();
  await expect(zoom).toHaveText(before!);
  const svgFile = page.waitForEvent('download');
  await viewer.getByRole('button', { name: 'Save as SVG' }).click();
  expect((await svgFile).suggestedFilename()).toBe('diagram.svg');
  const pngFile = page.waitForEvent('download');
  await viewer.getByRole('button', { name: 'Save as PNG' }).click();
  const png = await pngFile;
  expect(png.suggestedFilename()).toBe('diagram.png');
  await expect(viewer.locator('.diagram-viewer-status')).toHaveText('Saved diagram.png');
  await page.keyboard.press('Escape');
  await expect(viewer).toHaveCount(0);
  expect(await view.evaluate(host => host.shadowRoot?.activeElement?.getAttribute('aria-label'))).toBe('View diagram larger');
  // The header's button opens it too, and the close button returns focus to that button.
  await view.getByRole('button', { name: 'View larger' }).click();
  await expect(viewer).toBeVisible();
  await viewer.getByRole('button', { name: 'Close' }).click();
  await expect(viewer).toHaveCount(0);
  expect(await view.evaluate(host => host.shadowRoot?.activeElement?.getAttribute('aria-label'))).toBe('View larger');
});

test('show source swaps the diagram for its block and back', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#prompt')).toBeVisible();
  await render(page, FLOW);
  const view = diagram(page);
  await expect(view.locator('.gu-diagram-canvas svg')).toBeVisible();
  await view.getByRole('button', { name: 'Show source' }).click();
  await expect(view.locator('pre code')).toBeVisible();
  await expect(view.locator('.gu-diagram-canvas')).toHaveCount(0);
  await view.getByRole('button', { name: 'Show diagram' }).click();
  await expect(view.locator('.gu-diagram-canvas svg')).toBeVisible();
});

test('nothing a diagram says can run script, link out or fetch', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#prompt')).toBeVisible();
  const hostile = [
    '```mermaid',
    '%%{init: {"securityLevel": "loose", "htmlLabels": true, "themeCSS": ".node rect { fill: url(https://example.com/x.png) }"}}%%',
    'flowchart LR',
    '  A["<img src=x onerror=alert(1)>"] --> B["<a href=\'javascript:alert(2)\'>go</a>"]',
    '  click A "javascript:alert(3)"',
    '  click B href "https://example.com" _blank',
    '```',
  ].join('\n');
  const remote: string[] = [];
  page.on('request', request => { if (request.url().includes('example.com')) remote.push(request.url()); });
  await render(page, hostile);
  const svg = diagram(page).locator('.gu-diagram-canvas svg');
  await expect(diagram(page).locator('.gu-diagram')).toHaveAttribute('data-phase', /diagram|failed/);
  expect(await diagram(page).locator('.gu-diagram-note').getAttribute('title').catch(() => null)).toBeNull();
  await expect(svg).toBeVisible();
  const report = await svg.evaluate(node => ({
    dangerous: node.querySelectorAll('a, img, image, script, foreignObject, iframe, use').length,
    handlers: [...node.querySelectorAll('*')].filter(el => [...el.attributes].some(a => /^on/i.test(a.name) || /^(href|xlink:href|src)$/i.test(a.name))).length,
    remoteCss: /url\(\s*['"]?https?:/i.test(node.outerHTML),
  }));
  expect(report).toEqual({ dangerous: 0, handlers: 0, remoteCss: false });
  await diagram(page).locator('.gu-diagram-canvas').click();
  await page.keyboard.press('Escape');
  expect(remote).toEqual([]);
});

test('charts render in the theme: pie, xychart and gantt', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#prompt')).toBeVisible();
  await render(page, [
    '```mermaid\npie title Bundle\n  "main" : 80\n  "lazy" : 120\n  "vendor" : 40\n```',
    '```mermaid\nxychart-beta\n  title "Runs"\n  x-axis [Mon, Tue, Wed]\n  y-axis "Runs" 0 --> 10\n  bar [3, 5, 4]\n  line [3, 5, 4]\n```',
    '```mermaid\ngantt\n  title Release\n  dateFormat YYYY-MM-DD\n  section Port\n  Diagrams :done, d1, 2026-11-01, 5d\n  Charts :active, d2, after d1, 3d\n  Ship :crit, d3, after d2, 2d\n```',
  ].join('\n\n'));
  for (const index of [0, 1, 2]) await expect(diagram(page, index).locator('.gu-diagram-canvas svg')).toBeVisible();
  const slices = await diagram(page, 0).locator('svg path.pieCircle').evaluateAll(paths => [...new Set(paths.map(path => getComputedStyle(path).fill))]);
  expect(slices.length).toBe(3);
  // Mermaid's own default palette never shows: slices come from PandaCode's tones.
  expect(slices).not.toContain('rgb(236, 236, 255)');
  const background = await diagram(page, 1).locator('.gu-diagram-canvas > svg').evaluate(node => getComputedStyle(node.querySelector('rect.background, .main > rect, rect') as Element).fill);
  expect(background).not.toBe('rgb(255, 255, 255)');
});
