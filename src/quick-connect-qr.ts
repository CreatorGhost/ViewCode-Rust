/* The Quick connect pairing code as an SVG, drawn on this computer by the vendored uqr encoder
 * (src/vendor/uqr, loaded with this module only when a code is shown). The SVG holds modules
 * only: the link it encodes is never written into the markup, an attribute or a title. */
import { encode } from './vendor/uqr/uqr.js';

/** Modules of light margin inside the plate; with the plate's padding the quiet zone is four modules or more. */
const BORDER = 2;

export function qrSvg(text: string): string {
  // M recovers 15%: still a small code for a ~250-character link, and forgiving of a glare-y screen.
  const { size, data } = encode(text, { ecc: 'M', border: BORDER });
  let path = '';
  for (let y = 0; y < size; y++) {
    let x = 0;
    while (x < size) {
      if (!data[y][x]) { x++; continue; }
      const start = x;
      while (x < size && data[y][x]) x++;
      path += `M${start} ${y}h${x - start}v1h${start - x}z`;
    }
  }
  return `<svg class="qc-qr-svg" viewBox="0 0 ${size} ${size}" shape-rendering="crispEdges" aria-hidden="true" focusable="false"><path d="${path}" fill="currentColor"/></svg>`;
}
