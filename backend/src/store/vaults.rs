//! Statements for the vaults area. See the module docs in store/mod.rs.

use super::Stmt;

pub const CREATE_PERSONAL: Stmt = Stmt::new(
    "vaults.create_personal",
    r#"BEGIN TRANSACTION;
            CREATE $vault SET name = "Personal", kind = "personal";
            CREATE vault_member SET vault = $vault, user = $user, role = "owner";
            COMMIT TRANSACTION;"#,
);

pub const CREATE_VAULT: Stmt = Stmt::at(
    "vaults.create_vault",
    r#"BEGIN TRANSACTION;
                LET $v = (CREATE vault SET name = $name, kind = $kind RETURN AFTER);
                CREATE vault_member SET vault = $v[0].id, user = $user, role = "owner";
                RETURN $v;
                COMMIT TRANSACTION;"#,
    3, // BEGIN 0, LET 1, CREATE 2, RETURN 3
);

/// The target of a clone or merge: `status = "copying"` and no member yet, so nothing reads it until
/// [`PUBLISH_COPY`].
pub const STAGE_COPY: Stmt = Stmt::new(
    "vaults.stage_copy",
    r#"CREATE vault SET name = $name, kind = $kind, status = "copying" RETURN AFTER"#,
);

/// A finished copy becomes a vault: the marker goes and the caller becomes its owner, together. Throws
/// if it is no longer a copy in progress (the stale-copy cleanup removed it).
pub const PUBLISH_COPY: Stmt = Stmt::new(
    "vaults.publish_copy",
    r#"BEGIN TRANSACTION;
            LET $v = (UPDATE $vault SET status = NONE WHERE status = "copying" RETURN AFTER);
            IF array::len($v) = 0 { THROW "copy_gone" };
            CREATE vault_member SET vault = $vault, user = $user, role = "owner";
            COMMIT TRANSACTION;"#,
);

/// Everything a copy in progress wrote (relations first, while their ends still name the vault). A
/// statement per table, so no one transaction has to hold a whole vault; does nothing to a vault that
/// is not `copying`. [`DISCARD_COPY_VAULT`] runs after it succeeds, so a failure leaves the marker for
/// the next try.
pub const DISCARD_COPY_ROWS: Stmt = Stmt::new(
    "vaults.discard_copy_rows",
    r#"LET $copying = (SELECT VALUE id FROM $vault WHERE status = "copying");
            DELETE relates_to WHERE in.vault IN $copying OR out.vault IN $copying;
            DELETE memory WHERE vault IN $copying;
            DELETE person WHERE vault IN $copying;
            DELETE organisation WHERE vault IN $copying;
            DELETE location WHERE vault IN $copying;
            DELETE repository WHERE vault IN $copying;
            DELETE file WHERE vault IN $copying;
            DELETE symbol WHERE vault IN $copying;
            DELETE entity WHERE vault IN $copying;"#,
);
pub const DISCARD_COPY_VAULT: Stmt = Stmt::new("vaults.discard_copy_vault", r#"DELETE $vault WHERE status = "copying""#);

/// Copies in progress for longer than any copy takes: their process died mid-copy.
pub const STALE_COPIES: Stmt = Stmt::new(
    "vaults.stale_copies",
    r#"SELECT VALUE id FROM vault WHERE status = "copying" AND created_at < time::now() - <duration>$age"#,
);

pub const MEMBERSHIP_ACTIVE: Stmt = Stmt::new(
    "vaults.membership_active",
    r#"SELECT * FROM vault_member WHERE vault = $vault AND user = $user AND status = "active" LIMIT 1"#,
);

pub const MEMBERSHIP_ANY: Stmt =
    Stmt::new("vaults.membership_any", "SELECT * FROM vault_member WHERE vault = $vault AND user = $user LIMIT 1");

pub const ACCESSIBLE_IDS: Stmt =
    Stmt::new("vaults.accessible_ids", r#"SELECT vault FROM vault_member WHERE user = $user AND status = "active""#);

pub const DEFAULT_ID: Stmt = Stmt::new(
    "vaults.default_id",
    r#"SELECT vault, created_at FROM vault_member WHERE user = $user AND status = "active" AND vault.kind = "personal" ORDER BY created_at ASC LIMIT 1"#,
);

/// Every vault a user is an active member of, with its kind and name, oldest membership first
/// (`vaults::service::read_scope`).
pub const MY_MEMBERSHIPS: Stmt = Stmt::new(
    "vaults.my_memberships",
    r#"SELECT vault, vault.kind AS kind, vault.name AS name, created_at FROM vault_member WHERE user = $user AND status = "active" ORDER BY created_at"#,
);

pub const LIST_MINE: Stmt = Stmt::new(
    "vaults.list_mine",
    r#"SELECT vault.* AS vault, role FROM vault_member WHERE user = $user AND status = "active""#,
);

pub const RENAME: Stmt = Stmt::new("vaults.rename", "UPDATE $id SET name = $name RETURN AFTER");

pub const DELETE_VAULT: Stmt = Stmt::new(
    "vaults.delete_vault",
    "BEGIN TRANSACTION; DELETE vault_member WHERE vault = $vault; DELETE change WHERE vault = $vault; DELETE $counter; DELETE $vault; COMMIT TRANSACTION;",
);

pub const INVITE: Stmt = Stmt::new(
    "vaults.invite",
    r#"CREATE vault_member SET vault = $vault, user = $user, role = $role, status = "pending" RETURN AFTER"#,
);

pub const LIST_MEMBERS: Stmt = Stmt::new(
    "vaults.list_members",
    r#"SELECT user, role FROM vault_member WHERE vault = $vault AND status = "active""#,
);

pub const LIST_INVITATIONS: Stmt = Stmt::new(
    "vaults.list_invitations",
    r#"SELECT vault.* AS vault, role, created_at FROM vault_member WHERE user = $user AND status = "pending""#,
);

pub const ACCEPT_INVITATION: Stmt = Stmt::at(
    "vaults.accept_invitation",
    r#"BEGIN TRANSACTION;
            LET $flipped = (UPDATE $id SET status = "active" WHERE status = "pending" RETURN AFTER);
            RETURN IF array::len($flipped) > 0 { (SELECT * FROM $vault) } ELSE { [] };
            COMMIT TRANSACTION;"#,
    2, // BEGIN 0, LET 1, RETURN 2
);

pub const DELETE_RECORD: Stmt = Stmt::new("vaults.delete_record", "DELETE $id");

pub const REMOVE_MEMBER: Stmt = Stmt::new(
    "vaults.remove_member",
    r#"BEGIN TRANSACTION;
            LET $owners = (SELECT VALUE id FROM vault_member WHERE vault = $vault AND role = "owner" AND status = "active");
            IF $is_owner AND array::len($owners) <= 1 { THROW "last_owner" };
            DELETE $id;
            COMMIT TRANSACTION;"#,
);

pub const LEAVE: Stmt = Stmt::new(
    "vaults.leave",
    r#"BEGIN TRANSACTION;
            LET $owners = (SELECT VALUE id FROM vault_member WHERE vault = $vault AND role = "owner" AND status = "active");
            LET $others = (SELECT VALUE id FROM vault_member WHERE vault = $vault AND user != $user);
            IF $is_owner AND array::len($owners) <= 1 AND array::len($others) > 0 { THROW "last_owner" };
            DELETE $id;
            COMMIT TRANSACTION;"#,
);

pub const OBSERVATION_OF: Stmt = Stmt::new(
    "vaults.observation_of",
    r#"SELECT id, text FROM memory WHERE subject = $s AND type = "observation" LIMIT 1"#,
);

pub const APPEND_OBSERVATION: Stmt = Stmt::new(
    "vaults.append_observation",
    r#"UPDATE $id SET text = $text, version = version + 1, status = "stale", updated_at = time::now()"#,
);

pub const SAME_MEMORY: Stmt = Stmt::new(
    "vaults.same_memory",
    "SELECT id FROM memory WHERE subject = $s AND type = $type AND text = $text LIMIT 1",
);

pub const MERGE_ENTITY_INTO: Stmt = Stmt::new(
    "vaults.merge_entity_into",
    "UPDATE $id SET aliases = $aliases, summary = $summary, updated_at = time::now()",
);

pub const MEMORIES_OF: Stmt = Stmt::new("vaults.memories_of", "SELECT * FROM memory WHERE subject = $id AND vault = $vault");

pub const COPY_MEMORY: Stmt = Stmt::new(
    "vaults.copy_memory",
    "CREATE memory SET owner = $owner, vault = $vault, subject = $subject, text = $text, \
                 type = $type, source = $source RETURN AFTER",
);

pub const RELATIONS_FROM: Stmt = Stmt::new("vaults.relations_from", "SELECT * FROM relates_to WHERE in = $id");

pub const SAME_RELATION: Stmt = Stmt::new(
    "vaults.same_relation",
    "SELECT id FROM relates_to WHERE in = $in AND out = $out AND label = $label LIMIT 1",
);

pub const COPY_RELATION: Stmt =
    Stmt::new("vaults.copy_relation", "RELATE $in->relates_to->$out SET label = $label, owner = $owner");

pub const ALL: &[&Stmt] = &[
    &CREATE_PERSONAL,
    &CREATE_VAULT,
    &STAGE_COPY,
    &PUBLISH_COPY,
    &DISCARD_COPY_ROWS,
    &DISCARD_COPY_VAULT,
    &STALE_COPIES,
    &MEMBERSHIP_ACTIVE,
    &MEMBERSHIP_ANY,
    &ACCESSIBLE_IDS,
    &DEFAULT_ID,
    &MY_MEMBERSHIPS,
    &LIST_MINE,
    &RENAME,
    &DELETE_VAULT,
    &INVITE,
    &LIST_MEMBERS,
    &LIST_INVITATIONS,
    &ACCEPT_INVITATION,
    &DELETE_RECORD,
    &REMOVE_MEMBER,
    &LEAVE,
    &OBSERVATION_OF,
    &APPEND_OBSERVATION,
    &SAME_MEMORY,
    &MERGE_ENTITY_INTO,
    &MEMORIES_OF,
    &COPY_MEMORY,
    &RELATIONS_FROM,
    &SAME_RELATION,
    &COPY_RELATION,
];
