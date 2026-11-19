// Small controls shared by the settings sections.

import { type ReactNode, useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { api, message } from './api'
import { displayShortcut, specFromEvent } from './shortcuts'

export function Section({ title, description, children }: { title: string; description?: ReactNode; children: ReactNode }) {
  return (
    <section className="mb-8">
      <h2 className="text-[15px] font-semibold text-ink-100">{title}</h2>
      {description && <p className="mt-1 text-[13px] text-ink-400">{description}</p>}
      <div className="mt-3 divide-y divide-white/5 rounded-xl border border-white/8 bg-ink-900">{children}</div>
    </section>
  )
}

export function Row({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex items-center gap-6 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="text-[14px] text-ink-100">{label}</div>
        {hint && <div className="mt-0.5 text-[12px] text-ink-400">{hint}</div>}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  )
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  // The knob is a flex child, centered by layout and moved with a transform,
  // so it stays centered at any display scale (absolute 2 px offsets rounded
  // unevenly at 125%).
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`box-border inline-flex h-6 w-11 shrink-0 items-center rounded-full p-0.5 transition-colors ${checked ? 'bg-red-500' : 'bg-ink-700'}`}
    >
      <span
        className={`block h-5 w-5 rounded-full bg-white shadow transition-transform ${checked ? 'translate-x-5' : 'translate-x-0'}`}
      />
    </button>
  )
}

export function Button({
  children,
  onClick,
  kind = 'normal',
  disabled,
}: {
  children: ReactNode
  onClick: () => void
  kind?: 'normal' | 'danger' | 'primary' | 'ghost'
  disabled?: boolean
}) {
  const styles = {
    normal: 'bg-ink-800 hover:bg-ink-700 text-ink-100 border border-white/8',
    primary: 'bg-red-500 hover:bg-red-600 text-white',
    danger: 'bg-ink-800 hover:bg-red-700 hover:text-white text-red-500 border border-white/8',
    ghost: 'text-ink-300 hover:text-ink-100 hover:bg-white/5',
  }[kind]
  return (
    <button
      disabled={disabled}
      onClick={onClick}
      className={`h-8 rounded-lg px-3 text-[13px] font-medium transition-colors disabled:opacity-40 ${styles}`}
    >
      {children}
    </button>
  )
}

/** A text field that commits on Enter or when it loses focus. */
export function TextField({
  value,
  onCommit,
  placeholder,
  width = 'w-64',
  mono,
}: {
  value: string
  onCommit: (v: string) => void
  placeholder?: string
  width?: string
  mono?: boolean
}) {
  const [draft, setDraft] = useState(value)
  // A new saved value (from RedSnap) replaces the draft.
  const [saved, setSaved] = useState(value)
  if (value !== saved) {
    setSaved(value)
    setDraft(value)
  }
  const commit = () => {
    if (draft !== value) onCommit(draft)
  }
  return (
    <input
      value={draft}
      placeholder={placeholder}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        if (e.key === 'Escape') setDraft(value)
      }}
      className={`h-8 ${width} rounded-lg border border-white/10 bg-ink-950 px-2.5 text-[13px] text-ink-100 outline-none placeholder:text-ink-600 focus:border-red-500/70 ${mono ? 'font-mono' : ''}`}
    />
  )
}

/** A multi-line text field that commits when it loses focus (Ctrl/Cmd+Enter too). */
export function TextArea({ value, onCommit, placeholder, rows = 4 }: { value: string; onCommit: (v: string) => void; placeholder?: string; rows?: number }) {
  const [draft, setDraft] = useState(value)
  const [saved, setSaved] = useState(value)
  if (value !== saved) {
    setSaved(value)
    setDraft(value)
  }
  return (
    <textarea
      value={draft}
      rows={rows}
      placeholder={placeholder}
      spellCheck={false}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => draft !== value && onCommit(draft)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) (e.target as HTMLTextAreaElement).blur()
      }}
      className="w-full resize-y rounded-lg border border-white/10 bg-ink-950 px-2.5 py-2 font-mono text-[13px] leading-5 text-ink-100 outline-none placeholder:text-ink-600 focus:border-red-500/70"
    />
  )
}

export function NumberField({ value, onCommit, min, max }: { value: number; onCommit: (v: number) => void; min: number; max: number }) {
  return (
    <TextField
      width="w-24"
      value={String(value)}
      onCommit={(v) => {
        const n = Math.round(Number(v))
        if (Number.isFinite(n)) onCommit(Math.min(max, Math.max(min, n)))
      }}
    />
  )
}

// ----- shortcuts ---------------------------------------------------------------

/**
 * Click, then press the new shortcut. While recording, RedSnap's own
 * shortcuts are off, so pressing the current one records it instead of
 * opening RedSnap. The shortcut is checked with RedSnap before it's used.
 */
export function ShortcutRecorder({ value, onChange }: { value: string; onChange: (spec: string) => Promise<void> }) {
  const [recording, setRecording] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // The latest callback, without restarting the recording on every render.
  const onChangeRef = useRef(onChange)
  useEffect(() => {
    onChangeRef.current = onChange
  }, [onChange])

  useEffect(() => {
    if (!recording) return
    api.suspendHotkeys()
    const onKey = async (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()
      if (e.key === 'Escape' && !e.ctrlKey && !e.altKey && !e.metaKey) {
        setRecording(false)
        return
      }
      const spec = specFromEvent(e)
      if (!spec) return
      setRecording(false)
      try {
        await api.testHotkey(spec)
        await onChangeRef.current(spec)
        setError(null)
      } catch (err) {
        setError(message(err))
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      api.resumeHotkeys()
    }
  }, [recording])

  return (
    <div className="flex flex-col items-end gap-1">
      <button
        onClick={() => {
          setError(null)
          setRecording((r) => !r)
        }}
        onBlur={() => setRecording(false)}
        className={`h-8 min-w-40 rounded-lg border px-3 text-[13px] font-medium transition-colors ${
          recording ? 'border-red-500 bg-red-500/10 text-red-500' : 'border-white/10 bg-ink-950 text-ink-100 hover:border-white/25'
        }`}
      >
        {recording ? 'Press the new shortcut…' : displayShortcut(value)}
      </button>
      {error && <div className="max-w-72 text-right text-[12px] text-red-500">{error}</div>}
    </div>
  )
}

/** A list of strings with add and remove. */
export function StringList({
  items,
  onChange,
  placeholder,
  mono,
  empty,
}: {
  items: string[]
  onChange: (items: string[]) => void
  placeholder: string
  mono?: boolean
  empty: string
}) {
  const [draft, setDraft] = useState('')
  const add = () => {
    const v = draft.trim()
    if (v && !items.includes(v)) onChange([...items, v])
    setDraft('')
  }
  return (
    <div className="px-4 py-3">
      {items.length === 0 && <div className="mb-2 text-[13px] text-ink-400">{empty}</div>}
      <div className="flex flex-wrap gap-2">
        {items.map((it) => (
          <span
            key={it}
            className={`group flex h-7 items-center gap-1.5 rounded-md border border-white/8 bg-ink-800 pl-2.5 pr-1 text-[13px] ${mono ? 'font-mono' : ''}`}
          >
            {it}
            <button
              aria-label={`Remove ${it}`}
              onClick={() => onChange(items.filter((x) => x !== it))}
              className="flex h-5 w-5 items-center justify-center rounded text-ink-400 hover:bg-white/10 hover:text-ink-100"
            >
              <X size={13} />
            </button>
          </span>
        ))}
      </div>
      <div className="mt-3 flex gap-2">
        <input
          value={draft}
          placeholder={placeholder}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && add()}
          className="h-8 flex-1 rounded-lg border border-white/10 bg-ink-950 px-2.5 text-[13px] outline-none placeholder:text-ink-600 focus:border-red-500/70"
        />
        <Button onClick={add} disabled={!draft.trim()}>
          Add
        </Button>
      </div>
    </div>
  )
}

/** Asks first; the button turns into a confirmation for a few seconds. */
export function ConfirmButton({ label, confirm, onConfirm }: { label: string; confirm: string; onConfirm: () => void }) {
  const [armed, setArmed] = useState(false)
  useEffect(() => {
    if (!armed) return
    const t = setTimeout(() => setArmed(false), 4000)
    return () => clearTimeout(t)
  }, [armed])
  return (
    <Button
      kind="danger"
      onClick={() => {
        if (armed) {
          setArmed(false)
          onConfirm()
        } else setArmed(true)
      }}
    >
      {armed ? confirm : label}
    </Button>
  )
}
