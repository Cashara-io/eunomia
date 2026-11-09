"use client";
import dynamic from "next/dynamic";
const UiView = dynamic(() => import("../ui/UiView"), { loading: () => <p>Loading analysis...</p> });
// The chat's generative UI: `show_view` results drawn by view name. A view this app does not know yet
// falls back to the plain text the server sends with every view.
import type { ComponentType } from "react";
import ImpactView from "./ImpactView";
import MetricView from "./MetricView";
import ScenarioView from "./ScenarioView";
import SubgraphView from "./SubgraphView";
import type { ImpactView as Impact, MetricView as Metric, ScenarioView as Scenario, ShownView, SubgraphView as Subgraph } from "./types";

export const SHOW_VIEW = "show_view";

type Known = Impact | Subgraph | Metric | Scenario;

const VIEWS: { [K in Known["view"]]: ComponentType<{ view: Extract<Known, { view: K }> }> } = {
  impact: ImpactView,
  subgraph: SubgraphView,
  metric: MetricView,
  scenario: ScenarioView,
};

/** A `show_view` result as stored or streamed, or null when it is not a view (an error, bad JSON). */
export function parseView(result: string | undefined): ShownView | null {
  if (!result) return null;
  try {
    const v = JSON.parse(result);
    return v && typeof v.view === "string" && typeof v.text === "string" ? (v as ShownView) : null;
  } catch {
    return null;
  }
}

export default function ChatView({ view }: { view: ShownView }) {
  if (view.view === "ui") return <UiView view={view} />;
  // the server checked the view against the same catalogue, so its name picks its shape
  const View = (VIEWS as Record<string, ComponentType<{ view: ShownView }> | undefined>)[view.view];
  if (View) return <View view={view} />;
  return <pre className="tool-pre whitespace-pre-wrap">{view.text}</pre>;
}
