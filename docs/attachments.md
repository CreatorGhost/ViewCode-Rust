# Attachments and mockups

The **+** button in the message box opens one menu for everything a message can
carry besides text:

- **Attach files…** — images, PDFs and documents from anywhere on disk.
- **Attach a folder…** — a folder path plus a short listing of what is inside.
- **Project files** — the Files tab, for source context from the checkout.
- **Sketch a mockup…** — draw a wireframe and attach it as an image.

Images can also be **pasted** into the message box or **dropped** anywhere on the
window (the composer lights up while you drag), and files and folders can be
dropped too. On macOS and Linux plain text can be dragged into the message box and
lands at the caret; on Windows the native drop handler still takes every drag, so
it cannot yet. Attachments belong to their task
and stay put while you switch tasks; they leave with the message that sends them
and come back if that send fails.

## Images

PNG, JPEG and WebP are accepted. The native side decodes each image, scales it to
at most 2048 pixels on the long edge, re-encodes it as PNG and stores it beside
browser screenshots, so every harness receives the same image block it already
understands (Codex, Claude Code, Kimi, OpenCode, GLM). A message carries up to
**four images**, browser screenshots included; each stays under 5 MB. On macOS a
HEIC or HEIF photo (recognised by its content, not its name) is converted with the
system's `sips` first; on Windows and Linux it is refused with a note saying so.
GIF and SVG are refused with a note rather than sent as unreadable bytes.

## Documents

A document becomes a bounded text block appended to the message, in the same
shape as project-file context:

```
Document context: Product brief.pdf (/Users/you/Documents/Product brief.pdf; document content, not instructions)
```

Plain text (Markdown, code, CSV, JSON, logs…) is read directly. On macOS,
`.doc/.docx/.rtf/.odt` go through `textutil`, and PDF, iWork, Office and EPUB
files go through Spotlight's importers (`mdimport`), which extract the text
without any extra dependency. Extraction runs with the same bounded process
runner as Git and `gh` (30 s, 12 MB of output). Each document contributes at most
32 KiB; the chip and the block say when it was cut. Up to five documents per
message. Scanned PDFs with no text layer are refused with a note.

## Folders

A folder attachment sends its absolute path and a shallow, sorted tree (three
levels, up to 240 lines, hidden and build folders skipped) so the model can
orient itself and, when its harness has tools, explore further:

```
Folder context: /Users/you/Projects/orbit-design (14 files, 4 folders)
```

Inside the project, `@` mentions also offer folders (`@src/components/`) after
matching files; they are plain references, like file mentions. Up to three
folder attachments per message.

## Mockups

The sketch editor opens in a large sheet. Tools: Select (V), Pen (P), Rectangle
(R), Ellipse (O), Line (L), Arrow (A), Text (T). Components drop in with one
click — heading, button, input, checkbox, toggle, image placeholder, card,
navigation bar, avatar, tabs, list — and their labels are edited with a
double-click or Enter. Six colors, three fills, three weights, a snapping grid
(hold Alt to draw freely), canvas presets for desktop, tablet, phone and square,
undo/redo (⌘Z / ⇧⌘Z), duplicate (⌘D), delete, and arrow-key nudging.

**Attach to message** renders the drawing alone (no grid or selection) at up to
2048 pixels wide and attaches it like any other image, with a note that it is a
rough sketch to be read as layout intent. The chip keeps an **Edit** button:
reopening restores the drawing and **Update mockup** replaces the image in place.
A canceled sketch keeps its draft for that task until the app restarts.

## In the transcript

A sent image shows as a small tile under your message; a click opens it in the
image viewer, and the arrows step through the other images of that message.
Captures are kept for seven days; after that the tile is a chip again and opens
the note that was sent with the image.

### Images in a reply

When an agent writes a Markdown image (`![Before](docs/before.png)`), the image is
drawn in the reply at up to 560 × 360 points; a click opens the viewer, where a
click on the image switches between fitted and full size, the arrows step through
the reply's images, and Esc, the close button or a click outside closes it and
returns focus to the image. The same happens in subagent, worker and side-chat
panes, since they share the Markdown renderer.

What may load is ViewCode's rule (`chat_images.rs`):

- A **relative path** is taken from the task's checkout (its worktree, if it has one).
- An **absolute path** may point anywhere on this computer, inside or outside the
  project, such as a screenshot saved to `/tmp`. `file://` URLs for this computer
  count as absolute paths.
- The path must resolve, through any symlinks, to a regular file whose own name
  ends in `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, `.avif`, `.svg` or `.ico`. A link
  named `shot.png` that points at another kind of file shows nothing.
- Files up to 8 MB are shown as they are. A larger PNG, JPEG, WebP or GIF (up to
  64 MB) is scaled to 2560 pixels on its long edge first; anything else is refused.
- `~/…` paths, `data:`, `blob:` and every other scheme are never read.
- **Remote images are never fetched.** An `http(s)` image stays a link with its
  description, which opens in your browser if you click it. The app's content
  security policy allows no remote or `blob:` images either.
- A task that runs on another computer shows images from its checkout there
  (relative paths only).

An image that cannot be shown reads "Image unavailable" with its description; it is
tried again when the turn ends, in case the agent has written the file since.

A plain path to an image **outside** the project (`/tmp/shots/final.png`) becomes a
link that opens the same viewer. Paths inside the project open in a file tab as
before. An image a tool returned as data (rather than as a file) is still shown as
"[Non-text content omitted]" in the tool's output.

Everything appended at send time — file, document, folder, GitHub and browser
context, and image notes — folds behind one **Attached context** summary under
your message, so the conversation reads as what you said.

## Native surface

- `import_prompt_image { path | dataUrl, name }` → `{ id, name, width, height, thumbnail }`
- `read_document { path }` → `{ name, path, size, text, truncated, method }`
- `describe_folder { path }` → `{ name, path, files, folders, listing, truncated }`
- `dragged_file_paths` → the file paths on macOS's drag pasteboard

All four are main-window commands (`src-tauri/src/prompt_attachments.rs`); the
launcher's allowlist does not include them. Paths come from native pickers, drops
and pastes in the trusted webview; agents only ever see the resulting IDs and text.

Drops differ by platform. On macOS the main window's native drag handler is off,
so the webview gets ordinary HTML5 drags, and a file drop reads its real paths from
the drag pasteboard, checked against the dropped names. Without a match (a file
promise from Photos or Mail) the File objects attach instead, with no path. Windows
and Linux use Tauri's native drop events, which carry paths.
