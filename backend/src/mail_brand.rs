//! An optional HTML look for the server's emails.
//!
//! Every email this server sends is written as plain text. A hosted service run by Paperless Labs sets
//! `MAIL_BRAND=paperless-labs` and each message then also carries an HTML version in that company's
//! colours (the logo on a black band, off-white page, green button). The plain text is still sent in the
//! same message, word for word, so a client that shows only text loses nothing. With `MAIL_BRAND` unset
//! (the default, and what every self-hosted install runs) nothing changes: plain text only.
//!
//! The HTML is built from the plain message, not written alongside it, so the two cannot drift: the
//! subject is the heading, each blank-line-separated paragraph is a paragraph, and a paragraph that is
//! only a link becomes the button. Every piece of text is escaped. A link is shown only as the button's
//! target and once more as copyable text underneath, never anywhere else.

const GREEN: &str = "#3FA374";
const INK: &str = "#0E0E0E";
const PAGE: &str = "#F4F2EE";
const TEXT: &str = "#2B2A28";
const MUTED: &str = "#55524C";
const LINE: &str = "#D9D5CC";
const LINK: &str = "#1E6A47";
const LOGO_URL: &str = "https://paperlesslabs.ai/logos/paperless-labs/png/lockup-white-on-black-600w.png";
const FONT: &str = "system-ui,-apple-system,'Segoe UI',Roboto,Helvetica,Arial,sans-serif";
const LEGAL: &str = "Cashara Pty Ltd trading as Paperless Labs, ABN 92 684 673 836.";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A paragraph that is nothing but one web link.
fn only_a_link(p: &str) -> Option<&str> {
    let p = p.trim();
    let is_link = (p.starts_with("https://") || p.starts_with("http://")) && !p.chars().any(char::is_whitespace) && p.len() <= 2000 && !p.contains(['"', '<', '>']);
    is_link.then_some(p)
}

/// The button's words, from what the message is about.
fn button_label(subject: &str) -> &'static str {
    let s = subject.to_ascii_lowercase();
    if s.contains("invited") {
        "Accept the invitation"
    } else if s.contains("reset") {
        "Choose a new password"
    } else if s.contains("ready") {
        "Set up your workspace"
    } else {
        "Open the link"
    }
}

/// The Paperless Labs HTML for a plain-text message.
pub fn paperless_labs_html(subject: &str, body: &str) -> String {
    let font = format!("font-family:{FONT};");
    let label = button_label(subject);
    let mut inner = String::new();
    for p in body.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(link) = only_a_link(p) {
            inner.push_str(&format!(
                "<p style=\"margin:24px 0 8px;\"><a href=\"{link}\" style=\"display:inline-block;background:{GREEN};color:{INK};{font}font-weight:600;font-size:16px;text-decoration:none;padding:13px 22px;border-radius:10px;\">{label}</a></p>\
                 <p style=\"margin:0 0 20px;{font}font-size:13px;line-height:1.5;color:{MUTED};word-break:break-all;\">Or copy this link into your browser: <a href=\"{link}\" style=\"color:{LINK};text-decoration:underline;\">{text}</a></p>",
                link = esc(link),
                text = esc(link),
            ));
        } else {
            let html = esc(p).replace('\n', "<br>");
            inner.push_str(&format!("<p style=\"margin:0 0 16px;{font}font-size:16px;line-height:1.6;color:{TEXT};\">{html}</p>"));
        }
    }
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\"><title>{title}</title></head>\n\
         <body style=\"margin:0;padding:0;background:{PAGE};{font}\">\n\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"background:{PAGE};padding:32px 16px;\"><tr><td align=\"center\">\n\
         <table role=\"presentation\" width=\"560\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"width:100%;max-width:560px;background:#FFFFFF;border-radius:12px;overflow:hidden\">\n\
         <tr><td style=\"background:{INK};padding:22px 32px;\"><img src=\"{LOGO_URL}\" width=\"190\" alt=\"Paperless Labs\" style=\"display:block;border:0;height:auto;width:190px;max-width:100%;\"></td></tr>\n\
         <tr><td style=\"padding:32px 32px 8px;\"><h1 style=\"margin:0 0 20px;{font}font-size:26px;line-height:1.25;font-weight:700;letter-spacing:-0.01em;color:{INK};\">{title}</h1>{inner}</td></tr>\n\
         <tr><td style=\"padding:0 32px 32px;\"><hr style=\"border:none;border-top:1px solid {LINE};margin:8px 0 12px;\"><p style=\"margin:0;{font}font-size:12px;line-height:1.5;color:{MUTED};\">{LEGAL}</p></td></tr>\n\
         </table></td></tr></table></body></html>",
        title = esc(subject),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAIM: &str = "Your Cupel workspace \u{201c}Acme <b>Ltd</b>\u{201d} is ready, and you're its owner.\n\nTo set it up, open this link and sign in with this email address (a@example.com):\n\nhttps://app.example.test/invite#tok_ABC-123\n\nThe link works once and expires in 7 days.\n";

    #[test]
    fn the_subject_is_the_heading_and_the_brand_is_there() {
        let h = paperless_labs_html("Your Cupel workspace is ready", CLAIM);
        assert!(h.contains(">Your Cupel workspace is ready</h1>"));
        assert!(h.contains("lockup-white-on-black-600w.png"));
        assert!(h.contains("alt=\"Paperless Labs\""));
        assert!(h.contains(&format!("background:{PAGE}")) && h.contains(&format!("background:{INK}")));
        assert!(h.contains("ABN 92 684 673 836"));
        assert!(!h.contains("#D85608"));
    }

    #[test]
    fn a_link_only_paragraph_becomes_the_green_button_and_one_copy_line() {
        let h = paperless_labs_html("Your Cupel workspace is ready", CLAIM);
        assert!(h.contains("Set up your workspace</a>"));
        assert!(h.contains(&format!("background:{GREEN};color:{INK}")));
        // the link appears as the button target and as copyable text, and nowhere else
        assert_eq!(h.matches("https://app.example.test/invite#tok_ABC-123").count(), 3);
        assert_eq!(h.matches("href=\"https://app.example.test/invite#tok_ABC-123\"").count(), 2);
    }

    #[test]
    fn everything_in_the_message_is_escaped() {
        let h = paperless_labs_html("Hi <script>alert(1)</script>", CLAIM);
        assert!(!h.contains("<script>"));
        assert!(!h.contains("<b>Ltd</b>"));
        assert!(h.contains("Acme &lt;b&gt;Ltd&lt;/b&gt;"));
    }

    #[test]
    fn text_that_only_looks_like_a_link_is_not_a_button() {
        for p in ["https://a.example.test/x y", "https://a.example.test/\"onmouseover=\"x", "see https://a.example.test/x", "javascript:alert(1)"] {
            let h = paperless_labs_html("Reset your Cupel password", &format!("Hello\n\n{p}\n\nBye"));
            assert!(!h.contains("Choose a new password</a>"), "{p}");
        }
    }

    #[test]
    fn the_button_words_follow_the_message() {
        assert_eq!(button_label("You've been invited to Cupel"), "Accept the invitation");
        assert_eq!(button_label("Reset your Cupel password"), "Choose a new password");
        assert_eq!(button_label("Your Cupel workspace is ready"), "Set up your workspace");
        assert_eq!(button_label("Something else"), "Open the link");
    }
}
