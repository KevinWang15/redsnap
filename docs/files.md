# Files

Two ways to reach files from the search window:

- **File search:** `f` and a name (`f report q3`) asks the OS's file index.
- **Folder browsing:** typing a path (`~/dev/`, `C:\Users\`, `/etc/`) lists that
  folder. What follows the last separator filters its entries.

## File search (`f …`)

`f` is a built-in keyword, like `sn` for snippets. A web search or quick link
the user names `f` wins over it. Settings → Search → "File search" turns it
off (`providers.files`).

Each OS asks its own index (`crates/platform/src/files/`):

| OS | Index | How | Matches |
|---|---|---|---|
| Windows | Everything (voidtools) | Window-message IPC (`WM_COPYDATA`, the protocol its SDK uses), no SDK DLL. Needs Everything 1.4.1 or later, running. 1.5 alpha's default instance works too. | Everything's own syntax: words, `*.pdf`, `ext:docx`, `folder:`, a word with `\` matches paths |
| macOS | Spotlight | `mdfind -0` with `kMDItemFSName == "*word*"cd` per word | Names containing every word, any case and accents |
| Linux | plocate, else locate | `plocate -i -b -0 -l N -- words` (`locate` also gets `-A`) | Names containing every word; wildcards |

- At most 200 results are asked for. Everything returns them newest
  modified first; RedSnap then ranks them: name matches by class (exact,
  prefix, word, substring), what was picked for this query before, then the
  index's order. Results the index matched some other way (wildcards,
  paths, `ext:`) stay, after the name matches.
- Paths the index has but the disk doesn't (a stale `locate` database,
  Spotlight lag) are dropped on macOS and Linux.
- While the answer to the newest words is on its way, the previous answer
  stays, narrowed to what still matches, so the list doesn't blink.
- Enter opens; Ctrl/Cmd+Enter shows it in Explorer/Finder/the file manager;
  Tab adds Copy name and Copy path. Picks are learned like apps.
- `f` with nothing after it shows a hint row with what the index
  understands. When the index can't be used, one row says why:
  - Everything isn't running: Enter opens voidtools.com.
  - Everything is still indexing.
  - plocate isn't installed: Enter opens plocate's site.
  - The locate database isn't built (`sudo updatedb`).

## Folder browsing (typed paths)

A path is text starting with `/`, `~/` (or just `~`), a drive (`C:\`, `C:/`)
or `\\server\`. Drive and UNC paths take both separators; others only `/`.
It's part of "Open URLs and browse folders" (`providers.open`).

- The folder is listed once (on a background thread, up to 5,000 entries).
  Typing after the last separator filters that listing with the usual
  matcher, without listing again. Each show of the window lists again.
- Nothing typed after the separator: an "Open <folder>" row first, then
  folders, then files, by name. Hidden entries (dotfiles, and on Windows the
  hidden and system attributes) show only when the typed text starts their
  name (`.c` → `.cache`, `App` → `AppData`).
- Enter on a folder browses into it: the query becomes its path plus the
  separator that was typed (`~/dev/` + `redsnap` → `~/dev/redsnap/`).
  Ctrl/Cmd+Enter opens the folder in the OS. Enter on a file opens it.
- Before the listing arrives, or when the folder can't be read, one row
  opens the typed path, and the OS reports what's wrong.

## Code

- `redsnap_core::files`: what a query asks for (`Files::request`), the latest
  answer (`Files::set_results`), and the rows (`FilesProvider`). No IO.
- `redsnap_platform::files`: the background thread (`FileSearch`, newest
  request wins), folder listing, and per OS `BACKEND`, `SYNTAX` and `search`.
- The launcher sends each new request once after a search. The answer comes
  back as `Event::Files` and searches again.
- `cargo run -p redsnap-platform --example find_files -- <words>` (or
  `-- --list <folder>`) asks the index from the command line.
