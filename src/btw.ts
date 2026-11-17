// Side chats (docs/side-chat.md): a question beside a thread whose agent keeps working.
// A side chat is a real session (`kind: 'side'`, `parentSessionId`) made by the backend; what
// is here only picks which one a thread's panel shows. Pure, so tests/btw.test.mjs needs no DOM.

/** `/btw` → `''`, `/btw ask this` → `'ask this'`, anything else → `null`. */
export function parseBtw(text: string): string | null {
  const match = /^\/btw(?:\s+([\s\S]*))?$/i.exec(text.trim());
  return match ? (match[1] ?? '').trim() : null;
}

/** After "New side chat": an empty composer until the next question opens one. */
export const FRESH = 'fresh';
/** The side chat a thread's panel last showed: its id, `FRESH`, or nothing picked yet. */
export type SidePreference = string | typeof FRESH | undefined;

export interface SideShell { id: string; kind?: string; parentSessionId?: string | null; archived: boolean; updatedAt: number }

/** Sidebars, boards, tiles and search skip these; the panel owns them. */
export function isSideChat(task: Pick<SideShell, 'kind'>): boolean { return task.kind === 'side'; }

/** A thread's live side chats, most recently touched first. */
export function sideChatsOf<T extends SideShell>(tasks: readonly T[], parentId: string): T[] {
  return tasks.filter(task => isSideChat(task) && task.parentSessionId === parentId && !task.archived).sort((a, b) => b.updatedAt - a.updatedAt);
}

/** The one the panel shows: none after "New side chat", else the remembered one while it lives, else the newest. */
export function activeSideChat<T extends SideShell>(chats: readonly T[], preferred: SidePreference): T | undefined {
  if (preferred === FRESH) return undefined;
  return chats.find(task => task.id === preferred) ?? chats[0];
}

const ACTIVE_KEY = 'pandacode.sidechat.active';
/** What /btw kept before side chats were real sessions: links to ordinary tasks. Dropped, never migrated. */
const LEGACY_KEY = 'pandacode.btw';
type Storage_ = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

/**
 * Which side chat each thread last showed. A convenience only (the backend owns the chats):
 * read once, then served from memory, so nothing touches storage while a reply streams.
 */
export function sidePreferences(storage: Storage_): { get(mainId: string): SidePreference; set(mainId: string, value: SidePreference): void } {
  let known: Record<string, string> = {};
  try {
    storage.removeItem(LEGACY_KEY);
    const value = JSON.parse(storage.getItem(ACTIVE_KEY) ?? '{}');
    if (value && typeof value === 'object' && !Array.isArray(value)) known = Object.fromEntries(Object.entries(value).filter(entry => typeof entry[1] === 'string')) as Record<string, string>;
  } catch { known = {}; }
  return {
    get: mainId => known[mainId],
    set: (mainId, value) => {
      if (known[mainId] === value) return;
      if (value) known[mainId] = value; else delete known[mainId];
      try { storage.setItem(ACTIVE_KEY, JSON.stringify(known)); } catch { /* private mode: memory only */ }
    },
  };
}
