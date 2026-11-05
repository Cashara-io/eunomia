// Inspired by React Bits HoldButton (reactbits.dev). Original implementation for Eunomia.
// Usage: <HoldButton holdMs={900} onConfirm={revokeAll} className="btn-sm"><Trash2 /> Revoke all</HoldButton> renders a .btn.btn-danger (red edge and text; the hold fill is the soft red wash).
// Click, Enter or Space confirm at once (no fill); a pointer hold confirms after holdMs. Pass onClick to make the click do something else (e.g. arm a confirm step) while the hold still confirms.
"use client";

import { useEffect, useRef, useState } from "react";
import "./bits.css";

type Props = Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, "onClick"> & {
  holdMs?: number;
  onConfirm: () => void;
  onClick?: () => void;
};

// Matches the fill's transition-delay in bits.css, so an ordinary click never flashes red.
const HOLD_DELAY_MS = 150;

export default function HoldButton({ holdMs = 900, onConfirm, onClick, className = "", style, children, ...rest }: Props) {
  const [holding, setHolding] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  // Set when the hold fired, so the click that follows the release does not confirm twice.
  const fired = useRef(false);
  useEffect(() => () => clearTimeout(timer.current), []);

  function release() {
    clearTimeout(timer.current);
    setHolding(false);
  }

  return (
    <button
      type="button"
      {...rest}
      className={`btn btn-danger bits-hold ${className}`}
      style={{ ...style, ["--hold-ms" as string]: `${holdMs}ms` }}
      data-holding={holding ? "" : undefined}
      onPointerDown={(e) => {
        if (e.button !== 0 || rest.disabled) return;
        fired.current = false;
        setHolding(true);
        clearTimeout(timer.current);
        timer.current = setTimeout(() => {
          fired.current = true;
          setHolding(false);
          onConfirm();
        }, holdMs + HOLD_DELAY_MS);
      }}
      onPointerUp={release}
      onPointerLeave={() => {
        // A click never lands here once the pointer has left, so forget the fired hold.
        fired.current = false;
        release();
      }}
      onPointerCancel={release}
      onClick={() => {
        if (fired.current) {
          fired.current = false;
          return;
        }
        release();
        (onClick ?? onConfirm)();
      }}
    >
      <span className="bits-hold-fill" aria-hidden />
      {children}
    </button>
  );
}
