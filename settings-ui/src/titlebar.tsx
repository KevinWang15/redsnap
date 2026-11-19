// The window's title bar, painted by the page (the window has no system one).
// Windows and Linux get our buttons; macOS keeps its traffic lights, which sit
// over the bar's left end.
import { useEffect, useState } from "react";
import { Copy, Minus, Square, X } from "lucide-react";
import { type ResizeDir, api, isMac, isWindows } from "./api";

export function TitleBar() {
  const mac = isMac();
  const [maximized, setMaximized] = useState(false);
  useEffect(
    () =>
      api.onEvent((name, params) => {
        if (name === "window")
          setMaximized(Boolean((params as { maximized?: boolean }).maximized));
      }),
    [],
  );
  const win = api.window();

  // A press on the bar itself (not a button) moves the window.
  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    if (e.detail === 2) win.toggleMaximize();
    else win.drag();
  };

  return (
    <>
      <header
        onMouseDown={onMouseDown}
        className={`flex h-[38px] shrink-0 select-none items-center border-b border-white/5 bg-ink-900/80 ${mac ? "pl-[78px]" : "pl-3.5"}`}
      >
        {!mac && <SmallMark />}
        <span className={`text-[12.5px] text-ink-300 ${mac ? "" : "ml-2.5"}`}>
          RedSnap Settings
        </span>
        <div className="flex-1" />
        {!mac && (
          <div className="flex h-full">
            <Control label="Minimize" onClick={win.minimize}>
              <Minus size={16} strokeWidth={1.75} />
            </Control>
            <Control
              label={maximized ? "Restore" : "Maximize"}
              onClick={win.toggleMaximize}
            >
              {maximized ? (
                <Copy size={13} strokeWidth={1.75} className="-scale-x-100" />
              ) : (
                <Square size={13} strokeWidth={1.75} />
              )}
            </Control>
            <Control label="Close" onClick={win.close} close>
              <X size={17} strokeWidth={1.75} />
            </Control>
          </div>
        )}
      </header>
      {!mac && !maximized && <ResizeZones all={!isWindows()} />}
    </>
  );
}

function Control({
  label,
  onClick,
  close,
  children,
}: {
  label: string;
  onClick: () => void;
  close?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      title={label}
      onClick={onClick}
      className={`flex h-full w-[46px] items-center justify-center text-ink-300 transition-colors ${
        close
          ? "hover:bg-[#e81123] hover:text-white"
          : "hover:bg-white/10 hover:text-ink-100"
      }`}
    >
      {children}
    </button>
  );
}

/** The mark at title-bar size. */
function SmallMark() {
  return (
    <svg width="18" height="18" viewBox="0 0 44 44" aria-hidden="true">
      <g
        fill="none"
        stroke="rgb(255 255 255 / 0.9)"
        strokeWidth="3.5"
        strokeLinecap="round"
      >
        <path d="M4 15V7a3 3 0 0 1 3-3h8" />
        <path d="M29 4h8a3 3 0 0 1 3 3v8" />
        <path d="M40 29v8a3 3 0 0 1-3 3h-8" />
        <path d="M15 40H7a3 3 0 0 1-3-3v-8" />
      </g>
      <rect x="13" y="13" width="18" height="18" rx="4" fill="#e8344e" />
    </svg>
  );
}

// Thin strips along the window's edges that start a native resize. Windows
// resizes from its shadow frame on every side but the top, so it only needs
// the top ones; Linux has no frame at all.
const EDGE = 5;
const ZONES: { dir: ResizeDir; style: React.CSSProperties; cursor: string }[] =
  [
    {
      dir: "n",
      style: { top: 0, left: EDGE * 2, right: EDGE * 2, height: EDGE },
      cursor: "ns-resize",
    },
    {
      dir: "nw",
      style: { top: 0, left: 0, width: EDGE * 2, height: EDGE * 2 },
      cursor: "nwse-resize",
    },
    {
      dir: "ne",
      style: { top: 0, right: 0, width: EDGE * 2, height: EDGE * 2 },
      cursor: "nesw-resize",
    },
    {
      dir: "s",
      style: { bottom: 0, left: EDGE * 2, right: EDGE * 2, height: EDGE },
      cursor: "ns-resize",
    },
    {
      dir: "w",
      style: { left: 0, top: EDGE * 2, bottom: EDGE * 2, width: EDGE },
      cursor: "ew-resize",
    },
    {
      dir: "e",
      style: { right: 0, top: EDGE * 2, bottom: EDGE * 2, width: EDGE },
      cursor: "ew-resize",
    },
    {
      dir: "sw",
      style: { bottom: 0, left: 0, width: EDGE * 2, height: EDGE * 2 },
      cursor: "nesw-resize",
    },
    {
      dir: "se",
      style: { bottom: 0, right: 0, width: EDGE * 2, height: EDGE * 2 },
      cursor: "nwse-resize",
    },
  ];

function ResizeZones({ all }: { all: boolean }) {
  const win = api.window();
  return (
    <>
      {ZONES.filter((z) => all || z.dir.startsWith("n")).map((z) => (
        <div
          key={z.dir}
          aria-hidden="true"
          className="fixed z-50"
          style={{ ...z.style, cursor: z.cursor }}
          onMouseDown={(e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            win.resize(z.dir);
          }}
        />
      ))}
    </>
  );
}
