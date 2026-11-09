use super::Stmt;

macro_rules! activity_sql {
    ($order:literal) => { concat!(r#"SELECT seq, at, actor, actor_type, actor_name, action, definition_type, kind, name, target, revision, reason, before, after
        FROM ontology_event WHERE vault = $vault AND seq > $after AND ($before = NONE OR seq < $before)
        AND ($actor = NONE OR actor = $actor) AND ($actor_type = NONE OR actor_type = $actor_type)
        AND ($action = NONE OR action = $action) AND ($definition_type = NONE OR definition_type = $definition_type)
        AND ($kind = NONE OR kind = $kind) AND ($target = NONE OR target = $target) AND ($name = NONE OR name = $name)
        AND (!$review_only OR action IN ["proposed", "approved", "rejected", "merged", "described"])
        ORDER BY seq "#, $order, " LIMIT $limit") };
}

pub const PAGE: Stmt = Stmt::new("ontology_activity.page", activity_sql!("ASC"));
pub const NEWEST: Stmt = Stmt::new("ontology_activity.newest", activity_sql!("DESC"));
pub const HEAD: Stmt = Stmt::new(
    "ontology_activity.head",
    "SELECT VALUE ontology_seq FROM $vault",
);
pub const REVISION: Stmt = Stmt::new(
    "ontology_activity.revision",
    "SELECT VALUE revision FROM $id",
);
pub const ALL: &[&Stmt] = &[&PAGE, &NEWEST, &HEAD, &REVISION];
