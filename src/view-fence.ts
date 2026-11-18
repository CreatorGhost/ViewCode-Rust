/* Fence detection for views drawn from a fenced block (```ui, ```chart, ```svg, ```html and
 * ```mermaid diagrams). Pure, so Node tests can run it. A view is drawn only once its closing
 * fence has arrived: while a reply streams, an open view fence shows a placeholder, and an open
 * ```mermaid fence keeps showing its source (see generative-ui.ts), so a half-written diagram is
 * never parsed and never flickers into an error. */

/** Fence languages drawn as a view rather than as code. */
export const VIEW_LANGUAGES = ['ui', 'chart', 'svg', 'html', 'mermaid'] as const;
/** Fence languages drawn as a diagram. */
export const DIAGRAM_LANGUAGES = ['mermaid'] as const;

export const isViewLanguage = (language: string): boolean => (VIEW_LANGUAGES as readonly string[]).includes(language.toLowerCase());
export const isDiagramLanguage = (language: string): boolean => (DIAGRAM_LANGUAGES as readonly string[]).includes(language.toLowerCase());

const FENCE_LINE = /^ {0,3}(`{3,}|~{3,})[ \t]*([^\s`]*)/;

/** The language of the fence the text ends inside, or null when every fence is closed. */
export function openFenceLanguage(value: string): string | null {
  let open: { mark: string; language: string } | undefined;
  for (const line of value.split('\n')) {
    const fence = line.match(FENCE_LINE);
    if (!fence) continue;
    if (!open) open = { mark: fence[1], language: fence[2] };
    else if (fence[1][0] === open.mark[0] && fence[1].length >= open.mark.length && !fence[2] && !line.slice(line.indexOf(fence[1]) + fence[1].length).trim()) open = undefined;
  }
  return open ? open.language : null;
}

/** Whether the text ends inside an open generative-UI or diagram fence (a reply still streaming). */
export function openViewFence(value: string): boolean {
  const language = openFenceLanguage(value);
  return language !== null && isViewLanguage(language);
}

/** What a diagram block shows: its source until the fence has closed and holds something, then the diagram. */
export function diagramPhase(pending: boolean, source: string): 'source' | 'diagram' {
  return !pending && source.trim().length > 0 ? 'diagram' : 'source';
}
