// What RedSnap sends and accepts. Mirrors crates/core/src/config.rs; RedSnap
// validates everything, this app only edits.

export type Providers = {
  apps: boolean
  calculator: boolean
  webSearches: boolean
  quickLinks: boolean
  open: boolean
  commands: boolean
  systemCommands: boolean
}

export type WebSearch = { id: string; name: string; keyword: string; url: string }
export type QuickLink = { id: string; name: string; keyword: string; url: string }

export type ClipboardConfig = {
  enabled: boolean
  hotkey: string
  keepDays: number
  maxEntries: number
  excludedApps: string[]
  showSkipped: boolean
}

export type Config = {
  version: number
  hotkey: string
  startAtLogin: boolean
  rows: number
  providers: Providers
  webSearches: WebSearch[]
  quickLinks: QuickLink[]
  appAliases: Record<string, string[]>
  hiddenApps: string[]
  extraAppFolders: string[]
  clipboard: ClipboardConfig
}

export type Snapshot = { config: Config; revision: number }
export type AppInfo = { version: string; platform: string; configDir: string; dataDir: string }
export type AppStats = { count: number; lastScanMs: number; scanning: boolean; apps: { name: string; id: string }[] }
export type ClipStats = { available: boolean; entries?: number }

type Bridge = {
  call: (method: string, params?: unknown) => Promise<unknown>
  onEvent: (cb: (name: string, params: unknown) => void) => () => void
  platform: string
}

declare global {
  interface Window {
    redsnap?: Bridge
  }
}

function bridge(): Bridge {
  if (!window.redsnap) throw new Error('Not running inside RedSnap Settings')
  return window.redsnap
}

export const isMac = () => window.redsnap?.platform === 'darwin'

export const api = {
  getConfig: () => bridge().call('config.get') as Promise<Snapshot>,
  saveConfig: (config: Config, revision: number) =>
    bridge().call('config.save', { config, revision }) as Promise<{ revision: number }>,
  info: () => bridge().call('app.info') as Promise<AppInfo>,
  suspendHotkeys: () => bridge().call('hotkey.suspend'),
  resumeHotkeys: () => bridge().call('hotkey.resume'),
  testHotkey: (shortcut: string) => bridge().call('hotkey.test', { shortcut }),
  appStats: () => bridge().call('apps.stats') as Promise<AppStats>,
  reloadApps: () => bridge().call('apps.reload'),
  resetUsage: () => bridge().call('usage.reset'),
  clipboardStats: () => bridge().call('clipboard.stats') as Promise<ClipStats>,
  clearClipboard: () => bridge().call('clipboard.clear'),
  openLogs: () => bridge().call('logs.open'),
  openData: () => bridge().call('data.open'),
  onEvent: (cb: (name: string, params: unknown) => void) => bridge().onEvent(cb),
}

/** Errors from RedSnap arrive as "Error invoking remote method ...: Error: <message>". */
export function message(e: unknown): string {
  const s = e instanceof Error ? e.message : String(e)
  const i = s.lastIndexOf('Error: ')
  return i >= 0 ? s.slice(i + 7) : s
}
