//! Outbound email: one SMTP path that works with any provider (including Resend's SMTP relay).
//!
//! Off unless `SMTP_HOST` is set. With it off nothing in the product changes: no route sends anything,
//! and the routes that need email answer `email.unavailable`.
//!
//! - `SMTP_HOST`, `SMTP_PORT` (default by `SMTP_TLS`), `SMTP_USERNAME` and `SMTP_PASSWORD` (both or
//!   neither), `SMTP_FROM` (a mailbox such as `Eunomia <no-reply@example.com>`), `SMTP_TLS`:
//!   `starttls` (default: upgrade a plain connection, refuse to send if the server will not), `tls`
//!   (implicit TLS, usually port 465) or `none` (no encryption: a relay on a trusted local network only).
//! - TLS is rustls with the bundled webpki roots (no OpenSSL, no system trust store).
//! - A send waits at most [`SEND_TIMEOUT`]. A failure is logged with the purpose of the message and the
//!   error, never the message body, a link or a token, and is never an error for the request that
//!   wanted the mail: the caller learns only whether it was handed to the mail server.
//! - Tests use [`MailBackend::Capture`], which keeps messages in memory ([`Mailer::captured`]).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::error::{AppError, AppResult, ErrorCode};

/// The longest one send (connecting, TLS, the whole SMTP exchange) may take.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Connect in plain text and upgrade with STARTTLS; no send if the server will not upgrade.
    Starttls,
    /// TLS from the first byte (SMTPS).
    Implicit,
    /// No encryption.
    None,
}

impl TlsMode {
    fn default_port(self) -> u16 {
        match self {
            TlsMode::Starttls => 587,
            TlsMode::Implicit => 465,
            TlsMode::None => 25,
        }
    }
}

#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub credentials: Option<(String, String)>,
    pub from: Mailbox,
    pub tls: TlsMode,
}

// the password is never printed, whatever logs a `Settings`
impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.credentials.as_ref().map(|c| &c.0))
            .field("password", &self.credentials.as_ref().map(|_| "[hidden]"))
            .field("from", &self.from.to_string())
            .field("tls", &self.tls)
            .finish()
    }
}

/// Where outgoing mail goes.
#[derive(Clone, Debug, Default)]
pub enum MailBackend {
    /// No `SMTP_HOST`: email is off.
    #[default]
    Off,
    Smtp(SmtpConfig),
    /// Keeps every message in memory instead of sending it. For tests; no environment setting makes it.
    Capture,
}

impl MailBackend {
    /// Reads `SMTP_*`. `Ok(Off)` with `SMTP_HOST` unset or blank; an `Err` (a message for the operator)
    /// for a setting that cannot work, so the server stops at boot rather than run without the mail it
    /// was told to send.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<MailBackend, String> {
        let var = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let Some(host) = var("SMTP_HOST") else { return Ok(MailBackend::Off) };
        let tls = match var("SMTP_TLS").map(|v| v.to_ascii_lowercase()).as_deref() {
            None | Some("starttls") => TlsMode::Starttls,
            Some("tls" | "implicit" | "smtps") => TlsMode::Implicit,
            Some("none" | "off") => TlsMode::None,
            Some(other) => return Err(format!("SMTP_TLS must be starttls, tls or none, not {other:?}")),
        };
        let port = match var("SMTP_PORT") {
            None => tls.default_port(),
            Some(p) => p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| format!("SMTP_PORT must be a port number, not {p:?}"))?,
        };
        let from = var("SMTP_FROM").ok_or("SMTP_FROM is required when SMTP_HOST is set (for example: Eunomia <no-reply@example.com>)")?;
        let from: Mailbox = from.parse().map_err(|e| format!("SMTP_FROM is not a valid mailbox: {e}"))?;
        // the password is taken as typed: it may have leading or trailing spaces
        let credentials = match (var("SMTP_USERNAME"), get("SMTP_PASSWORD").filter(|p| !p.is_empty())) {
            (Some(u), Some(p)) => Some((u, p)),
            (None, None) => None,
            _ => return Err("SMTP_USERNAME and SMTP_PASSWORD go together: set both or neither".into()),
        };
        Ok(MailBackend::Smtp(SmtpConfig { host, port, credentials, from, tls }))
    }
}

/// One plain-text message.
#[derive(Clone, Debug)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

enum Transport {
    Off,
    Smtp { from: Mailbox, transport: Box<AsyncSmtpTransport<Tokio1Executor>> },
    Capture(Arc<Mutex<Vec<Mail>>>),
}

pub struct Mailer {
    transport: Transport,
    timeout: Duration,
}

impl Mailer {
    pub fn new(backend: &MailBackend) -> Mailer {
        Mailer::with_timeout(backend, SEND_TIMEOUT)
    }

    /// [`Self::new`] with another send timeout (a test of the timeout itself).
    pub fn with_timeout(backend: &MailBackend, timeout: Duration) -> Mailer {
        let transport = match backend {
            MailBackend::Off => Transport::Off,
            MailBackend::Capture => Transport::Capture(Default::default()),
            MailBackend::Smtp(c) => match build(c, timeout) {
                Ok(t) => Transport::Smtp { from: c.from.clone(), transport: Box::new(t) },
                Err(e) => {
                    // from_vars already checked everything that can be checked offline; a failure here is the TLS setup
                    tracing::error!(error = %e, "SMTP transport could not be set up: email is off");
                    Transport::Off
                }
            },
        };
        Mailer { transport, timeout }
    }

    /// Email can be sent: the capability the web app reads to show "Forgot password?".
    pub fn enabled(&self) -> bool {
        !matches!(self.transport, Transport::Off)
    }

    /// `email.unavailable` unless [`Self::enabled`].
    pub fn require(&self) -> AppResult<()> {
        if self.enabled() {
            return Ok(());
        }
        Err(AppError::coded(ErrorCode::EmailUnavailable, "This server doesn't send email, so this isn't available. Ask whoever runs it."))
    }

    /// Hands `mail` to the mail server, waiting at most [`SEND_TIMEOUT`]. `purpose` is for the log.
    /// `true` when the server accepted it (or the capture transport kept it). Never an error: a failure
    /// is logged, without the recipient, the body or any link.
    pub async fn send(&self, purpose: &'static str, mail: Mail) -> bool {
        match &self.transport {
            Transport::Off => false,
            Transport::Capture(sent) => {
                sent.lock().unwrap_or_else(|e| e.into_inner()).push(mail);
                true
            }
            Transport::Smtp { from, transport } => {
                let message = match message(from, &mail) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(purpose, error = %e, "email not sent: the message could not be built");
                        return false;
                    }
                };
                match tokio::time::timeout(self.timeout, transport.send(message)).await {
                    Ok(Ok(_)) => true,
                    Ok(Err(e)) => {
                        tracing::warn!(purpose, error = %e, "email not sent: the mail server refused or could not be reached");
                        false
                    }
                    Err(_) => {
                        tracing::warn!(purpose, timeout_ms = self.timeout.as_millis() as u64, "email not sent: the mail server took too long");
                        false
                    }
                }
            }
        }
    }

    /// [`Self::send`] in a task of its own, for a request that must not wait for (or reveal how long it
    /// took) the mail server.
    pub fn send_in_background(self: &Arc<Self>, purpose: &'static str, mail: Mail) {
        let me = self.clone();
        tokio::spawn(async move {
            me.send(purpose, mail).await;
        });
    }

    /// Everything the capture transport has been given, oldest first (empty for the other transports).
    pub fn captured(&self) -> Vec<Mail> {
        match &self.transport {
            Transport::Capture(sent) => sent.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            _ => Vec::new(),
        }
    }
}

fn build(c: &SmtpConfig, timeout: Duration) -> Result<AsyncSmtpTransport<Tokio1Executor>, lettre::transport::smtp::Error> {
    let builder = match c.tls {
        TlsMode::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&c.host)?,
        TlsMode::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&c.host)?,
        TlsMode::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&c.host),
    };
    let builder = builder.port(c.port).timeout(Some(timeout));
    let builder = match &c.credentials {
        Some((u, p)) => builder.credentials(Credentials::new(u.clone(), p.clone())),
        None => builder,
    };
    Ok(builder.build())
}

fn message(from: &Mailbox, mail: &Mail) -> Result<Message, String> {
    let to: Mailbox = mail.to.parse().map_err(|_| "the recipient is not a valid mailbox".to_string())?;
    Message::builder()
        .from(from.clone())
        .to(to)
        .subject(mail.subject.clone())
        .header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body(mail.body.clone())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn no_host_means_email_is_off() {
        assert!(matches!(MailBackend::from_vars(vars(&[])), Ok(MailBackend::Off)));
        assert!(matches!(MailBackend::from_vars(vars(&[("SMTP_HOST", "  ")])), Ok(MailBackend::Off)));
        // the other settings are ignored without a host
        assert!(matches!(MailBackend::from_vars(vars(&[("SMTP_FROM", "nonsense")])), Ok(MailBackend::Off)));
        assert!(!Mailer::new(&MailBackend::Off).enabled());
    }

    #[test]
    fn a_host_needs_a_sender_and_the_ports_follow_the_tls_mode() {
        assert!(MailBackend::from_vars(vars(&[("SMTP_HOST", "smtp.example.com")])).unwrap_err().contains("SMTP_FROM"));
        let from = ("SMTP_FROM", "Eunomia <no-reply@example.com>");
        let port = |extra: &[(&str, &str)]| {
            let mut all = vec![("SMTP_HOST", "smtp.example.com"), from];
            all.extend_from_slice(extra);
            match MailBackend::from_vars(vars(&all)).unwrap() {
                MailBackend::Smtp(c) => (c.port, c.tls),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(port(&[]), (587, TlsMode::Starttls));
        assert_eq!(port(&[("SMTP_TLS", "tls")]), (465, TlsMode::Implicit));
        assert_eq!(port(&[("SMTP_TLS", "NONE")]), (25, TlsMode::None));
        assert_eq!(port(&[("SMTP_TLS", "starttls"), ("SMTP_PORT", "2525")]), (2525, TlsMode::Starttls));
    }

    #[test]
    fn settings_that_cannot_work_stop_the_boot() {
        let base = [("SMTP_HOST", "smtp.example.com"), ("SMTP_FROM", "no-reply@example.com")];
        let with = |extra: (&'static str, &'static str)| MailBackend::from_vars(vars(&[base[0], base[1], extra])).unwrap_err();
        assert!(with(("SMTP_TLS", "ssl3")).contains("SMTP_TLS"));
        assert!(with(("SMTP_PORT", "smtp")).contains("SMTP_PORT"));
        assert!(with(("SMTP_PORT", "0")).contains("SMTP_PORT"));
        assert!(with(("SMTP_USERNAME", "apikey")).contains("together"));
        assert!(with(("SMTP_PASSWORD", "secret")).contains("together"));
        assert!(MailBackend::from_vars(vars(&[base[0], ("SMTP_FROM", "not a mailbox")])).unwrap_err().contains("SMTP_FROM"));
    }

    #[test]
    fn the_password_is_never_printed() {
        let b = MailBackend::from_vars(vars(&[
            ("SMTP_HOST", "smtp.example.com"),
            ("SMTP_FROM", "no-reply@example.com"),
            ("SMTP_USERNAME", "resend"),
            ("SMTP_PASSWORD", "re_super_secret"),
        ]))
        .unwrap();
        let shown = format!("{b:?}");
        assert!(!shown.contains("re_super_secret"), "{shown}");
        assert!(shown.contains("resend"));
    }

    #[tokio::test]
    async fn the_capture_transport_keeps_what_it_was_given() {
        let mailer = Arc::new(Mailer::new(&MailBackend::Capture));
        assert!(mailer.enabled());
        assert!(mailer.require().is_ok());
        assert!(mailer.send("test", Mail { to: "a@example.com".into(), subject: "s".into(), body: "b".into() }).await);
        assert_eq!(mailer.captured().len(), 1);
        let off = Mailer::new(&MailBackend::Off);
        assert_eq!(off.require().unwrap_err().code, ErrorCode::EmailUnavailable);
        assert!(!off.send("test", Mail { to: "a@example.com".into(), subject: "s".into(), body: "b".into() }).await);
    }

    fn cfg(port: u16) -> MailBackend {
        MailBackend::Smtp(SmtpConfig { host: "127.0.0.1".into(), port, credentials: None, from: "no-reply@example.com".parse().unwrap(), tls: TlsMode::None })
    }

    fn hello() -> Mail {
        Mail { to: "a@example.com".into(), subject: "s".into(), body: "b".into() }
    }

    #[tokio::test]
    async fn a_refused_connection_is_a_false_not_an_error() {
        let mailer = Mailer::new(&cfg(1)); // nothing listens
        assert!(mailer.enabled());
        assert!(!mailer.send("test", hello()).await);
    }

    #[tokio::test]
    async fn a_mail_server_that_never_answers_is_given_up_on_at_the_timeout() {
        // accepts the connection and says nothing, like a server that is hanging
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let _held = tokio::spawn(async move {
            let mut conns = Vec::new();
            while let Ok((c, _)) = listener.accept().await {
                conns.push(c);
            }
        });
        let mailer = Mailer::with_timeout(&cfg(port), Duration::from_millis(400));
        let started = std::time::Instant::now();
        assert!(!mailer.send("test", hello()).await);
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(300) && took < Duration::from_secs(5), "{took:?}");
    }
}
