"use client";

import Link from "next/link";
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useOntology } from "@/lib/queries/ontology";
import type { Vault } from "@/lib/types";
import { definitions, twinHref } from "@/components/twin/model";

/** Compatibility entry point for Memory spaces and its old section anchors. */
export default function OntologySection({ vault }: { vault: Vault }) {
  const query = useOntology(vault.id);
  const router = useRouter();
  useEffect(() => {
    const redirect = () => {
      if (
        ["#ontology", "#kinds-and-links", "#kinds", "#links"].includes(
          window.location.hash,
        )
      ) {
        router.replace(twinHref("/ontology/schema", vault.id));
      }
    };
    redirect();
    window.addEventListener("hashchange", redirect);
    return () => window.removeEventListener("hashchange", redirect);
  }, [router, vault.id]);
  const data = query.data;
  const waiting = data
    ? definitions(data).filter((d) => d.item.status === "proposed").length
    : null;
  return (
    <section className="flex flex-col gap-2" aria-label="Ontology">
      <h3 className="section-title">Ontology</h3>
      <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
        {data
          ? `${data.kinds.filter((k) => !k.built_in || k.entities > 0).length} kinds, ${waiting} suggestions to review.`
          : query.isError
            ? "The summary could not be loaded."
            : "Loading summary…"}{" "}
        <Link className="link" href={twinHref("/ontology", vault.id)}>
          Open Ontology
        </Link>
      </p>
    </section>
  );
}
