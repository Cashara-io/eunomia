"use client";
import { useEffect, useRef } from "react";
import { useObjectProfile } from "@/lib/queries/objects";
import { SourceReceiptBody } from "../SourceInspector";
import ReadState from "../ReadState";
import { receiptState } from "../receipt-model";
import styles from "./strata.module.css";

const explanations: Record<string, string> = {
  stale: "The source changed after this value was recorded.",
  missing: "No value has been recorded.",
  conflicting: "Different values share the latest as-of time.",
  unavailable: "The source cannot be read in this space.",
};

/** The evidence plane: the exact source row for one value, with its honest state in words. */
export default function StrataReceipt({ space, id, name, close, inline }: { space: string; id: string; name: string; close: () => void; inline: boolean }) {
  const query = useObjectProfile(space, id);
  const heading = useRef<HTMLHeadingElement>(null);
  const loaded = !!query.data;
  useEffect(() => {
    if (loaded) heading.current?.focus();
  }, [loaded, name]);
  const property = query.data?.properties.find((p) => p.name === name);
  return (
    <aside
      className={styles.receipt}
      data-receipt
      data-inline={inline}
      aria-label="Source inspector"
      onKeyDown={(e) => {
        if (e.key !== "Escape") return;
        e.preventDefault();
        e.stopPropagation();
        close();
      }}
    >
      {query.isError ? (
        <ReadState error={query.error} retry={() => void query.refetch()} subject="this source" />
      ) : !query.data ? (
        <p role="status">Loading the source row…</p>
      ) : (
        <>
          <SourceReceiptBody data={query.data} name={name} space={space} close={close} heading={heading} />
          {property && property.state !== "recorded" && (
            <p className={styles.receiptState} data-state={property.state} role="note">
              {receiptState(property)}. {explanations[property.state]}
            </p>
          )}
          <p className={styles.receiptNote}>Approval of a definition does not certify this value.</p>
        </>
      )}
    </aside>
  );
}
