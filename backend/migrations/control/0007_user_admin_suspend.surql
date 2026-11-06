-- Workspace administration from Settings > People (`routers/admin.rs`).
-- `admin`: an admin granted in the app, on top of the first account and `EUNOMIA_ADMIN_EMAILS` (`authz::is_admin`).
-- `suspended_at`: set while an admin has suspended the account. A suspended account cannot sign in, and its
-- sessions, API tokens and app connections are refused (`models_user::load_user`).
DEFINE FIELD OVERWRITE admin ON user TYPE option<bool>;
DEFINE FIELD OVERWRITE suspended_at ON user TYPE option<datetime>;
