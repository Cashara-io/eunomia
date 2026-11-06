"use client";

import { CodeIcon } from "@phosphor-icons/react/ssr";
import Link from "next/link";
import EntityGraph from "@/components/EntityGraph";
import PageHeader from "@/components/ui/PageHeader";
import RefreshEverythingButton from "@/components/live/RefreshEverythingButton";

export default function CodePage() {
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
          <div className="w-full flex flex-col gap-4">
            <PageHeader icon={CodeIcon} tone="company" title="Code map" intro="Repositories, files and functions your coding apps have mapped, and how they connect." />
            <RefreshEverythingButton />
          </div>
        }
      />
    </div>
  );
}
