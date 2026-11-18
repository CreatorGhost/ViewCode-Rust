/** Pan and zoom math for the diagram viewer. Ported from ViewCode (apps/web/src/components/chat/diagramViewport.ts). */

export interface ViewTransform {
  readonly x: number;
  readonly y: number;
  readonly scale: number;
}

export interface Size {
  readonly width: number;
  readonly height: number;
}

export const MIN_DIAGRAM_SCALE = 0.1;
export const MAX_DIAGRAM_SCALE = 8;

const clampScale = (scale: number) =>
  Math.min(MAX_DIAGRAM_SCALE, Math.max(MIN_DIAGRAM_SCALE, scale));

/** Centers the content and scales it to fit, never past `maxScale` so small diagrams stay crisp. */
export function fitTransform(
  content: Size,
  viewport: Size,
  padding = 32,
  maxScale = 2,
): ViewTransform {
  if (content.width <= 0 || content.height <= 0) return { x: 0, y: 0, scale: 1 };
  const scale = clampScale(
    Math.min(
      (viewport.width - padding * 2) / content.width,
      (viewport.height - padding * 2) / content.height,
      maxScale,
    ),
  );
  return {
    x: (viewport.width - content.width * scale) / 2,
    y: (viewport.height - content.height * scale) / 2,
    scale,
  };
}

/** Zooms by `factor` while the point under the cursor stays where it is. */
export function zoomAround(
  transform: ViewTransform,
  factor: number,
  point: { readonly x: number; readonly y: number },
): ViewTransform {
  const scale = clampScale(transform.scale * factor);
  const ratio = scale / transform.scale;
  return {
    x: point.x - (point.x - transform.x) * ratio,
    y: point.y - (point.y - transform.y) * ratio,
    scale,
  };
}

const WHEEL_LINE_HEIGHT = 16;

/**
 * Wheel delta to a zoom factor. Trackpad pinches arrive as ctrl+wheel with
 * small deltas, so they zoom faster per pixel than a mouse wheel notch.
 */
export function wheelZoomFactor(deltaY: number, deltaMode: number, pinch: boolean): number {
  const pixels = deltaMode === 1 ? deltaY * WHEEL_LINE_HEIGHT : deltaY;
  return Math.exp(-pixels * (pinch ? 0.01 : 0.002));
}
