/* Renders ```mermaid diagrams. Ported from ViewCode's MermaidDiagram.tsx without React:
 * the theme is read from the live tokens, renders run one at a time (Mermaid's config is
 * global) and are cached by theme and source, and the SVG is sanitized before it is shown.
 *
 * Mermaid (vendored, src/vendor/mermaid) loads only when the first diagram renders: it is
 * about 1 MB compressed, so it never joins the startup bundle. Mermaid runs with
 * `securityLevel: 'strict'` and SVG text labels; nothing it draws can run script or fetch. */

import {
  defaultClassSetsFill, deriveDiagramPalette, diagramPaletteSignature, flattenColor, mermaidConfig, mermaidRenderKey,
  parseCssColor, planFlowchartInk, planFlowchartTones,
  type DiagramPalette, type DiagramTokens, type FlowchartClassDef, type Rgb, type Rgba,
} from './mermaid-theme.ts';
import type { Mermaid } from './vendor/mermaid/mermaid.min.js';

export interface DiagramTheme { readonly palette: DiagramPalette; readonly signature: string }
export type RenderResult =
  | { readonly status: 'rendered'; readonly svg: string; readonly background: string }
  | { readonly status: 'error'; readonly message: string; readonly retryable: boolean };

/* ---------- theme ---------- */

// The diagram sits on --raised, like every other view in a reply.
const TOKEN_EXPRESSIONS: Record<keyof DiagramTokens, string> = {
  canvas: 'var(--raised)',
  text: 'var(--text)',
  muted: 'var(--muted)',
  border: 'var(--border)',
  accent: 'var(--accent)',
  warning: 'var(--ds-warn, var(--warning, #a8700a))',
  danger: 'var(--danger)',
};
const WHITE: Rgb = { r: 255, g: 255, b: 255 };
const BLACK: Rgb = { r: 0, g: 0, b: 0 };
const INVALID_COLOR_SENTINEL = '#010203';
let colorCanvas: CanvasRenderingContext2D | null | undefined;

/** Any CSS colour to sRGB through a 1px canvas (computed colours may come back as oklch or color-mix). */
export function resolveViaCanvas(color: string): Rgba | null {
  if (colorCanvas === undefined) {
    const canvas = document.createElement('canvas');
    canvas.width = 1; canvas.height = 1;
    colorCanvas = canvas.getContext('2d', { willReadFrequently: true });
  }
  if (!colorCanvas) return null;
  colorCanvas.clearRect(0, 0, 1, 1);
  colorCanvas.fillStyle = INVALID_COLOR_SENTINEL;
  colorCanvas.fillStyle = color;
  if (colorCanvas.fillStyle === INVALID_COLOR_SENTINEL) return null;
  colorCanvas.fillRect(0, 0, 1, 1);
  const [r = 0, g = 0, b = 0, a = 0] = colorCanvas.getImageData(0, 0, 1, 1).data;
  return { r, g, b, a: a / 255 };
}

function readDiagramTheme(): DiagramTheme {
  const probe = document.createElement('span');
  probe.style.cssText = 'position:absolute;visibility:hidden;pointer-events:none';
  document.body.append(probe);
  try {
    const read = (expression: string): Rgba | null => {
      probe.style.color = '';
      probe.style.color = expression;
      const computed = getComputedStyle(probe).color;
      return parseCssColor(computed) ?? resolveViaCanvas(computed);
    };
    const dark = document.documentElement.dataset.theme === 'dark';
    const canvasColor = read(TOKEN_EXPRESSIONS.canvas);
    const canvas = canvasColor ? flattenColor(canvasColor, dark ? BLACK : WHITE) : dark ? BLACK : WHITE;
    const token = (name: keyof DiagramTokens, fallback: Rgb): Rgb => {
      const color = read(TOKEN_EXPRESSIONS[name]);
      return color ? flattenColor(color, canvas) : fallback;
    };
    const text = token('text', dark ? WHITE : BLACK);
    const palette = deriveDiagramPalette({
      canvas, text,
      muted: token('muted', text), border: token('border', text), accent: token('accent', text),
      warning: token('warning', text), danger: token('danger', text),
    }, getComputedStyle(document.body).fontFamily);
    return { palette, signature: diagramPaletteSignature(palette) };
  } finally {
    probe.remove();
  }
}

let currentTheme: DiagramTheme | null = null;
let themeObserver: MutationObserver | null = null;
let colorScheme: MediaQueryList | null = null;
const themeListeners = new Set<() => void>();

function refreshDiagramTheme(): void {
  const next = readDiagramTheme();
  if (next.signature === currentTheme?.signature) return;
  currentTheme = next;
  for (const listener of themeListeners) listener();
}

/** The palette is applied as attributes and inline variables on <html> (palette.ts), so one
 * observer covers light/dark, accents, wallpapers and imported themes. It runs only while a
 * diagram is on screen. */
export function subscribeDiagramTheme(listener: () => void): () => void {
  themeListeners.add(listener);
  if (!themeObserver) {
    themeObserver = new MutationObserver(refreshDiagramTheme);
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style', 'data-theme', 'data-vsix', 'data-contrast'] });
    colorScheme = matchMedia('(prefers-color-scheme: dark)');
    colorScheme.addEventListener('change', refreshDiagramTheme);
  }
  return () => {
    themeListeners.delete(listener);
    if (themeListeners.size > 0) return;
    themeObserver?.disconnect();
    themeObserver = null;
    colorScheme?.removeEventListener('change', refreshDiagramTheme);
    colorScheme = null;
    currentTheme = null;
  };
}

export function diagramTheme(): DiagramTheme {
  currentTheme ??= readDiagramTheme();
  return currentTheme;
}

/* ---------- rendering ---------- */

interface Modules { readonly mermaid: Mermaid; readonly sanitize: (svg: string) => Promise<string> }
let modulesPromise: Promise<Modules> | null = null;
let renderQueue: Promise<unknown> = Promise.resolve();
let nextDiagramId = 0;
const MAX_CACHED_RENDERS = 64;
const renderCache = new Map<string, Promise<RenderResult>>();
const settledRenders = new WeakSet<Promise<RenderResult>>();

function loadModules(): Promise<Modules> {
  modulesPromise ??= Promise.all([import('./vendor/mermaid/mermaid.min.js'), import('./mermaid-sanitize.ts')])
    .then(([mermaid, sanitizer]) => ({ mermaid: mermaid.default, sanitize: sanitizer.sanitizeMermaidSvg }))
    .catch((error: unknown) => { modulesPromise = null; throw error; });
  return modulesPromise;
}

interface FlowVertexLike { id: string; type?: string; styles: ReadonlyArray<string>; classes: ReadonlyArray<string> }
interface FlowDbLike {
  getSubGraphs(): ReadonlyArray<{ id: string; nodes: ReadonlyArray<string>; classes: ReadonlyArray<string> }>;
  getVertices(): unknown;
  getClasses(): unknown;
}

/** Flowchart groups get palette colours as Mermaid `class` statements, and author-filled
 * nodes readable text the same way. Any failure just means a single-tone diagram. */
async function flowchartStatements(mermaid: Mermaid, source: string, palette: DiagramPalette): Promise<string[]> {
  try {
    const diagram = await mermaid.mermaidAPI.getDiagramFromText(source);
    if (!diagram.type.startsWith('flowchart')) return [];
    const db = diagram.db as Partial<FlowDbLike>;
    if (typeof db.getSubGraphs !== 'function' || typeof db.getVertices !== 'function' || typeof db.getClasses !== 'function') return [];
    const vertexMap = db.getVertices();
    const classes = db.getClasses();
    if (!(vertexMap instanceof Map) || !(classes instanceof Map)) return [];
    const vertices = [...(vertexMap as Map<string, FlowVertexLike>).values()];
    const subgraphs = db.getSubGraphs().map(subgraph => ({ ...subgraph, styles: (vertexMap as Map<string, FlowVertexLike>).get(subgraph.id)?.styles ?? [] }));
    const defaultFill = defaultClassSetsFill(classes as Map<string, FlowchartClassDef>);
    const tones = planFlowchartTones({
      subgraphs: subgraphs.map(subgraph => ({ id: subgraph.id, nodes: subgraph.nodes, styled: subgraph.classes.length > 0 || subgraph.styles.length > 0 })),
      vertices: vertices.map(vertex => ({ id: vertex.id, shape: vertex.type, styled: defaultFill || vertex.styles.length > 0 || vertex.classes.length > 0 })),
    }, palette.cycle);
    const ink = planFlowchartInk({ classes: classes as Map<string, FlowchartClassDef>, vertices, subgraphs }, palette, value => parseCssColor(value) ?? resolveViaCanvas(value));
    return [...tones, ...ink];
  } catch {
    return [];
  }
}

async function renderOnce(mermaid: Mermaid, text: string): Promise<string> {
  const id = `pandacode-diagram-${nextDiagramId++}`;
  try {
    return (await mermaid.render(id, text)).svg;
  } finally {
    // Mermaid leaves its measuring element behind when a render throws.
    document.getElementById(`d${id}`)?.remove();
    document.getElementById(id)?.remove();
  }
}

const LABEL_PILL_X = 6;
const LABEL_PILL_Y = 3;

/** Sanitizes, then gives edge labels a padded, rounded background. */
async function finishSvg(sanitize: Modules['sanitize'], svg: string): Promise<string> {
  const sanitized = await sanitize(svg);
  // Template content is inert: nothing in it loads or runs while it is edited.
  const template = document.createElement('template');
  template.innerHTML = sanitized;
  for (const rect of template.content.querySelectorAll('.edgeLabel rect.background')) {
    if (!rect.parentElement?.textContent?.trim()) { rect.remove(); continue; }
    const [x, y, width, height] = ['x', 'y', 'width', 'height'].map(name => Number(rect.getAttribute(name)));
    if (![x, y, width, height].every(Number.isFinite)) continue;
    const paddedHeight = height + LABEL_PILL_Y * 2;
    rect.setAttribute('x', String(x - LABEL_PILL_X));
    rect.setAttribute('y', String(y - LABEL_PILL_Y));
    rect.setAttribute('width', String(width + LABEL_PILL_X * 2));
    rect.setAttribute('height', String(paddedHeight));
    rect.setAttribute('rx', String(Math.min(6, paddedHeight / 2)));
  }
  return template.innerHTML.replaceAll('&nbsp;', '&#160;');
}

// A lost chunk is worth a retry, unlike a syntax error.
const CHUNK_LOAD_ERROR = /dynamically imported module|importing a module script|failed to fetch/i;

async function renderMermaid(source: string, theme: DiagramTheme): Promise<RenderResult> {
  let modules: Modules;
  try { modules = await loadModules(); }
  catch { return { status: 'error', message: 'The diagram renderer failed to load.', retryable: true }; }
  const { mermaid, sanitize } = modules;
  try {
    mermaid.initialize(mermaidConfig(theme.palette));
    const statements = await flowchartStatements(mermaid, source, theme.palette);
    let svg: string;
    try {
      svg = await renderOnce(mermaid, statements.length > 0 ? `${source}\n${statements.join('\n')}` : source);
    } catch (error) {
      if (statements.length === 0) throw error;
      svg = await renderOnce(mermaid, source);
    }
    return { status: 'rendered', svg: await finishSvg(sanitize, svg), background: theme.palette.canvas };
  } catch (error) {
    const message = error instanceof Error ? error.message : typeof (error as { str?: unknown })?.str === 'string' ? (error as { str: string }).str : 'The diagram could not be rendered.';
    return { status: 'error', message: message.slice(0, 400), retryable: CHUNK_LOAD_ERROR.test(message) };
  }
}

function evictSettledRenders(): void {
  for (const [key, result] of renderCache) {
    if (renderCache.size <= MAX_CACHED_RENDERS) return;
    if (settledRenders.has(result)) renderCache.delete(key);
  }
}

/** The diagram drawn in `theme`; the same source and theme share one render. */
export function renderDiagram(source: string, theme: DiagramTheme = diagramTheme()): Promise<RenderResult> {
  const trimmed = source.trim();
  const key = mermaidRenderKey(trimmed, theme.signature);
  const cached = renderCache.get(key);
  if (cached) { renderCache.delete(key); renderCache.set(key, cached); return cached; }
  const result = renderQueue.then(() => renderMermaid(trimmed, theme));
  renderQueue = result;
  void result.then(() => { settledRenders.add(result); evictSettledRenders(); });
  renderCache.set(key, result);
  evictSettledRenders();
  return result;
}

/** Drops a failed render so the next attempt tries again. */
export function forgetDiagram(source: string, theme: DiagramTheme = diagramTheme()): void {
  renderCache.delete(mermaidRenderKey(source.trim(), theme.signature));
}
