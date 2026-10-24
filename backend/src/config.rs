use std::env;

/// Release tag baked in at image build time (see backend/Dockerfile).
pub const APP_VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

#[derive(Clone, Debug)]
pub struct Settings {
    pub jwt_secret: String,
    pub surreal_url: String,
    pub surreal_user: String,
    pub surreal_pass: String,
    pub surreal_ns: String,
    pub surreal_db: String,
    pub openai_api_key: Option<String>,
    pub openai_base_url: String,
    /// OPENAI_CHAT_MODEL: default chat model ("" = auto, see embeddings::provider::chat_model)
    pub openai_chat_model: String,
    pub encryption_key: String,
    pub embeddings_backend: String,
    pub cors_allowed_origins: String,
    pub log_level: String,
    pub update_status_dir: String,
    /// `host:port` the HTTP server binds. `BIND_ADDR`, else `0.0.0.0:$PORT` (PORT defaults to 8001).
    pub bind_addr: String,
    /// Public origin of this server, used in OAuth metadata and as the token audience.
    pub public_url: String,
    /// `EUNOMIA_DOCUMENTS_BACKEND`: where uploaded documents' bytes go, a SurrealDB bucket backend
    /// (`file:/documents`, `s3://bucket?region=..`, `memory`). "" turns documents off. See docs/documents.md.
    pub documents_backend: String,
    /// `EUNOMIA_DOCUMENTS_MAX_BYTES`: the largest document accepted, default 25 MiB.
    pub documents_max_bytes: usize,
    /// `AUTH_MODE` and `OIDC_*`: how users sign in (password, an OpenID Connect provider, or both).
    pub auth: crate::oidc::AuthConfig,
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Settings {
    pub fn load() -> Self {
        let _ = dotenvy::dotenv();

        let jwt_secret = env::var("JWT_SECRET").unwrap_or_default();
        let jwt_secret = if jwt_secret.is_empty() {
            tracing::warn!(
                "JWT_SECRET not set -- generated a random one for this process. \
                 Sessions won't survive a restart; set JWT_SECRET explicitly in production."
            );
            uuid::Uuid::new_v4().to_string()
        } else {
            jwt_secret
        };

        Settings {
            jwt_secret,
            surreal_url: env_or("SURREAL_URL", "ws://localhost:8000/rpc"),
            surreal_user: env_or("SURREAL_USER", "root"),
            surreal_pass: env_or("SURREAL_PASS", "root"),
            surreal_ns: env_or("SURREAL_NS", "eunomia"),
            surreal_db: env_or("SURREAL_DB", "eunomia"),
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
            openai_base_url: env_or("OPENAI_BASE_URL", "https://api.openai.com/v1"),
            openai_chat_model: env_or("OPENAI_CHAT_MODEL", ""),
            encryption_key: env_or("ENCRYPTION_KEY", ""),
            embeddings_backend: env_or("EMBEDDINGS_BACKEND", "openai"),
            cors_allowed_origins: env_or("CORS_ALLOWED_ORIGINS", "http://localhost:3000"),
            log_level: env_or("LOG_LEVEL", "INFO"),
            update_status_dir: env_or("UPDATE_STATUS_DIR", "/update-status"),
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| format!("0.0.0.0:{}", env_or("PORT", "8001"))),
            public_url: env_or("PUBLIC_URL", "http://localhost:8001").trim_end_matches('/').to_string(),
            documents_backend: env_or("EUNOMIA_DOCUMENTS_BACKEND", "").trim().to_string(),
            documents_max_bytes: env::var("EUNOMIA_DOCUMENTS_MAX_BYTES").ok().and_then(|v| v.trim().parse().ok()).filter(|n| *n > 0).unwrap_or(25 << 20),
            auth: crate::oidc::AuthConfig::from_vars(|k| env::var(k).ok()).unwrap_or_else(|e| {
                // refuse to start rather than fall back to a sign-in method the operator did not choose
                eprintln!("invalid sign-in configuration: {e}");
                std::process::exit(2);
            }),
        }
    }
}
