# Two-step verification (MFA)

Eunomia supports two-step verification for password accounts with an authenticator app (TOTP, RFC 6238) and single-use recovery codes. An instance admin can require it for every password account. Accounts that sign in through single sign-on (OIDC) leave the second factor to their identity provider. This is readiness capability C02 in [docs/compliance](../compliance/README.md); passkeys (WebAuthn) are not implemented yet.

## For people

**Settings > Two-step verification > Set up.** Enter your password first: a signed-in browser alone cannot add an authenticator (10 wrong passwords in 15 minutes and it is refused for the rest of the window). Then scan the QR code with an authenticator app (1Password, Bitwarden, Google Authenticator, Microsoft Authenticator, Authy and others), or type the secret shown under it, then enter the six-digit code the app shows. Nothing changes until that code is accepted. You then get 10 recovery codes, shown once. Keep them somewhere safe: each one signs you in once if you lose the app.

Signing in then takes two steps: your email and password, then a code from the app (or a recovery code). You can make new recovery codes or turn two-step verification off from the same Settings section; both need a current code.

## For server operators

**Settings > Two-step verification > Require for everyone** is one switch for the whole server, every organisation on it included. Only the server's operator sees it: the account that set up the server, or an email listed in `EUNOMIA_ADMIN_EMAILS`. Admins granted in Settings > People do not, because those rights are given within one organisation and this setting reaches all of them. The count shown before switching covers every organisation. Before you can turn it on, your own account must have two-step verification on, or sign in only through single sign-on, so you cannot lock yourself out.

While it is on:

- A password sign-in without two-step verification must set it up before it gets a session. There is no skip.
- A new signup sets it up before its first session.
- Browser sessions that never passed a second factor (including every session from before the switch) are refused with `auth.mfa_required` and the person signs in again.
- Nobody can turn their own two-step verification off, except to re-enrol after an encryption key change (below).
- Single sign-on sessions of accounts without their own authenticator are not asked for a code. To require proof that the provider used a second factor, set `OIDC_REQUIRE_MFA=true` (below).
- Personal access tokens and connected MCP apps keep working. They are separate credentials, bounded by their scopes and expiry, and each can be revoked in Settings.

Turning the switch on or off is recorded in the operator's activity log, as are setting up and turning off two-step verification, recovery code use, wrong codes and wrong passwords at setup.

**Who is an admin.** Admin rights are the first account, `EUNOMIA_ADMIN_EMAILS`, or a grant in Settings > People. A grant can only be given to an account in the granting admin's own organisation. With `EUNOMIA_SIGNUP_ORG=personal` (one organisation per signup), an account in a customer organisation therefore becomes an admin only if the operator lists its email, or an admin of that same organisation grants it, which again needs a listed one. A per-organisation MFA policy is not implemented: this switch is instance-wide.

## Single sign-on

An account that set up its own authenticator passes it on every sign-in route, single sign-on included. When a single sign-on sign-in reaches such an account (for example one linked to a password account by its verified email) and the ID token does not say a second factor was used, the person gets the same pending second step as after a password: no session until the code. If the ID token does assert one (the `amr`/`acr` rules below), they go straight in and the session counts as verified. Accounts without their own authenticator are unchanged: the provider owns their second factor, unless `OIDC_REQUIRE_MFA` is set.

| Setting | Default | Meaning |
|---|---|---|
| `OIDC_REQUIRE_MFA` | `false` | `true` refuses a single sign-on sign-in unless the ID token says a second factor was used: an `amr` claim (RFC 8176) naming one (`mfa`, `otp`, `hwk`, `swk`, `sc`, `pop`, biometrics) or two different methods, or an `acr` listed in `OIDC_MFA_ACR`. The login page shows "Your sign-in provider did not confirm a second factor". Any other value stops the backend at boot. |
| `OIDC_MFA_ACR` | none | Comma-separated `acr` values your provider uses for multi-factor sign-in (provider specific, for example a Keycloak level or an Entra authentication context). |

Check that your provider sends `amr` (or the `acr` you list) before turning `OIDC_REQUIRE_MFA` on, or every single sign-on sign-in is refused.

## How it is protected

- The TOTP secret is encrypted with AES-256-GCM under `ENCRYPTION_KEY`, the same encryption as stored connector credentials. It is returned only while enrolling and never after. Like connector credentials, nothing re-encrypts it when the key changes ([deployment.md](../deployment.md), "Rotating ENCRYPTION_KEY"): an authenticator code then answers `auth.mfa_unreadable` (409, not a 500), costs no attempt and is audited. Sign in with a recovery code, turn two-step verification off with another recovery code (allowed even while it is required) and set it up again. Settings says so.
- Request bodies of `/api/auth/*` and the OAuth consent are never kept in failure capsules, and keys such as `code`, `mfa_code`, `recovery_codes`, `otpauth_uri` and `qr_svg` are masked everywhere capsules store arguments.
- Codes are six digits, 30-second steps, one step of clock skew either way, compared in constant time. Each step is accepted once per account, so a code cannot be replayed in its window.
- Recovery codes have 50 random bits each. Only their SHA-256 is stored, and each is spent atomically.
- A correct password gives a pending sign-in, not a session: a random token in an `HttpOnly`, `SameSite=Strict` cookie scoped to `/api/auth/mfa`, only its hash stored, valid for 5 minutes. It can only finish the second step (or enrol, when required). Five wrong codes end it.
- Every code tried counts per account: 10 in 15 minutes and every attempt, even a correct one, is refused with `rate.limited`. A right code starts the window over. Each attempt, on the account and on the sign-in challenge, is reserved with a conditional UPDATE in the control database **before** the code is checked, so parallel requests cannot get more guesses and every replica shares the same budget. Within one process an account's attempts also run one at a time.
- Setting up an authenticator signs out the account's other browser sessions that never passed a second factor.
- Creating an API token from a browser session, approving an OAuth app on the consent page, and the operator switch need a second factor from the last 10 minutes when your account has two-step verification. The web app asks for a code when needed (`auth.step_up_required`). Making new recovery codes and turning two-step verification off need a code every time.

## Upgrading

Control migration 0009 (`mfa`) adds optional fields to accounts and sessions and three new tables. Nothing changes for existing accounts, sessions, API tokens or app connections until someone sets up two-step verification or an admin requires it. Sessions from before the upgrade count as "no second factor yet".

## Not yet, and known limits

- Passkeys (WebAuthn), a second PR under C02.
- Step-up for tokens minting tokens, and for other sensitive actions (C06).
- Account recovery by an admin when someone loses both their app and their recovery codes. Today an operator clears `mfa_secret_enc` and the account's `mfa_recovery_code` rows in the control database.
- Forced enrolment trusts the first sign-in: while MFA is required, whoever holds a not-yet-enrolled account's password can enrol their own authenticator. There is no email confirmation (Eunomia sends no mail). The `auth.mfa_enrol` event is in the activity log.
- Anyone with the password can keep the owner locked out by using up the 10 attempts every 15 minutes. That is the cost of the limit. Change the password (an operator task today) to stop it.
- Turning the switch on takes effect at once on the replica that served it. Other replicas keep a verified-credential cache for up to 20 seconds.
- Within one process an account's attempts are serialised. Across replicas, two simultaneous correct answers rely on the database's write-conflict check for the replay and single-use guarantees.
- No per-organisation policy.
