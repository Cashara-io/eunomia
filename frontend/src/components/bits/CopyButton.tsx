// Inspired by React Bits SpringCheck (reactbits.dev). Original implementation for Eunomia.
// Usage: <CopyButton value={token} /> (icon only, name "Copy") or <CopyButton value={() => cmd} label="Copy command" size="sm" className="btn-primary" />.
// Names flip to "Copied" for 1.5s after a successful write; size="icon" puts the name in aria-label, size="sm" shows it as text.
"use client";

import { CopyIcon } from "@phosphor-icons/react/ssr";

import { useEffect, useRef, useState } from "react";
import "./bits.css";

type Props = {
  value: string | (() => string);
  label?: string;
  size?: "icon" | "sm";
  className?: string;
};

export default function CopyButton({ value, label = "Copy", size = "icon", className = "" }: Props) {
  const [copied, setCopied] = useState(false);
  const [drawn, setDrawn] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  async function copy(e: React.MouseEvent) {
    // detail is 0 for Enter/Space: keyboard copies swap the icon without the draw.
    const pointer = e.detail > 0;
    try {
      await navigator.clipboard.writeText(typeof value === "function" ? value() : value);
    } catch {
      return;
    }
    setDrawn(pointer);
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
  }

  const name = copied ? "Copied" : label;
  const icon = copied ? (
    <svg width={14} height={14} viewBox="0 0 16 16" fill="none" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-hidden className="bits-copy-check">
      <path d="M3.5 8.4 6.6 11.3 12.5 4.9" stroke="currentColor" pathLength={1} className="bits-draw" data-animate={drawn || undefined} />
    </svg>
  ) : (
    <CopyIcon weight="duotone" size={14} aria-hidden />
  );

  if (size === "icon")
    return (
      <button type="button" onClick={copy} aria-label={name} className={`btn btn-ghost btn-sm btn-icon ${className}`}>
        {icon}
      </button>
    );
  return (
    <button type="button" onClick={copy} aria-live="polite" className={`btn btn-sm ${className}`}>
      {icon}
      {name}
    </button>
  );
}
