# Documents

Upload a file, and Eunomia keeps the original and makes its text searchable. The original stays downloadable byte for byte, and its text is split into passages that `search`, `recall` and `get` find like synced records. Each hit names its document, so you can always open the source. Entities and facts are extracted from the passages too, when a model is configured.

Upload from the **Documents** page (sidebar) or with the `document_upload` MCP tool. A document lives in one vault: your personal vault unless you name another with `vault_id`. Whoever can read that vault can read the document, and its members can upload, re-index and delete there. A document in your personal vault is yours alone.

## What is supported

| | |
|---|---|
| Types | Plain text (`.txt`), Markdown (`.md`), JSON, CSV, and PDFs that contain text. A scanned PDF (only images) uploads, but indexing fails with "no text could be extracted", and the file is still downloadable. Office documents and images are not supported. |
| Size | Up to `EUNOMIA_DOCUMENTS_MAX_BYTES`, 25 MiB by default. An MCP upload also has to fit in the request size limit, `MAX_REQUEST_BODY_BYTES` (1 MiB by default, and base64 makes a file about a third larger). Upload bigger files from the Documents page or over HTTP (below). |
| Passages | About 6,000 characters each, cut at line ends where possible. Each passage records the character range it covers in the extracted text. |
| Status | `indexing` → `ready`, or `failed` with the reason. Indexing runs in the background job queue. A failed document is still downloadable, and **Re-index** on its page tries again. |

## How it works

- The original bytes go into a SurrealDB **file bucket** named `documents`, one per org database. Each vault has its own storage area in it: Eunomia picks the object path (`/<vault key>/<document key>/<revision>/<sanitised name>`), so one vault's files never sit beside another's (with the `file:` backend, each vault is its own folder). Nothing you send becomes a path or a URL.
- A document uploaded before 2.0.4 keeps the path it was stored at (`/<document key>/<revision>/<sanitised name>`, no vault folder) and still downloads after the upgrade: every record names its own path, so nothing is moved. Its next version, or a vault clone or merge, stores it under a vault.
- The extracted passages are ordinary cache records (source `documents`, type `document.chunk`, id `documents:document.chunk:<key>:<revision>:<part>`), filed in the document's vault, so `search` and `recall` show them to that vault's members. Their payload holds the document id, the revision, the part number, the character range and the filename. They are embedded into the same vector index as everything else, and keyword search works without an embedding key.
- A search, recall or get hit on a passage carries a `document` object: `document_id`, `chunk_id`, `revision`, `part`, `char_start`, `char_end`, `filename` and `download_url`.
- Uploading a new version (`document_id` on `document_upload`, or `?replace=` over HTTP) or re-indexing makes a new **revision**. Once that revision is indexed, the passages of the earlier ones and the facts extracted from them are removed, so no hit points at text that is gone.
- **Deleting** a document takes its passages out of search, recall and get in one transaction, together with the facts and relations extracted from them, then removes the stored file and the record. If removing the file fails, the document stays hidden and deleting it again finishes the job.
- **Cloning or merging a vault** copies its documents into the new vault: the file (under the new vault's folder, checked against its SHA-256), the record with a new id, and the passages with their vectors, so nothing is embedded again. Facts extracted from a passage point at the passage's copy. A document still indexing when it is copied arrives as `failed`; **Re-index** it. If the copy fails part way, the new vault is removed with every file and record it got.
- **Deleting a vault** deletes its documents first, files included. If removing a file fails, the vault stays and deleting it again finishes the job.
- Downloads and exports are written to the audit log with the document id, never the content. So are uploads (name and size only) and deletions.

## MCP tools

| Tool | Does |
|---|---|
| `document_upload` | `filename` plus `text` (UTF-8) or `content_base64` (for a PDF). Optional `content_type`, `document_id` (to upload a new version, which stays in that document's vault) and `vault_id` (an id or a vault name). Returns the document with status `indexing`. |
| `document_list` | One vault's documents (`vault_id`, default your personal vault), newest first, with status and passage count. |
| `document_get` | One document: metadata, the extracted text (up to `max_chars`, default 100,000), and each passage's id and character range. |
| `document_download` | The HTTP route for the original bytes (`GET /api/documents/<id>/download`), with size and SHA-256 to check them. The bytes never go through MCP. |
| `document_export` | The export manifest: every passage with its text, location and embedding state, plus the embedding model and dimension. Vectors are left out unless `include_vectors` is true. `vectors_url` serves the full manifest over HTTP. |
| `document_delete` | Delete the document, its passages and the facts extracted from them. Flagged `destructiveHint`. |

An agent that has an HTTP tool can download with the same `Authorization: Bearer <token>` header it uses for MCP. Paths are relative to the address the agent reaches Eunomia at, the same one as `/mcp`.

## HTTP

All routes take the session cookie or `Authorization: Bearer <token>`. Upload and list take an optional `vault_id` query parameter (default: your personal vault, or a vault-restricted token's own vault). A token restricted to another vault, or one without `memory:write`, cannot upload or delete.

```bash
# upload (the body is the file; the name goes in the query)
curl -H "Authorization: Bearer $TOKEN" -H "Content-Type: text/markdown" \
  --data-binary @project-notes.md "http://localhost:8001/api/documents?filename=project-notes.md"

curl -H "Authorization: Bearer $TOKEN" http://localhost:8001/api/documents                         # list
curl -H "Authorization: Bearer $TOKEN" "http://localhost:8001/api/documents?vault_id=vault:xyz"     # one vault's list
curl -H "Authorization: Bearer $TOKEN" http://localhost:8001/api/documents/document:abc            # text and passages
curl -OJ -H "Authorization: Bearer $TOKEN" http://localhost:8001/api/documents/document:abc/download  # original
curl -OJ -H "Authorization: Bearer $TOKEN" http://localhost:8001/api/documents/document:abc/export    # manifest with vectors
curl -X POST -H "Authorization: Bearer $TOKEN" http://localhost:8001/api/documents/document:abc/reindex
curl -X DELETE -H "Authorization: Bearer $TOKEN" http://localhost:8001/api/documents/document:abc
```

The export manifest has `format: "eunomia.document-export"` and `version: 1`. It includes the document (with its SHA-256) and an `embedding` block with `dimension` (1536), `provider`, `model`, `chunks_embedded` and `chunks_missing`. It also has one entry per passage with `chunk_id`, `part`, `char_start`, `char_end`, `text`, `embedding_state` (`present` or `missing`) and `embedding` (the vector, or `null`). The model is the one your settings name now: vectors do not yet record which model made them.

## Setup

Documents are stored in SurrealDB's own built-in **file bucket**: the backend writes and reads them through the database (`DEFINE BUCKET` and file pointers) and never talks to a storage service itself. With `docker-compose.yml` this works out of the box. The files go into a folder inside the database's own data volume, so no extra service or volume is needed.

File buckets are **experimental** in SurrealDB 3.3, so the database server must allow them, and must also allow the `file` functions. `docker-compose.yml` starts it with:

```
surreal start ... --allow-funcs=time,string,search,count,array,vector,math,file,type::file --allow-experimental=files ...
SURREAL_BUCKET_FOLDER_ALLOWLIST=/data/documents
```

Write `--allow-experimental=files` with the `=`. Without it, the flag takes the next argument (the database path) as a second feature name, and the server refuses to start. The environment variable `SURREAL_CAPS_ALLOW_EXPERIMENTAL=files` does the same. SurrealDB 2.x has no file buckets and refuses this flag.

The backend reads two settings:

| Variable | Default | Meaning |
|---|---|---|
| `EUNOMIA_DOCUMENTS_BACKEND` | `file:/data/documents` in `docker-compose.yml`; empty (off) when unset | Where the bucket keeps the files, as SurrealDB's `DEFINE BUCKET ... BACKEND`: `file:/absolute/path` on the database server (inside `SURREAL_BUCKET_FOLDER_ALLOWLIST`), or optionally an S3 URL (below). `memory` keeps them in the server's memory and loses them on restart (tests only). Empty turns documents off: uploads answer `document.storage_unavailable`, and everything else works. |
| `EUNOMIA_DOCUMENTS_MAX_BYTES` | `26214400` (25 MiB) | The largest upload accepted. |

Eunomia appends each org's database name to the folder (`/data/documents/org_<id>/...`) or the S3 key prefix, so orgs never share keys, and puts each vault's files in a folder of their own under it (`/data/documents/org_<id>/<vault key>/...`). Every org's bucket is defined when the org is created, and again at every backend start, so a changed setting applies after a restart. Only the root user can define a bucket. The org's own database user cannot.

The `file:` path is on the **database server**, not the backend. A folder outside the allowlist is refused.

### Optional: an S3 bucket as the backend

SurrealDB 3.3 can also keep a bucket's files in S3 or an S3-compatible store. This is optional configuration of the same built-in bucket: Eunomia does not ship or need an S3 service.

1. Give the **SurrealDB server** the credentials in its environment, never in the URL. `DEFINE BUCKET` text is visible to `INFO FOR DB`, so Eunomia refuses a URL with credentials in it. For example, in a `docker-compose.override.yml`:
   ```yaml
   services:
     surrealdb:
       environment:
         - AWS_ACCESS_KEY_ID=${DOCS_S3_KEY}
         - AWS_SECRET_ACCESS_KEY=${DOCS_S3_SECRET}
         - AWS_REGION=eu-west-1
   ```
   Use a key that can only `GetObject`, `PutObject`, `DeleteObject` and `ListBucket` on that one bucket. An instance or workload role works too: SurrealDB uses the standard AWS credential chain.
2. Set `EUNOMIA_DOCUMENTS_BACKEND=s3://my-bucket?region=eu-west-1`. A host in the URL (`s3+https://host/bucket`) is a custom S3-compatible endpoint with path-style addressing. Only `region` and `prefix` are accepted as options.
3. Restart: `docker compose up -d`.

Downloads always go through the backend, which checks access to the document's vault every time. There are no pre-signed URLs, so revoking access is immediate, and the bucket needs no CORS rules and no public access. Eunomia has not been tested against any particular S3 provider. Try an upload and a download before you rely on one.

## Backups

**A SurrealDB export does not contain the files.** It holds the document records, the passages and their vectors, and the bucket definition, but not the bytes. You need both to restore documents.

- With the default `file:` backend, the nightly `backup` service also writes `documents.tar.enc`: the documents folder of the data volume, encrypted the same way as the database exports, in the same backup directory. Restoring that backup (`backend/scripts/restore.sh`, or `eunomia-backup restore`) unpacks it back. With `--wipe`, files not in the backup are removed first. Check for `documents.tar.enc` after your first backup.
- With an S3 backend, back the bucket up with the provider's own tools, such as versioning or replication. Eunomia does not copy S3 buckets.

A database restored without its files lists the documents and finds their passages, but downloads answer `document.storage_unavailable` ("the stored file is missing"). Delete those documents or upload them again.

## Errors

| Code | When |
|---|---|
| `document.not_found` (404) | In a vault you are not a member of, never existed, or already deleted. Another user's or org's id answers exactly like a missing one. |
| `document.too_large` (413) | Over `EUNOMIA_DOCUMENTS_MAX_BYTES`. |
| `document.unsupported_type` (415) | Not text, Markdown, JSON, CSV or PDF. |
| `validation.invalid` (400) | Empty, not UTF-8 text, invalid JSON, a "PDF" without a PDF header, bad base64, no filename. |
| `document.storage_unavailable` (503) | Storage is off, the server lacks `--allow-experimental=files` or the `file` functions, or the store cannot be reached. The backend log has the cause. |

## Limits and what is not there yet

- No OCR, Office documents, images, public links or importing from a URL.
- Upload goes through the backend, which holds the file in memory while storing it. The default 25 MiB limit keeps that small.
- Bucket files are not written in the same transaction as the database. A crash between storing the file and writing its record can leave an unreferenced file in the bucket. It is harmless and nothing points at it.
- Extracted text is untrusted content. Agents should treat it as data, not instructions.
