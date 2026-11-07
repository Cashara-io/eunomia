"use client";

import Link from "next/link";
import { CodeIcon } from "@phosphor-icons/react/ssr";
import EntityGraph from "@/components/EntityGraph";
import PageHeader from "@/components/ui/PageHeader";
import LiveStatusBadge from "@/components/live/LiveStatusBadge";

export default function CodePage() {
  return (
    <div className="relative -mx-4 -my-6 sm:-mx-8 lg:-mx-12 lg:-my-[38px] h-[calc(100dvh-3.5rem)] md:h-dvh">
      <EntityGraph
        kinds={["repository", "file", "symbol"]}
        guide={{
          text: "Coding apps such as Claude Code and Codex add repositories, files and functions here as they work. Nothing to set up: they learn how from the AI instructions.",
          action: (
            <Link href="/skill" className="btn btn-sm">
              Open AI instructions
            </Link>
          ),
        }}
        header={
          <div className="w-full">
            <PageHeader
              icon={CodeIcon}
              title="Code map"
              intro="Repositories, files and functions your coding apps have mapped."
              // no Refresh button: the map refreshes on its own; the badge shows only when offline
              actions={<LiveStatusBadge />}
            />
          </div>
        }
      />
    </div>
  );
}
