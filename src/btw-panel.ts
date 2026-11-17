// The inspector's "By the way" tab: one side chat beside the main thread (docs/side-chat.md).
// It draws the side chat with the main transcript's own renderers and composer pieces,
// supplied by main.ts through the host, so both read and behave alike.
import { escape, icon, isImeKey } from './dom';
import { parseBtw } from './btw';
import { followTranscriptBottom } from './transcript-scroll';
import type { Message, MessagePage, Session } from './types';

export interface BtwPanelHost {
  call<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  /** The side chat shown for the current main thread, if any. */
  side(): Session | undefined;
  /** Where a question typed before any side chat exists is kept, per main thread. */
  draftKey(): string;
  running(sideId: string): boolean;
  /** A live or just-finished turn is on screen for the side chat. */
  streaming(sideId: string): boolean;
  waiting(sideId: string): boolean;
  mainRunning(): boolean;
  draft(key: string): string;
  setDraft(key: string, text: string): void;
  /** The saved rows that show while `messages` stream (the main transcript's slice). */
  visible(sideId: string, messages: Message[]): Message[];
  /** Replaces every row of `log` before `region` with the saved turns, as the main transcript draws them. */
  renderRows(log: HTMLElement, region: HTMLElement, side: Session, visible: Message[], all: Message[], follow: () => void): void;
  paintStream(region: HTMLElement, sideId: string, log: HTMLElement): void;
  /** The approval bar and queue above the prompt. */
  aboveHtml(side: Session): string;
  /** The composer toolbar: model, effort and permissions, then Steer and Send or Stop. */
  toolsHtml(side: Session | undefined): string;
  /** A menu is open or opening from an element inside `root`, so it must not be replaced. */
  menuBusy(root: HTMLElement): boolean;
  /** Slash commands and @ mentions on the side prompt; returns their cleanup. */
  mountPickers(textarea: HTMLTextAreaElement, side: Session): () => void;
  bindActions(root: HTMLElement): void;
  /** Sends to the side chat, opening one (with a snapshot of the main thread) if there is none. */
  send(text: string): Promise<void>;
  queue(side: Session, text: string): Promise<void>;
  steer(side: Session, text: string): Promise<void>;
  /** Runs an app slash command on the side chat: false when `text` is not one, a string to send instead, else true. */
  command(side: Session, text: string): Promise<boolean | string>;
  openFull(sideId: string): void;
  newThread(): void;
  close(): void;
  insertMain(text: string): void;
  report(error: unknown): void;
}

/** Agent events that only change the live turn. Each repaints that turn alone, once per frame;
 *  everything else (a run starting or ending, approvals, the queue, a steer) redraws the panel. */
const LIVE_ONLY = new Set(['text', 'thinking', 'tool', 'message_start', 'usage', 'subagent']);

/** The header, log and composer are built once; updates touch only what changed, so typing,
 *  the caret, IME composition, open menus and folded tools survive streaming. */
export class BtwPanel {
  private root?: HTMLElement;
  private messages: Message[] = [];
  private loadedFor = '';
  private rowsKey = '';
  private frame = 0;
  /** The next frame redraws the whole panel, not only the live turn. */
  private full = false;
  /** The last full redraw drew a live turn: until that changes, tokens need no other work. */
  private live = false;
  private retry = 0;
  /** Edit was pressed on a queued message here: the editor the next redraw adds takes focus. */
  private editing = false;
  private tick = 0;
  /** The run ended and its saved copy has not loaded yet: the live turn stays on screen until then. */
  private awaiting = false;
  private stick = true;
  /** Whose text the prompt holds: a side chat's id, or the main thread's stand-in before one exists. */
  private draftFor = '';
  private pickers?: { sideId: string; dispose: () => void };
  /** Keeps a followed log at its end after layout, so no paint has to measure it (as the main transcript does). */
  private resize = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(() => this.pin());
  constructor(private host: BtwPanelHost) {}

  /** Draws the panel into `target`; true when it was built anew (opened, or back from another tab). */
  show(target: HTMLElement): boolean {
    const built = !this.root || this.root.parentElement !== target;
    if (built) target.replaceChildren(this.build());
    this.update();
    return built;
  }
  /** Hidden: the next `show` reloads, so nothing stale comes back. */
  dispose(): void {
    this.pickers?.dispose(); this.pickers = undefined;
    this.resize?.disconnect();
    this.root = undefined; this.loadedFor = ''; this.rowsKey = ''; this.awaiting = false; this.draftFor = ''; this.live = false;
    cancelAnimationFrame(this.frame); this.frame = 0; clearTimeout(this.retry);
  }
  /** The id of the side chat on screen, if the panel shows one. */
  shownId(): string | undefined { return this.root?.isConnected ? this.loadedFor || undefined : undefined; }
  /** Called for agent events and the side chat's queue, steer and approval changes; redraws once per frame. */
  event(sessionId: string, kind: string): void {
    if (!this.root?.isConnected || sessionId !== this.loadedFor) return;
    if (kind === 'steer') this.tick++;
    if (kind === 'done') { this.awaiting = true; void this.reload(); return; }
    if (kind === 'turn_changes') void this.reload();
    this.refresh(LIVE_ONLY.has(kind) && this.live);
  }
  /** Schedules a redraw: of the live turn alone when `liveOnly`, else of the whole panel. */
  refresh(liveOnly = false): void {
    if (!this.root?.isConnected) return;
    if (!liveOnly) this.full = true;
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      const full = this.full; this.full = false;
      if (full) this.update(); else this.paintLive();
    });
  }
  /** The side chat's harness changed: its saved rows are re-read under the new one. */
  harnessChanged(): void { this.rowsKey = ''; void this.reload(); }
  /** A shown side-chat message, for its attachment chips and Copy. */
  message(id: number): Message | undefined { return this.messages.find(m => m.id === id); }
  async reload(): Promise<void> {
    const sideId = this.loadedFor;
    if (!sideId) return;
    try {
      // The backend returns only the side chat's own exchange, never the history it inherited.
      const page = await this.host.call<MessagePage>('get_messages', { sessionId: sideId });
      if (sideId !== this.loadedFor) return;
      // A sent message stays on screen until its saved copy arrives; once the run is over (or the
      // send failed) only saved messages remain.
      const running = this.host.running(sideId);
      const pending = running ? this.messages.filter(m => m.id < 0 && m.role === 'user' && !page.messages.some(saved => saved.role === 'user' && saved.content === m.content)) : [];
      this.messages = [...page.messages, ...pending];
      this.awaiting = false;
      this.update();
    } catch (error) { this.awaiting = false; this.update(); this.host.report(error); }
  }
  /** Only ever called for the user's own action (/btw, the tab, New side chat): streaming never moves focus. */
  focus(): void { this.root?.querySelector<HTMLTextAreaElement>('.btw-input')?.focus({ preventScroll: true }); }
  /** Shows a sent message until the saved one arrives. */
  sent(sideId: string, message: Message): void {
    if (sideId !== this.loadedFor || this.messages.includes(message)) return;
    this.messages = [...this.messages, message]; this.stick = true; this.update();
  }
  unsent(sideId: string, message: Message): void {
    if (sideId !== this.loadedFor) return;
    this.messages = this.messages.filter(m => m !== message); this.update();
  }

  private build(): HTMLElement {
    this.pickers?.dispose(); this.pickers = undefined; this.rowsKey = ''; this.draftFor = ''; this.live = false;
    this.resize?.disconnect();
    const root = this.root = document.createElement('div');
    root.className = 'btw-panel';
    root.innerHTML = `<header class="btw-head"><div><strong></strong><small role="status"></small></div><div class="btw-actions"><button class="icon-button" data-btw="new" aria-label="New side thread" title="New side thread">${icon('plus')}</button><button class="icon-button" data-btw="open" aria-label="Open side thread as a full thread" title="Open as full thread">${icon('external')}</button><button class="icon-button" data-btw="close" aria-label="Close side thread panel" title="Close (the side thread is kept)">${icon('close')}</button></div></header><div class="inspector-empty btw-empty">${icon('chat')}<h3>No side thread yet</h3><p>Ask something on the side while this task keeps working. It starts with what the task has said so far and never interrupts it. Type <code>/btw</code> and a question in the prompt, or write it here.</p></div><div class="transcript btw-transcript" role="log" aria-label="Side thread conversation" aria-live="off" tabindex="0"><div class="btw-stream-region"></div></div><div class="btw-composer-area"><div class="btw-above"></div><form class="composer btw-composer"><textarea class="btw-input" rows="2" aria-label="Message the side thread"></textarea><div class="composer-toolbar btw-toolbar"></div></form></div>`;
    const input = root.querySelector<HTMLTextAreaElement>('.btw-input')!, log = root.querySelector<HTMLElement>('.btw-transcript')!;
    const resize = () => { input.style.height = 'auto'; input.style.height = Math.min(input.scrollHeight, 180) + 'px'; input.style.overflowY = input.scrollHeight > input.clientHeight + 1 ? 'auto' : 'hidden'; };
    input.addEventListener('input', () => { if (this.draftFor) this.host.setDraft(this.draftFor, input.value); resize(); });
    // Following is decided here, where the reader scrolls, and read back without layout while text streams.
    log.addEventListener('scroll', () => { this.stick = log.scrollHeight - log.scrollTop - log.clientHeight < 48; }, { passive: true });
    followTranscriptBottom(log, () => this.stick);
    this.resize?.observe(log.querySelector('.btw-stream-region')!);
    root.querySelector('form')!.addEventListener('submit', event => { event.preventDefault(); this.submit(false); });
    // Listening on the form lets the slash and @ pickers (on the textarea) take Enter first.
    root.querySelector('form')!.addEventListener('keydown', event => {
      if (event.target !== input || event.key !== 'Enter' || event.shiftKey || isImeKey(event) || event.defaultPrevented) return;
      event.preventDefault(); this.submit(event.metaKey || event.ctrlKey);
    });
    // Esc leaves the panel for the main prompt; it never reaches the global stop-the-agent key.
    root.addEventListener('keydown', event => {
      if (event.key !== 'Escape' || event.isComposing || event.defaultPrevented) return;
      event.preventDefault(); event.stopPropagation();
      document.querySelector<HTMLTextAreaElement>('#prompt')?.focus();
    });
    // Header and Insert buttons act without taking focus or the text selection.
    root.addEventListener('mousedown', event => { if ((event.target as HTMLElement).closest('.btw-head button, [data-btw-insert]')) event.preventDefault(); });
    root.addEventListener('click', event => {
      if ((event.target as HTMLElement).closest('[data-action="queue-edit"]')) this.editing = true;
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>('[data-btw], [data-btw-insert]');
      if (!button || button.disabled) return;
      const side = this.host.side();
      if (button.dataset.btw === 'new') this.host.newThread();
      else if (button.dataset.btw === 'open' && side) this.host.openFull(side.id);
      else if (button.dataset.btw === 'close') this.host.close();
      else if (button.dataset.btw === 'steer') this.submit(true);
      else if (button.dataset.btwInsert) {
        const message = this.messages.find(m => String(m.id) === button.dataset.btwInsert);
        if (message) this.host.insertMain(message.content);
      }
    });
    this.host.bindActions(root);
    return root;
  }

  private pin(): void {
    const log = this.root?.querySelector<HTMLElement>('.btw-transcript');
    if (log && this.stick && !log.hidden) log.scrollTop = log.scrollHeight;
  }

  /** Enter sends (a new turn) while idle and queues while the side agent works; ⌘/Ctrl Enter or Steer steers. */
  private submit(steer: boolean): void {
    const input = this.root?.querySelector<HTMLTextAreaElement>('.btw-input');
    if (!input) return;
    const typed = input.value.trim();
    // `/btw` here just means this side chat.
    const text = parseBtw(typed) ?? typed;
    if (!text) { input.value = ''; return; }
    const side = this.host.side(), key = this.draftFor;
    const running = side ? this.host.running(side.id) : false;
    input.value = ''; if (key) this.host.setDraft(key, '');
    const work = !side ? this.host.send(text)
      : running ? (steer ? this.host.steer(side, text) : this.host.queue(side, text))
      : this.host.command(side, text).then(done => done === true ? undefined : this.host.send(done || text));
    this.stick = true;
    work.catch(error => {
      // Put the text back in whichever composer is showing now, unless something new was typed.
      const current = this.root?.querySelector<HTMLTextAreaElement>('.btw-input');
      if (key && !this.host.draft(key)) this.host.setDraft(key, typed);
      if (current && !current.value && key === this.draftFor) current.value = typed;
      this.host.report(error);
    }).finally(() => this.refresh());
  }

  /** One streamed frame: the live turn only. No saved rows, header or composer work, and no layout read. */
  private paintLive(): void {
    const root = this.root, id = this.loadedFor;
    if (!root?.isConnected || !id || !this.host.streaming(id)) { this.update(); return; }
    const log = root.querySelector<HTMLElement>('.btw-transcript')!;
    this.host.paintStream(log.querySelector<HTMLElement>('.btw-stream-region')!, id, log);
  }

  private update(): void {
    const root = this.root;
    if (!root?.isConnected) return;
    const side = this.host.side(), input = root.querySelector<HTMLTextAreaElement>('.btw-input')!;
    if (!side) { this.loadedFor = ''; this.messages = []; this.awaiting = false; this.live = false; this.pickers?.dispose(); this.pickers = undefined; }
    else if (side.id !== this.loadedFor) {
      this.loadedFor = side.id; this.messages = []; this.rowsKey = ''; this.awaiting = false; this.stick = true;
      void this.reload();
    }
    // The prompt holds one thread's unsent text at a time: switching threads swaps it, typing keeps it.
    const draftKey = side?.id ?? this.host.draftKey();
    if (draftKey !== this.draftFor) { this.draftFor = draftKey; input.value = this.host.draft(draftKey); input.style.height = ''; }
    const log = root.querySelector<HTMLElement>('.btw-transcript')!;
    root.querySelector<HTMLElement>('.btw-head')!.hidden = !side;
    root.querySelector<HTMLElement>('.btw-empty')!.hidden = Boolean(side);
    log.hidden = !side;
    this.paintComposer(root, side);
    if (!side) return;
    const id = side.id, running = this.host.running(id), waiting = this.host.waiting(id);
    const head = root.querySelector<HTMLElement>('.btw-head strong')!;
    if (head.textContent !== side.title) { head.textContent = side.title; head.title = side.title; }
    const state = waiting ? 'Needs your input' : running ? 'Working' : 'Ready', main = this.host.mainRunning();
    const status = state + (main ? ' · main running' : '');
    const small = root.querySelector<HTMLElement>('.btw-head small')!;
    if (small.textContent !== status) { small.textContent = status; small.title = state + (main ? ' · the main thread is still running' : ''); }
    if (this.pickers?.sideId !== id) {
      this.pickers?.dispose();
      this.pickers = { sideId: id, dispose: this.host.mountPickers(input, side) };
    }
    this.paintThread(log, side);
  }

  private paintThread(log: HTMLElement, side: Session): void {
    const region = log.querySelector<HTMLElement>('.btw-stream-region')!;
    const visible = this.host.visible(side.id, this.messages);
    const key = `${side.id}:${side.provider}:${this.tick}:` + visible.map(m => `${m.id}.${m.content.length}`).join(',');
    if (key !== this.rowsKey) {
      this.rowsKey = key;
      this.host.renderRows(log, region, side, visible, this.messages, () => this.pin());
      // Each final answer can go into the main prompt.
      for (const actions of log.querySelectorAll<HTMLElement>('.turn-card-reply-row > .message-actions')) {
        const messageId = actions.querySelector<HTMLElement>('[data-message-key]')?.dataset.messageKey;
        if (!messageId) continue;
        actions.insertAdjacentHTML('beforeend', `<button type="button" class="btw-insert" data-btw-insert="${escape(messageId)}" aria-label="Insert this answer into the main prompt" title="Insert into main prompt">${icon('forward')}</button>`);
      }
      if (!visible.length && !this.host.streaming(side.id)) region.insertAdjacentHTML('beforebegin', '<p class="inspector-hint">No messages yet.</p>');
      // New rows change the log's height: the observer pins a followed log after layout.
      this.resize?.disconnect();
      for (const row of log.children) this.resize?.observe(row);
      this.pin();
    }
    this.live = this.host.streaming(side.id);
    if (this.live) this.host.paintStream(region, side.id, log);
    // The live turn stays until the saved copy that replaces it has loaded.
    else if (!this.awaiting && region.firstChild) { region.replaceChildren(); delete region.dataset.sessionId; }
  }

  private paintComposer(root: HTMLElement, side: Session | undefined): void {
    const above = root.querySelector<HTMLElement>('.btw-above')!, toolbar = root.querySelector<HTMLElement>('.btw-toolbar')!;
    const aboveHtml = side ? this.host.aboveHtml(side) : '', toolsHtml = this.host.toolsHtml(side);
    // A control with its menu open is redrawn once the menu has closed.
    const held = (above.dataset.html !== aboveHtml && this.host.menuBusy(above)) || (toolbar.dataset.html !== toolsHtml && this.host.menuBusy(toolbar));
    if (held) { clearTimeout(this.retry); this.retry = window.setTimeout(() => this.refresh(), 150); }
    if (above.dataset.html !== aboveHtml && !this.host.menuBusy(above)) {
      const editing = above.querySelector('.queue-edit');
      above.innerHTML = aboveHtml; above.dataset.html = aboveHtml;
      // Edit on a queued message moves focus into the editor this redraw adds.
      if (!editing && this.editing) above.querySelector<HTMLTextAreaElement>('.queue-edit')?.focus();
      this.editing = false;
    }
    if (toolbar.dataset.html !== toolsHtml && !this.host.menuBusy(toolbar)) {
      // Keep keyboard focus on the same control when it is redrawn.
      const focused = toolbar.contains(document.activeElement) ? (document.activeElement as HTMLElement).dataset.action ?? (document.activeElement as HTMLElement).dataset.btw : undefined;
      toolbar.innerHTML = toolsHtml; toolbar.dataset.html = toolsHtml;
      if (focused) toolbar.querySelector<HTMLElement>(`[data-action="${focused}"], [data-btw="${focused}"]`)?.focus({ preventScroll: true });
    }
    const input = root.querySelector<HTMLTextAreaElement>('.btw-input')!;
    const running = side ? this.host.running(side.id) : false;
    const placeholder = running ? 'Queue a follow-up · ⌘↩ steers now' : 'Ask on the side…';
    if (input.placeholder !== placeholder) {
      input.placeholder = placeholder;
      input.setAttribute('aria-description', running ? 'Enter to queue for when the side agent finishes. Command Enter to steer its running turn.' : 'Enter to send. Shift Enter for a new line. Type / for commands or @ to mention a project file.');
    }
  }
}
