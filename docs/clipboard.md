# Clipboard history design

The two rules that matter most, from using an earlier clipboard history:

1. **Every copy is accounted for.** A copy is either in the history or listed as
   skipped, with the reason. Nothing disappears silently.
2. **Search finds only entries that contain what you typed.** No fuzzy matching.

## 1. Capture: nothing goes missing silently

What made the earlier capture unpredictable, and what RedSnap does instead:

| Earlier behavior | RedSnap |
|---|---|
| Read only plain text and bitmaps; files, PNG-only images and HTML-only copies were dropped | Read text, HTML (stored as text, with the HTML kept), RTF (as text), images (PNG, DIB, TIFF on macOS), and file lists (Explorer/Finder copies) |
| Gave up after about 0.8 s if another app held the clipboard | Keep retrying with backoff for up to 5 s. Clipboard formats that the source app renders on request (Excel, remote desktop) can take that long |
| Silently skipped content apps marked as private | Still skip it, and record a "skipped" entry: time, source app, reason ("marked private by KeePassXC"). No content is stored |
| A full internal queue dropped captures | No queue to fill: one capture worker reads the newest clipboard state; if copies come faster than it can read, it records "replaced before it could be read" with the count |
| Nothing logged | Every clipboard change ends as `captured`, `duplicate` (moved to top), or `skipped(reason)`, in the history store and in a small debug log (no content) |

Rules:

- **Duplicates move to the top.** Copying text that is already in the history
  updates its time instead of adding a second entry. That is the only kind of
  copy that doesn't add a row.
- **Size limits are explicit.** Text up to 4 MB, images up to 64 MB. A larger
  copy is recorded as skipped ("too large: 120 MB image"), not dropped.
- **Whitespace-only copies** are skipped with that reason.
- **RedSnap's own writes** (pasting an entry back) are marked as RedSnap's,
  so they move that entry to the top instead of creating a new one.
- **Skipped entries show in the history**, dimmed, so you can see what happened.
  Settings can hide them.
- **Per OS:**
  - Windows: `AddClipboardFormatListener` on a hidden message window; read on
    the capture worker thread.
  - macOS: there is no change notification. Poll `NSPasteboard.changeCount` every
    150 ms while running. If the count jumped by more than one between polls,
    record "N copies replaced before they could be read" so the gap is visible.
  - Linux X11: XFixes selection notifications for `CLIPBOARD`. Wayland: the
    `wlr-data-control` / `ext-data-control` protocol where the compositor offers
    it. GNOME doesn't, so there the history says that capture isn't available.
- **Private markers honored**: Windows `ExcludeClipboardContentFromMonitorProcessing`,
  `CanIncludeInClipboardHistory = 0`, `Clipboard Viewer Ignore`; macOS
  `org.nspasteboard.ConcealedType` / `TransientType`. Each skip names the marker.
- **Excluded apps** (Settings) are skipped with the reason "excluded app".

## 2. Search: literal, predictable

- The query is split on whitespace into words. An entry matches only if **every
  word appears in it as a substring**, ignoring case and diacritics (Unicode case
  folding + NFKD; Chinese text matched as typed).
- Results are ordered **newest first**. Search doesn't reorder by "relevance",
  so the list looks the same way as without a query, just filtered.
- Matches are **highlighted** in the list and the preview, so it's visible why
  each entry is there.
- A prefix narrows the kind: `img:` images, `file:` file lists, `url:` text that
  is a URL. Plain words search text, file names and URLs.
- Images match by their source app and time only (no OCR in phase 1).
- An empty query lists all entries, newest first.
- Search runs on an in-memory index of the normalized text, built off the UI
  thread. Text over 64 KB is indexed in full but shown in the list truncated.

## 3. Storage

- SQLite, owned by one thread (the clipboard store). In-memory state is
  updated only after the write commits.
- Entries have a random 128-bit id (never a reused row id).
- Table `clip(id, kind, created_at, last_copied_at, source_app, text, html,
  image_png, file_list, size_bytes, pinned, skip_reason)`; images stored as PNG
  with a 96 px thumbnail.
- Retention: 30 days and 5,000 entries by default; pinned entries are kept.
  Skipped entries are kept 7 days.

## 4. Using an entry

- **Enter** puts the entry on the clipboard and pastes it into the app that was in
  front before RedSnap opened (Ctrl+V / Cmd+V sent once that app has focus
  again).
- **Cmd/Ctrl+C** copies it without pasting.
- **Ctrl+P** (Cmd+P) pins or unpins it: pinned entries stay at the top of the
  full list and retention never removes them.
- **Ctrl+Delete** (Cmd+Backspace on macOS) deletes it; the selection moves to
  the next entry.
- **Tab** opens the actions: Paste, Copy, Pin/Unpin, Delete.
- The preview scrolls with the wheel, Shift+PageUp/PageDown and its scrollbar
  (drag the thumb, click the track); long texts preview whole.
- The pasted entry moves to the top.
- macOS needs the Accessibility permission to send Cmd+V. Without it, Enter
  only copies and the footer says so.

## 5. Where it lives in the UI

The clipboard view is part of the native search window, because searching it
needs the same instant response: `cb` + space, or its own shortcut
(Ctrl+Alt+C on Windows and Linux, Option+Cmd+C on macOS, as in Alfred), switches the window to a list on the left and a
preview on the right. Clipboard settings (retention, excluded apps, skipped
entries shown or hidden) are in the Electron settings app.
