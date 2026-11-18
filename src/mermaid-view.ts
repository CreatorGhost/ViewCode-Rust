/* A ```mermaid block once its fence has closed: the diagram, drawn in the current theme and
 * redrawn when the theme changes (the old drawing stays until the new one is ready). Ported
 * from ViewCode's MermaidCodeBlock.tsx and MermaidDiagram.tsx. Header actions: show the source,
 * view larger (diagram-viewer.ts: zoom, pan, copy and save as SVG or PNG) and copy the source.
 * A diagram Mermaid cannot parse keeps its source with a quiet note; its message is the note's
 * tooltip. The frame itself comes from generative-ui.ts. */

import { icon } from './dom';
import { diagramTheme, forgetDiagram, renderDiagram, subscribeDiagramTheme, type RenderResult } from './mermaid-render.ts';

export interface DiagramHandle { update(source: string): void; dispose(): void }

function button(action: string, label: string, glyph: string): HTMLButtonElement {
  const node = document.createElement('button');
  node.type = 'button';
  node.className = 'gu-diagram-action';
  node.dataset.action = action;
  node.setAttribute('aria-label', label);
  node.title = label;
  node.innerHTML = icon(glyph);
  return node;
}

export function mountDiagram(frame: HTMLElement, initial: string): DiagramHandle {
  const body = frame.querySelector<HTMLElement>('.gu-diagram-body')!;
  const actions = frame.querySelector<HTMLElement>('.gu-diagram-actions')!;
  const sourceBox = frame.querySelector<HTMLElement>('.gu-diagram-source')!;
  let source = initial;
  let showSource = false;
  let rendered: Extract<RenderResult, { status: 'rendered' }> | undefined;
  let generation = 0;
  let disposed = false;

  const canvas = document.createElement('button');
  canvas.type = 'button';
  canvas.className = 'gu-diagram-canvas';
  canvas.setAttribute('aria-label', 'View diagram larger');
  const note = document.createElement('p');
  note.className = 'gu-diagram-note';

  const copySource = button('copy', 'Copy diagram source', 'copy');
  const toggle = button('toggle', 'Show source', 'code');
  const expand = button('expand', 'View larger', 'expand');

  function flash(target: HTMLButtonElement, label: string, glyph: string, original: [string, string]): void {
    target.setAttribute('aria-label', label); target.title = label; target.innerHTML = icon(glyph);
    setTimeout(() => { target.setAttribute('aria-label', original[0]); target.title = original[0]; target.innerHTML = icon(original[1]); }, 1500);
  }
  copySource.addEventListener('click', () => {
    void navigator.clipboard?.writeText(source)
      .then(() => flash(copySource, 'Copied', 'check', ['Copy diagram source', 'copy']), () => flash(copySource, 'Copy failed', 'alert', ['Copy diagram source', 'copy']));
  });
  toggle.addEventListener('click', () => { showSource = !showSource; paint(); toggle.focus(); });
  const openViewer = (trigger: HTMLElement) => {
    if (!rendered) return;
    const current = rendered;
    void import('./diagram-viewer.ts').then(viewer => viewer.openDiagramViewer({ svg: current.svg, background: current.background, source, returnFocus: trigger }));
  };
  expand.addEventListener('click', () => openViewer(expand));
  canvas.addEventListener('click', () => openViewer(canvas));

  function paint(): void {
    if (disposed) return;
    if (!rendered) {
      // Failed: the source stays where it was, with the note under it.
      frame.dataset.phase = 'failed';
      sourceBox.hidden = false;
      canvas.remove();
      if (!note.isConnected) body.append(note);
      actions.replaceChildren(copySource);
      return;
    }
    note.remove();
    frame.dataset.phase = showSource ? 'source-shown' : 'diagram';
    sourceBox.hidden = !showSource;
    if (showSource) canvas.remove(); else if (!canvas.isConnected) body.prepend(canvas);
    toggle.setAttribute('aria-label', showSource ? 'Show diagram' : 'Show source');
    toggle.title = toggle.getAttribute('aria-label')!;
    toggle.innerHTML = icon(showSource ? 'diagram' : 'code');
    toggle.setAttribute('aria-pressed', String(showSource));
    actions.replaceChildren(toggle, ...(showSource ? [] : [expand]), copySource);
  }

  function failed(result: Extract<RenderResult, { status: 'error' }>): void {
    rendered = undefined;
    note.replaceChildren();
    note.title = result.message;
    note.innerHTML = `${icon('alert')}<span class="gu-diagram-note-text" data-label="Couldn’t render this diagram"></span><span class="sr-only">Couldn’t render this diagram: </span>`;
    const detail = document.createElement('span');
    detail.className = 'sr-only';
    detail.textContent = result.message;
    note.append(detail);
    if (result.retryable) {
      const retry = document.createElement('button');
      retry.type = 'button';
      retry.className = 'gu-diagram-retry';
      retry.textContent = 'Retry';
      retry.addEventListener('click', () => { forgetDiagram(source); draw(); });
      note.append(retry);
    }
    paint();
  }

  function draw(): void {
    const mine = ++generation;
    const text = source;
    void renderDiagram(text, diagramTheme()).then(result => {
      if (disposed || mine !== generation) return;
      if (result.status === 'error') { failed(result); return; }
      rendered = result;
      canvas.innerHTML = result.svg;
      paint();
    });
  }

  const unsubscribe = subscribeDiagramTheme(draw);
  draw();
  return {
    update(next: string) {
      if (next === source) return;
      source = next;
      draw();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      unsubscribe();
    },
  };
}
