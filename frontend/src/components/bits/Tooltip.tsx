// Inspired by React Bits WarmTooltip (reactbits.dev). Original implementation for Cupel.
// Usage: <Tooltip label="New chat" shortcut="N"><button aria-label="New chat" ...>{icon}</button></Tooltip>; wrap a toolbar in <TooltipGroup> to share its warm window.
// Visual only: the trigger keeps its own aria-label. Opens after 400ms on mouse hover, at once on keyboard focus, and instantly for 300ms after another one closed.
"use client";

import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import "./bits.css";

const FIRST_DELAY = 400;
const WARM_MS = 300;
const GAP = 6;
const EDGE = 8;

type Group = { open: number; warmUntil: number };
// A ref, so handlers can update the shared window without re-rendering every tooltip.
const GroupContext = createContext<React.RefObject<Group>>({ current: { open: 0, warmUntil: 0 } });

/** Tooltips inside share one warm window: once one has shown, neighbours open with no delay and no animation. */
export function TooltipGroup({ children }: { children: React.ReactNode }) {
  const groupRef = useRef<Group>({ open: 0, warmUntil: 0 });
  return <GroupContext.Provider value={groupRef}>{children}</GroupContext.Provider>;
}

type Shown = { rect: DOMRect; instant: boolean };

export default function Tooltip({ label, shortcut, children }: { label: string; shortcut?: string; children: React.ReactNode }) {
  const groupRef = useContext(GroupContext);
  const anchor = useRef<HTMLSpanElement>(null);
  const tip = useRef<HTMLSpanElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [shown, setShown] = useState<Shown | null>(null);

  function open(instant: boolean) {
    const el = anchor.current;
    if (!el) return;
    groupRef.current.open++;
    setShown({ rect: el.getBoundingClientRect(), instant });
  }
  function show(fromKeyboard: boolean) {
    clearTimeout(timer.current);
    if (shown) return;
    // Keyboard focus and a warm group skip the delay and the animation.
    const g = groupRef.current;
    if (fromKeyboard || g.open > 0 || performance.now() < g.warmUntil) open(true);
    else timer.current = setTimeout(() => open(false), FIRST_DELAY);
  }
  function hide() {
    clearTimeout(timer.current);
    if (!shown) return;
    const g = groupRef.current;
    g.open = Math.max(0, g.open - 1);
    g.warmUntil = performance.now() + WARM_MS;
    setShown(null);
  }

  useEffect(() => () => clearTimeout(timer.current), []);
  // Place above the trigger (below when there is no room), clamped inside the viewport.
  useLayoutEffect(() => {
    const el = tip.current;
    if (!shown || !el) return;
    const { rect } = shown;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const below = rect.top - GAP - h < EDGE;
    const left = Math.min(Math.max(rect.left + rect.width / 2 - w / 2, EDGE), window.innerWidth - EDGE - w);
    el.style.left = `${left}px`;
    el.style.top = `${below ? rect.bottom + GAP : rect.top - GAP - h}px`;
    el.style.setProperty("--tip-origin", `${rect.left + rect.width / 2 - left}px ${below ? "0" : "100%"}`);
  }, [shown]);
  useEffect(() => {
    if (!shown) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && hide();
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", hide, true);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", hide, true);
    };
  });

  return (
    <span
      ref={anchor}
      className="bits-tip-anchor"
      onPointerEnter={(e) => e.pointerType === "mouse" && show(false)}
      onPointerLeave={hide}
      onPointerDown={hide}
      onFocus={(e) => e.target.matches(":focus-visible") && show(true)}
      onBlur={hide}
    >
      {children}
      {shown &&
        createPortal(
          <span ref={tip} className="bits-tip" data-instant={shown.instant ? "" : undefined} aria-hidden style={{ left: -9999, top: -9999 }}>
            {label}
            {shortcut && <span className="kbd">{shortcut}</span>}
          </span>,
          document.body,
        )}
    </span>
  );
}
