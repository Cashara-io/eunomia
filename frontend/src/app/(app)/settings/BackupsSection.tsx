"use client";

import { CopyField, PanelHead } from "./shared";

/* The backup service writes only to its own volume, which the backend never mounts, so this page has no
   evidence that a backup ran. It says so instead of claiming one did. */
export function BackupsSection() {
  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="Backups">
        Eunomia can&apos;t see the backup service from here, so it can&apos;t tell you when the last backup ran.
      </PanelHead>
      <div className="flex flex-col gap-2 max-w-[560px]">
        <span className="text-[14px] font-medium">To check the last backup, run this on the server</span>
        <CopyField value="docker compose logs backup" />
      </div>
      <p className="text-[13px] max-w-[68ch]" style={{ color: "var(--ink-dim)" }}>
        When it runs, the backup service saves an encrypted copy every night at 03:00, server time. Keep the backup key somewhere off
        this machine: without it the backups can&apos;t be read.
      </p>
      <a
        href="https://github.com/Cashara-io/eunomia/blob/main/INSTALLATION.md#backups"
        target="_blank"
        rel="noopener noreferrer"
        className="text-[13px] self-start"
        style={{ color: "var(--accent-text)" }}
      >
        How backups and restores work
      </a>
    </div>
  );
}
