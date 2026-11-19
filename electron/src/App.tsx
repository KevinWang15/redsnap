// RedSnap Settings. RedSnap owns the settings: every edit is sent to it,
// applied there (shortcuts are registered before anything is written), and
// what comes back is what's shown.

import { useCallback, useEffect, useMemo, useState } from 'react'
import { type AppInfo, type AppStats, type ClipStats, type Config, type QuickLink, type Snapshot, type Snippet, type WebSearch, api, message } from './api'
import { Button, ConfirmButton, NumberField, Row, Section, ShortcutRecorder, StringList, TextArea, TextField, Toggle } from './ui'

type Page = 'general' | 'search' | 'apps' | 'snippets' | 'clipboard' | 'advanced'

const PAGES: { id: Page; label: string }[] = [
  { id: 'general', label: 'General' },
  { id: 'search', label: 'Search' },
  { id: 'apps', label: 'Apps' },
  { id: 'snippets', label: 'Snippets' },
  { id: 'clipboard', label: 'Clipboard' },
  { id: 'advanced', label: 'Advanced' },
]

type Notice = { kind: 'error' | 'ok'; text: string }

export default function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null)
  const [page, setPage] = useState<Page>('general')
  const [notice, setNotice] = useState<Notice | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)

  useEffect(() => {
    api.getConfig().then(setSnap, (e) => setLoadError(message(e)))
    return api.onEvent((name, params) => {
      if (name === 'config.changed') setSnap(params as Snapshot)
    })
  }, [])

  useEffect(() => {
    if (notice?.kind !== 'ok') return
    const t = setTimeout(() => setNotice(null), 2500)
    return () => clearTimeout(t)
  }, [notice])

  /** Sends a changed config; RedSnap answers with the new revision or why not. */
  const save = useCallback(
    async (change: (c: Config) => Config) => {
      if (!snap) return
      const next = change(structuredClone(snap.config))
      try {
        const { revision } = await api.saveConfig(next, snap.revision)
        setSnap({ config: next, revision })
        setNotice({ kind: 'ok', text: 'Saved' })
      } catch (e) {
        const text = message(e)
        setNotice({ kind: 'error', text })
        // Show what RedSnap has now (the edit was refused).
        api.getConfig().then(setSnap, () => {})
        throw e
      }
    },
    [snap],
  )
  // For controls that don't need the error: it's shown in the banner.
  const set = useCallback((change: (c: Config) => Config) => void save(change).catch(() => {}), [save])

  if (loadError) return <Centered>Couldn't load the settings: {loadError}</Centered>
  if (!snap) return <Centered>Loading…</Centered>
  const c = snap.config

  return (
    <div className="flex h-full">
      <nav className="flex w-52 shrink-0 flex-col border-r border-white/5 bg-ink-900/60 px-3 py-5">
        <div className="mb-6 flex items-center gap-2.5 px-2">
          <Logo />
          <span className="text-[15px] font-semibold">RedSnap</span>
        </div>
        {PAGES.map((p) => (
          <button
            key={p.id}
            onClick={() => setPage(p.id)}
            className={`mb-0.5 h-9 rounded-lg px-3 text-left text-[14px] transition-colors ${
              page === p.id ? 'bg-red-500/15 font-medium text-ink-100' : 'text-ink-300 hover:bg-white/5 hover:text-ink-100'
            }`}
          >
            {p.label}
          </button>
        ))}
      </nav>
      <main className="relative min-w-0 flex-1 overflow-y-auto px-8 py-7">
        {notice && (
          // Floating, so showing it never moves the page.
          <div
            role="status"
            className={`fixed bottom-5 right-5 z-10 flex max-w-md items-start gap-3 rounded-lg border px-3 py-2 text-[13px] shadow-lg shadow-black/40 ${
              notice.kind === 'error' ? 'border-red-500/40 bg-[#2a1219] text-ink-100' : 'border-white/10 bg-ink-800 text-ink-300'
            }`}
          >
            <span className="min-w-0 flex-1">{notice.text}</span>
            {notice.kind === 'error' && (
              <button className="text-ink-400 hover:text-ink-100" onClick={() => setNotice(null)}>
                ×
              </button>
            )}
          </div>
        )}
        <div className="mx-auto max-w-3xl">
          {page === 'general' && <General c={c} set={set} save={save} />}
          {page === 'search' && <Search c={c} set={set} />}
          {page === 'apps' && <Apps c={c} set={set} />}
          {page === 'snippets' && <SnippetsPage c={c} set={set} />}
          {page === 'clipboard' && <ClipboardPage c={c} set={set} save={save} notify={setNotice} />}
          {page === 'advanced' && <Advanced c={c} set={set} notify={setNotice} />}
        </div>
      </main>
    </div>
  )
}

type Edit = { c: Config; set: (change: (c: Config) => Config) => void }
type Save = (change: (c: Config) => Config) => Promise<void>

function Centered({ children }: { children: React.ReactNode }) {
  return <div className="flex h-full items-center justify-center text-[14px] text-ink-400">{children}</div>
}

function Logo() {
  return (
    <span className="relative flex h-7 w-7 items-center justify-center rounded-lg border border-white/10 bg-white/5">
      <span className="h-2.5 w-2.5 rounded-[3px] bg-red-500" />
    </span>
  )
}

const restart = <span className="text-ink-400"> · applies after RedSnap restarts</span>

function General({ c, set, save }: Edit & { save: Save }) {
  return (
    <>
      <Section title="Shortcuts" description="Click a shortcut, then press the keys you want.">
        <Row label="Open RedSnap" hint="Press it again to hide RedSnap.">
          <ShortcutRecorder value={c.hotkey} onChange={(spec) => save((x) => ({ ...x, hotkey: spec }))} />
        </Row>
        <Row label="Open clipboard history" hint="Lists everything you've copied.">
          <ShortcutRecorder
            value={c.clipboard.hotkey}
            onChange={(spec) => save((x) => ({ ...x, clipboard: { ...x.clipboard, hotkey: spec } }))}
          />
        </Row>
      </Section>
      <Section title="Startup">
        <Row label={startLabel()} hint="RedSnap starts in the background, ready for its shortcut.">
          <Toggle label="Start at login" checked={c.startAtLogin} onChange={(v) => set((x) => ({ ...x, startAtLogin: v }))} />
        </Row>
      </Section>
      <Section title="Window">
        <Row label="Visible results" hint={<>Rows in the search window, 1 to 12{restart}</>}>
          <NumberField value={c.rows} min={1} max={12} onCommit={(v) => set((x) => ({ ...x, rows: v }))} />
        </Row>
      </Section>
    </>
  )
}

function startLabel() {
  return window.redsnap?.platform === 'win32' ? 'Start with Windows' : 'Start at login'
}

const PROVIDERS: { key: keyof Config['providers']; label: string; hint: string }[] = [
  { key: 'apps', label: 'Apps', hint: 'Installed applications' },
  { key: 'calculator', label: 'Calculator', hint: 'Type an expression like 2*(3+4)' },
  { key: 'webSearches', label: 'Web searches', hint: 'A keyword and words: g rust' },
  { key: 'quickLinks', label: 'Quick links', hint: 'Your saved links' },
  { key: 'open', label: 'Open URLs and paths', hint: 'Type github.com or C:\\Users' },
  { key: 'commands', label: 'RedSnap commands', hint: 'Settings, Reload Apps, Quit…' },
  { key: 'systemCommands', label: 'System commands', hint: 'Lock, Sleep, Restart, Shut Down…' },
]

function newId(name: string) {
  const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'item'
  return `${slug}-${Math.random().toString(36).slice(2, 7)}`
}

function Search({ c, set }: Edit) {
  return (
    <>
      <Section title="What search finds">
        {PROVIDERS.map((p) => (
          <Row key={p.key} label={p.label} hint={p.hint}>
            <Toggle
              label={p.label}
              checked={c.providers[p.key]}
              onChange={(v) => set((x) => ({ ...x, providers: { ...x.providers, [p.key]: v } }))}
            />
          </Row>
        ))}
      </Section>
      <LinkTable
        title="Web searches"
        description="Type the keyword, a space and your words. {query} in the URL is replaced by them."
        items={c.webSearches}
        needsQuery
        onChange={(items) => set((x) => ({ ...x, webSearches: items as WebSearch[] }))}
      />
      <LinkTable
        title="Quick links"
        description="Found by name or keyword. With {query} in the URL, they take words like a web search."
        items={c.quickLinks}
        onChange={(items) => set((x) => ({ ...x, quickLinks: items as QuickLink[] }))}
      />
    </>
  )
}

function LinkTable({
  title,
  description,
  items,
  onChange,
  needsQuery,
}: {
  title: string
  description: string
  items: (WebSearch | QuickLink)[]
  onChange: (items: (WebSearch | QuickLink)[]) => void
  needsQuery?: boolean
}) {
  const update = (i: number, field: 'name' | 'keyword' | 'url', v: string) =>
    onChange(items.map((it, j) => (j === i ? { ...it, [field]: v.trim() } : it)))
  return (
    <Section title={title} description={description}>
      {items.length === 0 && <div className="px-4 py-3 text-[13px] text-ink-400">None yet.</div>}
      {items.map((it, i) => (
        <div key={it.id} className="flex items-center gap-2 px-4 py-2.5">
          <TextField width="w-36" value={it.name} placeholder="Name" onCommit={(v) => update(i, 'name', v)} />
          <TextField width="w-20" value={it.keyword} placeholder="Keyword" mono onCommit={(v) => update(i, 'keyword', v)} />
          <div className="min-w-0 flex-1">
            <TextField width="w-full" value={it.url} placeholder="https://…{query}" mono onCommit={(v) => update(i, 'url', v)} />
          </div>
          <Button kind="ghost" onClick={() => onChange(items.filter((_, j) => j !== i))}>
            Remove
          </Button>
        </div>
      ))}
      <div className="px-4 py-2.5">
        <Button
          onClick={() =>
            onChange([
              ...items,
              { id: newId(title), name: 'New', keyword: '', url: needsQuery ? 'https://example.com/search?q={query}' : 'https://example.com' },
            ])
          }
        >
          Add
        </Button>
      </div>
    </Section>
  )
}

function useStats() {
  const [stats, setStats] = useState<AppStats | null>(null)
  useEffect(() => {
    api.appStats().then(setStats, () => {})
    return api.onEvent((name, params) => {
      if (name === 'apps.stats') setStats(params as AppStats)
    })
  }, [])
  return stats
}

function Apps({ c, set }: Edit) {
  const stats = useStats()
  const [filter, setFilter] = useState('')
  const names = useMemo(() => new Map(stats?.apps.map((a) => [a.id, a.name]) ?? []), [stats])
  const shown = useMemo(() => {
    const f = filter.trim().toLowerCase()
    return (stats?.apps ?? []).filter((a) => !f || a.name.toLowerCase().includes(f)).slice(0, 200)
  }, [stats, filter])

  return (
    <>
      <Section
        title="Installed apps"
        description={
          stats
            ? `${stats.count} apps found${stats.lastScanMs ? `, last scan ${new Date(stats.lastScanMs).toLocaleTimeString()}` : ''}${stats.scanning ? ' · scanning…' : ''}`
            : 'Loading…'
        }
      >
        <div className="flex items-center gap-2 px-4 py-3">
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter apps"
            className="h-8 flex-1 rounded-lg border border-white/10 bg-ink-950 px-2.5 text-[13px] outline-none placeholder:text-ink-600 focus:border-red-500/70"
          />
          <Button onClick={() => api.reloadApps()}>Scan again</Button>
        </div>
        <div className="max-h-96 overflow-y-auto">
          {shown.map((a) => {
            const hidden = c.hiddenApps.includes(a.id)
            const aliases = c.appAliases[a.id] ?? []
            return (
              <div key={a.id} className="flex items-center gap-3 border-t border-white/5 px-4 py-2">
                <div className={`min-w-0 flex-1 truncate text-[14px] ${hidden ? 'text-ink-400 line-through' : ''}`}>{a.name}</div>
                <TextField
                  width="w-48"
                  value={aliases.join(', ')}
                  placeholder="Also find by…"
                  onCommit={(v) =>
                    set((x) => {
                      const list = v.split(',').map((s) => s.trim()).filter(Boolean)
                      const next = { ...x.appAliases }
                      if (list.length) next[a.id] = list
                      else delete next[a.id]
                      return { ...x, appAliases: next }
                    })
                  }
                />
                <Button
                  kind="ghost"
                  onClick={() =>
                    set((x) => ({
                      ...x,
                      hiddenApps: hidden ? x.hiddenApps.filter((h) => h !== a.id) : [...x.hiddenApps, a.id],
                    }))
                  }
                >
                  {hidden ? 'Show' : 'Hide'}
                </Button>
              </div>
            )
          })}
        </div>
      </Section>
      {c.hiddenApps.length > 0 && (
        <Section title="Hidden apps" description="Left out of search.">
          {c.hiddenApps.map((id) => (
            <Row key={id} label={names.get(id) ?? id}>
              <Button kind="ghost" onClick={() => set((x) => ({ ...x, hiddenApps: x.hiddenApps.filter((h) => h !== id) }))}>
                Show again
              </Button>
            </Row>
          ))}
        </Section>
      )}
      <Section title="More app folders" description="Searched for apps in addition to the usual places.">
        <StringList
          items={c.extraAppFolders}
          placeholder="A folder path"
          mono
          empty="Only the usual places."
          onChange={(items) => set((x) => ({ ...x, extraAppFolders: items }))}
        />
      </Section>
    </>
  )
}

const PLACEHOLDERS: [string, string][] = [
  ['{{cursor}}', 'where the caret ends up'],
  ['{{date}}', '2026-11-19'],
  ['{{time}}', '14:05'],
  ['{{datetime}}', '2026-11-19 14:05'],
  ['{{date:%d/%m/%Y}}', 'any date format'],
  ['{{clipboard}}', "the clipboard's text"],
]

function SnippetsPage({ c, set }: Edit) {
  const s = c.snippets
  const [filter, setFilter] = useState('')
  const setS = (change: Partial<Config['snippets']>) => set((x) => ({ ...x, snippets: { ...x.snippets, ...change } }))
  const setItem = (id: string, change: Partial<Snippet>) =>
    set((x) => ({ ...x, snippets: { ...x.snippets, items: x.snippets.items.map((i) => (i.id === id ? { ...i, ...change } : i)) } }))
  const f = filter.trim().toLowerCase()
  const shown = s.items.filter((i) => !f || `${i.name} ${i.trigger} ${i.text}`.toLowerCase().includes(f))
  return (
    <>
      <Section title="Snippets" description="Text you use often. Type a snippet's trigger in any app, or find it in RedSnap and press Enter.">
        <Row
          label="Expand as you type"
          hint={window.redsnap?.platform === 'win32' ? 'Typing a trigger replaces it with its snippet.' : 'Not available on this system yet; snippets still work from RedSnap.'}
        >
          <Toggle label="Expand as you type" checked={s.expandAsYouType} onChange={(v) => setS({ expandAsYouType: v })} />
        </Row>
        <div className="px-4 py-3 text-[12px] leading-5 text-ink-400">
          A trigger that starts with a letter expands only at the start of a word; triggers starting with a symbol, like{' '}
          <code className="text-ink-300">;sig</code>, expand anywhere. Placeholders:{' '}
          {PLACEHOLDERS.map(([p, what], i) => (
            <span key={p}>
              <code className="text-ink-300">{p}</code> {what}
              {i < PLACEHOLDERS.length - 1 ? ', ' : '.'}
            </span>
          ))}
        </div>
      </Section>

      <div className="mb-3 flex items-center gap-2">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter snippets"
          className="h-8 flex-1 rounded-lg border border-white/10 bg-ink-950 px-2.5 text-[13px] outline-none placeholder:text-ink-600 focus:border-red-500/70"
        />
        <Button
          kind="primary"
          onClick={() =>
            setS({
              items: [...s.items, { id: newId('snippet'), name: 'New snippet', trigger: '', text: '', autoExpand: true }],
            })
          }
        >
          Add snippet
        </Button>
      </div>
      {shown.length === 0 && <div className="mb-8 text-[13px] text-ink-400">{s.items.length ? 'No snippet matches.' : 'No snippets yet.'}</div>}
      {shown.map((i) => (
        <div key={i.id} className="mb-3 rounded-xl border border-white/8 bg-ink-900 p-4">
          <div className="mb-3 flex items-center gap-2">
            <TextField width="w-56" value={i.name} placeholder="Name" onCommit={(v) => setItem(i.id, { name: v })} />
            <TextField width="w-32" value={i.trigger} placeholder="Trigger, e.g. ;sig" mono onCommit={(v) => setItem(i.id, { trigger: v.trim() })} />
            <div className="ml-auto flex items-center gap-2 text-[12px] text-ink-400">
              Expand as typed
              <Toggle label="Expand as typed" checked={i.autoExpand} onChange={(v) => setItem(i.id, { autoExpand: v })} />
            </div>
            <Button kind="ghost" onClick={() => setS({ items: s.items.filter((x) => x.id !== i.id) })}>
              Remove
            </Button>
          </div>
          <TextArea value={i.text} placeholder="The text. Ctrl+Enter to save." onCommit={(v) => setItem(i.id, { text: v })} />
        </div>
      ))}

      <Section title="Excluded apps" description="Triggers never expand in these apps (executable or app name). Password fields are always skipped.">
        <StringList
          items={s.excludedApps}
          placeholder={window.redsnap?.platform === 'win32' ? 'WindowsTerminal.exe' : 'Terminal'}
          empty="No apps excluded."
          onChange={(items) => setS({ excludedApps: items })}
        />
      </Section>
    </>
  )
}

function ClipboardPage({ c, set, save, notify }: Edit & { save: Save; notify: (n: Notice) => void }) {
  const [stats, setStats] = useState<ClipStats | null>(null)
  const refresh = () => api.clipboardStats().then(setStats, () => {})
  useEffect(() => void refresh(), [])
  const cb = c.clipboard
  const setCb = (change: Partial<Config['clipboard']>) => set((x) => ({ ...x, clipboard: { ...x.clipboard, ...change } }))
  return (
    <>
      <Section title="Clipboard history" description="Every copy is either kept or listed as not kept, with the reason.">
        <Row label="Keep clipboard history" hint={<>Off: nothing is recorded{restart}</>}>
          <Toggle label="Keep clipboard history" checked={cb.enabled} onChange={(v) => setCb({ enabled: v })} />
        </Row>
        <Row label="Shortcut">
          <ShortcutRecorder value={cb.hotkey} onChange={(spec) => save((x) => ({ ...x, clipboard: { ...x.clipboard, hotkey: spec } }))} />
        </Row>
        <Row label="Show copies that weren't kept" hint="Dimmed, with the reason (marked private, too large, an excluded app…).">
          <Toggle label="Show skipped copies" checked={cb.showSkipped} onChange={(v) => setCb({ showSkipped: v })} />
        </Row>
      </Section>
      <Section title="Keeping" description={<>Pinned entries are always kept{restart}.</>}>
        <Row label="Keep entries for" hint="Days">
          <NumberField value={cb.keepDays} min={1} max={3650} onCommit={(v) => setCb({ keepDays: v })} />
        </Row>
        <Row label="Keep at most" hint="Entries">
          <NumberField value={cb.maxEntries} min={10} max={100000} onCommit={(v) => setCb({ maxEntries: v })} />
        </Row>
        <Row label="History" hint={stats?.available ? `${stats.entries} entries` : 'Not running'}>
          <ConfirmButton
            label="Clear history"
            confirm="Click again to clear (pinned stay)"
            onConfirm={async () => {
              await api.clearClipboard()
              refresh()
              notify({ kind: 'ok', text: 'Clipboard history cleared; pinned entries kept' })
            }}
          />
        </Row>
      </Section>
      <Section title="Excluded apps" description="Copies from these apps are listed as not kept. Use the app's executable or name.">
        <StringList
          items={cb.excludedApps}
          placeholder={window.redsnap?.platform === 'win32' ? 'KeePassXC.exe' : '1Password'}
          empty="No apps excluded. Apps that mark copies private (password managers) are skipped anyway."
          onChange={(items) => setCb({ excludedApps: items })}
        />
      </Section>
    </>
  )
}

function Advanced({ notify }: Edit & { notify: (n: Notice) => void }) {
  const [info, setInfo] = useState<AppInfo | null>(null)
  useEffect(() => void api.info().then(setInfo, () => {}), [])
  return (
    <>
      <Section title="Learning">
        <Row label="Forget what RedSnap learned" hint="Which results you picked for which words.">
          <ConfirmButton
            label="Reset"
            confirm="Click again to reset"
            onConfirm={async () => {
              try {
                await api.resetUsage()
                notify({ kind: 'ok', text: 'Learned ranking reset' })
              } catch (e) {
                notify({ kind: 'error', text: message(e) })
              }
            }}
          />
        </Row>
      </Section>
      <Section title="Files">
        <Row label="Logs" hint="For reporting a problem.">
          <Button onClick={() => api.openLogs()}>Open folder</Button>
        </Row>
        <Row label="Data" hint={info?.dataDir}>
          <Button onClick={() => api.openData()}>Open folder</Button>
        </Row>
        <Row label="Settings file" hint={info ? `${info.configDir}` : undefined}>
          <span className="text-[12px] text-ink-400">config.json</span>
        </Row>
      </Section>
      <Section title="About">
        <Row label="RedSnap" hint={info ? `${info.platform}` : undefined}>
          <span className="text-[13px] text-ink-300">{info?.version}</span>
        </Row>
      </Section>
    </>
  )
}
