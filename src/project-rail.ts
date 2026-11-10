/** The rail at the window's left edge: project tiles, one usage ring per agent, then the shell's
 * icon buttons (`.sidebar-bottom`, which `main.ts` moves in). Pure markup; `main.ts` draws it. */
import { escape } from './dom';
import { providerLogo } from './provider-logos';
import type { Provider, ProviderId } from './types';
import { usagePeek } from './usage-peek';

/** Tiles shown before the rest fold into the "+N" tile. */
export const RAIL_PROJECTS = 5;
/** Rings shown; every agent stays in the Usage dialog a ring opens. */
export const RAIL_RINGS = 4;
/** Tints in `viewcode-shell.css` (`.rail-tile[data-tint]`), each a design-system tone. */
const TINTS = 7;

export interface RailProject { id: string; name: string; /** Stored colour, once projects carry one. */ color?: string | null; }

/** A project's tile colour. Until projects store one, a stable tint derived from the id; a stored
 * `#rrggbb` wins. Returns the attributes for the tile. */
export function projectTint(project: RailProject): string {
  if (project.color && /^#[0-9a-f]{6}$/i.test(project.color)) return `style="--tile-tone:${project.color}"`;
  let hash = 2166136261;
  for (let index = 0; index < project.id.length; index++) hash = Math.imul(hash ^ project.id.charCodeAt(index), 16777619);
  return `data-tint="${(hash >>> 0) % TINTS}"`;
}

/** The first letter (or digit) of a name, for its tile. */
function initial(name: string): string {
  const letter = [...name.trim()].find(character => /[\p{L}\p{N}]/u.test(character)) ?? [...name.trim()][0] ?? '?';
  return letter.toUpperCase();
}

/** Project tiles: the filtered project is pressed, the open thread's project carries a small mark.
 * Past `RAIL_PROJECTS` the rest sit behind a "+N" tile, which opens the project list. */
export function railProjectsHtml(projects: readonly RailProject[], filterId: string, openId: string): string {
  // One hidden project would cost a tile to say "+1", so it is simply shown.
  const all = projects.length <= RAIL_PROJECTS + 1;
  const shown = all ? [...projects] : projects.slice(0, RAIL_PROJECTS);
  // The chosen project never hides behind "+N".
  const chosen = projects.find(project => project.id === filterId);
  if (chosen && !shown.includes(chosen)) shown[shown.length - 1] = chosen;
  const rest = projects.length - shown.length;
  const tiles = shown.map(project => {
    const pressed = project.id === filterId, open = !pressed && project.id === openId;
    const name = escape(project.name);
    return `<button type="button" class="rail-tile" data-action="rail-project" data-id="${escape(project.id)}" ${projectTint(project)} aria-pressed="${pressed}"${open ? ' data-open' : ''} aria-label="${name}" title="${pressed ? `${name} · click to show all projects` : name}"><span aria-hidden="true">${escape(initial(project.name))}</span></button>`;
  }).join('');
  const more = rest > 0 ? `<button type="button" class="rail-tile rail-more" data-action="rail-projects-more" aria-haspopup="dialog" aria-expanded="false" aria-label="${rest} more project${rest === 1 ? '' : 's'}" title="${rest} more project${rest === 1 ? '' : 's'}"><span aria-hidden="true">+${rest}</span></button>` : '';
  return tiles + more;
}

const RADIUS = 13;
/** One ring per installed agent: the arc is the plan's most used window, as last checked. */
export function railUsageHtml(providers: readonly Provider[], shortName: (id: ProviderId) => string, now = Date.now()): string {
  return providers.filter(provider => provider.installed).slice(0, RAIL_RINGS).map(provider => {
    const percent = usagePeek(provider.id, now)?.percent ?? null, name = escape(shortName(provider.id));
    const level = percent === null ? 'unknown' : percent >= 100 ? 'danger' : percent >= 80 ? 'warn' : 'normal';
    const label = percent === null ? `${name} usage: not checked yet` : `${name} usage: ${percent}% used`;
    const arc = percent === null ? '' : `<circle class="rail-ring-arc" cx="16" cy="16" r="${RADIUS}" pathLength="100" stroke-dasharray="${percent} 100" transform="rotate(-90 16 16)"/>`;
    return `<button type="button" class="rail-ring" data-action="agent-usage" data-provider="${provider.id}" data-level="${level}" aria-label="${label}" title="${label}"><span class="rail-ring-dial"><svg viewBox="0 0 32 32" aria-hidden="true"><circle class="rail-ring-track" cx="16" cy="16" r="${RADIUS}"/>${arc}</svg>${providerLogo(provider.id, 'rail-ring-logo')}</span><span class="rail-ring-label" aria-hidden="true">${percent === null ? '–' : `${percent}%`}</span></button>`;
  }).join('');
}
