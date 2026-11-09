# Connectors

Connectors pull your data from other services into Eunomia, where agents read it with the `search`, `get`, `list`, `links` and `recall` tools. Each record is filed in a vault (by default your personal vault; see [which vault records go to](#which-vault-records-go-to)). Set one up on the **Connectors** page: paste the credential, **Save**, then **Test connection** (one real call to the provider) and **Sync now**. Credentials are encrypted at rest and only ever sent to that provider.

After the first sync, each connector syncs again on its interval (changeable on its setup page). A failed sync shows the provider's error on the connector's page and the dashboard, and retries with backoff (15 min → 6 h).

When the machine has no internet connection, scheduled syncs are skipped quietly instead of failing: no error is shown, no failure is counted and the backoff does not grow, and the sources sync again on the first scheduler tick after the connection returns. This is the **Skip syncs while offline** switch under Settings, General (on by default, per user). "Sync now" checks afresh and answers that you are offline rather than attempting a sync that cannot work. The check is a short TCP connect to a few well-known addresses; see `EUNOMIA_CONNECTIVITY_PROBE` in [deployment](deployment.md) to change them.

Every sync is a `sync` job in the background job queue (see `docs/architecture/jobs.md`): scheduled syncs and **Sync now** share it, so only one sync of a connector runs at a time. If one is already waiting or running, **Sync now** says so instead of starting another.

| Connector | What it syncs | Credential (where to get it) | Scopes / permissions | Default interval |
|---|---|---|---|---|
| Up Bank | Transactions (from 30 days back, then incremental), accounts, categories; webhooks optional | Personal access token: [api.up.com.au](https://api.up.com.au/getting_started) or Up app → Profile → Data sharing | Read-only by design | 15 min |
| PocketAI (HeyPocket) | Meeting recordings: full transcript with speakers, summary, action items, tags (30 days back, then incremental). Everything Pocket returns is stored whole (below) | API key from your Pocket account's developer settings |: | 24 h |
| GitHub | Issues and pull requests you created, are assigned or mentioned in (90 days back, then incremental) | [Fine-grained token](https://github.com/settings/personal-access-tokens/new), or a classic token | Fine-grained: Issues + Pull requests (read) on chosen repos. Classic: `repo` | 15 min |
| Slack | Messages in channels the app has joined (30 days back, then incremental; thread replies not included) | Bot token `xoxb-…` from your app at [api.slack.com/apps](https://api.slack.com/apps), then `/invite` it to channels | `channels:read`, `channels:history`, `groups:read`, `groups:history` | 15 min |
| Notion | Pages (title + top-level text) and databases shared with the integration | Internal integration secret from [notion.so/profile/integrations](https://www.notion.so/profile/integrations); add it to pages via ••• → Connections | Read content | 15 min |
| Linear | Issues assigned to you | Personal API key: Linear → Settings → Security & access | Read | 15 min |
| Gmail | Messages from the last 30 days onwards: subject, sender, plain-text body (200 per sync, oldest first; a larger backlog drains over the next syncs) | Your own Google OAuth client ID + secret and a refresh token (below) | `https://www.googleapis.com/auth/gmail.readonly` | 15 min |
| Google Calendar | Primary-calendar events, 30 days back to 6 months ahead, then every change (cancellations remove the event) | Same as Gmail | `https://www.googleapis.com/auth/calendar.readonly` | 15 min |
| Discord | Messages in the channels you list (comma-separated channel IDs) | Bot token from the [Developer Portal](https://discord.com/developers/applications) | Message Content Intent; View Channels + Read Message History | 15 min |
| Spotify | Recently played tracks (Spotify keeps only the last 50, so history builds up from when you connect) | Your own Spotify app's client ID + secret and a refresh token (below) | `user-read-recently-played` | 15 min |
| Todoist | Active tasks with project, due date, labels | API token: Todoist → Settings → Integrations → Developer | Full API token (Todoist has no read-only token) | 15 min |
| Stripe | Charges (incremental) | [Restricted key](https://dashboard.stripe.com/apikeys) `rk_…` | Charges: Read only | 15 min |
| Ed Discussion | One record per thread, with its answers and comments in the text (30 days back, then every thread that changed; 100 threads per sync, oldest first). Active courses, or the course IDs you list; region `us` (default), `au` (edstem.org/au, served from `edstem.org/api`) or `eu`. Only the newest 5,000 threads per course are scanned; threads deeper than that are never synced | API token from [edstem.org/us/settings/api-tokens](https://edstem.org/us/settings/api-tokens) | Ed's API is unofficial and has no scopes; the token reads what you can | 30 min |

## More than one connection of a kind

You can connect the same kind more than once, for example two Gmail accounts or two Pocket devices. On the Connectors page, **Add another** under a connected kind starts setup for a new connection; give it a name (the account's email address or the device name) so you can tell them apart. Each connection has its own credentials, sync cursor, health, routing and records: every synced record names the connection it came from (`cache_record.connector`).

- API: `POST /api/connectors` with `{"kind": "gmail", "label": "work@example.com"}` creates the connection (empty and disabled); set it up with `PUT /api/connectors/{connector}` using the returned `id`. `label` in the same body renames a connection.
- Every `/api/connectors/{connector}` route takes a connection id (`connector:...` or just its key) or, as before, a kind. A kind means that kind's first connection, so existing scripts and clients keep working unchanged.
- `GET /api/sources` lists one row per connection. The first connection of a kind keeps the source key as its `key` (`gmail`); each added one is `gmail~<instance>`, which is also what `POST /api/sources/{key}/sync` takes. Each connection row in `GET /api/connectors` carries this as `source_key`.
- `DELETE /api/connectors/{connector}` removes one connection with everything synced through it (recorded in the audit log as `connector.delete`). Other connections of the same kind are untouched. Removing a kind's first connection leaves an empty, disconnected one in its place.
- Webhooks: one URL per kind (`/api/sources/up_bank/webhook/{your user id}`) serves every connection of it. A delivery goes to the connection whose own webhook secret signs it.
- The Up Bank finance summary, the Pocket summary routes and the dashboard snapshot read the kind's first connection.
- The same item synced through two connections (a GitHub issue both accounts can see) is stored once, under the connection that synced it last.

**Upgrading:** every connection made before 2.0.4 becomes its kind's first connection and keeps its id, credentials, sync cursor and records. Records synced before the upgrade carry no connection and count as the first connection's (its **Delete all data** still removes them).

## Which vault records go to

Every record a connector syncs is filed in one vault, and only that vault's members can read it. Set it under **Where records go** on the connector's page (or `routing` in `PUT /api/connectors/{connector}`):

- **Destination vault**: where records go by default. Unset, that is your personal vault, which only you read.
- **Rules**: an ordered list of *field*, *value*, *vault*. The first rule a record matches sends it to that rule's vault; a record no rule matches goes to the destination vault. Values compare case-insensitively; `title` matches a word or phrase the title contains, every other field must equal the value. There is no AI classification.

| Connector | Rule fields |
|---|---|
| PocketAI | `tag`, `folder`, `title` |
| GitHub | `repo` (`owner/name`), `title` |
| Slack | `channel` (ID or name, `#` optional), `title` |
| Gmail | `label` (label ID: `INBOX`, `IMPORTANT`, `CATEGORY_PERSONAL`, or a label of your own as `Label_…`), `title` |
| Google Calendar | `calendar` (`primary`, or your address), `title` |
| Discord | `channel` (ID or name), `title` |
| Notion | `database` (parent database ID), `page` (parent page ID), `title` |
| Linear | `team` (name), `title` |
| Todoist | `project` (ID or name), `title` |
| Ed Discussion | `course` (ID or name), `category`, `title` |
| Up Bank, Spotify, Stripe | `title` |

Example: one Pocket account with both work calls and personal conversations. Make a "Build Paperless" vault, add the rule *tag* `work` → Build Paperless, and leave the destination as your personal vault: recordings tagged `work` (their summary, transcript and every fact extracted from them) are read by Build Paperless's members; the rest stay yours.

Rules only name vaults you are an active member of (saving another is refused). If you later leave a vault a rule names, records it would have sent there go to the destination vault instead, and the server logs it; a record already in that vault moves to where the rules send it the next time it syncs. Each record keeps its source, owner and provider id wherever it is filed.

**Changes apply to future syncs.** A record stays in the vault it was first filed in when it syncs again, so changing the destination or the rules never moves anything by itself. To re-file what is already synced, use **Move existing data** (two steps; `POST /api/connectors/{connector}/move`): every record the current routing sends elsewhere moves, with the facts and relations extracted from it, onto the same-named entities in the destination (created if missing). A fact the destination already has is not copied twice, and running it again moves nothing more. Facts in a vault you have left stay there (`left` in the result); the records themselves still move, they are yours. Observations in both vaults are marked stale and rebuilt. Recorded in the audit log as `connector.move_data`.

**After merging vaults**, the Vaults page offers to point connectors that filed into either merged vault at the new one (`POST /api/connectors/repoint` with `{"from": [...], "to": "vault:..."}`). Like any routing change it applies to future syncs.

## Pocket recordings

Each recording is stored whole: everything Pocket's API returns, verbatim, plus the full speaker-labelled transcript, so an agent can always read the entire meeting. `get` on a Pocket record (the recording or any of its transcript chunks) returns the whole stored recording. For search and memory, the recording is also cached as a summary record (summary, action items, speakers, tags) plus transcript chunks small enough to embed. With a chat model available (Settings, OpenAI), people, organisations and their relations are extracted from each chunk into the memory graph (as queued jobs).

### Pocket audio

A Pocket connection can also keep each recording's audio. It is off by default: tick **Keep audio
recordings** on the connection's setup page (it sets `config.store_audio`). Then:

- Audio is stored in the org's `documents` bucket, so it needs `EUNOMIA_DOCUMENTS_BACKEND` (see
  [documents](documents.md)). Without one, syncs carry on and keep transcripts only, and the server logs
  why once.
- It is downloaded once, after the transcript. Recall and search still use the transcript only.
- Pocket keeps audio on the device first. Until the device uploads it, Pocket answers 404 and Eunomia tries
  again on the next sync, but only while the recording is inside the sync window (the last day). A
  recording whose audio arrives later than that keeps its transcript only.
- Recordings over 200 MiB keep their transcript only. A failed or refused download never fails the sync.
- An opened Pocket record on the connection's page has an audio player and a **Download** button.
  API: `GET /api/connectors/pocketai/recordings/{recording_id}/audio` (`?download=true` for an
  attachment), the caller's own recordings only, logged in the audit log by id as `pocket_audio_download`.
- **Delete all data** removes the audio files too.

Unverified: the audio endpoint, `GET /public/recordings/{id}/audio` on `https://public.heypocketai.com/api/v1`
with the same API key, comes from third-party integrations, not from Pocket's documentation, and was not
checked against a real account. Eunomia accepts either answer: the audio itself (its `Content-Type` is
kept), or JSON with a signed link (`url`, `audio_url`, `signed_url` or `download_url`, at the top level
or under `data`), which it then downloads without sending your key. If Pocket answers differently, the
sync still succeeds and the server logs `pocket audio: not downloaded`.

## Deleting a connector's data

**Delete all data** on a connection's page (two steps) permanently removes everything synced through that connection: its records and their search index, Pocket's stored recordings, and every fact and relation extracted from them. Observations built on those facts are rebuilt from what remains. Entities themselves, the connection and its sync position are kept, so later syncs bring in new data only. API: `DELETE /api/connectors/{connector}/data` (account-level: a vault-restricted token cannot call it; recorded in the audit log as `connector.delete_data`).

## Google (Gmail, Google Calendar): getting a refresh token

Google has no personal access tokens, and a self-hosted server has no public redirect URL, so you use your own OAuth client and mint a long-lived refresh token once:

1. In [Google Cloud Console](https://console.cloud.google.com/apis/library), enable the Gmail API and/or Google Calendar API.
2. Configure the OAuth consent screen (External), add yourself as a test user, then **publish** it ("In production"): refresh tokens of apps left in "Testing" expire after 7 days.
3. Create an OAuth client ID, type **Web application**, with redirect URI `https://developers.google.com/oauthplayground`.
4. In the [OAuth 2.0 Playground](https://developers.google.com/oauthplayground), click ⚙ → "Use your own OAuth credentials" and enter the client ID and secret. Authorize the scope(s) above (both at once if you want one token for both connectors), then "Exchange authorization code for tokens".
5. Paste the client ID, client secret and refresh token into the connector. Eunomia exchanges the refresh token for a fresh access token on every sync.

### Large Gmail imports and Google rate limits

A large first Gmail import can take several syncs. Eunomia reads up to 200 messages per sync,
oldest first, at one full-message request per second per connection. A full batch can take several
minutes. Unread message IDs are saved
with the checkpoint, so later syncs continue the backlog without listing it again. Lists request
500 IDs per page. An incomplete listing never advances the checkpoint past older unread mail.
If more than 25,000 messages match the initial query, the sync reports the listing limit and
keeps its checkpoint rather than silently skipping older messages.

Google can report rate limits as HTTP 403 as well as 429. Gmail and Calendar retry rate limits
and temporary provider failures with exponential backoff and random delays, sharing at most four
retries and ten seconds of retry waiting and request time per sync. A longer `Retry-After` is left
for the next sync rather than retried early. Other clients using the same Google account can also
consume its quota.

If a limit persists after some records were fetched, Eunomia keeps that progress. The server log
says that Gmail is rate limiting the account, how many messages were fetched for import, and that
the next sync will continue. Calendar saves its next page and original query window. A partial
sync that saves all fetched records clears failure backoff and continues at the configured sync
interval. With no progress, or with a hard credentials or permissions error, the sync fails visibly
and uses the normal 15-minute to 6-hour failure backoff. There is no fixed five-minute retry loop.

The [current Gmail quota table](https://developers.google.com/workspace/gmail/api/reference/quota)
lists 6,000 quota units per minute per user per project, 5 units for `messages.list`, and 60 for
`messages.get` with `format=full`. Some older Google projects retain earlier quotas. Google also
applies concurrent-request limits; its Gmail docs do not specify a separate per-user per-second
quota-unit allowance. See [Google's error and backoff guidance](https://developers.google.com/workspace/gmail/api/guides/handle-errors).

## Spotify: getting a refresh token

1. Create an app at [developer.spotify.com/dashboard](https://developer.spotify.com/dashboard) (Web API) with redirect URI `http://127.0.0.1:8888/callback`.
2. Open `https://accounts.spotify.com/authorize?response_type=code&scope=user-read-recently-played&redirect_uri=http://127.0.0.1:8888/callback&client_id=CLIENT_ID`, approve, and copy the `code` parameter from the address bar (the page itself won't load: nothing is listening there).
3. Exchange it:
   `curl -u CLIENT_ID:CLIENT_SECRET -d grant_type=authorization_code -d code=CODE -d redirect_uri=http://127.0.0.1:8888/callback https://accounts.spotify.com/api/token`
4. Paste the client ID, client secret and the response's `refresh_token` into the connector.

## Troubleshooting

- **HTTP 401 / non-quota 403**: the token is wrong, revoked or missing a scope above. Fix it on the connector's page and **Test connection**.
- **HTTP 429 / Google quota 403**: the provider rate-limited the sync; it retries automatically and keeps any safe partial progress.
- **`not configured: missing …`**: a required field is empty.
- **Slack `not_in_channel` or nothing synced**: `/invite` the app to the channels.
- **Discord messages with no text**: enable Message Content Intent for the bot.
- **Google `invalid_grant`**: the refresh token expired or was revoked (an app still in "Testing" expires it after 7 days); mint a new one.

## Self-hosted providers (e.g. GitHub Enterprise)

Connectors talk to each provider's public API. To point one at a different
host (a GitHub Enterprise server, or a mock in tests), the server operator
sets `EUNOMIA_ALLOW_CONNECTOR_BASE_URL=1` on the backend, and the connector's
config can then carry a `base_url`. It's off by default because on a shared
server it would let any user make the backend call internal addresses.

Even with the override on, connector requests go through the same outbound guard as model calls: cloud metadata and the database address are refused, redirects are not followed, and `ALLOW_PRIVATE_LLM_URL=0` also blocks private ranges.
