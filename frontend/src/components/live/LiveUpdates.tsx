"use client";

import { useLiveUpdates } from "@/lib/queries/live";

/** Keeps open pages in step with changes made elsewhere (an AI agent, chat, another tab). Renders nothing; mount once in the signed-in layout. */
export default function LiveUpdates() {
  useLiveUpdates();
  return null;
}
