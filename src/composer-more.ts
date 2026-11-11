/**
 * The composer's `+` menu. The control row shows only `+`, the access mode, the model, the mic and
 * Send; every other composer control (Team, Token saver, Council, the computer, the branch, a linked
 * PR or issue) is still rendered by its own module, but parked: `.composer-parked` lays them,
 * invisible, over `+`. The menu lists them by their own names, icons and disabled states, and a pick
 * presses the parked control, so its handler, its state and its announcements are the ones it always
 * had and whatever it opens (a menu, a panel) opens from `+`.
 */
export const PARKED = '.composer-parked';
const PLUS = '#composer-more';
const PREFIX = 'parked:';

export type ParkedItem = { id: string; label: string; description?: string; leading?: string; disabled?: boolean; separator?: boolean };

const plus = () => document.querySelector<HTMLElement>(PLUS);
/** Parked controls in the order they were drawn: buttons, and the fixed notes (where the thread runs) among them. */
function parked(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(`${PARKED} :is(button,[role=img],.branch-chip-fixed)`)]
    // A thread that hides a control (a chat or a bot thread has no branch) does not list it.
    .filter(el => el.getClientRects().length > 0 && el.getAttribute('aria-hidden') !== 'true' && !el.parentElement?.closest(`${PARKED} :is(button,[role=img])`));
}
/** What the parked control says about itself beyond its name (which already carries on or off):
 * that a change is on its way (its spinner cannot be seen), or its own description. */
function detail(el: HTMLElement, label: string): string {
  if (el.getAttribute('aria-busy') === 'true') return 'Working…';
  const text = el.getAttribute('aria-description') ?? '';
  return (text.startsWith(label) ? text.slice(label.length).replace(/^[.\s]+/, '') : text).trim();
}

/** One menu row per parked control. Fixed notes are listed disabled: they say where the thread runs. */
export function parkedItems(): ParkedItem[] {
  return parked().map((el, index) => {
    const label = (el.getAttribute('aria-label') ?? el.textContent ?? '').trim();
    const mark = el.querySelector('svg, img');
    const button = el instanceof HTMLButtonElement;
    return {
      id: `${PREFIX}${index}`, label, description: detail(el, label) || undefined, leading: mark?.outerHTML,
      disabled: !button || el.disabled || el.getAttribute('aria-disabled') === 'true', separator: index === 0,
    };
  }).filter(item => item.label);
}

export const isParkedChoice = (id: string): boolean => id.startsWith(PREFIX);

/** Presses the parked control a menu row stands for. */
export function pressParked(id: string): void {
  const el = parked()[Number(id.slice(PREFIX.length))];
  if (!(el instanceof HTMLButtonElement) || el.disabled) return;
  el.click();
}

/** A menu a parked control opens belongs to `+`: it opens there and hands focus back there. */
export function menuAnchor(anchor: HTMLElement): HTMLElement {
  return anchor.closest(PARKED) ? plus() ?? anchor : anchor;
}

/** Focus handed to a parked control (a panel closing returns it to its opener) goes to `+`: an
 * invisible control cannot take it. Parked controls are redrawn by their modules, so this follows them. */
export function mountComposerMore(area: HTMLElement): void {
  const redirect = (options?: FocusOptions) => plus()?.focus(options);
  const adopt = () => { for (const el of area.querySelectorAll<HTMLElement>(`${PARKED} :is(button,[tabindex])`)) if (el.focus !== redirect) el.focus = redirect; };
  new MutationObserver(adopt).observe(area, { childList: true, subtree: true });
  adopt();
}
