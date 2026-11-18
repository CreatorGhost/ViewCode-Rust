/* Pure helpers for diagram files and the viewer. `standaloneDiagramSvg` is ported from ViewCode
 * (apps/web/src/components/chat/diagramExport.ts); the rest is PandaCode's: the app CSP allows
 * `data:` images but never `blob:` ones, so a diagram is rasterized from a data URL. */

const SVG_NAMESPACE = 'http://www.w3.org/2000/svg';

/**
 * Makes rendered diagram markup stand alone as a file or image: a pixel size
 * from its viewBox (in the app CSS sizes it), the SVG namespace, and the
 * canvas colour painted behind it (in the app the chat frame paints it).
 * Expects XML-serialized markup. Returns null without a usable viewBox.
 */
export function standaloneDiagramSvg(svg: string, background: string): string | null {
  const open = /<svg\b[^>]*>/i.exec(svg);
  if (!open || open[0].endsWith('/>')) return null;
  const tag = open[0];
  const viewBox = /\sviewBox="([^"]*)"/i.exec(tag)?.[1]?.trim().split(/[\s,]+/).map(Number);
  if (viewBox?.length !== 4 || !viewBox.every(Number.isFinite)) return null;
  const [x = 0, y = 0, width = 0, height = 0] = viewBox;
  if (!(width > 0 && height > 0)) return null;

  // Mermaid sizes the element with width="100%" and a max-width style.
  const attributes = tag.slice(4, -1).replace(/\s(?:width|height|style)="[^"]*"/gi, '');
  const namespace = /\sxmlns="/.test(attributes) ? '' : ` xmlns="${SVG_NAMESPACE}"`;
  // Inline, so the diagram's own `rect` rules cannot repaint it.
  const backdrop = `<rect x="${x}" y="${y}" width="${width}" height="${height}" style="fill:${background};stroke:none"/>`;
  return `${svg.slice(0, open.index)}<svg${namespace}${attributes} width="${width}" height="${height}">${backdrop}${svg.slice(open.index + tag.length)}`;
}

/** The width and height of a viewBox attribute, or null when it has no area. */
export function viewBoxSize(viewBox: string | null | undefined): { width: number; height: number } | null {
  const [, , width = 0, height = 0] = viewBox?.trim().split(/[\s,]+/).map(Number) ?? [];
  return width > 0 && height > 0 ? { width, height } : null;
}

/**
 * The viewer's copy of a diagram stays beside the inline one, so it gets its own element ids;
 * otherwise its markers and id-scoped styles would resolve to the inline copy.
 */
export function withUniqueIds(svg: string, suffix: string): string {
  const id = /<svg[^>]*\sid="([^"]+)"/.exec(svg)?.[1];
  return id ? svg.split(id).join(`${id}-${suffix}`) : svg;
}

/** An SVG file as a `data:` URL (UTF-8, base64), which the app CSP lets an <img> load. */
export function svgDataUrl(svg: string): string {
  const bytes = new TextEncoder().encode(svg);
  let binary = '';
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return `data:image/svg+xml;base64,${btoa(binary)}`;
}

/** The PNG's pixel size for a diagram: `scale`× its own size, capped so no canvas runs out of memory. */
export function rasterSize(size: { width: number; height: number }, scale = 2, maxSide = 8192): { width: number; height: number } {
  const factor = Math.min(scale, maxSide / size.width, maxSide / size.height);
  return { width: Math.max(1, Math.round(size.width * factor)), height: Math.max(1, Math.round(size.height * factor)) };
}
