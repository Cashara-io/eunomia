// What `show_view` returns (backend/src/views/mod.rs): a view the server resolved and checked, with
// names and kinds from the database, plus `text`, the same answer as plain text.

export type EntityRef = { id: string; name: string; kind: string; vault: string };

/** One relation, in its stored direction. */
export type Step = { id: string; from: EntityRef; label: string; to: EntityRef };

export type Affected = { entity: EntityRef; path: Step[]; why: string };

export type ImpactView = { view: "impact"; start: EntityRef; headline: string; affected: Affected[]; text: string };

export type SubgraphView = { view: "subgraph"; title: string; entities: EntityRef[]; relations: Step[]; text: string };

/** Any view, known to this app or not: an unknown one still has its text. */
export type ShownView = ImpactView | SubgraphView | { view: string; text: string };
