"use client";

import { ListIcon, MagnifyingGlassIcon, SignOutIcon, XIcon } from "@phosphor-icons/react/ssr";
import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import EunomiaMark from "./EunomiaMark";
import ThemeToggle from "./ThemeToggle";
import { openCommandPalette } from "./CommandPalette";
import { useMe, useLogout } from "@/lib/queries/auth";
import { useSources } from "@/lib/queries/sources";
import { useUpdateStatus } from "@/lib/queries/settings";
import { isLiveSource, sourceHealth, sourceLabel } from "@/lib/sourceState";

// Visible labels follow the design spec; the routes stay as they were. Each
// section has its own shape glyph (see .nav-glyph in globals.css).
export type NavGlyph = "home" | "chat" | "people" | "spaces" | "docs" | "sources" | "help" | "settings" | "instructions" | "code";

export const MAIN_NAV: { href: string; label: string; glyph: NavGlyph }[] = [
  { href: "/", label: "Home", glyph: "home" },
  { href: "/chat", label: "Chat", glyph: "chat" },
  { href: "/entities", label: "People & topics", glyph: "people" },
  { href: "/vaults", label: "Memory spaces", glyph: "spaces" },
  { href: "/documents", label: "Documents", glyph: "docs" },
  { href: "/connectors", label: "Connected sources", glyph: "sources" },
  { href: "/docs", label: "Help", glyph: "help" },
  { href: "/settings", label: "Settings", glyph: "settings" },
];

export const ADVANCED_NAV: { href: string; label: string; glyph: NavGlyph }[] = [
  { href: "/skill", label: "AI instructions", glyph: "instructions" },
  { href: "/code", label: "Code map", glyph: "code" },
];

/** A section's glyph: three flat shapes that rearrange on hover. */
export function NavGlyphShapes({ glyph }: { glyph: NavGlyph }) {
  return (
    <span className="nav-glyph" data-glyph={glyph} aria-hidden>
      <span className="g-c" />
      <span className="g-s" />
      <span className="g-t" />
    </span>
  );
}

function isActive(pathname: string, href: string) {
  // The connection guides (/connect/...) belong to Home.
  return href === "/" ? pathname === "/" || pathname.startsWith("/connect/") : pathname === href || pathname.startsWith(`${href}/`);
}

function NavChip({ href, label, glyph, active }: { href: string; label: string; glyph: NavGlyph; active: boolean }) {
  return (
    <Link href={href} aria-current={active ? "page" : undefined} className="nav-chip" data-active={active || undefined}>
      <NavGlyphShapes glyph={glyph} />
      <span className="flex-1 truncate">{label}</span>
    </Link>
  );
}

/** The wordmark: the shape trio plus "eunomia". */
function Brand() {
  return (
    <Link href="/" className="flex items-center gap-2.5 min-w-0 rounded-[4px]" aria-label="Eunomia home">
      <EunomiaMark size={20} />
      <span className="text-[18px] font-semibold tracking-[-0.03em] truncate">eunomia</span>
    </Link>
  );
}

export default function Sidebar() {
  const pathname = usePathname();
  const router = useRouter();
  const sourcesQuery = useSources();
  const rows = sourcesQuery.data ?? (sourcesQuery.isError ? [] : null);
  const me = useMe().data ?? null;
  const status = useUpdateStatus().data;
  const newVersion = status?.configured && status.update_available ? status.latest_version : null;
  const logoutMutation = useLogout();
  // The drawer is open only for the page it was opened on, so navigating
  // closes it without an effect.
  const [drawerPath, setDrawerPath] = useState<string | null>(null);
  const drawerOpen = drawerPath === pathname;
  const menuButton = useRef<HTMLButtonElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const wasOpen = useRef(false);

  // Focus moves into the drawer when it opens and back to the menu button when it closes; Escape closes it.
  useEffect(() => {
    if (drawerOpen) {
      wasOpen.current = true;
      closeButton.current?.focus();
      const onKey = (e: KeyboardEvent) => {
        if (e.key === "Escape") setDrawerPath(null);
      };
      window.addEventListener("keydown", onKey);
      return () => window.removeEventListener("keydown", onKey);
    }
    if (wasOpen.current) {
      wasOpen.current = false;
      menuButton.current?.focus();
    }
  }, [drawerOpen]);

  const connected = rows?.filter(isLiveSource) ?? [];
  const failing = connected.filter((s) => sourceHealth(s).label === "Failing");

  async function logout() {
    await logoutMutation.mutateAsync().catch(() => {});
    router.replace("/login");
  }

  // Source status, written in words.
  const sourceStatus =
    rows === null
      ? "Checking sources…"
      : failing.length > 0
        ? `Source: ${sourceLabel(failing[0])} needs attention`
        : connected.length === 0
          ? "No sources connected"
          : connected.length === 1
            ? `Source: ${sourceLabel(connected[0])} connected`
            : `Sources: ${connected.length} connected`;

  const version = process.env.NEXT_PUBLIC_APP_VERSION ?? "dev";

  const navBody = (
    <>
      <div className="px-4">
        <button onClick={openCommandPalette} className="sidebar-search w-full min-h-9 px-3 flex items-center gap-2 text-[13.5px] text-left">
          <MagnifyingGlassIcon size={16} weight="duotone" className="duo" style={{ ["--duo" as string]: "var(--vault-project)" }} aria-hidden />
          <span className="flex-1">Search</span>
          <span className="kbd" aria-hidden>
            ⌘K
          </span>
          <span className="sr-only">(Command K)</span>
        </button>
      </div>

      <div className="px-4 flex flex-col gap-1">
        {MAIN_NAV.map((n) => (
          <NavChip key={n.href} {...n} active={isActive(pathname, n.href)} />
        ))}
        <div className="caps px-3 pt-4 pb-1" id="nav-advanced">
          Advanced
        </div>
        <div role="group" aria-labelledby="nav-advanced" className="flex flex-col gap-1">
          {ADVANCED_NAV.map((n) => (
            <NavChip key={n.href} {...n} active={isActive(pathname, n.href)} />
          ))}
        </div>
      </div>

      <div className="flex-1" />

      <div className="px-4 flex flex-col gap-2 text-[12.5px] leading-snug">
        <Link href="/connectors" className="sidebar-muted hover:underline underline-offset-2 rounded-[4px]">
          {sourceStatus}
        </Link>
        {newVersion ? (
          <span className="sidebar-muted">
            v{version.replace(/^v/, "")} ·{" "}
            <Link href="/settings?tab=updates" className="underline underline-offset-2 rounded-[4px] font-medium" style={{ color: "var(--ink)" }}>
              Update available · {newVersion}
            </Link>
          </span>
        ) : (
          <span className="sidebar-muted">v{version.replace(/^v/, "")} · Up to date</span>
        )}
        <div className="flex items-center gap-2 pt-2" style={{ borderTop: "1px solid var(--border)" }}>
          <span className="flex-1 min-w-0 truncate" title={me?.email ?? undefined}>
            <span className="sr-only">Signed in as </span>
            {me?.email ?? ""}
          </span>
          <ThemeToggle />
          <button onClick={logout} className="side-button min-h-9 px-2.5 text-[12.5px] shrink-0" aria-label="Log out">
            <SignOutIcon size={16} weight="duotone" className="duo" style={{ ["--duo" as string]: "var(--vault-personal)" }} aria-hidden />
            Log out
          </button>
        </div>
      </div>
    </>
  );

  return (
    <>
      {/* Narrow screens: a top bar with the menu button; the sidebar becomes a drawer. */}
      <div className="sidebar md:hidden sticky top-0 z-40 flex items-center gap-2 px-3 h-14 shrink-0" style={{ borderBottom: "1px solid var(--border)" }}>
        <button
          ref={menuButton}
          onClick={() => setDrawerPath(pathname)}
          aria-label="Open menu"
          aria-expanded={drawerOpen}
          aria-controls="nav-drawer"
          className="side-button w-11 h-11"
        >
          <ListIcon weight="duotone" size={22} aria-hidden />
        </button>
        <Brand />
        <button onClick={openCommandPalette} aria-label="Search" className="side-button w-11 h-11 ml-auto">
          <MagnifyingGlassIcon size={20} weight="duotone" className="duo" style={{ ["--duo" as string]: "var(--vault-project)" }} aria-hidden />
        </button>
      </div>

      {/* Desktop sidebar: 232px, white, with a right hairline. */}
      <nav aria-label="Main" className="sidebar sidebar-rail hidden md:flex w-[232px] shrink-0 flex-col gap-6 py-6 sticky top-0 h-screen overflow-y-auto">
        <div className="px-4">
          <Brand />
        </div>
        {navBody}
      </nav>

      {drawerOpen && (
        <div className="md:hidden fixed inset-0 z-50 flex fade-in" style={{ background: "var(--scrim)" }} onClick={() => setDrawerPath(null)}>
          <nav
            id="nav-drawer"
            aria-label="Main"
            role="dialog"
            aria-modal="true"
            className="sidebar w-[280px] max-w-[85vw] h-full flex flex-col gap-6 py-4 drawer-in overflow-y-auto"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="pl-4 pr-2 flex items-center justify-between">
              <Brand />
              <button ref={closeButton} onClick={() => setDrawerPath(null)} aria-label="Close menu" className="side-button w-11 h-11">
                <XIcon weight="duotone" size={20} aria-hidden />
              </button>
            </div>
            {navBody}
          </nav>
        </div>
      )}
    </>
  );
}
