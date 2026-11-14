# Applicability and assessment boundaries

Review date: 9 November 2026. Record the entity, installation, purpose, people affected, jurisdictions, roles, assessor and next review for each decision. Unknown is an honest initial state. An exclusion needs a reason and an authorised approver. The source registry in [controls.json](controls.json) retains editions, URLs and research limits.

| Standard | When to assess it and what remains outside a product finding |
|---|---|
| OWASP ASVS 5.0.0 L2 | Use as the application security target, including applicable L1 requirements. Assess every requirement against the deployed app. The 36 capabilities are a crosswalk, not that assessment. See the [full checklist](asvs-checklist.md). |
| SOC 2 Security, Availability, Confidentiality | An organisation chooses a service and examination scope with its assessor. The product can support controls; management processes and period evidence still need review. The crosswalk uses 2017 TSC with revised points of focus, 2022, provisionally. |
| ISO/IEC 27001:2022 Annex A | The organisation defines its ISMS, risks, treatment and Statement of Applicability. People and physical controls extend beyond software. Annex A and management-clause mappings need authorised-text review. |
| Privacy Act 1988, APPs and NDB | Check coverage for each entity and any exception to a small-business exemption. Record lawful handling, notices, rights and reasonable security steps in context. All 13 APPs need a decision, as below. |
| GDPR | Check Article 3 territorial scope and the actual controller/processor roles. Establish lawful bases, rights, contracts, records and any DPIA or Article 22 duties. Model-provider handling is excluded by this wave's owner boundary. |
| ASD Essential Eight | Select a maturity target for the customer's environment. Eunomia supports only parts of several strategies; endpoint/host configuration and continuing operation determine maturity. See all eight [responsibility boundaries](responsibilities.md#essential-eight-boundaries). |
| CIS Docker Benchmark 1.9.0 | Select the authorised edition and applicable profile for a real Docker host, daemon and containers. Record helper socket exceptions and evidence from Linux runtime checks. Compose lint and Docker Desktop are insufficient. Section IDs are provisional, informed by Docker Bench scripts rather than a complete benchmark assessment. |
| ISO/IEC 42001:2023 | Organisations making or using AI define their AIMS scope, roles, risk/impact assessment, competence, monitoring and review. Maker governance remains in this wave even with the model path excluded. Annex A crosswalks need authorised-text review. |
| EU AI Act | Check Article 2 territorial scope, intended use, provider/deployer roles and feature-specific duties. Ordinary business assistance is assessed below. Annex III use needs a separate workstream. |

## Australian privacy principles

These are applicability prompts from the [OAIC APP guidance](https://www.oaic.gov.au/privacy/australian-privacy-principles). They are not adopted policies.

| APP | Product support and remaining customer decision |
|---|---|
| 1 | C23/C36 support records and visibility. Name the privacy programme owner, notice and complaints contact. |
| 2 | Decide when identity is necessary and whether business records can use pseudonyms. No general anonymity feature is claimed. |
| 3 | C14/C23 support minimisation and purpose records. Decide lawful solicited collection and sensitive-information grounds. |
| 4 | Review unsolicited imports and arrange deletion or de-identification when required. Connector access is not permission for every collection. |
| 5 | C27 supports AI disclosure only. Deliver and record collection notices separately. |
| 6 | C04/C23/C24/C32 support access and purpose limits. Decide permitted use and disclosure. |
| 7 | No direct-marketing function has been assessed. Decide applicability and opt-out handling if downstream marketing uses the data. |
| 8 | Record actual non-model connector, support, export and remote-backup destinations. Assess cross-border disclosures. The model-path exclusion stays explicit. |
| 9 | Assess government identifiers, avoid them as account identifiers and document any lawful special use. |
| 10 | C24/C26/C35 support correction, citations and numeric checks. People assess factual quality. |
| 11 | C01 to C20 and C22 support security and disposal. Host protection, retention and organisational measures remain shared. |
| 12 | C21 exports a person's data across every workspace they belong to, with third-party redaction, from a browser session. Verify subject identity, scope, redaction, refusal reasons and response records. |
| 13 | C24 supports edits. The full correction case, disagreement statement and downstream notification process remains to be built and operated. |

For a suspected eligible breach, take reasonable steps to complete assessment within 30 calendar days. Once the eligible-breach test is met, notification must be prompt. The assessment period is not permission to delay known notification duties. [OAIC NDB guidance](https://www.oaic.gov.au/privacy/notifiable-data-breaches/preventing-preparing-for-and-responding-to-data-breaches/data-breach-preparation-and-response/part-4-notifiable-data-breach-ndb-scheme).

GDPR breach decisions have different clocks: controllers normally notify the authority within 72 hours where feasible, subject to the risk test; processors notify the controller without undue delay. Notification to people is a separate high-risk decision. Use reviewed decision templates and human judgement, not automatic outbound notifications. [GDPR Articles 33 and 34](https://eur-lex.europa.eu/legal-content/EN/ALL/?uri=CELEX:32016R0679).

## EU AI Act use and roles

The working assessment is ordinary business memory and ontology assistance, often described as limited risk, with transparency duties. It is not an Annex III decision system merely because AI is used or a regulated business installs it. Intended purpose and actual integration determine the assessment. This is a reasoned product assessment, not a blanket exemption.

Cashara is the provider of its branded AI features. The customer is usually the deployer. No evidence establishes that Cashara supplies a general-purpose model. Check EU market provision and outputs used in the EU under Article 2. Open-source distribution does not remove every system duty, including applicable Article 50 duties. [AI Act consolidated text](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:02024R1689-20260727).

Article 50 calls for first-interaction disclosure, accessible presentation and technically appropriate machine-readable marking of synthetic outputs. Human labels, citations and a custom JSON boolean each address only part of that work. Public-interest text and deepfake deployer rules have their own conditions. Record exceptions individually. A real MCP client must display a notice to the person; server metadata cannot establish that display. [Article 50](https://ai-act-service-desk.ec.europa.eu/en/ai-act/article-50).

As reviewed on 9 November 2026, Article 50 generally applies from 2 August 2026. Under amended Article 111(4), systems placed on the market before that date have until 2 December 2026 for Article 50(2) marking. Eunomia's eligibility is unknown until release and placing-on-market evidence is reviewed. That transition does not postpone interaction disclosure. Chapter III sections 1 to 3 apply to Annex III systems from 2 December 2027 and Annex I systems from 2 August 2028, subject to Article 113's exceptions. [Article 111](https://ai-act-service-desk.ec.europa.eu/en/ai-act/article-111), [Article 113](https://ai-act-service-desk.ec.europa.eu/en/ai-act/article-113).

Article 4 requires context-appropriate measures supporting AI literacy. Keep material versions, role, attendance and review records for Cashara staff and customer operators. A click-through is not proof of competence. [Article 4](https://ai-act-service-desk.ec.europa.eu/en/ai-act/article-4).

The policy track must cover Article 5 prohibited practices, including harmful manipulation, vulnerability exploitation, prohibited scoring/profiling and biometric/emotion uses, with the Act's conditions and exceptions. The 2026 amendment adds prohibitions concerning non-consensual intimate material and child sexual abuse material from 2 December 2026. Terms need a reporting and enforcement process; they cannot replace maker duties or prevent all misuse. [Consolidated Article 5](https://ai-act-service-desk.ec.europa.eu/en/ai-act/article-5).

### What changes for Annex III uses

Applicant ranking, worker assessment, credit access, education admission and other Annex III purposes trigger a new Article 6 assessment. A narrow procedural/preparatory use may meet a documented Article 6(3) exception; profiling natural persons defeats that exception. Human approval alone does not make a high-risk use low risk. A changed purpose or substantial modification can make the customer a provider under Article 25. Cashara marketing that high-risk purpose can create provider duties for Cashara. [Article 6](https://ai-act-service-desk.ec.europa.eu/en/ai-act/article-6), [Article 25](https://ai-act-service-desk.ec.europa.eu/en/ai-act/article-25).

Open a separate legal and release workstream for risk management, data governance over data Eunomia controls, technical documentation, logging, instructions, oversight, accuracy, cybersecurity, quality management, conformity assessment, registration and any representative duty. Deployers need appropriate use, competent oversight, monitoring and log retention. Article 27 fundamental-rights assessments apply only to the specified deployers. Reassess GDPR Article 22 and DPIA duties too. C25 to C35 provide future building blocks, not a high-risk assessment result. [AI Act chapters III and IX](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:02024R1689-20260727).

The AI Omnibus amendment and its dates are recorded in sources S11 to S14. The separate data/privacy Digital Omnibus negotiating position in S15 is not treated as an adopted GDPR relaxation. Recheck law and official guidance when purpose, release or jurisdiction changes.
