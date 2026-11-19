# Snippets

Text you use often, inserted by typing its trigger in any app (expansion as
you type) or by finding it in RedSnap and pressing Enter.

## Model

In `config.json` under `snippets` (edited in Settings → Snippets):
`expandAsYouType`, `excludedApps`, and `items` with `id`, `name`, `trigger`,
`text` (a template) and `autoExpand`.

Validation (crates/core/src/config.rs): triggers have no spaces, are at most
64 characters and unique; a trigger may not be the start of another one (it
would always fire first); templates are checked (below); at most 2,000
snippets of up to 100,000 characters.

## Templates (crates/core/src/snippets.rs)

`{{cursor}}` (once: where the caret ends up), `{{date}}`, `{{time}}`,
`{{datetime}}`, `{{date:FORMAT}}` (strftime), `{{clipboard}}` (the
clipboard's text), and `\{{` for a literal `{{`.

## Matching what's typed

- The longest trigger that ends what was typed wins.
- A trigger starting with a letter or digit must start a word: the character
  before it isn't a letter or digit (`brb` doesn't fire in "abrb"). Triggers
  starting with a symbol (`;sig`) fire anywhere.
- What was typed resets on a click, a focus change, navigation keys (arrows,
  Home, Enter, Tab, Esc…) and Ctrl/Alt/Win chords, so a stale prefix never
  expands. Backspace removes the last character.

## Expansion (Windows: crates/platform/src/snippets/windows.rs)

- A low-level keyboard hook and a mouse hook on their own thread only queue
  events (Windows drops hooks that take too long); a worker decodes keys with
  the foreground app's keyboard layout (`ToUnicodeEx` with the
  don't-change-state flag, so dead keys keep working).
- Nothing expands in excluded apps, in password fields (standard Edit
  controls), or while an IME is open in native mode (Chinese pinyin is being
  composed, not typed).
- The expansion is one `SendInput` batch: backspaces for the trigger, the
  text as Unicode keystrokes (newlines as Enter, tabs as Tab), Left arrows to
  the `{{cursor}}`. RedSnap's own keystrokes carry a marker the hook skips.

From the launcher, Enter on a snippet hides RedSnap, returns focus to the app
that was in front, and types it the same way.

macOS and Linux: snippets are searchable; inserting and expansion as you type
aren't implemented yet (see docs/macos-linux-catch-up.md).
