"use client";

import Link from "next/link";
import { useState } from "react";
import { Code, RefreshCw } from "lucide-react";
import EntityGraph from "@/components/EntityGraph";
import PageHeader from "@/components/ui/PageHeader";
import LiveStatusBadge from "@/components/live/LiveStatusBadge";
import { useRefreshEverything } from "@/lib/queries/live";

export default function CodePage() {
  const refresh = useRefreshEverything();
  const [busy, setBusy] = useState(false);
  return (
    <div className="relative -mx-4 -my-6 sm:-mx-8 lg:-mx-12 lg:-my-[38px] h-[calc(100dvh-3.5rem)] md:h-dvh">
      <EntityGraph
        kinds={["repository", "file", "symbol"]}
        guide={
          <>
            <p>
              Coding apps such as Claude Code and Codex add repositories, files and functions here as they work. Nothing to set up: they learn
              how from the AI instructions.
            </p>
            <Link href="/skill" className="btn btn-sm">
              Open AI instructions
            </Link>
          </>
        }
        header={
          <div className="w-full">
            <PageHeader
              icon={Code}
              title="Code map"
              intro="Repositories, files and functions your coding apps have mapped."
              actions={
                <>
                  <LiveStatusBadge />
                  <button
                    type="button"
                    className="btn btn-ghost btn-sm btn-icon"
                    aria-label="Refresh code map"
                    title="Refresh"
                    disabled={busy}
                    onClick={async () => {
                      setBusy(true);
                      try {
                        await refresh();
                      } finally {
                        setBusy(false);
                      }
                    }}
                  >
                    <RefreshCw size={16} strokeWidth={1.75} className={busy ? "animate-spin" : undefined} aria-hidden />
                  </button>
                </>
              }
            />
          </div>
        }
      />
    </div>
  );
}
