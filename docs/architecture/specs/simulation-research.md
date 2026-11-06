# Eunomia Phase 4: Simulation / what-if research

Research for Avi, 31 Oct 2026. Scope (per Avi's correction): this phase is **simulation and what-if scenario modelling** over the ontology (SurrealDB 3.3.5) and event history (ClickHouse). It runs on stated assumptions, not trained models. Trained prediction models (forecasting, foundation models, causal ML) are a **later release**, listed briefly in the appendix and not designed here.

Every repo was checked with `gh api` on 31 Oct 2026. Last-push dates are in Melbourne time (AEDT). SimPy lives on GitLab and was checked with the GitLab API.

Licence note: where a Rust crate is shown as "Apache-2.0 / MIT", GitHub reports one SPDX id and the crate is published dual-licensed; confirm in `Cargo.toml` before adding. Licence key: **OK** = MIT, Apache-2.0, BSD, ISC, MPL-2.0, CC0 (fine to embed, keep notices). **LGPL/EPL** = care, run separately or link dynamically. **GPL/AGPL** = do not embed; reference only. **NC / custom** = model weights or code with non-commercial or revenue-gated terms, flagged because a later commercial licence of Eunomia would break.

Role key: **Depend**, **Sidecar** (separate Python service), **Borrow** (idea only), **Reference**.

---

## 1. What "simulation / what-if" means for Eunomia

Five engines, picked per question, all reading the same ontology and ClickHouse history:

| Engine | Answers | Example |
|---|---|---|
| **Graph propagation** | "If X changes, what does it touch, and by how much?" Blast radius plus formulas on edges (elasticity, lead time, capacity, share) | Wine price up 10%: wine units, attached food spend, margin |
| **Markov chains / Markov reward models** | "How do customers, deliveries or orders move between states, and what is that worth?" | Regulars to lapsed if price rises; deliveries on time to delayed to missed |
| **Discrete-event simulation (DES)** | Queues, resources, timing: trucks, docks, kitchen, bar staff | 6 hour DC delay rippling through dock slots and store deliveries |
| **Agent-based modelling (ABM)** | Heterogeneous actors with rules: customer segments, stores, drivers | Customers with different price sensitivity choosing to stay or switch |
| **System dynamics (SD)** | Stocks and flows with feedback over months | Inventory, backlog and service level loops |

Wrapped by: **Monte Carlo** over user-set assumption ranges, and **scenario comparison** (baseline vs A vs B) with P10 / P50 / P90 bands.

## 2. Simulation engines

### 2.1 Discrete-event simulation

| Repo | What it does | Lang | Licence | Stars | Last push | Role |
|---|---|---|---|---|---|---|
| [asynchronics/nexosim](https://github.com/asynchronics/nexosim) (formerly asynchronix) | High-performance async DES framework, models as actors with ports, deterministic, gRPC server option | Rust | Apache-2.0 / MIT | 315 | 23 Oct 2026 | **Depend** (Rust-native DES for logistics) |
| [ndebuhr/sim](https://github.com/ndebuhr/sim) | DEVS-style DES engine with prebuilt models (generators, queues, processors), also builds to WASM/npm | Rust | Apache-2.0 / MIT | 55 | 21 May 2026 | **Borrow / optional**: WASM build means in-browser what-if sliders |
| [garro95/desim](https://github.com/garro95/desim) | Generator-based DES | Rust | **GPL-3.0** | 65 | 24 Sep 2024 | **Avoid** (GPL, nightly features) |
| [team-simpy/simpy](https://gitlab.com/team-simpy/simpy) (GitLab) | The standard process-based DES in Python | Python | MIT | 122 (GitLab) | 15 Jul 2026 | **Sidecar / prototype**: fastest way to prototype a model before porting to Rust |
| [salabim/salabim](https://github.com/salabim/salabim) | DES with built-in animation and stats | Python | MIT (per readme) | 411 | 29 May 2026 | Reference |
| [CiwPython/Ciw](https://github.com/CiwPython/Ciw) | Open queueing network simulation | Python | MIT | 176 | 6 Dec 2025 | Borrow: queue network patterns (bar service, dock queues) |

### 2.2 Agent-based modelling

| Repo | What it does | Lang | Licence | Stars | Last push | Role |
|---|---|---|---|---|---|---|
| [krABMaga/krABMaga](https://github.com/krABMaga/krABMaga) | Rust ABM framework (MASON-inspired), parallel, model exploration and parameter sweeps, optional Bevy visualisation | Rust | MIT | 228 | 26 Oct 2026 | **Depend (later)** for customer and store ABMs; its parameter-sweep macros fit Monte Carlo |
| [mesa/mesa](https://github.com/mesa/mesa) | The standard Python ABM library, with Solara visualisation | Python | Apache-2.0 | 3,929 | 30 Oct 2026 | **Sidecar / prototype**; Borrow its data collector and batch-run API |
| [mesa/mesa-geo](https://github.com/mesa/mesa-geo) | GIS extension for Mesa | Python | Apache-2.0 | 237 | 6 Oct 2026 | Reference for geo agents (stores, trucks) |
| [bevyengine/bevy](https://github.com/bevyengine/bevy) | ECS game engine (krABMaga's visual layer) | Rust | Apache-2.0 / MIT | 49,403 | 30 Oct 2026 | Reference only; we render in the web UI instead |

### 2.3 System dynamics

| Repo | What it does | Lang | Licence | Stars | Last push | Role |
|---|---|---|---|---|---|---|
| [SDXorg/pysd](https://github.com/SDXorg/pysd) | Runs Vensim/XMILE system dynamics models in Python | Python | MIT | 466 | 5 Oct 2026 | **Borrow** XMILE as an import format; Sidecar if customers bring Vensim models |
| [transentis/bptk_py](https://github.com/transentis/bptk_py) | SD plus ABM in Python, scenario manager built in | Python | MIT | 37 | 29 Oct 2026 | **Borrow** scenario manager design (named scenarios overriding constants) |
| [scottfr/simulation](https://github.com/scottfr/simulation) | Insight Maker's JS engine: SD, ABM, DES in browser | JavaScript | **AGPL-3.0** | 164 | 28 Jun 2026 | **Avoid embedding** (AGPL). Reference for UX only |

For our needs SD is simple enough to implement natively: Euler/RK4 integration of stocks and flows defined on ontology edges (about 300 lines of Rust).

### 2.4 Markov chains (core method)

| Repo | What it does | Lang | Licence | Stars | Last push | Role |
|---|---|---|---|---|---|---|
| [dimforge/nalgebra](https://github.com/dimforge/nalgebra) | Linear algebra (matrix power, inverse, eigen) | Rust | Apache-2.0 | 4,810 | 1 Oct 2026 | **Depend**: transition matrices, fundamental matrix N = (I - Q)^-1, stationary distribution |
| [rust-ndarray/ndarray](https://github.com/rust-ndarray/ndarray) | N-d arrays | Rust | Apache-2.0 / MIT | 4,337 | 28 Oct 2026 | Optional alternative to nalgebra (pairs with polars/linfa) |
| [aatxe/markov](https://github.com/aatxe/markov) | Generic Markov chain crate (mostly text generation) | Rust | CC0-1.0 | 103 | 19 Jul 2024 | Reference only: too text-oriented; write our own ~200 line module on nalgebra |
| [TommasoBelluzzo/PyDTMC](https://github.com/TommasoBelluzzo/PyDTMC) | Full-featured DTMC library: fitting from sequences, absorption, mean first passage, stationary, plotting | Python | MIT | 96 | 27 Oct 2026 | **CI oracle**: validate our Rust results; Borrow its API (fit, absorption_probabilities, mfpt) |
| [spedygiorgio/markovchain](https://github.com/spedygiorgio/markovchain) | The reference R package for DTMCs, MLE / bootstrap fitting with confidence intervals | R | MIT (+ file) | 117 | 27 Oct 2026 | **Borrow** bootstrap CI method for transition rates |
| [QuantEcon/QuantEcon.py](https://github.com/QuantEcon/QuantEcon.py) | Economics toolkit with a solid MarkovChain class | Python | MIT | 2,415 | 31 Oct 2026 | Reference / oracle |
| [hmmlearn/hmmlearn](https://github.com/hmmlearn/hmmlearn) | Hidden Markov models | Python | BSD-3-Clause | 3,446 | 31 Oct 2024 | Later: hidden lifecycle states (appendix territory) |
| [jmschrei/pomegranate](https://github.com/jmschrei/pomegranate) | Probabilistic models incl. Markov chains and HMMs (PyTorch) | Python | MIT | 3,547 | 6 Mar 2025 | Reference |
| [josdejong/mathjs](https://github.com/josdejong/mathjs) | Math library with matrices | JavaScript | Apache-2.0 | 15,101 | 31 Oct 2026 | **Depend (frontend)**: recompute a small chain live when the user drags a transition slider. No good dedicated JS DTMC library exists; JS Markov repos are text generators |

### 2.5 Monte Carlo, graph and rules building blocks (Rust)

| Repo | What it does | Lang | Licence | Stars | Last push | Role |
|---|---|---|---|---|---|---|
| [rust-random/rand](https://github.com/rust-random/rand) (+ `rand_distr`) | RNG and distributions (triangular, PERT via beta, normal, lognormal) | Rust | Apache-2.0 / MIT | 2,091 | 23 Oct 2026 | **Depend** |
| [statrs-dev/statrs](https://github.com/statrs-dev/statrs) | Distributions and statistics | Rust | MIT | 835 | 23 Sep 2026 | **Depend** (quantiles, Beta/Dirichlet for transition uncertainty) |
| [petgraph/petgraph](https://github.com/petgraph/petgraph) | Graph data structures and algorithms | Rust | Apache-2.0 / MIT | 4,043 | 31 Oct 2026 | **Depend**: load the scenario subgraph from SurrealDB, topological propagation, cycle detection |
| [rhaiscript/rhai](https://github.com/rhaiscript/rhai) | Embedded, sandboxed scripting language | Rust | Apache-2.0 / MIT | 5,770 | 31 Oct 2026 | **Option** for richer edge rules |
| [cel-rust/cel-rust](https://github.com/cel-rust/cel-rust) | Common Expression Language interpreter | Rust | MIT | 701 | 23 Oct 2026 | **Depend** (preferred for edge formulas: non-Turing-complete, safe, fast, LLM-writable) |
| [ISibboI/evalexpr](https://github.com/ISibboI/evalexpr) | Expression evaluator | Rust | **AGPL-3.0** | 416 | 4 Oct 2026 | **Avoid** (relicensed to AGPL) |
| [pola-rs/polars](https://github.com/pola-rs/polars) | DataFrames | Rust | MIT | 40,237 | 31 Oct 2026 | **Depend**: post-process Monte Carlo runs into quantile tables |
| [google/or-tools](https://github.com/google/or-tools) | Optimisation (routing, scheduling, LP/MIP) | C++ | Apache-2.0 | 14,307 | 31 Oct 2026 | Later: "what is the best reschedule?" via Python sidecar |
| [NVIDIA/cuopt](https://github.com/NVIDIA/cuopt) | GPU routing and optimisation | CUDA | Apache-2.0 | 1,086 | 31 Oct 2026 | Reference |

### 2.6 Supply chain digital twins (prior art)

No mature open-source supply chain what-if platform exists. Small projects worth reading:

| Repo | What it does | Lang | Licence | Stars | Last push | Role |
|---|---|---|---|---|---|---|
| [amzn/supply-chain-simulation-environment](https://github.com/amzn/supply-chain-simulation-environment) | Amazon's SCSE: discrete-time supply chain sim with pluggable agents and policies | Python | Apache-2.0 | 65 | 19 Jan 2022 | **Borrow** its module split (network, demand, policy, metrics); stale |
| [tuhinsahai/ISOMORPH](https://github.com/tuhinsahai/ISOMORPH) | User-extensible supply chain digital twin for simulation and synthetic data | Python | MIT | 8 | 6 Aug 2026 | Borrow synthetic data generation for demos and tests |
| [Fatih180804/enterprise-supply-chain-digital-twin](https://github.com/Fatih180804/enterprise-supply-chain-digital-twin) | Supply chain twin with Monte Carlo risk | Python | MIT | 1 | 9 Aug 2026 | Reference |
| [Project-OSRM/osrm-backend](https://github.com/Project-OSRM/osrm-backend) | Road routing on OpenStreetMap | C++ | BSD-2-Clause | 8,186 | 31 Oct 2026 | **Sidecar** for real drive times on Coles maps |
| [valhalla/valhalla](https://github.com/valhalla/valhalla) | Routing with isochrones and time-dependent costs | C++ | MIT | 6,389 | 31 Oct 2026 | Alternative to OSRM (isochrones are nice for "who is reachable in 2h") |
| [uber/h3](https://github.com/uber/h3) / [HydroniumLabs/h3o](https://github.com/HydroniumLabs/h3o) | Hexagonal geo index (C, and Rust port) | C / Rust | Apache-2.0 / BSD-3 | 6,671 / 546 | 30 Oct 2026 / 29 Aug 2026 | Depend (h3o) for hex aggregation on maps |

## 3. Generative UI and visualisation

### 3.1 Agent-to-UI frameworks and protocols

| Repo | What it does | Lang | Licence | Stars | Last push | Role |
|---|---|---|---|---|---|---|
| [vercel/ai](https://github.com/vercel/ai) | AI SDK: `streamText` + tools, `useChat` renders tool results as React components. **AI SDK RSC `streamUI` is experimental and development is paused**; Vercel recommends AI SDK UI with tool parts | TypeScript | Apache-2.0 | 27,641 | 31 Oct 2026 | **Depend** (fits the Next.js frontend) |
| [assistant-ui/assistant-ui](https://github.com/assistant-ui/assistant-ui) | React chat primitives, tool UI registry, works with AI SDK | TypeScript | MIT | 12,860 | 31 Oct 2026 | **Depend** (chat shell + `makeAssistantToolUI` per scenario component) |
| [ag-ui-protocol/ag-ui](https://github.com/ag-ui-protocol/ag-ui) | Agent-User Interaction protocol: event stream for messages, tool calls, state deltas | TypeScript | MIT | 16,942 | 31 Oct 2026 | **Borrow** event model; adopt if agents run outside Next.js (Rust backend streaming state deltas) |
| [CopilotKit/CopilotKit](https://github.com/CopilotKit/CopilotKit) | Frontend stack for agents, generative UI, shared state; makers of AG-UI | TypeScript | MIT | 38,418 | 31 Oct 2026 | Alternative to assistant-ui; heavier |
| [a2ui-project/a2ui](https://github.com/a2ui-project/a2ui) | Google's A2UI: declarative UI JSON that agents emit and clients render from a trusted catalogue | TypeScript | Apache-2.0 | 16,904 | 31 Oct 2026 | **Borrow**: "agent emits JSON spec, client renders from an allow-listed component catalogue" is the safe pattern |
| [MCP-UI-Org/mcp-ui](https://github.com/MCP-UI-Org/mcp-ui) | UI resources over MCP | TypeScript | Apache-2.0 | 5,246 | 17 Sep 2026 | Borrow / later: expose scenario views to external MCP clients |
| [modelcontextprotocol/ext-apps](https://github.com/modelcontextprotocol/ext-apps) | Official MCP Apps spec and SDK (embedded UIs from MCP servers) | TypeScript | Apache-2.0 / MIT (transition) | 3,029 | 30 Oct 2026 | **Depend later**: our MCP server can return a scenario chart app to Claude/ChatGPT |
| [thesysdev/openui](https://github.com/thesysdev/openui) | Thesys' open generative UI standard; **Thesys C1 itself is a hosted closed API** | TypeScript | MIT | 14,510 | 31 Oct 2026 | Reference |
| [microsoft/data-formulator](https://github.com/microsoft/data-formulator) | AI-driven chart building over data | Python/TS | MIT | 17,634 | 31 Oct 2026 | Borrow interaction ideas (concept fields, chart refinement) |

### 3.2 Charts, maps, graphs

| Repo | What it does | Lang | Licence | Stars | Last push | Role |
|---|---|---|---|---|---|---|
| [apache/echarts](https://github.com/apache/echarts) | Full charting incl. bands, graph/sankey, geo | TypeScript | Apache-2.0 | 67,641 | 13 Oct 2026 | **Depend**: fan charts (stacked area bands), sankey of state flows, graph series for Markov diagrams |
| [hustcc/echarts-for-react](https://github.com/hustcc/echarts-for-react) | React wrapper | TypeScript | MIT | 5,012 | 21 Jan 2026 | Depend (or thin own wrapper) |
| [recharts/recharts](https://github.com/recharts/recharts) | React + D3 charts | TypeScript | MIT | 27,675 | 31 Oct 2026 | Depend for simple bar/line cards (already common in Next.js) |
| [vega/vega-lite](https://github.com/vega/vega-lite) / [react-vega](https://github.com/vega/react-vega) | Declarative JSON grammar of graphics | TypeScript | BSD-3-Clause | 5,538 / 426 | 29 Oct / 30 Oct 2026 | **Depend** for agent-authored ad-hoc charts: LLMs write Vega-Lite JSON reliably and it is sandboxed (no code) |
| [observablehq/plot](https://github.com/observablehq/plot) | Concise layered plotting | JS | ISC | 5,417 | 31 Oct 2026 | Optional (great for dot/interval uncertainty plots) |
| [maplibre/maplibre-gl-js](https://github.com/maplibre/maplibre-gl-js) | Open vector maps | TypeScript | BSD-3-Clause | 11,981 | 30 Oct 2026 | **Depend**: base map |
| [visgl/deck.gl](https://github.com/visgl/deck.gl) | WebGL layers: arcs, trips (animated routes), hexagons | TypeScript | MIT | 14,723 | 31 Oct 2026 | **Depend**: delayed route arcs coloured by delay, affected stores |
| [keplergl/kepler.gl](https://github.com/keplergl/kepler.gl) | Geo analysis app on deck.gl | TypeScript | MIT | 12,073 | 31 Oct 2026 | Borrow (time playback UX) |
| [maplibre/martin](https://github.com/maplibre/martin) | Rust tile server | Rust | Apache-2.0 / MIT | 4,035 | 31 Oct 2026 | Optional self-hosted tiles |
| [xyflow/xyflow](https://github.com/xyflow/xyflow) | React Flow node graphs | TypeScript | MIT | 38,847 | 29 Oct 2026 | **Depend**: Markov state diagrams and blast radius graphs (already in our design system) |
| [dagrejs/dagre](https://github.com/dagrejs/dagre) | Directed graph layout | TypeScript | MIT | 5,842 | 25 Oct 2026 | Depend for auto-layout in React Flow |
| [kieler/elkjs](https://github.com/kieler/elkjs) | ELK layout in JS | JavaScript | **EPL-2.0** | 2,867 | 29 Oct 2026 | OK as an unmodified npm dependency (weak copyleft on file level) but prefer dagre |
| [cytoscape/cytoscape.js](https://github.com/cytoscape/cytoscape.js) | Graph analysis and viz | JavaScript | MIT | 11,258 | 31 Oct 2026 | Alternative for large blast-radius graphs |
| [mermaid-js/mermaid](https://github.com/mermaid-js/mermaid) | Text-to-diagram (`stateDiagram-v2`) | TypeScript | MIT | 90,943 | 31 Oct 2026 | Depend for static exports / reports of state diagrams |

### 3.3 Showing uncertainty

- **Fan chart**: median line with nested P10 to P90 and P25 to P75 bands (ECharts stacked areas with transparent base). Baseline in grey, scenario in brand colour.
- **Interval / dot plot** for scenario comparison: one row per scenario, P50 dot and P10 to P90 whisker, zero line marked.
- **Probability of outcome**: "Margin falls in 87% of runs" as a big number beside the chart. Plain language beats sigma.
- **Quantile dotplot** (20 dots, each a 5% chance) for non-technical users: research shows people read these better than bands.
- **Tornado chart** for sensitivity: which assumption moves the result most.
- Always print the assumptions panel next to the result, editable, with "these are assumptions, not data" wording.

## 4. Recommended shortlist

**Rust core (embed):**
1. [nalgebra](https://github.com/dimforge/nalgebra): Markov chains and Markov reward models.
2. [petgraph](https://github.com/petgraph/petgraph): scenario subgraph propagation.
3. [cel-rust](https://github.com/cel-rust/cel-rust): safe formulas on edges (elasticity, lead time, capacity).
4. [rand / rand_distr](https://github.com/rust-random/rand) + [statrs](https://github.com/statrs-dev/statrs): Monte Carlo and Beta/Dirichlet uncertainty.
5. [nexosim](https://github.com/asynchronics/nexosim): DES for logistics (Coles-type).
6. [krABMaga](https://github.com/krABMaga/krABMaga): ABM, second wave.
7. [polars](https://github.com/pola-rs/polars): quantiles over runs.

**Frontend (embed):** [AI SDK UI](https://github.com/vercel/ai) + [assistant-ui](https://github.com/assistant-ui/assistant-ui), [ECharts](https://github.com/apache/echarts) (fan charts, sankey), [Vega-Lite](https://github.com/vega/vega-lite) (agent-authored charts), [React Flow](https://github.com/xyflow/xyflow) + dagre (state diagrams), [MapLibre](https://github.com/maplibre/maplibre-gl-js) + [deck.gl](https://github.com/visgl/deck.gl) (maps), [mathjs](https://github.com/josdejong/mathjs) (live slider recompute).

**Borrow ideas:** A2UI (allow-listed component catalogue), AG-UI (state delta events), BPTK scenario manager, Amazon SCSE module split, PyDTMC / markovchain (fitting and bootstrap CIs), SimPy and Mesa for prototyping.

**Avoid:** desim (GPL-3.0), evalexpr (AGPL-3.0), Insight Maker `simulation` (AGPL-3.0).

## 5. Proposed design: Phase 4 "Simulation / what-if"

### 5.1 Principles

1. **Assumptions in, ranges out.** Every number is either measured (from ClickHouse, with its query) or assumed (set by the user or a template default, with a range). The UI labels each one.
2. **The LLM plans, the engine computes.** The agent turns the question into a typed `ScenarioSpec`, picks the engine, and explains results. It never produces a number itself; every number shown carries the run id that produced it.
3. **Deterministic and reproducible.** Seeded RNG, spec hashed and stored, rerunnable from the audit log.
4. **Same migration and update rules as every Eunomia PR** (2.0.7 upgrade test, migrations reversible).

### 5.2 Data model (SurrealDB ontology)

New tables (all SCHEMAFULL):

- `assumption`: `{ key, target (record id or edge id), kind: elasticity | lead_time | capacity | share | rate | transition, distribution: fixed | uniform | triangular | pert | normal | lognormal | beta | dirichlet, params, unit, source: measured | user | template, measured_query?, note }`.
- `edge_rule`: a CEL expression attached to an ontology edge type or edge instance, e.g. on `menu_item -> sold_in -> venue`: `units_new = units_base * (1 + elasticity * pct_price_change)`.
- `state_model`: Markov chain definition: `{ entity_type, states[], absorbing[], step: day | week | month, estimated_from: clickhouse query, window, counts matrix, smoothing }`.
- `scenario`: `{ name, base: baseline | scenario id, interventions[] (set/scale/delay/remove/add on records or edges or transition rates), horizon, runs, seed, engine, status }`.
- `scenario_run`: result summary, quantiles, sensitivity, links to the ClickHouse rows.

ClickHouse:

- Reads: `events` (sales, line items, deliveries, status changes) aggregated into series and into state transition counts: `SELECT from_state, to_state, count() ... GROUP BY` over a window, per segment.
- Writes: `sim_results` (scenario_id, run_id, metric, t, value), MergeTree ordered by (org, scenario_id, metric, t). Quantiles via `quantilesTDigest(0.1, 0.5, 0.9)` in SQL. Built-in helpers like `simpleLinearRegression` and `seriesDecomposeSTL` can supply the **measured** baseline trend and seasonality without any Python.

### 5.3 Engines (new Rust crate `eunomia-sim`)

1. **Propagation engine**: load the blast-radius subgraph (Phase 3 walker) into petgraph, topologically order it, evaluate CEL edge rules per node, sampling assumptions per run. Handles cycles by bounded iteration and reports them.
2. **Markov engine** (core):
   - **Estimate** P from ClickHouse transition counts with Laplace or Dirichlet smoothing; per segment and per season window.
   - **Uncertainty**: sample each row from Dirichlet(counts + alpha), so a row built from 12 observations is visibly wider than one from 12,000 (the markovchain R package's bootstrap is the cross-check).
   - **Scenario** = perturbation of rates: "regular to lapsed +3 points (range 0 to 7)", always renormalising rows and rejecting invalid matrices.
   - **Outputs**: n-step distribution, stationary distribution, absorbing probabilities and expected time to absorption via the fundamental matrix N = (I - Q)^-1, and a **Markov reward model** (reward per state per step, e.g. spend per regular visit) for expected value over the horizon.
   - **Limits, shown in the UI**: memoryless (the next state depends only on the current one, so loyalty built over years is not captured unless states encode it, e.g. "regular 1y+"); seasonality (time-homogeneous P is wrong across Christmas, so estimate per season or use time-varying P_t); the perturbations themselves are assumptions, not data.
3. **DES engine** on nexosim: trucks, docks, routes and stores as models; lead time and service distributions from ClickHouse; outputs per store arrival delays and stockout hours.
4. **ABM (wave 2)** on krABMaga: customer agents with segment-specific price sensitivity, seeded from Markov segment shares.
5. **SD**: native Euler/RK4 stock and flow evaluation over `edge_rule`s.
6. **Monte Carlo wrapper**: N runs (default 2,000, capped by a time budget, e.g. 3 s), seeded, parallel with rayon; results to quantiles with polars and to ClickHouse; tornado sensitivity by one-at-a-time swings of each assumption between its P10 and P90.

Exposed through REST and MCP tools: `scenario.create`, `scenario.run`, `scenario.compare`, `markov.estimate`, `assumption.suggest`, `sim.explain`.

### 5.4 How the agent picks the engine

Rules first, LLM second:

| Question shape | Engine |
|---|---|
| Change a price, cost, share or capacity and ask about money or units next period | Propagation |
| Ask about customers, retention, churn, lifecycle, or delivery states over time | Markov (+ reward) |
| Queues, resources, timing, schedules, delays rippling | DES |
| Heterogeneous behaviour or interactions matter | ABM |
| Feedback loops over months (inventory, backlog) | SD |

Combinations are allowed (the wine example uses propagation + Markov). The agent must show its chosen engine and why, and the user can switch.

### 5.5 Not enough history

Checks before any run: minimum observations per transition row (e.g. 30), minimum weeks of sales for the item (e.g. 8), no price variation ever observed for an elasticity claim.

If they fail:

- Use **template assumptions** (e.g. restaurant wine elasticity range from published hospitality studies, flagged "industry range, not your data") with wide ranges.
- Offer an **experiment instead**: "Run a 4-week price test: raise the price on 2 of 4 by-the-glass wines, or alternate weeks, then re-run with measured elasticity." Eunomia creates the test plan as a task, tracks it through Square events, and updates the assumption to `measured` when done.
- Label the result "low confidence" with the reason, and widen bands.

### 5.6 Guardrails against overclaiming

- Every chart caption states: horizon, number of runs, which inputs are measured vs assumed.
- Never say "will"; say "in 80% of runs, between X and Y, given these assumptions".
- Results whose P10 to P90 range spans zero are labelled "direction uncertain".
- The agent cannot write numbers into text unless they come from a `scenario_run` (enforced by templated text slots filled by the backend, and a check that every number in the reply matches a run field).
- Assumptions outside sane bounds (elasticity > 0 for a normal good, probabilities outside 0 to 1, capacity negative) are rejected.
- Back-test button: rerun the baseline from 3 months ago and compare to what actually happened, to show the user how wrong the model tends to be.
- Audit: specs, seeds and results go into the #102 tool-call audit.

### 5.7 Generative UI output components

Allow-listed catalogue (A2UI-style): the agent returns a typed JSON spec per component, the frontend renders it via AI SDK tool parts in assistant-ui. No agent-generated code or HTML.

| Component | Shows | Library |
|---|---|---|
| `ScenarioSummaryCard` | Headline deltas with P10/P50/P90, probability of loss, confidence label | React + design system |
| `FanChart` | Baseline vs scenario over time with bands | ECharts |
| `ScenarioCompare` | Interval plot of baseline vs A vs B per metric | ECharts or Vega-Lite |
| `TornadoChart` | Sensitivity to each assumption | ECharts |
| `AssumptionsPanel` | Editable sliders with ranges, measured/assumed badges, "re-run" | React (mathjs live preview for small Markov chains) |
| `MarkovStateDiagram` | States as nodes (size = share of population), arrows labelled with probability, baseline vs scenario delta colouring, thickness = flow volume | React Flow + dagre; Mermaid `stateDiagram-v2` for exports |
| `StateFlowSankey` | Population flow between states over steps | ECharts sankey |
| `BlastRadiusGraph` | Affected objects and edges with impact | React Flow |
| `ImpactMap` | Stores / routes coloured by impact, animated trip delays | MapLibre + deck.gl (ArcLayer, TripsLayer, H3HexagonLayer) |
| `ExperimentPlanCard` | Proposed A/B or price test with dates and success metric | React |
| `VegaChart` | Escape hatch: agent-authored Vega-Lite spec, validated against a schema | react-vega |

**Markov state diagram spec example** (what the agent emits):

```json
{
  "component": "MarkovStateDiagram",
  "run_id": "run_01J...",
  "step": "month",
  "states": [
    {"id": "new", "label": "New", "share": 0.06},
    {"id": "regular", "label": "Regular", "share": 0.34},
    {"id": "lapsed", "label": "Lapsed", "share": 0.14},
    {"id": "lost", "label": "Lost", "share": 0.46, "absorbing": true}
  ],
  "edges": [
    {"from": "regular", "to": "regular", "p": 0.80, "p_scenario": 0.77, "p10": 0.76, "p90": 0.79},
    {"from": "regular", "to": "lapsed", "p": 0.15, "p_scenario": 0.18, "p10": 0.16, "p90": 0.21}
  ]
}
```

Rendered as: rounded state nodes, edge labels "15% to 18%", red edges where the scenario raises a bad transition, a toggle "baseline / scenario / difference", and hover showing the observation count behind each probability.

### 5.8 Worked example A: wine at a Square POS restaurant

> **All numbers below are EXAMPLE numbers for illustration, computed by a quick prototype simulation on made-up assumptions. They are not real data or predictions.**

Question: "If we raise wine prices 10%, or widen the range, will we gain or lose customers and revenue?"

**Measured (example) from ClickHouse via the Square connector:** 600 glasses a week at an average $14, cost $4 a glass, about 1.5 glasses per wine-drinking cover, $38 average food spend per wine-drinking cover. Monthly customer lifecycle transitions from repeat-visit events (card fingerprint / customer id).

**Assumed (user sets ranges):**

- Wine price elasticity: triangular, low -1.8, mode -1.2, high -0.6.
- Share of wine drinkers who stop visiting because of the rise: triangular 0% to 6%, mode 2%.
- Markov perturbation: regular to lapsed rises by 0 to 7 points (mode 3).
- Food margin 65%.

**Step 1, propagation (one week, 10,000 runs, example output):**

| Metric | P10 | P50 | P90 |
|---|---|---|---|
| Glasses a week | -92 | -72 | -52 |
| Covers lost | 4 | 10 | 18 |
| Revenue (wine + lost food) | -$1,053 | -$667 | -$286 |
| Gross margin | -$511 | -$240 | +$30 |

Margin falls in about 87% of runs (example).

**Step 2, Markov reward model (6 months):** example chain, monthly states New, Regular, Lapsed, Lost (absorbing):

| From \ To | New | Regular | Lapsed | Lost |
|---|---|---|---|---|
| New | 0 | 0.45 | 0.35 | 0.20 |
| Regular | 0 | 0.80 | 0.15 | 0.05 |
| Lapsed | 0 | 0.25 | 0.45 | 0.30 |
| Lost | 0 | 0 | 0 | 1.00 |

Starting with 60 new, 400 regular, 180 lapsed and 60 new arrivals a month; reward $55 per new visit and $62 per regular visit, with wine being about a quarter of the bill. Scenario perturbs Regular to Lapsed (+0 to +7 points) and raises wine spend per visit 10 to 20% net.

Example result after 6 months: about 29 fewer regulars (P10 to P90: 47 to 13 fewer); revenue change P10 -$9,283, P50 -$3,815, P90 +$1,165. The agent says: "Under these assumptions, the price rise more likely than not loses money over 6 months, mainly through regulars lapsing. The biggest lever is the regular-to-lapsed assumption (tornado chart). This is an assumption, so a price test would settle it."

**Range expansion variant:** add 4 new labels. Assumptions: each sells 15 to 30 glasses a week, cannibalisation 40% to 70% of existing wine sales, small uplift in New to Regular (0 to +3 points) from variety. Same pipeline, shown as Scenario B beside A in `ScenarioCompare`.

**Fallback:** if the venue has never changed wine prices, elasticity is a template range, the card says "low confidence", and `ExperimentPlanCard` offers: "Raise 2 of 6 by-the-glass wines by 10% for 4 weeks; keep the other 4 as control; measure glasses, attach rate and repeat visits."

**UI:** summary card, fan chart of weekly margin (baseline vs A vs B), Markov state diagram with Regular to Lapsed highlighted, tornado chart, assumptions panel.

### 5.9 Worked example B: Coles transport delay

> **All numbers below are EXAMPLE numbers for illustration on made-up assumptions.**

Question: "If outbound trucks from the Laverton DC are delayed 6 hours tomorrow, what's affected?"

**Step 1, blast radius (Phase 3):** DC Laverton, then routes, then 40 stores (example), then chilled categories, then promotions running.

**Step 2, DES / propagation:** per store chilled stock cover triangular 4 to 18 hours (mode 10, measured from stock events in a real deployment); delay = 6 hours plus knock-on triangular 0 to 4 hours (mode 1); lost sales 120 units an hour once stock runs out, $6.50 a unit (example).

| Metric (example, 10,000 runs) | P10 | P50 | P90 |
|---|---|---|---|
| Stores with a stockout | 4 | 7 | 10 |
| Lost units | 496 | 1,051 | 1,768 |
| Lost sales | $3,225 | $6,829 | $11,495 |

**Step 3, Markov delivery states:** delivery legs move through Scheduled, Delayed, Rescheduled (transient) to Delivered on time, Delivered late, Missed (absorbing). Baseline transition rates from ClickHouse delivery status events; example baseline: Scheduled to Delayed 8%, Delayed to Rescheduled 20%. Absorption probabilities via the fundamental matrix:

| Outcome | Baseline (example) | Scenario P10 | P50 | P90 |
|---|---|---|---|---|
| Delivered on time | 92.0% | 46.8% | 55.0% | 63.5% |
| Delivered late | 7.5% | 34.0% | 41.8% | 49.5% |
| Missed | 0.5% | 2.6% | 3.2% | 3.8% |

Scenario assumptions: Scheduled to Delayed rises to 30% to 60% (mode 45%), Delayed to Rescheduled to 20% to 50% (mode 35%).

**Scenario comparison:** A = accept delay; B = reroute 10 stores from Dandenong DC (capacity assumption 30% spare). The agent compares stockouts and missed deliveries in `ScenarioCompare`.

**UI:** `ImpactMap` (MapLibre + deck.gl arcs from the DC, stores coloured by stockout probability, OSRM drive times), `MarkovStateDiagram` of delivery states with the Scheduled to Delayed edge in red, fan chart of cumulative lost sales across the day, summary card.

## 6. PR breakdown (rough sizes)

S = under 300 lines, M = 300 to 1,000, L = 1,000 to 2,500, XL = above. Each PR must pass the standing rule: tests on SurrealDB 3.3.5, migration from 2.0.7 with realistic data, re-run safe, restore path, real updater upgrade.

| # | PR | Size |
|---|---|---|
| 1 | Schema: `assumption`, `edge_rule`, `state_model`, `scenario`, `scenario_run` tables + migration + ClickHouse `sim_results` table | M |
| 2 | `eunomia-sim` crate skeleton: seeded Monte Carlo runner (rand_distr, statrs, rayon), quantiles (polars), time budget | M |
| 3 | CEL edge rules: parser, validator, unit checks, sandbox limits | M |
| 4 | Propagation engine on petgraph using the Phase 3 blast-radius walker | L |
| 5 | Markov engine: estimate from ClickHouse counts, Dirichlet uncertainty, perturbations, n-step, stationary, absorption, reward model; PyDTMC parity tests in CI | L |
| 6 | Scenario API: REST + MCP tools (`scenario.create/run/compare`, `markov.estimate`, `assumption.suggest`) and audit logging | M |
| 7 | Data sufficiency checks + template assumption packs (hospitality, grocery logistics) + experiment plan generator | M |
| 8 | Guardrails: number provenance check on agent replies, sane-bounds validation, back-test endpoint | M |
| 9 | Generative UI catalogue part 1: summary card, fan chart, scenario compare, tornado, assumptions panel (AI SDK UI + assistant-ui + ECharts) | L |
| 10 | Generative UI part 2: Markov state diagram (React Flow + dagre), state sankey, blast radius graph | M |
| 11 | Generative UI part 3: impact map (MapLibre + deck.gl), optional OSRM sidecar container | L |
| 12 | Agent planner: question to `ScenarioSpec`, engine selection rules, explanation templates | M |
| 13 | DES engine on nexosim for logistics (trucks, docks, stores) | L |
| 14 | Square wine template end-to-end (connector mapping to series and lifecycle states) + demo data | M |
| 15 | Coles-style logistics template end-to-end with synthetic data | M |
| 16 | Wave 2: ABM on krABMaga; SD stock and flow; MCP Apps chart output | XL (split later) |

Rough total: PRs 1 to 12 about 5 to 7 weeks; 13 to 15 about 3 weeks; 16 later.

## Appendix: Later, predictions pipeline (not designed here)

Trained or zero-shot prediction models for a future release. Listed only; licences noted because some weights are non-commercial.

**Time-series foundation models**

| Repo | What it does | Licence (code / weights) | Stars | Last push |
|---|---|---|---|---|
| [amazon-science/chronos-forecasting](https://github.com/amazon-science/chronos-forecasting) | Chronos, Chronos-Bolt, **Chronos-2** (zero-shot with past and known-future covariates, e.g. planned prices) | Apache-2.0 / Apache-2.0 | 6,218 | 6 Oct 2026 |
| [google-research/timesfm](https://github.com/google-research/timesfm) | TimesFM 2.5 | Apache-2.0 / Apache-2.0 | 34,847 | 30 Sep 2026 |
| [SalesforceAIResearch/uni2ts](https://github.com/SalesforceAIResearch/uni2ts) | Moirai | Apache-2.0 / **CC-BY-NC-4.0 weights** | 1,617 | 3 Jun 2026 |
| [time-series-foundation-models/lag-llama](https://github.com/time-series-foundation-models/lag-llama) | Lag-Llama | Apache-2.0 | 1,601 | 6 Jun 2025 (stale) |
| [Nixtla/nixtla](https://github.com/Nixtla/nixtla) | TimeGPT SDK; **model is a closed hosted API** | Apache-2.0 SDK | 4,039 | 28 Oct 2026 |
| [DataDog/toto](https://github.com/DataDog/toto) | Toto (observability-focused) | Apache-2.0 / Apache-2.0 | 566 | 27 Oct 2026 |
| [NX-AI/tirex](https://github.com/NX-AI/tirex) | TiRex (xLSTM) | **NXAI Community Licence** (large enterprises must pay) | 307 | 8 Oct 2026 |
| [thuml/Sundial](https://github.com/thuml/Sundial) | Sundial | Apache-2.0 | 234 | 12 Sep 2025 |
| [ibm-granite/granite-tsfm](https://github.com/ibm-granite/granite-tsfm) | Granite TTM (tiny, CPU-fast) | Apache-2.0 | 917 | 30 Oct 2026 |

**Classic forecasting:** [statsforecast](https://github.com/Nixtla/statsforecast) (Apache-2.0, 4,957, 29 Oct 2026), [neuralforecast](https://github.com/Nixtla/neuralforecast) (Apache-2.0, 4,301, 28 Oct 2026), [mlforecast](https://github.com/Nixtla/mlforecast) (Apache-2.0, 1,300, 30 Oct 2026), [Prophet](https://github.com/meta/prophet) (MIT, 20,466, 22 Oct 2026), [Darts](https://github.com/unit8co/darts) (Apache-2.0, 9,569, 31 Oct 2026), [sktime](https://github.com/sktime/sktime) (BSD-3, 10,082, 31 Oct 2026), [AutoGluon](https://github.com/autogluon/autogluon) (Apache-2.0, 10,804, 31 Oct 2026), Rust: [grafana/augurs](https://github.com/grafana/augurs) (Apache-2.0, 579, 31 Oct 2026; MSTL, ETS, Prophet in Rust; could even supply simulation baselines now).

**Causal and what-if ML:** [DoWhy](https://github.com/py-why/dowhy) (MIT, 8,383), [EconML](https://github.com/py-why/EconML) (MIT, 4,835; DML price elasticity), [CausalML](https://github.com/uber/causalml) (Apache-2.0, 6,037; uplift), [CausalImpact](https://github.com/google/CausalImpact) (Apache-2.0, R) and [tfcausalimpact](https://github.com/WillianFuks/tfcausalimpact) (Apache-2.0), [CausalPy](https://github.com/pymc-labs/CausalPy) (Apache-2.0; synthetic control, diff-in-diff), [pysyncon](https://github.com/sdfordham/pysyncon) (MIT), [Meridian](https://github.com/google/meridian) (Apache-2.0), [Robyn](https://github.com/facebookexperimental/Robyn) (MIT), [PyMC-Marketing](https://github.com/pymc-labs/pymc-marketing) (Apache-2.0; MMM and CLV, a natural upgrade path from the Markov customer model).

**Tabular foundation models:** [TabPFN](https://github.com/PriorLabs/TabPFN) (code Apache-2.0; **v2 weights Prior Labs licence with attribution, v2.5 and later weights non-commercial**), [TabICL](https://github.com/soda-inria/tabicl) (BSD-3), [CARTE](https://github.com/soda-inria/carte) (BSD-3, stale since Aug 2025).

**Serving from Rust (when we get there):** Python sidecar via [FastAPI](https://github.com/fastapi/fastapi) (MIT) or gRPC, ONNX via [ort](https://github.com/pykeio/ort) (Apache-2.0 / MIT), [candle](https://github.com/huggingface/candle) (Apache-2.0 / MIT), [linfa](https://github.com/rust-ml/linfa) (Apache-2.0 / MIT), [PyO3](https://github.com/PyO3/pyo3), and ClickHouse built-in functions for simple baselines.

**Agent frameworks considered:** [pydantic-ai](https://github.com/pydantic/pydantic-ai) (MIT), [openai-agents-python](https://github.com/openai/openai-agents-python) (MIT). Archived and not to use: vanna, TaskWeaver, Salesforce Merlion, lifetimes.
