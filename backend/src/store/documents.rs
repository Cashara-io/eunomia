//! Statements for uploaded documents (`documents/`). See the module docs in store/mod.rs.
//!
//! The original bytes go through SurrealDB's file functions on the org database's `documents` bucket
//! (`type::file` builds the pointer from a bound path, so no path is ever spliced into SQL text), each
//! vault's under its own key prefix. Access is by the document's vault (`documents::load` authorizes
//! it); statements that touch chunks or revisions bind the document's own `owner`.

use super::Stmt;

pub const OBJECT_PUT: Stmt = Stmt::new("documents.object_put", "file::put(type::file(\"documents\", $path), $bytes)");
/// The bytes, or NONE when the object is missing.
pub const OBJECT_GET: Stmt = Stmt::new("documents.object_get", "file::get(type::file(\"documents\", $path))");
pub const OBJECT_DELETE: Stmt = Stmt::new("documents.object_delete", "file::delete(type::file(\"documents\", $path))");

pub const CREATE: Stmt = Stmt::new(
    "documents.create",
    "CREATE $id SET owner = $owner, vault = $vault, filename = $filename, media_type = $media_type, \
     size_bytes = $size_bytes, sha256 = $sha256, object_path = $object_path, revision = 1, status = \"indexing\", \
     chunk_count = 0 RETURN AFTER",
);

/// One live document (a deleted one reads as absent). The caller authorizes its vault.
pub const GET: Stmt = Stmt::new("documents.get", "SELECT * FROM $id WHERE status != \"deleted\"");
/// Also a deleted one whose cleanup did not finish (a repeated delete retries it).
pub const GET_ANY: Stmt = Stmt::new("documents.get_any", "SELECT * FROM $id");

pub const LIST: Stmt = Stmt::new(
    "documents.list",
    "SELECT * FROM document WHERE vault = $vault AND status != \"deleted\" ORDER BY created_at DESC LIMIT $limit START $offset",
);
pub const COUNT: Stmt =
    Stmt::new("documents.count", "SELECT count() FROM document WHERE vault = $vault AND status != \"deleted\" GROUP ALL");
/// Every document of a vault, deleted ones too (a vault clone, merge or delete).
pub const IN_VAULT: Stmt = Stmt::new("documents.in_vault", "SELECT * FROM document WHERE vault = $vault");

/// A clone or merge's copy of a document, in the vault being built.
pub const COPY: Stmt = Stmt::new(
    "documents.copy",
    "CREATE $id SET owner = $owner, vault = $vault, filename = $filename, media_type = $media_type, \
     size_bytes = $size_bytes, sha256 = $sha256, object_path = $object_path, revision = $revision, status = $status, \
     error = $error, chunk_count = $chunk_count, indexed_at = $indexed_at RETURN AFTER",
);

/// A new revision (a re-upload or a re-index), only if nobody else published one since `$expected` was read.
pub const REVISE: Stmt = Stmt::new(
    "documents.revise",
    "UPDATE $id SET filename = $filename, media_type = $media_type, size_bytes = $size_bytes, sha256 = $sha256, \
     object_path = $object_path, revision = $revision, status = \"indexing\", error = NONE, updated_at = time::now() \
     WHERE owner = $owner AND revision = $expected AND status != \"deleted\" RETURN AFTER",
);

/// The indexing job's outcome for `$revision`; a newer revision or a deletion since then wins.
pub const FINISH: Stmt = Stmt::new(
    "documents.finish",
    "UPDATE $id SET status = $status, error = $error, chunk_count = $chunk_count, indexed_at = time::now(), \
     updated_at = time::now() WHERE owner = $owner AND revision = $revision AND status = \"indexing\" RETURN AFTER",
);

pub const MARK_DELETED: Stmt =
    Stmt::new("documents.mark_deleted", "UPDATE $id SET status = \"deleted\", updated_at = time::now() WHERE owner = $owner RETURN AFTER");
pub const REMOVE: Stmt = Stmt::new("documents.remove", "DELETE $id WHERE owner = $owner AND status = \"deleted\"");

/// The live chunks of one revision, without their vectors.
pub const CHUNKS: Stmt = Stmt::new(
    "documents.chunks",
    "SELECT id, body_text, payload, embedding != NONE AS has_embedding FROM cache_record WHERE owner = $owner \
     AND source = \"documents\" AND deleted = false AND payload.document_id = $document AND payload.revision = $revision",
);
/// What a clone or merge copies of one revision's chunks.
pub const CHUNKS_TO_COPY: Stmt = Stmt::new(
    "documents.chunks_to_copy",
    "SELECT id, title, body_text, occurred_at, payload, embedding FROM cache_record WHERE owner = $owner \
     AND source = \"documents\" AND deleted = false AND payload.document_id = $document AND payload.revision = $revision",
);
/// The same with the vectors (the export).
pub const CHUNK_VECTORS: Stmt = Stmt::new(
    "documents.chunk_vectors",
    "SELECT id, body_text, payload, embedding FROM cache_record WHERE owner = $owner \
     AND source = \"documents\" AND deleted = false AND payload.document_id = $document AND payload.revision = $revision",
);

/// Removes a document's chunks of every revision but `$keep` (-1: none kept), or only those of revision
/// `$only` (-1: any), and what was drawn from
/// them, like `cache.delete_source_data` does for a whole source: the facts extracted from those chunks
/// (each affected subject's observations lose them from their lineage and go stale, or go when nothing
/// else backs them), relations extracted from them, links either end of which is one of them, and the
/// chunks. One transaction, so retrieval never sees half of it.
pub const DELETE_CHUNKS: Stmt = Stmt::at(
    "documents.delete_chunks",
    r#"BEGIN TRANSACTION;
        LET $records = (SELECT VALUE id FROM cache_record WHERE owner = $owner AND source = "documents"
            AND payload.document_id = $document AND payload.revision != $keep AND ($only < 0 OR payload.revision = $only)
            AND ($below < 0 OR payload.revision < $below));
        LET $mems = (SELECT id, subject FROM memory WHERE source IN $records);
        LET $ids = $mems.id;
        LET $subjects = array::distinct($mems.subject);
        DELETE $ids;
        DELETE memory WHERE type = "observation" AND subject IN $subjects
            AND array::len(source_memories ?? []) > 0 AND array::len(array::complement(source_memories ?? [], $ids)) = 0;
        UPDATE memory SET status = "stale", updated_at = time::now(),
            source_memories = IF source_memories THEN array::complement(source_memories, $ids) ELSE NONE END
            WHERE type = "observation" AND subject IN $subjects;
        DELETE relates_to WHERE source IN $records;
        DELETE linked_to WHERE in IN $records OR out IN $records;
        DELETE $records;
        RETURN { records: array::len($records), memories: array::len($ids) };
        COMMIT TRANSACTION;"#,
    11, // BEGIN 0, four LETs 1 to 4, six writes 5 to 10, RETURN 11
);

pub const ALL: &[&Stmt] = &[
    &OBJECT_PUT,
    &OBJECT_GET,
    &OBJECT_DELETE,
    &CREATE,
    &GET,
    &GET_ANY,
    &LIST,
    &COUNT,
    &IN_VAULT,
    &COPY,
    &REVISE,
    &FINISH,
    &MARK_DELETED,
    &REMOVE,
    &CHUNKS,
    &CHUNKS_TO_COPY,
    &CHUNK_VECTORS,
    &DELETE_CHUNKS,
];
