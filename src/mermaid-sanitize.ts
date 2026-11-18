/* Sanitizing a rendered Mermaid SVG, on top of Mermaid's own `securityLevel: 'strict'`.
 * Diagrams come from agents (and through them from untrusted files and pull requests), so
 * anything that can navigate, run script or fetch remote content is stripped. Stricter than
 * ViewCode's sanitizer (apps/web/src/components/chat/mermaidSvgSanitizer.ts): PandaCode draws
 * every label as SVG text, so <foreignObject> and the MathML it would carry are not allowed. */

import type { Config } from 'dompurify';

/** `url(...)` that points anywhere but a fragment in the same SVG (markers, gradients). */
export const REMOTE_CSS_URL = /url\(\s*(?!['"]?#)[^)]*\)/gi;
/** Replaces remote `url()` references in CSS with `none`, so a style can never fetch. */
export const scrubRemoteCss = (css: string): string => css.replace(REMOTE_CSS_URL, 'none');

export const MERMAID_PURIFY: Config = {
  USE_PROFILES: { svg: true, svgFilters: true },
  FORBID_TAGS: ['a', 'img', 'image', 'script', 'foreignObject', 'iframe', 'use', 'animate', 'set', 'animateMotion', 'animateTransform'],
  FORBID_ATTR: ['href', 'xlink:href', 'src', 'srcset', 'action', 'formaction'],
};

type Purifier = ReturnType<typeof import('dompurify').default>;
let purifier: Purifier | undefined;

/** Sanitizes Mermaid's SVG markup; the result is safe to mount in a shadow root. */
export async function sanitizeMermaidSvg(svg: string): Promise<string> {
  if (!purifier) {
    const { default: DOMPurify } = await import('dompurify');
    const instance = DOMPurify(window);
    instance.addHook('uponSanitizeElement', (node, data) => {
      if (data.tagName === 'style' && node.textContent) node.textContent = scrubRemoteCss(node.textContent);
    });
    instance.addHook('uponSanitizeAttribute', (_node, data) => {
      if (data.attrName === 'style') data.attrValue = scrubRemoteCss(data.attrValue);
      // Event handlers never survive DOMPurify; this also drops any `on*` it might not know.
      if (/^on/i.test(data.attrName)) data.keepAttr = false;
    });
    purifier = instance;
  }
  // The viewer parses this as XML, which has no &nbsp; entity.
  return String(purifier.sanitize(svg, MERMAID_PURIFY)).replaceAll('&nbsp;', '&#160;');
}
