"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import {
  BookOpen,
  BrainCircuit,
  Code,
  Files,
  House,
  Layers,
  LogOut,
  Menu,
  MessagesSquare,
  Plug,
  Search,
  SlidersHorizontal,
  Users,
  X,
} from "lucide-react";
import EunomiaMark from "./EunomiaMark";
import { openCommandPalette } from "./CommandPalette";
import { useMe, useLogout } from "@/lib/queries/auth";
import { useSources } from "@/lib/queries/sources";
import { useUpdateStatus } from "@/lib/queries/settings";
import { useInvitations } from "@/lib/queries/vaults";
import { modKey } from "@/lib/platform";

const noSubscribe = () => () => {};
import { isLiveSource, sourceHealth, sourceLabel } from "@/lib/sourceState";

// Visible labels follow the design spec; the routes stay as they were.
export const MAIN_NAV = [
  { href: "/", label: "Home", icon: House },
  { href: "/chat", label: "Chat", icon: MessagesSquare },
  { href: "/entities", label: "People & topics", icon: Users },
  { href: "/vaults", label: "Memory spaces", icon: Layers },
  { href: "/documents", label: "Documents", icon: Files },
  { href: "/connectors", label: "Connected sources", icon: Plug },
  { href: "/docs", label: "Help", icon: BookOpen },
  { href: "/settings", label: "Settings", icon: SlidersHorizontal },
];

export const ADVANCED_NAV = [
  { href: "/skill", label: "AI instructions", icon: BrainCircuit },
  { href: "/code", label: "Code map", icon: Code },
];

function isActive(pathname: string, href: string) {
  // The connection guides (/connect/...) belong to Home; /connectors is its own page.
  return href === "/" ? pathname === "/" || pathname === "/connect" || pathname.startsWith("/connect/") : pathname === href || pathname.startsWith(`${href}/`);
}

function NavLink({ href, active, children }: { href: string; active: boolean; children: React.ReactNode }) {
  return (
    <Link
      href={href}
      aria-current={active ? "page" : undefined}
      className="nav-link flex items-center gap-3 min-h-9 px-3 text-[14px]"
      data-active={active || undefined}
    >
      {children}
    </Link>
  );
}

/** The wordmark: the mark plus "eunomia", cream on the ink sidebar, and "by Paperless" under it. */
function Brand() {
  return (
    <div className="flex flex-col gap-1.5 min-w-0">
      <Link href="/" className="flex items-center gap-2 min-w-0 rounded-[3px]" aria-label="Eunomia home">
        <EunomiaMark size={22} />
        <span className="text-[18px] font-semibold tracking-[-0.03em] truncate">eunomia</span>
      </Link>
      <a
        href="https://www.buildpaperless.com.au/eunomia"
        target="_blank"
        rel="noreferrer"
        className="flex items-center gap-1.5 self-start rounded-[3px] text-[12px] sidebar-muted hover:opacity-100 transition-opacity"
        aria-label="Made by Paperless (opens in a new tab)"
      >
        <span aria-hidden>by</span>
        {/* eslint-disable-next-line @next/next/no-img-element -- a fixed 560x192 PNG, nothing to optimise */}
        <img src="/paperless-logo-light.png" alt="" width={64} height={22} className="block h-[22px] w-auto" />
      </a>
    </div>
  );
}

export default function Sidebar() {
  const pathname = usePathname();
  const router = useRouter();
  const sourcesQuery = useSources();
  const rows = sourcesQuery.data ?? (sourcesQuery.isError ? [] : null);
  const me = useMe().data ?? null;
  const status = useUpdateStatus().data;
  // Only admins can act on an update (Settings > Updates is admin-only), so members don't see the prompt.
  const newVersion = me?.is_admin && status?.configured && status.update_available ? status.latest_version : null;
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

  const invites = useInvitations().data?.length ?? 0;
  const isAdmin = me?.is_admin ?? false;
  // The platform's shortcut key, read after hydration (the server can't know it).
  const mod = useSyncExternalStore(noSubscribe, modKey, () => "⌘" as const);
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
        <button onClick={openCommandPalette} className="sidebar-search w-full min-h-9 px-3 flex items-center gap-2 text-[13px] text-left">
          <Search size={18} strokeWidth={1.75} aria-hidden />
          <span className="flex-1">Search</span>
          <span className="kbd" aria-hidden>
            {mod === "⌘" ? "⌘K" : "Ctrl K"}
          </span>
          <span className="sr-only">({mod === "⌘" ? "Command" : "Control"} K)</span>
        </button>
      </div>

      <div className="px-4 flex flex-col gap-1">
        {MAIN_NAV.map(({ href, label, icon: Icon }) => (
          <NavLink key={href} href={href} active={isActive(pathname, href)}>
            <Icon size={18} strokeWidth={1.75} className="nav-icon shrink-0" aria-hidden />
            <span className="flex-1">{label}</span>
            {href === "/vaults" && invites > 0 && (
              <span className="nav-badge" aria-label={`${invites} invitation${invites === 1 ? "" : "s"}`}>
                {invites}
              </span>
            )}
          </NavLink>
        ))}
        {/* Developer tools: shown to admins, and to anyone already on one of these pages. */}
        {(isAdmin || ADVANCED_NAV.some(({ href }) => isActive(pathname, href))) && (
          <>
            <div className="caps sidebar-muted px-1 pt-4 pb-1.5" id="nav-advanced">
              Advanced
            </div>
            <div role="group" aria-labelledby="nav-advanced" className="flex flex-col gap-1">
              {ADVANCED_NAV.map(({ href, label, icon: Icon }) => (
                <NavLink key={href} href={href} active={isActive(pathname, href)}>
                  <Icon size={18} strokeWidth={1.75} className="nav-icon shrink-0" aria-hidden />
                  <span className="flex-1">{label}</span>
                </NavLink>
              ))}
            </div>
          </>
        )}
      </div>

      <div className="flex-1" />

      <div className="px-4 flex flex-col gap-2 text-[12px] leading-snug">
        {/* Server status is for whoever runs this Eunomia; members get a clean footer. */}
        {isAdmin && (
          <Link href="/connectors" className="sidebar-muted hover:underline underline-offset-2 rounded-[3px]">
            {sourceStatus}
          </Link>
        )}
        {!isAdmin ? null : !status?.configured ? (
          <Link href="/settings?tab=updates" className="sidebar-muted hover:underline underline-offset-2 rounded-[3px]">
            v{version.replace(/^v/, "")} · Automatic updates off
          </Link>
        ) : newVersion ? (
          <span className="sidebar-muted">
            v{version.replace(/^v/, "")} ·{" "}
            <Link href="/settings?tab=updates" className="underline underline-offset-2 rounded-[3px]" style={{ color: "var(--bauhaus-app-yellow)" }}>
              Update available · {newVersion}
            </Link>
          </span>
        ) : (
          <span className="sidebar-muted" title={status.checked_at ? `Checked ${new Date(status.checked_at).toLocaleString()}` : undefined}>
            v{version.replace(/^v/, "")} · Up to date
          </span>
        )}
        <div className="flex items-center gap-2 pt-2" style={{ borderTop: "1px solid var(--bauhaus-app-sidebar-line)" }}>
          <span className="flex-1 min-w-0 truncate" title={me?.email ?? undefined}>
            <span className="sr-only">Signed in as </span>
            {me?.email ?? ""}
          </span>
          <button
            onClick={logout}
            className="nav-link inline-flex items-center gap-1.5 min-h-9 px-2 text-[12px] shrink-0"
            aria-label="Log out"
          >
            <LogOut size={16} strokeWidth={1.75} aria-hidden />
            Log out
          </button>
        </div>
      </div>
    </>
  );

  return (
    <>
      {/* Narrow screens: a top bar with the menu button; the sidebar becomes a drawer. */}
      <div className="sidebar md:hidden sticky top-0 z-40 flex items-center gap-2 px-3 h-14 shrink-0">
        <button
          ref={menuButton}
          onClick={() => setDrawerPath(pathname)}
          aria-label="Open menu"
          aria-expanded={drawerOpen}
          aria-controls="nav-drawer"
          className="nav-link inline-flex items-center justify-center w-11 h-11"
        >
          <Menu size={22} aria-hidden />
        </button>
        <Brand />
        <button onClick={openCommandPalette} aria-label="Search" className="nav-link inline-flex items-center justify-center w-11 h-11 ml-auto">
          <Search size={20} aria-hidden />
        </button>
      </div>

      {/* Desktop sidebar: 232px, ink. */}
      <nav aria-label="Main" className="sidebar hidden md:flex w-[232px] shrink-0 flex-col gap-6 py-6 sticky top-0 h-screen overflow-y-auto">
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
              <button ref={closeButton} onClick={() => setDrawerPath(null)} aria-label="Close menu" className="nav-link inline-flex items-center justify-center w-11 h-11">
                <X size={20} aria-hidden />
              </button>
            </div>
            {navBody}
          </nav>
        </div>
      )}
    </>
  );
}
