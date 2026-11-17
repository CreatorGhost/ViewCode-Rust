/**
 * Project colours (from ViewCode): a project can carry one of 18 named colours (`Project.color`,
 * `set_project_color`). It shows on the project's rail tile and its badge in the thread rows always,
 * and, while "Colour the app by the active project" is on, on what follows the open thread's project:
 * the composer glow and the open row. Tones are the `--project-<name>` tokens (palette-core.ts);
 * never a literal colour. Pure markup and one webview preference; `main.ts` draws with it.
 */
import { PROJECT_COLORS } from './palette-core.ts';
import type { ProjectColor } from './types';

/** Names as the picker reads them, in `ProjectColor` order (ViewCode's labels). */
export const PROJECT_COLOR_LABELS: Record<ProjectColor, string> = {
  gray: 'Gray', red: 'Red', orange: 'Orange', amber: 'Amber', yellow: 'Yellow', lime: 'Lime', green: 'Green', emerald: 'Emerald', teal: 'Teal',
  cyan: 'Cyan', sky: 'Sky', blue: 'Blue', indigo: 'Indigo', violet: 'Violet', purple: 'Purple', fuchsia: 'Fuchsia', pink: 'Pink', rose: 'Rose',
};
/** The picker's id for "no colour". */
export const NO_COLOR = 'none';
/** The group the swatches share in a menu. */
export const COLOR_GROUP = 'Colour';

/** A stored colour this build can draw, else ''. A name from a newer build draws as no colour. */
export function knownProjectColor(color?: string | null): ProjectColor | '' {
  return color && PROJECT_COLORS.includes(color) ? color as ProjectColor : '';
}
/** The tone token of a colour, e.g. `var(--project-violet)`; '' without one. */
export function projectTone(color?: string | null): string {
  const known = knownProjectColor(color);
  return known ? `var(--project-${known})` : '';
}
/** Attributes that hand an element its project's tone as `property` (` data-color="violet" style="--tile-tone:var(--project-violet)"`);
 * '' for a project without a colour, which stays neutral. */
export function projectToneAttributes(color: string | null | undefined, property: string): string {
  const known = knownProjectColor(color);
  return known ? ` data-color="${known}" style="${property}:var(--project-${known})"` : '';
}

export interface ColorChoice { id: string; label: string; segment: string; swatch: string; selected: boolean; separator?: boolean }
/** The picker's swatches for a menu: None, then the 18 colours. `used` are colours other projects wear, named in the
 * label so each project can be kept distinct (as ViewCode marks them). */
export function projectColorChoices(current?: string | null, used: Iterable<string> = []): ColorChoice[] {
  const chosen = knownProjectColor(current), taken = new Set(used);
  return [
    { id: NO_COLOR, label: 'None', segment: COLOR_GROUP, swatch: '', selected: !chosen },
    ...PROJECT_COLORS.map(name => ({
      id: name, label: `${PROJECT_COLOR_LABELS[name as ProjectColor]}${taken.has(name) && name !== chosen ? ' (used by another project)' : ''}`,
      segment: COLOR_GROUP, swatch: `var(--project-${name})`, selected: name === chosen,
    })),
  ];
}
/** A picked swatch id as `set_project_color` takes it: a colour name, or null for None. Undefined if the id is not a swatch. */
export function pickedProjectColor(id: string): ProjectColor | null | undefined {
  if (id === NO_COLOR) return null;
  return knownProjectColor(id) || undefined;
}

const KEY = 'pandacode.projectTint';
/** Fired on `window` when the setting changes. */
export const TINT_EVENT = 'pandacode-project-tint-change';
/** Settings → Appearance → Colour the app by the active project (default on, as ViewCode). */
export function projectTintEnabled(): boolean {
  try { return localStorage.getItem(KEY) !== 'off'; } catch { return true; }
}
export function setProjectTintEnabled(on: boolean): void {
  try { localStorage.setItem(KEY, on ? 'on' : 'off'); } catch { /* A choice that cannot be saved leaves the tint on. */ }
  window.dispatchEvent(new Event(TINT_EVENT));
}
/** The colour the app follows for the open thread's project: its colour while the setting is on, else '' (the accent). */
export function activeProjectTint(color?: string | null): ProjectColor | '' {
  return projectTintEnabled() ? knownProjectColor(color) : '';
}
