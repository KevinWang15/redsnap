// Shortcuts as global-hotkey specs (`Ctrl+Alt+Space`), from key events and for display.

import { isMac } from './api'

const MODIFIERS = new Set(['Control', 'Alt', 'Shift', 'Meta', 'AltGraph', 'OS'])

/** A key event as a global-hotkey spec: `Ctrl+Alt+Space`, `Alt+Super+C`. */
export function specFromEvent(e: KeyboardEvent): string | null {
  if (MODIFIERS.has(e.key)) return null
  const code = e.code
  let key: string
  if (code.startsWith('Key')) key = code.slice(3)
  else if (code.startsWith('Digit')) key = code.slice(5)
  else if (/^F\d{1,2}$/.test(code)) key = code
  else if (['Space', 'Enter', 'Tab', 'Backquote', 'Minus', 'Equal', 'Comma', 'Period', 'Slash', 'Semicolon', 'Quote', 'BracketLeft', 'BracketRight', 'Backslash'].includes(code)) key = code
  else if (code.startsWith('Arrow')) key = code
  else return null
  const mods: string[] = []
  if (e.ctrlKey) mods.push('Ctrl')
  if (e.altKey) mods.push('Alt')
  if (e.shiftKey) mods.push('Shift')
  if (e.metaKey) mods.push('Super')
  // A plain key would fire while typing anywhere; function keys may stand alone.
  if (mods.length === 0 && !/^F\d/.test(key)) return null
  return [...mods, key].join('+')
}

/** How a spec reads: `Ctrl + Alt + Space`, or `⌥⌘C` on macOS. */
export function displayShortcut(spec: string): string {
  const parts = spec.split('+')
  if (isMac()) {
    const sym: Record<string, string> = { Ctrl: '⌃', Alt: '⌥', Shift: '⇧', Super: '⌘', Cmd: '⌘', Space: 'Space' }
    return parts.map((p) => sym[p] ?? p).join('')
  }
  return parts.map((p) => (p === 'Super' ? 'Win' : p)).join(' + ')
}
