# Two-step verification (MFA)

Eunomia supports two-step verification for password accounts with an authenticator app (TOTP, RFC 6238) and single-use recovery codes. An instance admin can require it for every password account. Accounts that sign in through single sign-on (OIDC) leave the second factor to their identity provider. This is readiness capability C02 in [docs/compliance](../compliance/README.md); passkeys (WebAuthn) are not implemented yet.

## For people

**Settings > Two-step verification > Set up.** Scan the QR code with an authenticator app (1Password, Bitwarden, Google Authenticator, Microsoft Authenticator, Authy and others), or type the secret shown under it, then enter the six-digit code the app shows. Nothing changes until that code is accepted. You then get 10 recovery codes, shown once. Keep them somewhere safe: each one signs you in once if you lose the app.

Signing in then takes two steps: your email and password, then a code from the app (or a recovery code). You can make new recovery codes or turn two-step verification off from the same Settings section; both need a current code.

## For admins

**Settings > Two-step verification > Require for everyone** is one switch for the whole server. It is an instance setting, not a per-organisation one, because admin rights in Eunomia are instance-wide (the first account, `EUNOMIA_ADMIN_EMAILS`, or granted in Settings > People) and the people list already shows every account. Before you can turn it on, your own account must have two-step verification on, or sign in only through single sign-on, so you cannot lock yourself out. The People list shows who has it on.

While it is on:

- A password sign-in without two-step verification must set it up before it gets a session. There is no skip.
- A new signup sets it up before its first session.
- Browser sessions that never passed a second factor (including every session from before the switch) are refused with `auth.mfa_required` and the person signs in again.
- Nobody can turn their own two-step verification off.
- Single sign-on sessions are not asked for a code. To require proof that the provider used a second factor, set `OIDC_REQUIRE_MFA=true` (below).
- Personal access tokens and connected MCP apps keep working. They are separate credentials, bounded by their scopes and expiry, and each can be revoked in Settings.

Turning the switch on or off is recorded in the activity log, as are setting up and turning off two-step verification, recovery code use and wrong codes.

## Single sign-on

| Setting | Default | Meaning |
|---|---|---|
| `OIDC_REQUIRE_MFA` | `false` | `true` refuses a single sign-on sign-in unless the ID token says a second factor was used: an `amr` claim (RFC 8176) naming one (`mfa`, `otp`, `hwk`, `swk`, `sc`, `pop`, biometrics) or two different methods, or an `acr` listed in `OIDC_MFA_ACR`. The login page shows "Your sign-in provider did not confirm a second factor". Any other value stops the backend at boot. |
| `OIDC_MFA_ACR` | none | Comma-separated `acr` values your provider uses for multi-factor sign-in (provider specific, for example a Keycloak level or an Entra authentication context). |

Check that your provider sends `amr` (or the `acr` you list) before turning `OIDC_REQUIRE_MFA` on, or every single sign-on sign-in is refused.

## How it is protected

- The TOTP secret is encrypted with AES-256-GCM under `ENCRYPTION_KEY`, the same encryption as stored connector credentials. It is returned only while enrolling and never after. Rotating `ENCRYPTION_KEY` follows the usual procedure in [deployment.md](../deployment.md).
- Codes are six digits, 30-second steps, one step of clock skew either way, compared in constant time. Each step is accepted once per account, so a code cannot be replayed in its window.
- Recovery codes have 50 random bits each. Only their SHA-256 is stored, and each is spent atomically.
- A correct password gives a pending sign-in, not a session: a random token in an `HttpOnly`, `SameSite=Strict` cookie scoped to `/api/auth/mfa`, only its hash stored, valid for 5 minutes. It can only finish the second step (or enrol, when required). Five wrong codes end it.
- Wrong codes also count per account: 10 in 15 minutes and every attempt, even a correct one, is refused with `rate.limited`. Like the password lockout this is counted per process.
- Creating an API token from a browser session, and the admin switch, need a second factor from the last 10 minutes when your account has two-step verification. The web app asks for a code when needed (`auth.step_up_required`). Making new recovery codes and turning two-step verification off need a code every time.

## Upgrading

Control migration 0009 (`mfa`) adds optional fields to accounts and sessions and three new tables. Nothing changes for existing accounts, sessions, API tokens or app connections until someone sets up two-step verification or an admin requires it. Sessions from before the upgrade count as "no second factor yet".

## Not yet

- Passkeys (WebAuthn), a second PR under C02.
- Step-up for tokens minting tokens, and for other sensitive actions (C06).
- Account recovery by an admin when someone loses both their app and their recovery codes. Today an operator clears `mfa_secret_enc` and the account's `mfa_recovery_code` rows in the control database.
- The lockout counters are per process, so N replicas allow N times the budget.
