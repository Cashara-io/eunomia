# ASVS 5.0.0 L1/L2 assessment checklist

The [machine-readable checklist](asvs-checklist.json) contains all 253 L1/L2 requirements from the version-pinned official ASVS 5.0.0 JSON: 70 at L1 and 183 at L2. Every item starts with unknown applicability and unknown verification. No item has evidence of a verification pass in PR01.

Use the [official requirement wording](https://raw.githubusercontent.com/OWASP/ASVS/v5.0.0/5.0/docs_en/OWASP_Application_Security_Verification_Standard_5.0.0_en.flat.json) by matching the item's ID with the upstream `V` prefix. The checklist stores IDs, levels and headings, not redistributed normative paragraphs. OWASP publishes ASVS under CC BY-SA 4.0; attribution belongs to the OWASP ASVS project. The source byte digest and a checked ID/level inventory make edition drift visible.

L2 includes every applicable L1 requirement. A capability marked Done in the catalogue cannot mark a checklist item verified. Assess input/output encoding, injection, files/uploads, APIs, OAuth/OIDC, JWTs, configuration and error handling even where the capability matrix has no matching row. Any feature-specific exclusion needs a reason and approver, not an empty evidence field treated as a pass.

For an assessment, record the deployment/build, requirement, applicability decision, test procedure, expected and observed results, check time, scope, evidence and reviewer. Preserve failures and unknowns. An independent security reviewer must check the full L2 inventory before any verification claim. A later assessment change should update this initial checklist contract deliberately; PR01's guard keeps its baseline unknowns honest.

| Chapter | Area | L1 | L2 |
|---|---|---|---|
| V1 | Encoding and Sanitization | 8 | 19 |
| V2 | Validation and Business Logic | 4 | 7 |
| V3 | Web Frontend Security | 8 | 11 |
| V4 | API and Web Service | 2 | 8 |
| V5 | File Handling | 4 | 5 |
| V6 | Authentication | 13 | 22 |
| V7 | Session Management | 6 | 12 |
| V8 | Authorization | 4 | 3 |
| V9 | Self-contained Tokens | 4 | 3 |
| V10 | OAuth and OIDC | 5 | 24 |
| V11 | Cryptography | 3 | 11 |
| V12 | Secure Communication | 3 | 6 |
| V13 | Configuration | 1 | 12 |
| V14 | Data Protection | 2 | 7 |
| V15 | Secure Coding and Architecture | 3 | 10 |
| V16 | Security Logging and Error Handling | 0 | 16 |
| V17 | WebRTC | 0 | 7 |
