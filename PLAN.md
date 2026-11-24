# RedSnap project plan

RedSnap is a keyboard launcher for Windows, macOS and Linux. Press a global
shortcut, type, press Enter. The search window is native (Rust + gpui) because it
must respond on every keystroke without a visible delay. The settings window is
gpui too, built from the components of the gpui fork RedSnap uses (§0).

This document has these parts:

0. Decisions so far
1. Goals and non-goals
2. Phase plan
3. Phase-1 feature list
4. Phase-1 code architecture
5. Implementation details that matter (the rules that keep it fast and correct)
6. Build, packaging and testing
7. Milestones for phase 1, with the risky spikes first
8. Open questions

---

## 0. Decisions so far

- **UI toolkit: gpui, from the fork at github.com/KevinWang15/gpui**, pinned
  by revision in `Cargo.toml`. RedSnap's changes to gpui are commits there
  (§6.1), so there is one fork to maintain. One codebase covers all three OSes, and
  gpui already handles text shaping, CJK font fallback, IME and per-monitor
  scaling. Writing a native window per OS (Win32 + Direct2D, AppKit, GTK/X11)
  would mean three text fields, lists and focus models.
- **The decision is confirmed by measurement first.** The M0 spike (§7) must show
  that gpui reaches the latency of a hand-written Win32/Direct2D launcher:
  hotkey → painted window ≤ 16.7 ms p95, keystroke → results ≤ 10 ms p95.
  - If gpui misses on one OS, only that OS's search window becomes native code.
  - The search engine, ranking and app index stay shared, because they are plain
    Rust behind the `Platform` trait.
- **Hosting and CI: GitHub**, with GitHub Actions on `ubuntu-latest`,
  `windows-latest` and `macos-latest`. Releases come from `vX.Y.Z` tags.
- **The settings window is gpui too, in RedSnap's own process**, built from
  the fork's components (`gpui::components`). It sends requests (`config.get`,
  `config.save` with its revision, shortcut suspend/test/resume) to the app
  loop over the app's event channel, and the app loop validates and applies
  every change (§4.5).

---

## 1. Goals and non-goals

Goals:

- **Instant.** Hotkey to fully painted window, and keystroke to updated results,
  both within one frame on a warm process (budgets in §5.10).
- **Cross-platform from day one.** Every OS call sits behind one `Platform`
  trait; the search engine and ranking are plain Rust and tested on Linux.
- **Small, explicit design.** One owner per piece of mutable state, typed actions,
  stable ids. Guard trust boundaries (IPC, files we parse); don't wrap every
  internal value in limits.
- **The app loop owns all state.** The settings window renders and edits; the
  app loop validates, applies and persists.

Non-goals for now (each one was expensive and gave little value before):

- Settings sync between machines (if wanted later: export/import a file, or point
  the config dir at a synced folder).
- Running things as administrator, elevation helpers.
- Signature/trust checks for plugins, package transaction journals.
- Per-field size limits on every internal structure, evidence files checked into
  the repo, OS-specific PowerShell smoke suites. One cross-platform test harness
  instead (§6.3).

---

## 2. Phase plan

| Phase | Theme | Contents |
|---|---|---|
| **1** | Core launcher | Search window, app search + launch on all three OSes, calculator, web searches and quick links, open URL/path, RedSnap and system commands, action panel, learned ranking, tray, hotkey, start at login, single instance, the settings window. Then clipboard history (native view, it needs the same instant search) with paste into the previous app ([docs/clipboard.md](docs/clipboard.md)). |
| 2 | Snippets and files | Snippets with text expansion ([docs/snippets.md](docs/snippets.md)), file search (Everything on Windows, Spotlight on macOS, plocate on Linux) and folder browsing ([docs/files.md](docs/files.md)). |
| 3 | Extensions | Plugin protocol (JSON-RPC over stdio, long-lived process option), window switcher, per-plugin hotkeys, plugin management pages in the settings window. |

Phase 1 is the subject of the rest of this document. Phases 2 and 3 appear only
where phase 1 must leave room for them.

---

## 3. Phase-1 feature list

### 3.1 Search window (gpui)

- Opens on the global shortcut. Default: **Alt+Space** on Windows and Linux,
  **Option+Space** on macOS. Configurable.
- The shortcut toggles: pressing it while the window is shown hides it.
- Opens on the monitor under the mouse cursor, centered horizontally, about a
  quarter of the way down the work area.
- Look: a dark panel with a red accent, specified in
  [docs/design.md](docs/design.md).
- Fixed size (760 × 474): query field, 5 result rows and a footer with the result
  count and key hints. More results scroll. With an empty query, the rows show the
  most used items, so the window never needs to resize.
- Hides when it loses focus, on Esc, after running an action, and when the
  session locks.
- Query field with full caret and selection editing, mouse included, clipboard
  shortcuts, and IME composition (Chinese/Japanese input must work).
- Result rows: icon, title, subtitle, a right-side hint (shortcut or type label).
  Up/Down, Page Up/Down, Ctrl+N/P, mouse hover and click, Cmd/Ctrl+1…9 to run row N.
- **Enter** runs the primary action. **Cmd/Ctrl+Enter** runs the secondary action
  (for apps: show in Finder/Explorer/file manager). **Tab** or **Cmd/Ctrl+K** opens
  the action panel for the selected row; the actions are computed only when it opens.
- Phase 1 ships the dark design only; a light variant comes later.

### 3.2 Providers

| Provider | Trigger | Primary action |
|---|---|---|
| Applications | any text | launch |
| Calculator | input that parses as an expression (`2*(3+4)`, `10%3`, `2^10`, `sqrt(2)`) | copy result |
| Web searches | keyword + text (`g rust gpui`), and a fallback row "Search the web for …" when nothing else matches | open in default browser |
| Quick links | keyword, optionally with `{query}` (`gh redsnap`) | open URL |
| Open URL / path | text that is a URL, `~/…`, `/…`, `C:\…` | open with default handler |
| RedSnap commands | `settings`, `quit`, `reload apps`, `about` (normal fuzzy match, no prefix) | run |
| System commands | `lock`, `sleep`, `log out`, `restart`, `shut down`, `empty trash` (per OS where supported) | run; restart, shut down and log out ask for confirmation in the window |

App search matches the display name, the localized name, the executable or bundle
name, user aliases, and **pinyin** (full and initials) for names with Chinese
characters: `wx` and `weixin` both find 微信.

### 3.3 Learned ranking

- Picking a result for a query teaches RedSnap: the next time the same query
  (or a prefix of it) is typed, that result moves up.
- Global frequency and recency boost items in the empty-query list.
- Settings has "Reset learned ranking".

### 3.4 Native shell

- Tray / menu bar icon: Open RedSnap, Settings…, Start at login (check item), Quit.
  macOS: menu-bar only, no Dock icon. Windows: left click opens the search window.
- Start at login per OS (§5.12).
- Single instance. Running `redsnap` again shows the window; `redsnap toggle`,
  `redsnap show`, `redsnap settings` and `redsnap quit` act on the running
  instance. On Wayland, where apps cannot grab global keys, the user binds
  `redsnap toggle` in their desktop's shortcut settings.
- If the configured shortcut can't be registered at startup, RedSnap stays in the
  tray and opens Settings on the Shortcut field with the error shown.
- Crash details appended to `crash.log`; a rotating `redsnap.log`.

### 3.5 Settings (a gpui window)

- **General**: shortcut recorder, start at login, theme, number of rows.
- **Search**: enable or disable each provider; web searches and quick links
  (add/edit/reorder, keyword, URL template); app aliases; hidden apps; extra app
  folders.
- **Advanced**: reset learned ranking, reload apps, open log folder, index stats
  (app count, last scan, scan time), version.
- Opened from the tray, from the `settings` command, or by `redsnap settings`.
  Changes apply immediately without a restart.

### 3.6 Clipboard history (after the core launcher)

Designed in [docs/clipboard.md](docs/clipboard.md). The two rules that matter:

- Every copy is accounted for: in the history, or listed as skipped with the
  reason.
- Search shows only entries that contain every typed word; newest first; matches
  highlighted.

---

## 4. Phase-1 code architecture

### 4.1 Repository layout

```
Cargo.toml                 workspace (resolver 3, edition 2024); gpui from the fork (§6.1)
crates/
  core/        redsnap-core: no OS or UI code, tested anywhere
    query.rs       parse input → Query { text, prepared matcher input, keyword route }
    matcher.rs     nucleo-matcher, match classes, names with pinyin spellings
    text.rs        folding, acronyms, pinyin forms
    search.rs      Provider trait, Engine, ranking
    item.rs        Item, ItemId, IconRef, typed Action
    providers/     apps.rs (over an AppCatalog); mod.rs: calculator, open URL,
                   commands, snippets, web searches, quick links
    files.rs       file search and folder browsing: requests and rows (no IO)
    calc.rs        expression parser/evaluator
    usage.rs       usage store (SQLite, single owner thread, snapshot after commit)
    clipboard.rs   clipboard store (docs/clipboard.md)
    snippets.rs    snippet templates and trigger matching (docs/snippets.md)
    config.rs      Config schema, validation, defaults, atomic save/load
  platform/    redsnap-platform: every OS call, per OS behind the same functions
    window.rs      show/hide/place/cloak/paint the pre-created window, the look
    apps/          app discovery and icons (windows.rs, macos.rs, linux.rs)
    clipboard/     capture, write, paste into the previous app (Windows)
    files/         the files thread, folder listing, per-OS index (Everything,
                   Spotlight, plocate)
    snippets/      expansion as you type, inserting (Windows)
    autostart.rs, instance.rs (single instance + CLI endpoint), open.rs,
    paths.rs, tray.rs, icon.rs (the RedSnap mark and app icon, drawn in code)
  app/         redsnap (binary): the gpui frontend and process owner
    main.rs        startup, logging, CLI subcommands
    app.rs         the event loop and its State: config, catalog, usage, hotkeys
    launcher/      the search window: view.rs, query_field.rs (IME), clip.rs
                   (clipboard view)
    settings/      the settings window: view.rs, pages.rs, widgets.rs
    hotkeys.rs, icons.rs (icon loading and disk cache), execute.rs (runs
    actions off the UI thread), perf.rs (latency log), assets.rs (Lucide SVGs)
packaging/icons            the app icon (.ico/.png, made by examples/make_icons.rs)
docs/                      design.md, clipboard.md, snippets.md, files.md,
                           macos-linux-catch-up.md
```

`core` is where most logic and tests live. `platform` is thin and only translates
OS calls. `app` binds them to gpui and owns the process.

### 4.2 Threads and owners

| Owner | Thread | Owns |
|---|---|---|
| gpui main thread | UI | window, query field, `SearchSession`, rendering, config in memory |
| Catalog builder | background thread | scanning and watching app folders; publishes an immutable `Arc<AppCatalog>` |
| Icon loader | background pool (2 threads) | extracting icons, the disk icon cache |
| Usage store | dedicated thread | the SQLite connection; in-memory boost map published after each commit |
| Executor | background | launching apps, opening URLs, system commands |
| Instance endpoint | background | the socket/pipe for `redsnap toggle` and the like; forwards commands through a channel |

Messages between threads use typed channels (`futures::channel::mpsc` into a
`cx.spawn` loop on the UI side). No window messages, no shared mutable state
besides `Arc` snapshots swapped atomically (`arc-swap`).

### 4.3 Search pipeline

```
keystroke
  → Query::parse(text)            once per keystroke: lowercase, case-fold,
                                  keyword route, nucleo pattern
  → session.begin(generation+1)
  → sync providers on snapshots   apps, commands, links, web, open, calc
                                  (all in memory, no IO; total budget 3 ms)
  → rank → top-K (K = rows + one page of lookahead)
  → render                        icons and subtitles only for visible rows
  [phase 2/3] async providers stream Batch{generation, items} → merge if the
  generation is current, otherwise drop
```

- **Provider trait**

  ```rust
  trait Provider: Send + Sync {
      fn id(&self) -> ProviderId;                       // enum, not a string
      fn search(&self, q: &Query, cx: &SearchContext, out: &mut Vec<Candidate>);
      fn actions(&self, item: &Item) -> Vec<Action>;    // lazy: action panel only
  }
  ```

  Phase 1 needs only sync providers. Phase 2 adds `AsyncProvider`, which receives
  the query plus a cancellation token and sends batches tagged with the
  generation. It is designed now so the session merge already handles it.

- **Keyword routing** is a table built from config (`g` → Google, `gh` → GitHub…)
  plus built-in routes, not a chain of `if`s. A keyword route narrows the
  providers that run; plain text runs all of them.

- **Selection is kept by `ItemId`** across result updates. Only if that item is
  gone does the selection fall back to the first row.

### 4.4 Item model

```rust
struct Item {
    id: ItemId,              // stable string: "app:<platform key>", "cmd:lock",
                             // "web:google", "link:<config id>", "calc"
    title: SharedString,
    subtitle: SharedString,
    icon: IconRef,           // App(key) | Builtin(name) | Url favicon later
    primary: Action,
    secondary: Option<Action>,
    learnable: bool,         // calculator results don't teach ranking
}

enum ActionKind {
    Launch(LaunchTarget),    // Windows AUMID / path, macOS bundle URL, Linux desktop id
    OpenUrl(Url),
    OpenPath(PathBuf),
    RevealPath(PathBuf),
    CopyText(String),
    System(SystemCommand),
    RedSnap(InternalCommand),  // settings(page), quit, reload apps
}
```

All actions are typed. URLs are restricted to http(s) and a short list of OS
settings schemes at the point where they are executed. Nothing builds a shell
command line from strings.

### 4.5 Settings window ↔ app loop

- The settings window (`crates/app/src/settings`) is a gpui window in the same
  process. It sends requests to the app loop as `Event::Settings(method,
  params, reply)` on the app's event channel; the loop answers through the
  reply.
- **Methods**:
  - `config.get` → `{config, revision}`
  - `config.save {config, revision}` → `{revision}` or a stale/validation error
  - `hotkey.suspend` / `hotkey.resume` (while the shortcut recorder listens)
  - `hotkey.test {shortcut}` → available or not
  - `usage.reset`, `apps.reload`, `apps.stats`, `clipboard.stats`,
    `clipboard.clear`, `logs.open`, `data.open`, `app.info`
- **Events** pushed to the open window: `config.changed`, `apps.stats`.
- **Authority**: the app loop owns `config.json`.
  1. The window sends a full config with the revision it started from.
  2. The loop rejects a stale revision, then validates and normalizes.
  3. It applies side effects that can fail (hotkey registration, start at
     login) and rolls back on failure.
  4. Only then does it write the file and push `config.changed`.
- **Lifecycle**: the window opens on demand (tray, the RedSnap Settings
  command, `redsnap settings`) and is brought forward if already open. When it
  closes, suspended shortcuts are resumed.

### 4.6 Config and data locations

| File | Location | Owner |
|---|---|---|
| `config.json` | `dirs::config_dir()/redsnap/` | the app loop (the settings window edits through requests) |
| `usage.db` (SQLite) | `dirs::data_local_dir()/redsnap/` | usage thread |
| `icons/` (PNG cache) | `dirs::cache_dir()/redsnap/icons/` | icon loader |
| `redsnap.log`, `crash.log` | `dirs::data_local_dir()/redsnap/logs/` | native |

`config.json` has a `"version": 1` field. Phase 1 reads only the current version;
there are no users yet, so no migration code until a release needs it.

---

## 5. Implementation details that matter

These are the rules most likely to be broken by accident. Each one is a known
failure mode of launchers built on this kind of stack.

### 5.1 The window is created once and never destroyed

Creating a gpui window costs 65–115 ms (most of it window creation and the first
frame). That is far too slow for every hotkey press. So:

- Create the launcher window at startup, hidden, and keep it for the life of the
  process. It also keeps gpui from quitting when no window is open (on Windows,
  gpui exits after its last window closes).
- Show and hide it with platform calls on the raw window handle:
  - Windows: `ShowWindow`, plus `SetWindowPos(HWND_TOPMOST)` re-asserted on every
    show.
  - macOS: `orderOut:` / `makeKeyAndOrderFront:`.
  - X11: map/unmap.
- **Show sequence**:
  1. The hotkey handler only queues a "show" event.
  2. Reset the query and compute the empty-state results synchronously.
  3. Place the window if the monitor changed (skip it otherwise).
  4. Show it hidden from the compositor, paint the first frame synchronously,
     reveal it, then take focus.
- **Reveal only a complete frame.** The first frame must already contain the new
  query state (empty field, most-used list).
  - Windows: show cloaked (`DWMWA_CLOAK`), paint, then uncloak.
  - macOS: alpha 0 → paint → alpha 1.
  - Otherwise the user sees the previous session's results for one frame.
- Don't free caches or trim memory when hiding. Pruning on hide caused visible
  stalls on the next show. Bound the caches instead (§5.6).
- Window style:
  - Windows: `WS_POPUP`, `WS_EX_TOOLWINDOW` (no taskbar button), dark mode
    attribute.
  - macOS: borderless, Accessory activation policy, `LSUIElement`.
- Panel (docs/design.md): on Windows the compositor's acrylic blurs what's
  behind, live (gpui `Blurred`), under a fill at 0.5, with Windows rounding the
  window at 8 px. On macOS and Linux the window is transparent outside the
  panel, and the panel has its own 20 px corners and a nearly opaque fill.
- **Spike first (M0)**: gpui 0.2.2 has no public hide/show for a window (hiding
  the raw handle with `ShowWindow(SW_HIDE/SW_SHOW)` from outside gpui is known to
  work on Windows). Confirm
  the platform calls work with gpui's own window state on all three OSes,
  including moving the window between monitors with different scale factors.

### 5.2 gpui re-entrancy rule

Any OS call that synchronously re-enters gpui's window procedure panics with
"RefCell already borrowed" if it runs inside a gpui update or event callback. On
Windows that includes `SetWindowPos`, `ShowWindow`, activation and modal dialogs.
All such calls go through one helper, `after_update(cx, f)`, which runs `f` in a
spawned task after the current update returns. Tray menus are shown the same way.

### 5.3 Focus

- **Windows**: the process that receives the hotkey may take the foreground, so
  `SetForegroundWindow` works directly from the hotkey path.
  - A second instance (`redsnap toggle`, or a shortcut bound outside RedSnap)
    first calls `AllowSetForegroundWindow(<primary pid>)` and then sends its IPC
    message. Without that, the primary can't take focus.
  - The settings window must not be owned by the launcher window, or Windows
    keeps one above the other.
- **macOS**: call `cx.activate(true)` before ordering the window front. After any
  style mask change call `makeFirstResponder` on the view, or keys produce a beep.
  - When hiding without launching anything, hide the app (`NSApp.hide`) so focus
    returns to the previous app.
- **Hide on blur** comes from gpui's window activation observer. One exception:
  activation moving to the confirmation step inside our own window.
- Record the previously focused window when showing. Phase 1 doesn't use it;
  phase 2 needs it to paste back into it.

### 5.4 Query field and IME

- Build the field from gpui's text input pattern: implement `EntityInputHandler`
  (marked text, `replace_and_mark_text_in_range`, `bounds_for_range`,
  `character_index_for_point`) and register it with `window.handle_input` in paint.
  - Without it, CJK input doesn't work and the IME candidate window appears in the
    wrong place.
- Search runs on committed text only. Marked (composing) text is shown underlined
  but doesn't trigger a search, so pinyin keystrokes don't flash results.
- Key bindings use `actions!` + `KeyBinding` with the `secondary-` modifier (Cmd on
  macOS, Ctrl elsewhere) in a `"Launcher"` key context.

### 5.5 Keystroke path is in-memory and bounded

- Parse and normalize the query **once** per keystroke (`PreparedQuery`). Earlier
  bugs: re-normalizing the query per candidate, recomputing sort keys inside
  comparators, and taking a lock plus hashing inside the sort comparator. Each one
  turned an O(n) pass into a slowdown you could see.
- Take the learned-ranking boosts as **one snapshot** per keystroke (an `Arc` map),
  never a lock per comparison.
- Rank, then use `select_nth_unstable` for top-K, then sort only the K. Build icons,
  subtitles and actions only for visible rows. Actions are built when the action
  panel opens, not for every candidate.
- Haystacks (lowercased name, pinyin, initials, aliases) are precomputed when the
  catalog is built, not per query.
- **No IO, no SQLite, no locks held across provider calls on the UI thread.**

### 5.6 Matching and ranking

- Matcher: `nucleo-matcher` (fzf-style optimal alignment, Unicode case folding).
  The greedy leftmost subsequence matcher it replaces missed word boundaries
  (`vsc` matched the first `s`, not `Studio`).
- Sort key, in order:
  1. **Literal before fuzzy**: a literal match (exact, prefix, word-prefix/acronym,
     substring) never loses to scattered fuzzy letters, whatever was learned.
     This was a real bug in long-text search.
  2. **Learned boost for this query** (§5.7): picking Chrome for "c" puts it
     above other literal matches of "c".
  3. **Match class**: exact > prefix > word-prefix/acronym > substring.
  4. Matcher score + a small global frequency/recency term.
  5. Provider priority (enum order), then `ItemId` for stable ties.
- Calculator and URL/path rows get a fixed slot when they apply. A typed expression
  shows the result first; it is never outranked by an app that fuzzily matches
  `2*3`.
- Exact-title comparison uses Unicode case folding, not ASCII-only.

### 5.7 Learned ranking

- Table `query_pick(query_prefix TEXT, item_id TEXT, count INT, last_used INT,
  PRIMARY KEY(query_prefix, item_id))`.
  - On pick, write every prefix of the typed query: `c`, `ch`, `chr` for "chr".
    Cap at 16 characters and at most 50,000 rows; prune the oldest in the same
    transaction.
- Table `item_use(item_id PRIMARY KEY, count INT, last_used INT)` for the
  empty-query list and the global term.
- `count` is capped (e.g. 20). `last_used = MAX(old, now)`, so a clock jump
  backwards can't reorder history (a real bug).
- **One owner thread** holds the connection. The in-memory snapshot is replaced
  only **after** the commit succeeds. Updating the cache first and then failing to
  commit led to bugs several times.
- `ItemId` is a stable string from the source (AUMID, bundle id, desktop id), never
  a database rowid. Rowids get reused and the old items were credited with picks
  they never got.

### 5.8 Calculator

A small hand-written Pratt parser over `f64`:
- operators `+ - * / % ^` and parentheses;
- functions `sqrt abs round floor ceil sin cos tan log ln`;
- constants `pi`, `e`.

Division is always floating point. Integer division was a real bug (`1/2 = 0`)
with a general expression crate. Results are formatted without float noise: round
to 12 significant digits and trim zeros. Inputs that are only a number, or that
also match an app name better, don't produce a calculator row.

### 5.9 App discovery, launch and icons

Shared rules:
- **Bounded scans**: a depth limit per root and a total entry cap (e.g. 20,000
  files visited per root). Hitting the cap skips that root with a log line; it
  doesn't drop everything.
- **Dedupe by identity**, not by display name: AUMID / resolved target path /
  bundle id / desktop file id. Two different apps can share a name.
- Watch roots with the `notify` crate, debounce 500 ms, then rescan only the
  changed root and publish a new immutable catalog with `arc-swap`.
- Rescan everything on demand ("reload apps"), and when the window is shown if the
  last full scan is older than 10 minutes. The rescan runs in the background and
  the current catalog stays in use meanwhile.

Windows:
- Enumerate `shell:AppsFolder` (`SHGetKnownFolderItem(FOLDERID_AppsFolder)` +
  `IEnumShellItems`). It lists both Start-menu shortcuts and packaged apps under
  one id (`SIGDN_PARENTRELATIVEPARSING` gives the AUMID or path) and the localized
  name (`SIGDN_NORMALDISPLAY`).
- Add Desktop shortcuts and optional user folders.
- Watch the Start-menu folders. AppsFolder itself can't be watched, which is why
  there's a staleness rescan.
- Launch with `ShellExecuteExW("shell:AppsFolder\<AUMID>")` or the path.
- Icons: `SHCreateItemFromParsingName` → `IShellItemImageFactory::GetImage(64 px
  × scale)`. Convert premultiplied BGRA to straight RGBA and reject fully
  transparent results, then PNG. COM runs on the icon threads, never on the UI
  thread.

macOS:
- Scan `/Applications`, `/System/Applications` (+ `Utilities`), `~/Applications`,
  `/System/Library/CoreServices/Applications`. Don't descend into `.app` bundles
  except `/Applications/*/` folders one level deep.
- Name: `NSFileManager displayNameAtPath` (localized, e.g. Chinese names), plus
  `CFBundleName` and the file stem as extra haystacks.
- Launch: `NSWorkspace openApplicationAtURL`. Icons: `NSWorkspace iconForFile:`
  rendered to PNG at the target size.

Linux:
- Parse `.desktop` files from `$XDG_DATA_HOME/applications` and every
  `$XDG_DATA_DIRS/*/applications`, so Flatpak and Snap exports are included. The
  first file with a given desktop id wins.
- Honor `NoDisplay`, `Hidden`, `OnlyShowIn`/`NotShowIn` (against
  `$XDG_CURRENT_DESKTOP`) and `TryExec`. Read `Name[<locale>]`, `GenericName` and
  `Keywords` as haystacks.
- Launch: strip `Exec` field codes, split per the spec's quoting rules, spawn
  detached (`setsid`) with the working directory from `Path`. Honor `Terminal=true`
  with the user's configured terminal.
- Icons: theme lookup (`freedesktop-icons` crate, current theme then `hicolor`);
  SVGs rasterized with `resvg`.

Icon cache:
- The disk cache is keyed by `ItemId` + source mtime + pixel size.
- The memory LRU holds 256 decoded images.
- Only visible rows request icons. A finished icon updates its row; it doesn't
  re-run the search. Re-running the whole search for each icon was wasted work.

### 5.10 Performance budgets and measurement

These numbers are what "feels instant" needs: the window appears within one
display frame of the key press, and results change within the frame that shows the
keystroke. The risk is gpui's frame cost, which M0 measures against them.

| Measure | Budget (p95, warm) |
|---|---|
| Process start → hidden window ready, hotkey registered | 150 ms |
| Hotkey → first complete frame visible | 16.7 ms (one 60 Hz frame) |
| Keystroke → results frame | 10 ms (sync providers ≤ 3 ms) |
| Matching 10,000 catalog items | 2 ms |
| Idle CPU | 0 (no polling timers while hidden) |
| Hidden private memory | 60 MB (gpui baseline to be measured in M0, then tightened) |

- With `RUST_LOG=redsnap=debug` (or a `--perf` flag), the app writes timeline
  events (`hotkey`, `show`, `first_frame`, `query_start`, `query_end`, `frame`) to
  `perf.jsonl`.
- A harness script drives the window (§6.3) and reports p50/p95. Criterion benches
  in `core` cover the matcher and the whole sync pipeline over a fixture catalog of
  10,000 apps with mixed Latin/CJK names.

### 5.11 Global shortcut

- `global-hotkey` crate: `RegisterHotKey` on Windows, Carbon `RegisterEventHotKey`
  on macOS, `XGrabKey` on X11. Events go into a channel drained by a `cx.spawn`
  loop.
- **Changing the shortcut**: register the new one first; if that fails, keep the
  old one and return the error to Settings. The file is written only after
  registration succeeds.
- **While the recorder field in Settings is focused**, native suspends its hotkeys
  (`hotkey.suspend`). That way pressing the current shortcut records it instead of
  toggling the launcher.
  - The recorder reads gpui key events in the settings window. On Windows,
    Alt+Space in a framed window may open the system menu; check it, and if it
    does, register the chord temporarily to capture it.
- **Wayland**: no global grabs. Use the `redsnap toggle` CLI bound in the
  desktop's shortcut settings. The XDG GlobalShortcuts portal can come later where
  the desktop supports it.
- **X11 caveat**: the hotkey crate can miss a press made within about a second of
  the window closing. Test it, and if it shows up, re-grab on hide.

### 5.12 Tray, start at login, single instance, session

- Tray:
  - Windows uses `tray-icon`; macOS uses a native `NSStatusItem` + `NSMenu` with a 2× Retina template image.
  - Linux uses `ksni` (StatusNotifierItem over D-Bus), which needs no GTK main
    loop next to gpui's.
  - Re-add the Windows tray icon on the `TaskbarCreated` message (Explorer
    restarts).
- Start at login:
  - Windows: HKCU `…\CurrentVersion\Run` value.
  - macOS: `~/Library/LaunchAgents/<bundle id>.plist` (SMAppService later, once
    the app is signed).
  - Linux: `~/.config/autostart/redsnap.desktop`.
  - On every start, re-point the entry if the executable has moved.
- Single instance: if the IPC endpoint already answers, send the CLI command and
  exit. Otherwise create it. Remove a stale Unix socket file if connecting to it
  fails.
- Session lock / sleep: hide the window and clear the query. Windows:
  `WTSRegisterSessionNotification`. macOS: `com.apple.screenIsLocked`
  distributed notification. Linux: logind `Lock` signal over D-Bus. Don't reopen on
  unlock.

### 5.13 Config persistence

- Write to a temp file in the same directory, flush (`sync_all`), then rename over
  the target.
  - On Windows, antivirus and indexers cause sharing violations, so retry the
    rename with backoff for up to 5 s.
  - Keep `config.json.bak` (the last good file).
- Load: if `config.json` is malformed, rename it to `config.invalid-<timestamp>.json`,
  load `.bak` or defaults, and show the problem in Settings → Advanced and as a
  one-time row in the launcher. Never overwrite the user's broken file silently.
- Strict decoding: `deny_unknown_fields` everywhere. JSON with duplicate keys is
  rejected (serde normally takes the last one silently).

### 5.14 Settings window details

- `crates/app/src/settings/`: `mod.rs` opens or focuses the window and sends
  requests to the app loop (`Event::Settings`, answered with a `Reply`);
  `view.rs` is the window (painted title bar, page list, notice); `pages.rs`
  the pages; `widgets.rs` the shortcut format.
- Text fields are bound to the config: each knows how to read its value and
  write it back. They save on Enter or when they lose focus, and a change
  made elsewhere updates every field that isn't being edited.
- The title bar is painted. Its areas are gpui window-control areas, so on
  Windows the OS handles dragging, snapping, minimize, maximize and close;
  elsewhere clicks do it. macOS keeps its traffic lights.
- The shortcut recorder suspends RedSnap's shortcuts while it listens, tests
  the new one (registered for real) before saving it, and resumes them when
  it stops or the window closes.

### 5.15 Shutdown and test switches

- Quit: stop accepting IPC, unregister hotkeys, close the settings window, and give
  each background owner a bounded join (500 ms). A worker blocked in a scan must
  not hang quit; scans check a stop flag between directory entries.
- Test-only environment switches (demo catalog, fake platform, fixed clock) exist
  only under the `demo` cargo feature, which release builds don't enable. A test
  switch that leaked into normal builds was a real bug.

---

## 6. Build, packaging and testing

### 6.1 gpui fork

RedSnap uses the gpui fork at github.com/KevinWang15/gpui (`gpui` and
`gpui_platform`, with the `x11` and `wayland` features), pinned by revision.
RedSnap's own changes are commits in that fork:

1. Windows `PopUp` windows use `WS_POPUP` and skip the non-client insets in
   `WM_NCCALCSIZE`. Style 0 is `WS_OVERLAPPED`, which Windows frames, so the
   content drew short of the window.
2. Report a mouse move on `WM_MOUSELEAVE` so hover states clear.
3. A hidden window gets its bounds at creation, so a pre-created window the app
   placed itself doesn't jump back on its first `activate()`.
4. `CursorStyle::ClosedHand` shows Windows' move cursor.
5. `PRESENT_HOOK`: called after each presented frame, for latency measurement.
6. `gpui_windows`' build script follows the target, not the host, and release
   builds use precompiled HLSL shaders committed in `src/shaders_bytes.rs`,
   guarded by a hash of the `.hlsl` sources, so a Linux cross build needs no
   `fxc.exe`. After editing a `.hlsl` file, regenerate them from WSL with
   the fork's `scripts/regen-windows-shaders.sh`.


### 6.2 Per-OS builds

| OS | Build | Package (phase 1) |
|---|---|---|
| Linux | native `cargo build`; needs `libxkbcommon-x11-dev`, `libwayland-dev`, `libvulkan-dev`, `libfontconfig-dev`, `libdbus-1-dev` | `tar.gz` with binary, `.desktop` file and icons; AppImage later |
| Windows | native MSVC on the GitHub `windows-latest` runner (fxc from the Windows SDK) for releases; cross-build from Linux with `x86_64-pc-windows-gnu` + mingw-w64 and precompiled shaders for local iteration | zip (per-user copy to `%LOCALAPPDATA%\Programs\RedSnap`); installer in phase 2 |
| macOS | GitHub `macos-latest` runner (Metal shaders need Xcode); universal binary via `lipo` of arm64 + x86_64 | `RedSnap.app` (`LSUIElement`, icon), ad-hoc signed or with a Developer ID from CI secrets, zipped |

- The exe icon is embedded with `embed-resource`, generated from
  `packaging/icon.svg`.
- `windows_subsystem = "windows"` in release builds, so no console window.

### 6.3 Testing

- **core**: unit tests for query parsing, matcher classes, pinyin haystacks,
  ranking order, the calculator, config validation and the usage store (temp
  database). Criterion benches (§5.10).
- **platform**: tests against fixture trees: sample `.desktop` files (locale names,
  `NoDisplay`, `OnlyShowIn`, field codes) and sample `.app` bundles with
  `Info.plist`. The Windows enumeration code is tested on Windows only.
- **Demo mode** (feature `demo`):
  - `REDSNAP_DEMO_CATALOG=<json>` replaces app discovery. `REDSNAP_DEMO_LAYOUT=<dir>`
    makes the window write its control bounds as JSON.
  - A Linux harness runs the app on a private Xvfb display, types with `xdotool`,
    and captures only RedSnap's window. Uses: screenshots for UI review,
    latency runs, and regression checks (type `wx` → 微信 is row 1).
  - It never captures or sends input to the user's own desktop.
- **Settings window**: the same Xvfb harness opens it (`redsnap settings`),
  clicks through the pages and checks what `config.json` ends up with.
- CI is GitHub Actions in the GitHub repo (`ubuntu-latest`, `windows-latest`,
  `macos-latest`). It runs `cargo test --workspace`, clippy and fmt on all three
  OSes and builds the packages. A `vX.Y.Z` tag builds the
  three packages and attaches them to a GitHub Release.

---

## 7. Phase-1 milestones

Risky spikes come first, so a dead end is found before the bulk of the code exists.
A spike is a short experiment with throwaway code that answers one question.

**M0 result (2026-11-19): passed.** The spike code has been removed; what it
proved is in the app. On Windows 11 (Intel iGPU):

| | p50 | p95 |
|---|---|---|
| hotkey → frame presented | 1.4 ms | 5.8 ms |
| keystroke → results frame, painting right after input | 0.8 ms | 3.6 ms |
| keystroke → results frame, stock gpui (next vsync tick) | 6.3 ms | 9.8 ms |
| gpui layout + paint | 0.5 ms | 1.0 ms |

Findings that shaped the app: stock gpui paints only on its vsync tick, so the
app paints right after input; Windows sends no `WM_PAINT` to cloaked or
off-screen windows; gpui's Windows `activate()` replays creation bounds and
injects a fake Alt keypress (the app takes the foreground itself); the matte
glass's screen capture cost 9–13 ms on Windows (the matte glass was removed
later). Chinese IME input worked.
**macOS result (2026-11-25): passed.** Measured in release build on Apple Silicon (macOS 27.2, 20 warm show/hide cycles, `paint_now` via `CALayerDelegate::displayLayer:`, native blur enabled):

| | p50 | p95 |
|---|---|---|
| show → frame presented (`paint_now`) | 0.92 ms | 1.30 ms |
| show results | 0.18 ms | 0.26 ms |
| gpui layout + paint (`frame_draw`) | 0.26 ms | 0.38 ms |
| gpui layout + paint + present | 0.38 ms | 0.58 ms |

App scan (191 `.app` bundles): 50 ms. Cold first show: 11.9 ms.

**M0 latency spike, as planned:**

- A minimal app: vendored gpui 0.2.2 with patches 1–5 (§6.1), one pre-created
  hidden search window drawn to docs/design.md, a global
  hotkey, a query field, and 5 result rows with icons filtered from a fixture list
  of 2,000 names.
- Timeline logging to `perf.jsonl`: hotkey received, show requested, first frame
  presented, window visible; keystroke received, results computed, frame
  presented.
- A driver that sends 30 hotkey activations and 200 keystrokes and reports
  p50/p95/max. The driver sends input only to the spike's own window.
- Run on the real Windows host first (cross-built from WSL), then Linux/X11 here,
  then macOS on a GitHub runner or a Mac.
- Compare two paint modes: stock gpui, which paints on the next refresh tick of
  its vsync thread (up to 16.7 ms after input), and paint right after input
  (`RedrawWindow(RDW_UPDATENOW)` from a spawned task).
- Pass: hotkey → painted window ≤ 16.7 ms p95 and keystroke → results frame
  ≤ 10 ms p95 on Windows.

| # | Milestone | Done when |
|---|---|---|
| M0 | **Spikes** (throwaway code; the latency spike decides §0) | (a) Pre-created gpui window hidden/shown by hotkey on Windows, macOS, X11; hotkey → frame measured; no RefCell panics; correct across two monitors with different scales. (b) IME field: Chinese input on Windows and macOS, candidate window at the caret. (c) Windows cross-build from Linux with vendored gpui (macOS intact) runs on a Windows host. (d) Wayland session: window shows via `redsnap toggle`. |
| M1 | **Core engine** | Query, matcher + pinyin, ranking, usage store, calculator, phase-1 providers over a fixture catalog; benches meet §5.10 on Linux. |
| M2 | **Launcher UI on Linux** | Window, query field, results, action panel, theme, empty state, tray (ksni), hotkey, single instance + CLI, start at login, demo harness running. |
| M3 | **Platform backends** | App discovery, icons and launch on Windows, macOS and Linux; system commands; session lock hide. |
| M4 | **Settings window** | All pages, config ownership with revisions, shortcut recorder with suspend, all settings applied live. |
| M5 | **Clipboard history** | docs/clipboard.md on Windows, macOS and X11: capture with every copy accounted for, literal search, preview, paste back. |
| M6 | **Packaging + CI + dogfood** | Packages for three OSes from CI; budgets met on a real Windows and Mac machine; a week of daily use without a crash. |

Rough order of work after M0: M1 and M2 in parallel on Linux; M3 per OS, Windows
then macOS; M4; M5; M6.

---

## 8. Open questions

1. Decided (§0): hosted on GitHub, built with GitHub Actions.
2. Decided (2026-11-19): clipboard history is in phase 1, built after the core
   launcher (M5).
3. **Wayland priority.** Phase 1 covers it through `redsnap toggle`. A native
   portal shortcut would come later.
4. **Default shortcut.** Alt+Space / Option+Space. These conflict with the Windows
   window menu in some apps and with input-method switching on some Linux setups.
