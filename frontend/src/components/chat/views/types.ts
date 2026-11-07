// What `show_view` returns (backend/src/views/mod.rs): a view the server resolved and checked, with
// names and kinds from the database, plus `text`, the same answer as plain text.

export type EntityRef = { id: string; name: string; kind: string; vault: string };

/** One relation, in its stored direction. */
export type Step = { id: string; from: EntityRef; label: string; to: EntityRef };

export type Affected = { entity: EntityRef; path: Step[]; why: string };

export type ImpactView = { view: "impact"; start: EntityRef; headline: string; affected: Affected[]; text: string };

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
  series?: { period: string; value: number | null; n: number }[];
};

/** A metric's number, with the entities it came from as receipts. */
export type MetricView = { view: "metric"; metric: MetricRun; entities: EntityRef[]; text: string };

/** Any view, known to this app or not: an unknown one still has its text. */
export type ShownView = ImpactView | SubgraphView | MetricView | { view: string; text: string };
