/* The large view of a diagram: a modal <dialog> on the shared glass with a pannable, zoomable
 * stage. Wheel zooms around the pointer, drag pans, double-click fits, + / - / 0 zoom from the
 * keyboard and the arrow keys pan. The toolbar copies the source, copies the picture (PNG) and
 * saves it as SVG or PNG. Ported from ViewCode's MermaidDiagramDialog.tsx.
 *
 * The app CSP allows `data:` images but never `blob:` ones, so the PNG is rasterized from a
 * `data:` SVG; the diagram has no <foreignObject> (labels are SVG text), so the canvas stays
 * clean. Esc, the close button or a click outside closes it and focus returns to the trigger. */

import './diagram-viewer.css';
import { icon } from './dom';
import { rasterSize, standaloneDiagramSvg, svgDataUrl, viewBoxSize, withUniqueIds } from './diagram-export.ts';
import { fitTransform, wheelZoomFactor, zoomAround, type Size, type ViewTransform } from './diagram-viewport.ts';
import { materializeIn, materializeOut } from './motion';

const BUTTON_ZOOM_STEP = 1.25;
const PAN_STEP = 48;
const PNG_SCALE = 2;
let opened = 0;

export interface DiagramViewerOptions {
  /** Sanitized, HTML-serialized SVG markup, as shown inline. */
  svg: string;
  /** The canvas colour it was drawn for; files and pictures paint it behind the diagram. */
  background: string;
  source: string;
  /** Focused again when the viewer closes. */
  returnFocus?: HTMLElement | null;
  title?: string;
}

/** The diagram as a standalone SVG file on its canvas colour. */
export function diagramFile(svg: string, background: string): string {
  const template = document.createElement('template');
  template.innerHTML = svg;
  const element = template.content.querySelector('svg');
  const file = element ? standaloneDiagramSvg(new XMLSerializer().serializeToString(element), background) : null;
  if (!file) throw new Error('The diagram has no usable size.');
  return file;
}

async function diagramPng(file: string): Promise<Blob> {
  const image = new Image();
  image.decoding = 'async';
  image.src = svgDataUrl(file);
  await image.decode();
  const natural = viewBoxSize(/\sviewBox="([^"]*)"/.exec(file)?.[1]) ?? { width: image.naturalWidth, height: image.naturalHeight };
  const { width, height } = rasterSize(natural, PNG_SCALE);
  const canvas = document.createElement('canvas');
  canvas.width = width; canvas.height = height;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('No canvas to draw the picture on.');
  context.drawImage(image, 0, 0, width, height);
  return new Promise((resolve, reject) => canvas.toBlob(blob => (blob ? resolve(blob) : reject(new Error('The picture could not be made.'))), 'image/png'));
}

function save(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url; link.download = name; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function tool(action: string, label: string, glyph: string): string {
  return `<button type="button" class="diagram-viewer-button" data-action="${action}" aria-label="${label}" title="${label}">${icon(glyph)}</button>`;
}

export function openDiagramViewer(options: DiagramViewerOptions): { close(): void } {
  const dialog = document.createElement('dialog');
  dialog.className = 'diagram-viewer';
  dialog.setAttribute('aria-label', options.title ?? 'Diagram');
  const canCopyImage = Boolean(navigator.clipboard?.write) && typeof ClipboardItem !== 'undefined';
  dialog.innerHTML = `<div class="diagram-viewer-plate">
    <header class="diagram-viewer-head">
      <span class="diagram-viewer-title">${icon('diagram')}<span>${options.title ? options.title.replace(/[&<>"]/g, c => `&#${c.charCodeAt(0)};`) : 'Diagram'}</span></span>
      <div class="diagram-viewer-tools" role="toolbar" aria-label="Diagram view">
        ${tool('copy-source', 'Copy source', 'code')}${canCopyImage ? tool('copy-image', 'Copy image', 'copy') : ''}${tool('save-svg', 'Save as SVG', 'download')}${tool('save-png', 'Save as PNG', 'images')}
        <span class="diagram-viewer-sep" aria-hidden="true"></span>
        ${tool('zoom-out', 'Zoom out', 'zoomOut')}<output class="diagram-viewer-zoom" aria-live="off">100%</output>${tool('zoom-in', 'Zoom in', 'zoomIn')}${tool('fit', 'Fit to view', 'fit')}
        <span class="diagram-viewer-sep" aria-hidden="true"></span>
        ${tool('close', 'Close', 'close')}
      </div>
    </header>
    <div class="diagram-viewer-stage" tabindex="0" role="img" aria-label="Diagram. Drag or use the arrow keys to pan; scroll or press plus and minus to zoom; 0 fits it.">
      <div class="diagram-viewer-content"></div>
    </div>
    <p class="diagram-viewer-status" role="status" aria-live="polite"></p>
  </div>`;
  document.body.append(dialog);
  const plate = dialog.querySelector<HTMLElement>('.diagram-viewer-plate')!;
  const stage = dialog.querySelector<HTMLElement>('.diagram-viewer-stage')!;
  const content = dialog.querySelector<HTMLElement>('.diagram-viewer-content')!;
  const zoom = dialog.querySelector<HTMLOutputElement>('.diagram-viewer-zoom')!;
  const status = dialog.querySelector<HTMLElement>('.diagram-viewer-status')!;
  content.innerHTML = withUniqueIds(options.svg, `viewer-${++opened}`);
  stage.style.setProperty('--diagram-canvas', options.background);

  let transform: ViewTransform = { x: 0, y: 0, scale: 1 };
  let size: Size | null = null;
  let closing = false;
  let statusTimer: ReturnType<typeof setTimeout> | undefined;

  const element = content.querySelector('svg');
  if (element) {
    size = viewBoxSize(element.getAttribute('viewBox'));
    if (size) { element.setAttribute('width', String(size.width)); element.setAttribute('height', String(size.height)); }
    element.style.maxWidth = 'none';
  }
  function apply(next: ViewTransform): void {
    transform = next;
    content.style.transform = `translate(${next.x}px, ${next.y}px) scale(${next.scale})`;
    zoom.value = `${Math.round(next.scale * 100)}%`;
  }
  const fit = () => { if (size) apply(fitTransform(size, { width: stage.clientWidth, height: stage.clientHeight })); };
  const zoomBy = (factor: number) => apply(zoomAround(transform, factor, { x: stage.clientWidth / 2, y: stage.clientHeight / 2 }));
  function say(text: string): void {
    status.textContent = text;
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => { status.textContent = ''; }, 2400);
  }

  async function act(action: string): Promise<void> {
    try {
      if (action === 'copy-source') { await navigator.clipboard.writeText(options.source); say('Source copied'); return; }
      const file = diagramFile(options.svg, options.background);
      if (action === 'save-svg') { save(new Blob([file], { type: 'image/svg+xml' }), 'diagram.svg'); say('Saved diagram.svg'); }
      else if (action === 'save-png') { save(await diagramPng(file), 'diagram.png'); say('Saved diagram.png'); }
      else if (action === 'copy-image') {
        // Start the clipboard write inside the click; rasterizing finishes later.
        await navigator.clipboard.write([new ClipboardItem({ 'image/png': diagramPng(file) })]);
        say('Image copied');
      }
    } catch (error) {
      say(`Couldn’t ${action.startsWith('copy') ? 'copy' : 'save'} the diagram${error instanceof Error && error.message ? `: ${error.message}` : '.'}`);
    }
  }

  function close(): void {
    if (closing) return;
    closing = true;
    dialog.classList.add('is-closing');
    clearTimeout(statusTimer);
    const done = () => {
      dialog.close(); dialog.remove();
      const target = options.returnFocus;
      if (target?.isConnected) target.focus({ preventScroll: true });
    };
    void materializeOut(plate).then(done, done);
  }

  dialog.addEventListener('click', event => {
    if (closing) return;
    if (event.target === dialog) { close(); return; }
    const action = (event.target as Element).closest<HTMLElement>('[data-action]')?.dataset.action;
    if (!action) return;
    if (action === 'close') close();
    else if (action === 'zoom-in') zoomBy(BUTTON_ZOOM_STEP);
    else if (action === 'zoom-out') zoomBy(1 / BUTTON_ZOOM_STEP);
    else if (action === 'fit') fit();
    else void act(action);
  });
  dialog.addEventListener('keydown', event => {
    // Esc is ours here: it must never reach the app's "Esc stops the agent".
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); return; }
    if (closing) { event.preventDefault(); return; }
    if (event.target !== stage) return;
    const pan = { ArrowLeft: [PAN_STEP, 0], ArrowRight: [-PAN_STEP, 0], ArrowUp: [0, PAN_STEP], ArrowDown: [0, -PAN_STEP] }[event.key];
    if (pan) { event.preventDefault(); apply({ ...transform, x: transform.x + pan[0], y: transform.y + pan[1] }); }
    else if (event.key === '+' || event.key === '=') { event.preventDefault(); zoomBy(BUTTON_ZOOM_STEP); }
    else if (event.key === '-') { event.preventDefault(); zoomBy(1 / BUTTON_ZOOM_STEP); }
    else if (event.key === '0') { event.preventDefault(); fit(); }
  });
  dialog.addEventListener('cancel', event => { event.preventDefault(); close(); });

  stage.addEventListener('wheel', event => {
    event.preventDefault();
    const bounds = stage.getBoundingClientRect();
    apply(zoomAround(transform, wheelZoomFactor(event.deltaY, event.deltaMode, event.ctrlKey), { x: event.clientX - bounds.left, y: event.clientY - bounds.top }));
  }, { passive: false });
  let drag: { id: number; x: number; y: number } | null = null;
  stage.addEventListener('pointerdown', event => {
    if (event.button !== 0) return;
    stage.setPointerCapture(event.pointerId);
    drag = { id: event.pointerId, x: event.clientX - transform.x, y: event.clientY - transform.y };
    stage.classList.add('is-dragging');
  });
  stage.addEventListener('pointermove', event => {
    if (!drag || drag.id !== event.pointerId) return;
    apply({ ...transform, x: event.clientX - drag.x, y: event.clientY - drag.y });
  });
  const endDrag = (event: PointerEvent) => { if (drag?.id === event.pointerId) { drag = null; stage.classList.remove('is-dragging'); } };
  stage.addEventListener('pointerup', endDrag);
  stage.addEventListener('pointercancel', endDrag);
  stage.addEventListener('dblclick', fit);

  dialog.showModal();
  fit();
  stage.focus({ preventScroll: true });
  materializeIn(plate, { scale: 0.97, y: 8 });
  return { close };
}
