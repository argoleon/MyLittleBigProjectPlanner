//! Transactional emails, sent by the worker through SMTP.
//!
//! The API never sends an email itself: it enqueues an [`EMAIL_JOB`] job
//! holding an [`EmailJob`], in the transaction of the change that causes it.
//! The worker renders it in the recipient's language and sends it.

use std::time::Duration;

use lettre::message::{Mailbox, MultiPart};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::{Deserialize, Serialize};

use crate::config::Secret;

/// Kind of the job sending an email.
pub const EMAIL_JOB: &str = "email.send";

/// Payload of an [`EMAIL_JOB`] job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailJob {
    pub to: String,
    /// `fr` or `en`.
    pub locale: String,
    pub message: EmailMessage,
}

/// The emails the product sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EmailMessage {
    /// Link to choose a new password.
    PasswordReset {
        display_name: String,
        tenant_name: String,
        link: String,
        valid_minutes: u64,
    },
}

/// A rendered email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEmail {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// Renders `message` in `locale` (French unless `en`).
pub fn render(message: &EmailMessage, locale: &str) -> RenderedEmail {
    match message {
        EmailMessage::PasswordReset {
            display_name,
            tenant_name,
            link,
            valid_minutes,
        } => password_reset(locale, display_name, tenant_name, link, *valid_minutes),
    }
}

fn password_reset(locale: &str, name: &str, tenant: &str, link: &str, minutes: u64) -> RenderedEmail {
    let (subject, hello, body, button, ignore) = if locale == "en" {
        (
            format!("Reset your password · {tenant}"),
            format!("Hello {name},"),
            format!(
                "Someone asked to reset the password of your account on {tenant}. \
                 This link lets you choose a new one for the next {minutes} minutes, once:"
            ),
            "Choose a new password".to_owned(),
            "If you did not ask for it, ignore this email: your password stays the same.".to_owned(),
        )
    } else {
        (
            format!("Réinitialisez votre mot de passe · {tenant}"),
            format!("Bonjour {name},"),
            format!(
                "Une demande de réinitialisation du mot de passe de votre compte {tenant} a été faite. \
                 Ce lien permet d’en choisir un nouveau pendant {minutes} minutes, une seule fois :"
            ),
            "Choisir un nouveau mot de passe".to_owned(),
            "Si vous n’êtes pas à l’origine de cette demande, ignorez cet e-mail : votre mot de passe reste inchangé."
                .to_owned(),
        )
    };
    let text = format!("{hello}\n\n{body}\n\n{link}\n\n{ignore}\n");
    let html = format!(
        "<!doctype html><html lang=\"{lang}\"><body>\
         <p>{hello}</p><p>{body}</p>\
         <p><a href=\"{link_attr}\">{button}</a></p>\
         <p>{ignore}</p></body></html>",
        lang = if locale == "en" { "en" } else { "fr" },
        hello = escape_html(&hello),
        body = escape_html(&body),
        link_attr = escape_html(link),
        button = escape_html(&button),
        ignore = escape_html(&ignore),
    );
    RenderedEmail { subject, text, html }
}

/// Escapes text for HTML: names and links come from users and settings.
fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Why an email could not be sent.
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("invalid SMTP_URL: {0}")]
    Url(String),
    #[error("invalid address: {0}")]
    Address(String),
    #[error("cannot build the email: {0}")]
    Build(String),
    #[error("the SMTP server refused or failed: {0}")]
    Send(String),
}

/// Sends emails through the SMTP server of `SMTP_URL`.
#[derive(Clone)]
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailer")
            .field("from", &self.from.to_string())
            .finish_non_exhaustive()
    }
}

impl Mailer {
    /// `smtp_url`: `smtp://host:port` (STARTTLS with `?tls=required`) or
    /// `smtps://host:port` (TLS), with optional credentials. `from`: the
    /// sender, e.g. `Planner <noreply@example.org>`.
    pub fn new(smtp_url: &Secret, from: &str) -> Result<Self, MailError> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(smtp_url.expose())
            .map_err(|e| MailError::Url(e.to_string()))?
            .timeout(Some(Duration::from_secs(20)))
            .build();
        let from = from
            .parse()
            .map_err(|e: lettre::address::AddressError| MailError::Address(e.to_string()))?;
        Ok(Self { transport, from })
    }

    /// Sends `email` to `to`.
    pub async fn send(&self, to: &str, email: &RenderedEmail) -> Result<(), MailError> {
        let to: Mailbox = to
            .parse()
            .map_err(|e: lettre::address::AddressError| MailError::Address(e.to_string()))?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(&email.subject)
            .multipart(MultiPart::alternative_plain_html(
                email.text.clone(),
                email.html.clone(),
            ))
            .map_err(|e| MailError::Build(e.to_string()))?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| MailError::Send(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() -> EmailMessage {
        EmailMessage::PasswordReset {
            display_name: "Léa <b>".to_owned(),
            tenant_name: "Acme".to_owned(),
            link: "https://acme.example.org/reset-password?token=abc&x=1".to_owned(),
            valid_minutes: 60,
        }
    }

    #[test]
    fn password_reset_is_rendered_in_french_by_default() {
        let email = render(&reset(), "fr");
        assert_eq!(email.subject, "Réinitialisez votre mot de passe · Acme");
        assert!(email.text.starts_with("Bonjour Léa <b>,"));
        assert!(email.text.contains("pendant 60 minutes"));
        assert!(
            email
                .text
                .contains("https://acme.example.org/reset-password?token=abc&x=1")
        );
        assert_eq!(render(&reset(), "de").subject, email.subject);
    }

    #[test]
    fn password_reset_is_rendered_in_english() {
        let email = render(&reset(), "en");
        assert_eq!(email.subject, "Reset your password · Acme");
        assert!(email.text.contains("for the next 60 minutes"));
        assert!(email.html.contains("lang=\"en\""));
    }

    #[test]
    fn html_escapes_names_and_links() {
        let email = render(&reset(), "fr");
        assert!(email.html.contains("Bonjour Léa &lt;b&gt;,"), "{}", email.html);
        assert!(
            email
                .html
                .contains("href=\"https://acme.example.org/reset-password?token=abc&amp;x=1\"")
        );
        assert!(!email.html.contains("<b>"));
    }

    #[test]
    fn job_payload_round_trips() {
        let job = EmailJob {
            to: "lea@acme.io".to_owned(),
            locale: "fr".to_owned(),
            message: reset(),
        };
        let json = serde_json::to_value(&job).unwrap();
        assert_eq!(json["message"]["type"], "password_reset");
        assert_eq!(serde_json::from_value::<EmailJob>(json).unwrap(), job);
    }

    #[test]
    fn mailer_rejects_bad_settings() {
        assert!(matches!(
            Mailer::new(&Secret::new("not a url"), "a@b.io"),
            Err(MailError::Url(_))
        ));
        assert!(matches!(
            Mailer::new(&Secret::new("smtp://mailpit:1025"), "nobody"),
            Err(MailError::Address(_))
        ));
    }
}
