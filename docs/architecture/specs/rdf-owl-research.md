# RDF/OWL repos for Eunomia: import/export, SHACL and blast radius

Research for the Eunomia Phase 3 ontology work. Checked with `gh api` on 31 Oct 2026 (Melbourne time). Stars and last-push dates are from GitHub on that day; push dates are shown in Melbourne time (AEDT).

Eunomia context: Rust (axum), SurrealDB 3.3.5 as the ontology and graph store, ClickHouse for events, MCP and REST, PolyForm Noncommercial (source available).

## Licence key

- **Safe to embed:** MIT, Apache-2.0, BSD, MIT-0, W3C Software licence. Keep the notices.
- **Care needed:** LGPL. Fine to call as a separate process or dynamically link, but static linking into a Rust binary (the normal Rust model) pulls LGPL obligations onto the combined binary (users must be able to relink). Treat LGPL Rust crates as "run as a sidecar or avoid".
- **Risky:** GPL/AGPL. Don't embed. Only use as a separate, unmodified tool, or reference only.
- **Commercial/closed:** reference only.
- **Vocabularies:** check data licence separately (most below are MIT, Apache or CC-BY, all fine to ship as import packs with attribution).

Usage key: **Depend** (add as a dependency), **Sidecar** (run as a separate process or container), **Borrow** (copy the idea, not the code), **Reference** (read only).

## 0. The reference point

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [fabio-rovai/open-ontologies](https://github.com/fabio-rovai/open-ontologies) | Terraform-style plan, apply, monitor, drift and rollback for production ontologies, with blast radius, risk score, locked IRIs, OWL RL consequence diff and Lean 4 checked proof certificates; ships as an MCP server | Rust | MIT | 2,485 | 31 Oct 2026 | **Borrow** heavily. Its stack (oxigraph 0.5 + `rmcp` MCP crate + tokio) matches ours, so code is also MIT-liftable with attribution |

Notable: open-ontologies builds on **oxigraph 0.5** and the official Rust MCP SDK **rmcp**, which is a good signal that oxigraph is the right Rust base for us too.

## 1. Rust RDF/OWL libraries

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [oxigraph/oxigraph](https://github.com/oxigraph/oxigraph) | SPARQL 1.1/1.2 graph DB plus standalone crates: `oxrdf` (model), `oxttl` (Turtle, TriG, N-Triples, N-Quads, N3), `oxrdfxml`, `oxjsonld`, `oxrdfio` (one parser/serializer API), `spargebra` (SPARQL parser) | Rust | Apache-2.0 / MIT | 2,015 | 24 Oct 2026 | **Depend.** Use `oxrdfio` for import/export parsing and serialising; optionally the in-memory `Store` as a staging graph for SPARQL checks during import |
| [pchampin/sophia_rs](https://github.com/pchampin/sophia_rs) | Generic RDF toolkit with trait-based graph/dataset abstractions, JSON-LD, canonicalisation (RDFC-1.0) | Rust | Apache-2.0 / CeCILL-B | 332 | 30 Sep 2026 | **Borrow/optional.** Nice traits, and RDF canonicalisation is handy for stable diffs and hashes; oxigraph has the bigger ecosystem |
| [phillord/horned-owl](https://github.com/phillord/horned-owl) | OWL 2 model and parsers (OWL/XML, RDF/XML, Functional, Manchester) | Rust | LGPL-3.0 | 111 | 20 Oct 2026 | **Care: LGPL.** Best Rust OWL axiom model, but static linking makes it awkward. Use as a sidecar CLI or **Borrow** the axiom model design; map OWL from triples ourselves |
| [oxigraph/rio](https://github.com/oxigraph/rio) | Streaming RDF parsers | Rust | Apache-2.0 / MIT | 87 | 11 Apr 2026 | **Archived.** Superseded by `oxttl`/`oxrdfxml`/`oxrdfio` inside oxigraph. Don't start on rio |
| [rudof-project/rudof](https://github.com/rudof-project/rudof) | SHACL and ShEx validation, DCTAP, PGSchema, conversions between data modelling formalisms; library, CLI, Python bindings | Rust | Apache-2.0 / MIT | 132 | 30 Oct 2026 | **Depend** for SHACL (`shacl_validation` crate). Also its property-graph schema support is relevant to SurrealDB |
| [ensaremirerol/shacl-rust](https://github.com/ensaremirerol/shacl-rust) | Small SHACL validator | Rust | MIT | 5 | 29 Sep 2026 | **Reference.** Too young; fallback only |
| [knowsys/nemo](https://github.com/knowsys/nemo) | Fast in-memory Datalog rule engine with RDF import/export | Rust | Apache-2.0 / MIT | 310 | 21 Oct 2026 | **Borrow/optional.** Good for rule-based inference and "what derives from what" if we outgrow OWL RL |

## 2. Reasoners

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [gtfierro/reasonable](https://github.com/gtfierro/reasonable) | OWL 2 RL reasoner built on Datafrog (Datalog), very fast on RDF graphs; Rust crate and Python bindings | Rust | BSD-3-Clause | 138 | 26 Oct 2026 | **Depend.** Materialise OWL RL consequences on import and compute "new consequences" in schema plans, like open-ontologies does |
| [INCATools/whelk-rs](https://github.com/INCATools/whelk-rs) | OWL EL reasoner (classification) | Rust | MIT | 24 | 30 Jun 2026 | **Optional depend.** For large EL ontologies (GS1, FIBO subsets); smaller community |
| [INCATools/whelk](https://github.com/INCATools/whelk) | Original Scala Whelk EL reasoner | Scala | BSD-3-Clause | 47 | 24 Oct 2026 | Reference |
| [liveontologies/elk-reasoner](https://github.com/liveontologies/elk-reasoner) | Fast OWL 2 EL reasoner | Java | Apache-2.0 | 129 | 14 Jun 2026 | **Sidecar** via ROBOT if needed |
| [phillord/hermit-reasoner](https://github.com/phillord/hermit-reasoner) | GitHub mirror of HermiT, the full OWL 2 DL tableau reasoner (Oxford) | Java | LGPL-3.0 (no GitHub licence file in mirror) | 85 | 9 Mar 2021 (mirror, stale) | **Reference / sidecar only.** Gold standard for DL consistency checks; use via ROBOT in CI, never embed |
| [RDFLib/OWL-RL](https://github.com/RDFLib/OWL-RL) | OWL 2 RL closure on top of rdflib | Python | W3C Software licence | 181 | 30 Oct 2026 | **Reference.** Good test oracle against `reasonable` |
| [owlcs/owlapi](https://github.com/owlcs/owlapi) | The Java OWL API that HermiT, ELK, ROBOT, Protégé build on | Java | LGPL-3.0 or Apache-2.0 (dual) | 944 | 21 Oct 2026 | Reference |

## 3. SHACL validators

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [rudof-project/rudof](https://github.com/rudof-project/rudof) | See above | Rust | Apache-2.0 / MIT | 132 | 30 Oct 2026 | **Depend** (primary) |
| [RDFLib/pySHACL](https://github.com/RDFLib/pySHACL) | Mature SHACL Core + SHACL-AF validator, with optional RDFS/OWL RL pre-inference | Python | Apache-2.0 | 364 | 29 Oct 2026 | **CI oracle.** Run W3C SHACL test suite and our shape fixtures against both rudof and pySHACL; flag disagreements |
| [TopQuadrant/shacl](https://github.com/TopQuadrant/shacl) | Reference SHACL implementation on Jena (TopBraid) incl. SHACL-AF rules | Java | Apache-2.0 | 246 | 1 Jul 2026 | Reference / CI oracle |
| [zazuko/rdf-validate-shacl](https://github.com/zazuko/rdf-validate-shacl) | SHACL validation in pure JS on RDF/JS | TypeScript | MIT (per package.json) | 126 | 3 Apr 2026 | **Optional depend** in the Next.js frontend for instant form/import previews |

## 4. Python and JS tooling

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [RDFLib/rdflib](https://github.com/RDFLib/rdflib) | The standard Python RDF library: parsers, serialisers, SPARQL | Python | BSD-3-Clause | 2,542 | 29 Oct 2026 | Reference; test fixtures and round-trip oracle |
| [oxigraph/oxrdflib](https://github.com/oxigraph/oxrdflib) | rdflib store backed by pyoxigraph | Python | BSD-3-Clause | 56 | 17 Oct 2026 | Reference |
| [pwin/owlready2](https://github.com/pwin/owlready2) | Mirror of Owlready2: ontology-oriented Python, bundles HermiT/Pellet | Python | LGPL-3.0 | 218 | 30 Jan 2023 (mirror; upstream is on Bitbucket/PyPI) | Reference only |
| [rdfjs/N3.js](https://github.com/rdfjs/N3.js) | Fast streaming Turtle/TriG/N-Triples/N3 parser and writer, in-memory store | JavaScript | MIT | 797 | 26 Oct 2026 | **Depend** in frontend (preview of uploaded `.ttl`, export download) and in the TypeScript helper container if we put mapping there |
| [rdf-ext/rdf-ext](https://github.com/rdf-ext/rdf-ext) | Developer-friendly RDF/JS factory, datasets, I/O | JavaScript | MIT | 126 | 15 Sep 2026 | Optional |
| [comunica/comunica](https://github.com/comunica/comunica) | Modular SPARQL/GraphQL-LD query engine over heterogeneous sources (files, endpoints, Solid) | TypeScript | MIT | 620 | 31 Oct 2026 | **Borrow** federated-query idea; the team has an ESWC 2026 "Comunica MCP SPARQL" demo ([poster](https://github.com/rubensworks/poster-eswc2026-demo-comunica-mcp)) |
| [DerwenAI/kglab](https://github.com/DerwenAI/kglab) | Python abstraction for knowledge graphs over rdflib, pySHACL, NetworkX | Jupyter | MIT | 695 | 26 Jan 2026 | Reference (graph analytics patterns) |

## 5. Triple stores and graph DBs with RDF support

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [apache/jena](https://github.com/apache/jena) | Java RDF framework, TDB2 store, Fuseki SPARQL server, SHACL, ShEx | Java | Apache-2.0 | 1,491 | 28 Oct 2026 | **Sidecar for testing** (Fuseki as an external SPARQL endpoint to test export interop) |
| [eclipse-rdf4j/rdf4j](https://github.com/eclipse-rdf4j/rdf4j) | Java RDF framework; base of GraphDB | Java | BSD-3-Clause | 423 | 31 Oct 2026 | Reference |
| [ad-freiburg/qlever](https://github.com/ad-freiburg/qlever) | Very fast SPARQL engine, scales to 1T+ triples on one machine | C++ | Apache-2.0 | 934 | 31 Oct 2026 | Reference; only relevant if a customer brings Wikidata-scale RDF |
| [qlever-dev/qlever-control](https://github.com/qlever-dev/qlever-control) | CLI to run QLever | Python | Apache-2.0 | 80 | 28 Oct 2026 | Reference |
| [neo4j-labs/neosemantics](https://github.com/neo4j-labs/neosemantics) | n10s: RDF import/export for Neo4j, OWL/RDFS mapping onto labels and relationships, SHACL validation, model mapping | Java | Apache-2.0 | 971 | 10 Oct 2026 | **Borrow** (closest analogue to what we're building: RDF into a labelled property graph). Copy its config knobs: `handleVocabUris` (SHORTEN/KEEP/IGNORE/MAP), `handleMultival`, `handleRDFTypes`, `keepLangTag`, explicit mapping table |
| [neo4j-contrib/mcp-neo4j](https://github.com/neo4j-contrib/mcp-neo4j) | MCP servers for Neo4j (Cypher, memory, data modelling) | Python | MIT | 989 | 10 Sep 2026 | Borrow the "data modelling" MCP tool shape for our ontology MCP tools |
| [blazegraph/database](https://github.com/blazegraph/database) | Blazegraph RDF DB (old Wikidata backend) | Java | GPL-2.0 | 987 | 17 Apr 2023 | **Archived and GPL.** Avoid |
| GraphDB (Ontotext) [graphdb.js driver](https://github.com/Ontotext-AD/graphdb.js) | Commercial RDF DB; driver is Apache-2.0 | JS | Apache-2.0 (driver) / proprietary (DB) | 40 | 30 Oct 2026 | Reference only (export target interop) |
| Stardog [stardog-examples](https://github.com/stardog-union/stardog-examples) | Commercial knowledge graph platform; examples repo | JS | Apache-2.0 (examples) / proprietary (DB) | 86 | 20 Oct 2026 | Reference only. Its "virtual graph" and "impact analysis" product features are worth reading as competitor UX |

## 6. Ontology change, diff and impact tools

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [ontodev/robot](https://github.com/ontodev/robot) | OBO ontology CLI: `diff`, `report` (QC rules), `reason`, `validate-profile`, `merge`, `extract` (modules), `template`, `verify` (SPARQL checks) | Java | BSD-3-Clause | 329 | 31 Oct 2026 | **Sidecar in CI** for imported vocabularies. **Borrow** `robot report` rule set (missing labels, dangling refs, duplicate labels) and `extract` (MIREOT/SLME modules) for "import only what you use" |
| [INCATools/ontology-development-kit](https://github.com/INCATools/ontology-development-kit) | ODK: Docker + Makefile lifecycle for ontologies (release pipeline, QC, imports refresh) | Dockerfile | BSD-3-Clause | 387 | 31 Oct 2026 | **Borrow** release discipline (versioned IRIs, import refresh, QC gates) for vocabulary packs |
| [balhoff/owl-diff](https://github.com/balhoff/owl-diff) | OWL axiom-level diff library (used behind ROBOT diff output) | Scala | BSD-3-Clause | 2 | 7 Oct 2026 | Borrow approach: diff on axioms, not text |
| [protegeproject/owl-diff-engine](https://github.com/protegeproject/owl-diff-engine) | Protégé's engine for OWL structural diffs incl. renames | Java | BSD-2-Clause | 12 | 13 Oct 2020 | Reference (rename detection heuristics) |
| [cmungall/bubastis](https://github.com/cmungall/bubastis) | Mirror of EBI's Bubastis OWL diff (logical changes per class) | Java | none stated | 0 | 29 Oct 2013 | Reference only; stale, use ROBOT diff instead |
| [rsgoncalves/ecco](https://github.com/rsgoncalves/ecco) | OWL 2 diff that separates effectual vs ineffectual changes (entailment-aware) | Java | LGPL-3.0 | 20 | 5 Jul 2017 | **Borrow idea only.** The effectual/ineffectual split is exactly the "blast radius 0 but 901 new consequences" insight open-ontologies sells |

## 7. Relational / JSON to RDF mapping

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [morph-kgc/morph-kgc](https://github.com/morph-kgc/morph-kgc) | RML/R2RML/YARRRML engine; CSV, JSON, SQL, Parquet to RDF; outputs to files, rdflib, Oxigraph, Kafka | Python | Apache-2.0 | 300 | 15 Oct 2026 | **Sidecar** option or **Borrow** the mapping model |
| [RMLio/rmlmapper-java](https://github.com/RMLio/rmlmapper-java) | Reference RML engine | Java | MIT | 205 | 18 Feb 2026 | Reference / conformance |
| [RMLio/yarrrml-parser](https://github.com/RMLio/yarrrml-parser) | YARRRML (human-friendly YAML for RML) to RML | JavaScript | MIT | 58 | 12 Jun 2026 | **Borrow:** YARRRML-style YAML is a good user-facing mapping format for connector-to-ontology mappings (Square, CSV) |
| [kg-construct/rml-core](https://github.com/kg-construct/rml-core) | W3C CG spec for RML-Core | spec | CC-BY-4.0 | 17 | 19 Oct 2026 | Reference spec |
| [ontop/ontop](https://github.com/ontop/ontop) | Virtual knowledge graphs: SPARQL over relational DBs via R2RML/OBDA mappings, no copy | Java | Apache-2.0 | 1,008 | 15 Oct 2026 | **Borrow** "virtual" idea for ClickHouse: expose events as ontology-linked views without materialising triples |

## 8. MCP servers for RDF/SPARQL/ontologies

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [fabio-rovai/open-ontologies](https://github.com/fabio-rovai/open-ontologies) | See section 0 | Rust | MIT | 2,485 | 31 Oct 2026 | **Optional MCP add-on** for customers with OWL vocabularies, plus Borrow |
| [emekaokoye/mcp-rdf-explorer](https://github.com/emekaokoye/mcp-rdf-explorer) | MCP server for conversational exploration of RDF (local Turtle or SPARQL endpoint), full-text search, graph stats | Python | MIT | 56 | 30 Oct 2026 | Borrow tool list (describe class, sample instances, stats) |
| [tmdaidevs/ontology-mcp-server](https://github.com/tmdaidevs/ontology-mcp-server) | Ontology MCP server | Python | none stated | 26 | 23 Apr 2026 | Reference only (no licence) |
| [aws-samples/sample-industrial-ontology-mcp-server](https://github.com/aws-samples/sample-industrial-ontology-mcp-server) | Turns tabular data into a reviewable RDF/OWL graph via MCP + Bedrock, human review step | Python | MIT-0 | 3 | 14 Oct 2026 | **Borrow** "LLM proposes mapping, human approves" flow for CSV/connector import |
| [ralfbecher/database-ontology-mcp](https://github.com/ralfbecher/database-ontology-mcp) | Generates an ontology from a relational schema via MCP | Python | MIT | 8 | 13 Feb 2026 | Borrow idea (bootstrap ontology from a connected DB) |
| [daedalus/mcp-sparql](https://github.com/daedalus/mcp-sparql) | Generic SPARQL MCP server | Python | MIT | 0 | 18 Jun 2026 | Reference |

## 9. Standard vocabularies (business, logistics, finance)

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [gs1/WebVoc](https://github.com/gs1/WebVoc) | GS1 Web Vocabulary: products, GTIN, locations (GLN), logistics units (SSCC), EPCIS-adjacent terms | Turtle/JSON-LD | Apache-2.0 | 53 | 30 Jan 2026 | **Import pack** #1 for retail/logistics (Coles-type customers). Pair with GS1 EPCIS 2.0 event vocabulary for ClickHouse events |
| [schemaorg/schemaorg](https://github.com/schemaorg/schemaorg) | schema.org vocabulary and site source | HTML/TTL | Apache-2.0 | 6,293 | 30 Oct 2026 | **Import pack**: Organization, Place, Product, Order, Offer, Invoice (fits Square POS mapping) |
| [edmcouncil/fibo](https://github.com/edmcouncil/fibo) | Financial Industry Business Ontology | OWL | MIT | 784 | 28 Oct 2026 | **Import pack** (modules only, via ROBOT extract); large |
| [iofoundry/ontology](https://github.com/iofoundry/ontology) | Industrial Ontologies Foundry: core, `supplychain/`, `productionplanning/`, `maintenance/` (BFO-based) | OWL/RDF | MIT | 205 | 29 Oct 2026 | **Import pack** for supply chain; confirmed `supplychain` folder exists. BFO-heavy, so map selectively |

## 10. Supply chain / logistics impact over a knowledge graph

Honest finding: there is **no mature open-source "blast radius over a supply chain knowledge graph" project**. Everything below is small or a demo. This is a gap Eunomia can own.

| Repo | What it does | Lang | Licence | Stars | Last push | Eunomia use |
|---|---|---|---|---|---|---|
| [mercer14k/SupplyGraphAI](https://github.com/mercer14k/SupplyGraphAI) | Supply chain KG for tracing multi-tier disruptions, inventory and alternate supplier evaluation | HTML/Python | Apache-2.0 | 2 | 28 Sep 2026 | Borrow scenario ideas (multi-tier propagation, alternate supplier) |
| [reasonableplan/ontolens](https://github.com/reasonableplan/ontolens) | Supply chain KG explorer: relational DB to OWL/RDF to RDF/LPG DB to SHACL to graph algorithms | Python | MIT | 1 | 25 Jun 2026 | Borrow end-to-end pipeline shape (it's almost our Phase 3 diagram) |
| [microsoft/fabric-clinical-supply-ontology](https://github.com/microsoft/fabric-clinical-supply-ontology) | Deployable clinical supply chain ontology for Microsoft Fabric IQ with synthetic data | HTML | MIT | 3 | 28 Jul 2026 | Reference: competitor (Fabric IQ ontology) and synthetic data ideas |
| [pkjPathania/dependency-risk-graph](https://github.com/pkjPathania/dependency-risk-graph) | Software supply chain KG (SBOM, vuln enrichment, dependency blast radius) | Java | Apache-2.0 | 1 | 3 Aug 2026 | Borrow: the SBOM world has the best-developed "blast radius" UX (affected paths, depth, severity) |

## Licence flags summary

- **Avoid embedding:** Blazegraph (GPL-2.0, archived).
- **LGPL, sidecar or idea only:** horned-owl, HermiT, Owlready2, ecco, OWL API (dual-licensed, so Apache option exists in Java land).
- **No licence stated, reference only:** tmdaidevs/ontology-mcp-server, cmungall/bubastis, phillord/hermit-reasoner mirror.
- **Proprietary, reference only:** GraphDB, Stardog.
- Everything on the shortlist below is MIT, Apache-2.0 or BSD.

## Recommended shortlist (7)

1. **[oxigraph](https://github.com/oxigraph/oxigraph)** (Apache/MIT, Rust). Depend: `oxrdfio`/`oxttl`/`oxrdfxml`/`oxjsonld` for parse/serialise, in-memory `Store` + SPARQL as an import staging area. Replaces rio.
2. **[rudof](https://github.com/rudof-project/rudof)** (Apache/MIT, Rust). Depend: SHACL validation of imports and exports, and later ShEx/DCTAP. Most active Rust SHACL.
3. **[reasonable](https://github.com/gtfierro/reasonable)** (BSD-3, Rust). Depend: OWL 2 RL materialisation, used both on import and to count "new consequences" in schema plans.
4. **[open-ontologies](https://github.com/fabio-rovai/open-ontologies)** (MIT, Rust). Borrow the plan/apply/drift/rollback loop and ship it as an optional MCP add-on.
5. **[neosemantics (n10s)](https://github.com/neo4j-labs/neosemantics)** (Apache, Java). Borrow the RDF to property graph mapping config and the export semantics. Closest prior art to "triples to SurrealDB records and edges".
6. **[ROBOT](https://github.com/ontodev/robot)** (BSD-3, Java). Sidecar in CI and for vocabulary packs: `report`, `diff`, `extract` modules, HermiT/ELK consistency checks without embedding LGPL code.
7. **[pySHACL](https://github.com/RDFLib/pySHACL)** (Apache, Python). CI oracle only: cross-check rudof results on the W3C SHACL test suite and our fixtures.

Plus data: **[GS1 WebVoc](https://github.com/gs1/WebVoc)**, **[schema.org](https://github.com/schemaorg/schemaorg)** and **[IOF supplychain](https://github.com/iofoundry/ontology)** as the first import packs. Frontend nice-to-have: **[N3.js](https://github.com/rdfjs/N3.js)** (MIT).

## Suggested approach

### Feature 1: RDF/OWL import and export

Pipeline (all Rust, in a new `eunomia-rdf` crate):

1. **Parse** with `oxrdfio` (Turtle, RDF/XML, N-Triples, N-Quads, TriG, JSON-LD). Stream large files.
2. **Stage** into an in-memory oxigraph `Store` per import job. Never write straight into SurrealDB.
3. **Infer (optional toggle)** with `reasonable` (OWL 2 RL) so subclass and domain/range consequences are explicit before mapping.
4. **Validate** with rudof SHACL: customer shapes plus our own "Eunomia mappability" shapes (every class has a label, every object property has a known range, etc.). Produce a SHACL report the UI shows before commit.
5. **Map** using a declarative mapping (n10s-style options, YARRRML-style YAML for overrides):
   - `owl:Class` / `rdfs:Class` to Eunomia type (SurrealDB `DEFINE TABLE`, SCHEMAFULL or FLEXIBLE per type). `rdfs:subClassOf` stored as type edges (`type_extends`), not table inheritance.
   - `owl:DatatypeProperty` to typed field (`xsd:*` to SurrealDB types; `rdfs:range` decides the type; `owl:FunctionalProperty` = single value else array).
   - `owl:ObjectProperty` to a `RELATE` edge table (`inverseOf`, `TransitiveProperty`, `SymmetricProperty` stored as edge metadata the blast radius walker uses).
   - Individuals (`rdf:type X`) to records; datatype triples to fields; object triples to edges.
   - IRIs: keep the full IRI on every type, field, record and edge (`iri` field) and a prefix map per org, so export round-trips losslessly. Language tags and blank nodes kept as structured values (n10s `keepLangTag` style).
6. **Plan, then apply** (borrowed from open-ontologies): show counts of new types, fields, edges, records, SHACL violations and the schema blast radius (below) before one transaction commits. Write the import as a versioned ontology change with rollback.
7. **Export** by walking SurrealDB and emitting triples via `oxrdfio`, then validating with the same shapes. CI round-trip test: import, export, isomorphism check (sophia's RDFC-1.0 canonicalisation or oxigraph's isomorphism) on GS1 WebVoc, schema.org subset and IOF supplychain fixtures.

This fits the existing migration rule: every import is a numbered, reversible ontology change, and the 2.0.7 update test gains an RDF fixture.

### Feature 2: blast radius

Two engines, one UI:

- **Data blast radius (main value):** SurrealQL graph traversal from a chosen object across typed edges to a depth limit, honouring edge metadata (transitive, direction, weight, lead time). Join hit record ids to ClickHouse for volume, revenue and time at risk ("Laverton DC delayed 6h: 142 stores, 3,900 orders, $1.2m"). Borrow propagation ideas from SupplyGraphAI and SBOM dependency tools (path listing, depth, severity).
- **Schema blast radius (governance):** for a proposed type/field/edge change, compute
  - structural diff (added/removed/changed axioms, borrowing ROBOT diff and owl-diff),
  - affected records/edges count from SurrealDB,
  - **effectual vs ineffectual** consequences using `reasonable` before vs after (ecco's idea, which is what open-ontologies' "blast radius 0, 901 new consequences" demonstrates),
  - SHACL report delta (rudof before vs after),
  - risk score.

### Ideas to borrow from open-ontologies

1. The **plan, apply, monitor, drift, rollback** loop as the only way an ontology changes in production (Terraform model). Fits our "migrations must be safe" rule directly.
2. **Consequence diff, not just shape diff:** count new entailments from the reasoner, because a one-line change (a new `rdfs:domain`) can retype thousands of objects.
3. **Risk score** per plan plus **locked IRIs** (types that can't change without elevated approval), mapped to our approvals and #102 tool-call audit.
4. **Drift detection:** compare live SurrealDB schema to the last applied plan and alert.
5. **Verifiable evidence:** each applied plan writes a signed, re-checkable artefact (asserted facts + derivations). We can start with a hashed JSON evidence bundle in the audit log and leave Lean 4 certificates as a later stretch.
6. **Ship it as MCP tools** (`ontology.plan`, `ontology.apply`, `ontology.rollback`, `ontology.blast_radius`) so agents can propose changes but humans approve them.
7. **Same Rust stack** (oxigraph 0.5, rmcp, tokio) means lifting MIT code with attribution is realistic if we want to save time.

## Not found / corrected

- `ConsistentPlanning/hermit`, `EBISPOT/bubastis`, `cmungall/owl-diff`, `iofoundry/supplychain`, `kglab/kglab`, `comunica/comunica-mcp` do not exist at those paths. Real locations used above.
- `oxigraph/rio` is archived; use oxigraph's own I/O crates.
- No official Stardog or GraphDB MCP repo found on GitHub.
