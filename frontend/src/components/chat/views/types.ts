// What `show_view` returns (backend/src/views/mod.rs): a view the server resolved and checked, with
// names and kinds from the database, plus `text`, the same answer as plain text.

export type EntityRef = { id: string; name: string; kind: string; vault: string };

/** One relation, in its stored direction. */
export type Step = { id: string; from: EntityRef; label: string; to: EntityRef };

export type Affected = { entity: EntityRef; path: Step[]; why: string };

/** `linked` is false when the start has no saved links at all, so there was nothing to follow. */
export type ImpactReach = { depth: number; kinds: string[]; labels: string[] };
export type ImpactPage = { reach: ImpactReach; page: number; page_size: number; total: number; has_more: boolean; truncated: boolean };
export type ImpactView = { view: "impact"; start: EntityRef; headline: string; affected: Affected[]; pagination?: ImpactPage; linked?: boolean; text: string };

export type SubgraphView = { view: "subgraph"; title: string; entities: EntityRef[]; relations: Step[]; text: string };

/** One run of a metric (`metric_eval`, backend/src/ontology/metric.rs): the server's own number. */
export type MetricRun = {
  metric: string;
  kind: string;
  vault: string;
  description: string | null;
  expr_words: string;
  value: number | null;
  unit: string | null;
  as_of: string;
  n_entities: number;
  entity_ids: string[];
  truncated: boolean;
  status: "proposed" | "approved" | "rejected";
  warnings: string[];
  metric_mapped?: { from: string; to: string };
  groups?: { group: string | number | boolean | null | { entity_id: string; name: string }; value: number | null; n_entities: number; entity_ids: string[]; truncated: boolean }[];
  series?: { period: string; value: number | null; n: number }[];
};

/** A metric's number, with the entities it came from as receipts. */
export type MetricView = { view: "metric"; metric: MetricRun; entities: EntityRef[]; text: string };

/** One metric in a what-if (`scenario_run`, backend/src/ontology/scenario.rs). */
export type ScenarioMetric = {
  metric: string;
  kind: string;
  expr_words: string;
  value: number | null;
  unit: string | null;
  n_entities: number;
  status: "proposed" | "approved" | "rejected";
};

/** A what-if, as the server ran it: nothing in it was saved. */
export type ScenarioRun = {
  vault: string;
  as_of: string;
  changes: string[];
  baseline: ScenarioMetric[];
  scenario: (ScenarioMetric & { delta: number | null; delta_pct: number | null })[];
  changed: { entity_id: string; name: string; kind: string; property: string; before: unknown; after: unknown; unit: string | null }[];
  n_changed: number;
  removed: { entity_id: string; name: string; kind: string }[];
  reached: { entity_id: string; name: string; kind: string; depth: number; via: string; from: string; path: string[] }[];
  reached_truncated: boolean;
  warnings: string[];
};

export type ScenarioView = { view: "scenario"; headline: string; result: ScenarioRun; text: string };

/** Any view, known to this app or not: an unknown one still has its text. */
export type ShownView = ImpactView | SubgraphView | MetricView | ScenarioView | { view: string; text: string };
