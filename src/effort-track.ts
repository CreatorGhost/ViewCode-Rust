/**
 * The effort track's drag: the model picker's Effort row is a track with one labelled stop per level
 * and a thumb (`.segment-thumb`, placed by `trackSegment()` with `translate`). The thumb can be
 * dragged: it follows the pointer 1:1, the stop it is nearest lights up, and on release it settles
 * onto that stop and the level is saved. A tap on a stop, and the arrow keys, stay with the stops
 * themselves (radios) in `model-picker.ts`.
 *
 * Pointer events only (never native drag, which Tauri's file-drop handler swallows). The maths
 * follows ViewCode's effort slider (`composerModelEffort.logic.ts`).
 */

/** Where stop `index` of `count` sits along the thumb's travel, 0 to 1. */
export function stopFraction(index: number, count: number): number {
  if (count <= 1) return 0;
  return Math.min(Math.max(index, 0), count - 1) / (count - 1);
}
/** The stop nearest a travel fraction. */
export function nearestStop(fraction: number, count: number): number {
  if (count <= 1) return 0;
  return Math.min(Math.max(Math.round(fraction * (count - 1)), 0), count - 1);
}
/** The thumb's left edge for a pointer at `pointer`, held `grab` px from that edge, kept between the first
 * stop (`min`) and the last (`max`). */
export function thumbLeft(pointer: number, grab: number, min: number, max: number): number {
  return Math.min(Math.max(pointer - grab, min), Math.max(min, max));
}
/** How far along its travel a thumb at `left` is, 0 to 1. */
export function travelFraction(left: number, min: number, max: number): number {
  return max <= min ? 0 : Math.min(Math.max((left - min) / (max - min), 0), 1);
}
/** The stop a key moves to from `index`, or null for a key the track does not handle. The stops are radios,
 * so Down goes with Right (the next stop) and Up with Left, as in any radio group. */
export function stopForKey(key: string, index: number, count: number): number | null {
  const last = count - 1;
  switch (key) {
    case 'ArrowRight': case 'ArrowDown': return Math.min(index + 1, last);
    case 'ArrowLeft': case 'ArrowUp': return Math.max(index - 1, 0);
    case 'Home': return 0;
    case 'End': return last;
    default: return null;
  }
}

/** Pointer travel before a press on the track becomes a drag; less is a tap on a stop. */
export const DRAG_SLOP = 3;
const TRACK = '.model-menu-effort', STOP = ':scope > [role="radio"]', THUMB = ':scope > .segment-thumb';

export interface EffortDragHooks {
  /** No drag may start (a save is on its way, the harness is missing). */
  disabled: () => boolean;
  /** The stop under the thumb changed mid-drag (null: the drag ended). */
  preview: (stop: HTMLElement | null) => void;
  /** The thumb was released over `stop`. */
  commit: (stop: HTMLElement) => void;
}

/** Wires the drag on `host`, which holds (and redraws) the track. Returns whether a drag is under way, so the
 * owner leaves the thumb alone meanwhile, and whether a click is the tail of a drag. */
export function mountEffortDrag(host: HTMLElement, hooks: EffortDragHooks): { dragging: () => boolean; swallowClick: () => boolean } {
  let drag: { id: number; track: HTMLElement; startX: number; grab: number; moved: boolean; near: HTMLElement | null } | null = null;
  let swallow = 0;
  const stops = (track: HTMLElement) => [...track.querySelectorAll<HTMLElement>(STOP)];

  function move(clientX: number): void {
    if (!drag) return;
    const { track } = drag, all = stops(track);
    if (!all.length) return;
    const origin = track.getBoundingClientRect().left + track.clientLeft;
    const min = all[0].offsetLeft, max = all[all.length - 1].offsetLeft;
    const left = thumbLeft(clientX - origin, drag.grab, min, max);
    track.style.setProperty('--seg-x', `${left}px`);
    const near = all[nearestStop(travelFraction(left, min, max), all.length)];
    if (near === drag.near) return;
    drag.near?.removeAttribute('data-near');
    near.setAttribute('data-near', '');
    drag.near = near;
    hooks.preview(near);
  }
  function end(commit: boolean): void {
    if (!drag) return;
    const { track, near, moved, id } = drag;
    drag = null;
    if (!moved) return;
    try { track.releasePointerCapture(id); } catch { /* Already released. */ }
    near?.removeAttribute('data-near');
    // The thumb settles onto its stop with the track's own transition (model-picker.css); the flag keeps that
    // transition armed when the pointer was let go outside the track.
    track.dataset.drag = 'settle';
    window.setTimeout(() => { if (track.dataset.drag === 'settle') delete track.dataset.drag; }, 400);
    // The click that follows a release belongs to the drag, not to the stop the press began on.
    swallow = performance.now() + 250;
    hooks.preview(null);
    if (commit && near) hooks.commit(near);
  }

  host.addEventListener('pointerdown', event => {
    const track = (event.target as Element).closest<HTMLElement>(TRACK);
    if (!track || !host.contains(track) || event.button !== 0 || !event.isPrimary || hooks.disabled()) return;
    const thumb = track.querySelector<HTMLElement>(THUMB)?.getBoundingClientRect();
    // Grabbed by the thumb, it keeps its place under the pointer; pressed elsewhere, it comes to the pointer by its middle.
    const held = thumb && thumb.width > 0 && event.clientX >= thumb.left && event.clientX <= thumb.right;
    drag = { id: event.pointerId, track, startX: event.clientX, grab: held ? event.clientX - thumb.left : (thumb?.width ?? 0) / 2, moved: false, near: null };
  });
  host.addEventListener('pointermove', event => {
    if (!drag || event.pointerId !== drag.id) return;
    if (!drag.moved) {
      // A press let go somewhere else never ended here: a later hover is not a drag.
      if (event.buttons === 0) { drag = null; return; }
      if (Math.abs(event.clientX - drag.startX) < DRAG_SLOP) return;
      if (hooks.disabled() || !drag.track.isConnected) { drag = null; return; }
      drag.moved = true;
      drag.track.dataset.drag = 'move';
      try { drag.track.setPointerCapture(drag.id); } catch { /* The pointer is gone; the next event ends the drag. */ }
    }
    event.preventDefault();
    move(event.clientX);
  });
  host.addEventListener('pointerup', event => { if (drag && event.pointerId === drag.id) end(true); });
  host.addEventListener('pointercancel', event => { if (drag && event.pointerId === drag.id) end(false); });
  host.addEventListener('lostpointercapture', event => { if (drag?.moved && event.pointerId === drag.id) end(false); });

  return {
    dragging: () => Boolean(drag?.moved),
    swallowClick: () => { const tail = performance.now() < swallow; swallow = 0; return tail; },
  };
}
