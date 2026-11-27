//! Transactional emails, sent by the worker through SMTP.
//!
//! The API never sends an email itself: it enqueues an [`EMAIL_JOB`] job
//! holding an [`EmailJob`], in the transaction of the change that causes it.
//! The worker renders it in the recipient's language and sends it.

use std::time::Duration;

use lettre::message::{Mailbox, MultiPart};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::collections::BTreeMap;

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
    /// Invitation to join a tenant.
    Invitation {
        inviter_name: String,
        tenant_name: String,
        link: String,
        valid_days: u64,
        /// The invitee will use the request portal, not the application.
        external: bool,
    },
    /// A notification about a ticket (L05-US07).
    Notification {
        tenant_name: String,
        line: NotificationLine,
        /// The ticket's page.
        link: Option<String>,
    },
    /// A member's export of a list view is ready (L06-US06).
    ExportReady {
        tenant_name: String,
        file_name: String,
        rows: i64,
        /// The export's page, which downloads it.
        link: Option<String>,
        /// How long the file is kept.
        valid_hours: i64,
    },
    /// A member's bulk action, run in the background, is done (L06-US08).
    BulkDone {
        tenant_name: String,
        /// `modify`, `transition` or `move`.
        kind: String,
        total: i64,
        changed: i64,
        refused: i64,
        /// The report's page.
        link: Option<String>,
    },
    /// The daily digest of a member's notifications, of every organization.
    Digest {
        display_name: String,
        lines: Vec<DigestLine>,
    },
}

/// What a notification tells about a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationLine {
    /// `domain::notifications::Event` as a string: `mentioned`, `assigned`…
    pub event: String,
    pub actor: String,
    pub key: String,
    pub summary: String,
    /// The status reached, by language.
    #[serde(default)]
    pub status: BTreeMap<String, String>,
    /// The beginning of the comment.
    #[serde(default)]
    pub excerpt: Option<String>,
    /// The hours to reschedule, for `absence_conflict`.
    #[serde(default)]
    pub conflict: Option<ConflictLine>,
}

/// Hours of a ticket an absence leaves no room for (L08-US04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictLine {
    pub resource_name: String,
    pub minutes: u32,
}

/// A duration in `locale`: `7 h`, `3 h 30` (`3 h 30 min` in English).
fn duration(minutes: u32, en: bool) -> String {
    let (hours, rest) = (minutes / 60, minutes % 60);
    match (rest, en) {
        (0, _) => format!("{hours} h"),
        (_, true) => format!("{hours} h {rest:02} min"),
        (_, false) => format!("{hours} h {rest:02}"),
    }
}

/// A notification in a digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestLine {
    pub tenant_name: String,
    pub line: NotificationLine,
    pub link: Option<String>,
}

/// What `line` says, in `locale`: "Inès Moreau mentioned you on PLAT-151".
pub fn notification_sentence(line: &NotificationLine, locale: &str) -> String {
    let en = locale == "en";
    let (actor, key) = (&line.actor, &line.key);
    let status = line
        .status
        .get(if en { "en" } else { "fr" })
        .or_else(|| line.status.values().next())
        .cloned()
        .unwrap_or_default();
    if let (Some(conflict), "absence_conflict") = (&line.conflict, line.event.as_str()) {
        let hours = duration(conflict.minutes, en);
        let resource = &conflict.resource_name;
        return if en {
            format!("{hours} to reschedule on {key} (absence of {resource})")
        } else {
            format!("{hours} à replanifier sur {key} (absence de {resource})")
        };
    }
    match (line.event.as_str(), en) {
        ("mentioned", true) => format!("{actor} mentioned you on {key}"),
        ("mentioned", false) => format!("{actor} vous a mentionné sur {key}"),
        ("assigned", true) => format!("{actor} assigned {key} to you"),
        ("assigned", false) => format!("{actor} vous a assigné {key}"),
        ("status_changed", true) => format!("{actor} moved {key} to {status}"),
        ("status_changed", false) => format!("{actor} a passé {key} à {status}"),
        ("commented", true) => format!("{actor} commented on {key}"),
        ("commented", false) => format!("{actor} a commenté {key}"),
        (_, true) => format!("{actor} created {key}"),
        (_, false) => format!("{actor} a créé {key}"),
    }
}

/// The lines of a notification: its sentence, the ticket's summary, the
/// comment's beginning.
fn notification_lines(line: &NotificationLine, locale: &str) -> Vec<String> {
    let mut lines = vec![notification_sentence(line, locale), line.summary.clone()];
    if let Some(excerpt) = &line.excerpt {
        lines.push(format!("« {excerpt} »"));
    }
    lines
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
        EmailMessage::Invitation {
            inviter_name,
            tenant_name,
            link,
            valid_days,
            external,
        } => invitation(locale, inviter_name, tenant_name, link, *valid_days, *external),
        EmailMessage::Notification {
            tenant_name,
            line,
            link,
        } => notification(locale, tenant_name, line, link.as_deref()),
        EmailMessage::ExportReady {
            tenant_name,
            file_name,
            rows,
            link,
            valid_hours,
        } => export_ready(
            locale,
            tenant_name,
            file_name,
            *rows,
            link.as_deref(),
            *valid_hours,
        ),
        EmailMessage::BulkDone {
            tenant_name,
            kind,
            total,
            changed,
            refused,
            link,
        } => bulk_done(
            locale,
            tenant_name,
            kind,
            *total,
            *changed,
            *refused,
            link.as_deref(),
        ),
        EmailMessage::Digest { display_name, lines } => digest(locale, display_name, lines),
    }
}

fn bulk_done(
    locale: &str,
    tenant: &str,
    kind: &str,
    total: i64,
    changed: i64,
    refused: i64,
    link: Option<&str>,
) -> RenderedEmail {
    let en = locale == "en";
    let action = match kind {
        "transition" if en => "status change",
        "transition" => "changement de statut",
        "move" if en => "move",
        "move" => "déplacement",
        // A modification, in both languages.
        _ => "modification",
    };
    let (subject, body, button, settings) = if en {
        (
            format!("Your bulk {action} is done · {tenant}"),
            vec![
                format!(
                    "Your bulk {action} of {total} tickets is done: {changed} changed, {refused} left unchanged for a reason."
                ),
                "The report tells what became of each ticket, for 7 days.".to_owned(),
            ],
            "See the report",
            "You receive this email as your notification preferences allow; change them in the application.",
        )
    } else {
        (
            format!("Votre {action} groupé est terminé · {tenant}"),
            vec![
                format!(
                    "Votre {action} groupé de {total} tickets est terminé : {changed} modifiés, {refused} laissés tels quels pour une raison."
                ),
                "Le rapport dit ce qu’il est advenu de chaque ticket, pendant 7 jours.".to_owned(),
            ],
            "Voir le rapport",
            "Vous recevez cet e-mail selon vos préférences de notification ; modifiez-les dans l’application.",
        )
    };
    let mut text = body.join("\n");
    let mut html = format!(
        "<!doctype html><html lang=\"{lang}\"><body>",
        lang = if en { "en" } else { "fr" }
    );
    for part in &body {
        html.push_str(&format!("<p>{}</p>", escape_html(part)));
    }
    if let Some(link) = link {
        text.push_str(&format!("\n\n{link}"));
        html.push_str(&format!(
            "<p><a href=\"{}\">{}</a></p>",
            escape_html(link),
            escape_html(button)
        ));
    }
    text.push_str(&format!("\n\n{settings}\n"));
    html.push_str(&format!("<p>{}</p></body></html>", escape_html(settings)));
    RenderedEmail { subject, text, html }
}

fn export_ready(
    locale: &str,
    tenant: &str,
    file_name: &str,
    rows: i64,
    link: Option<&str>,
    valid_hours: i64,
) -> RenderedEmail {
    let en = locale == "en";
    let (subject, body, button, settings) = if en {
        (
            format!("Your export {file_name} is ready · {tenant}"),
            vec![
                format!("Your export {file_name} is ready: {rows} rows."),
                format!("It can be downloaded for {valid_hours} hours, by you only."),
            ],
            "Download the export",
            "You receive this email as your notification preferences allow; change them in the application.",
        )
    } else {
        (
            format!("Votre export {file_name} est prêt · {tenant}"),
            vec![
                format!("Votre export {file_name} est prêt : {rows} lignes."),
                format!("Il peut être téléchargé pendant {valid_hours} heures, par vous seul."),
            ],
            "Télécharger l’export",
            "Vous recevez cet e-mail selon vos préférences de notification ; modifiez-les dans l’application.",
        )
    };
    let mut text = body.join("\n");
    let mut html = format!(
        "<!doctype html><html lang=\"{lang}\"><body>",
        lang = if en { "en" } else { "fr" }
    );
    for part in &body {
        html.push_str(&format!("<p>{}</p>", escape_html(part)));
    }
    if let Some(link) = link {
        text.push_str(&format!("\n\n{link}"));
        html.push_str(&format!(
            "<p><a href=\"{}\">{}</a></p>",
            escape_html(link),
            escape_html(button)
        ));
    }
    text.push_str(&format!("\n\n{settings}\n"));
    html.push_str(&format!("<p>{}</p></body></html>", escape_html(settings)));
    RenderedEmail { subject, text, html }
}

fn notification(locale: &str, tenant: &str, line: &NotificationLine, link: Option<&str>) -> RenderedEmail {
    let en = locale == "en";
    let subject = format!("{} · {tenant}", notification_sentence(line, locale));
    let (button, settings) = if en {
        (
            "Open the ticket",
            "You receive this email as your notification preferences allow; change them in the application.",
        )
    } else {
        (
            "Ouvrir le ticket",
            "Vous recevez cet e-mail selon vos préférences de notification ; modifiez-les dans l’application.",
        )
    };
    let body = notification_lines(line, locale);
    let mut text = body.join("\n");
    let mut html = format!(
        "<!doctype html><html lang=\"{lang}\"><body>",
        lang = if en { "en" } else { "fr" }
    );
    for part in &body {
        html.push_str(&format!("<p>{}</p>", escape_html(part)));
    }
    if let Some(link) = link {
        text.push_str(&format!("\n\n{link}"));
        html.push_str(&format!(
            "<p><a href=\"{}\">{}</a></p>",
            escape_html(link),
            escape_html(button)
        ));
    }
    text.push_str(&format!("\n\n{settings}\n"));
    html.push_str(&format!("<p>{}</p></body></html>", escape_html(settings)));
    RenderedEmail { subject, text, html }
}

fn digest(locale: &str, name: &str, lines: &[DigestLine]) -> RenderedEmail {
    let en = locale == "en";
    let count = lines.len();
    let (subject, hello, settings) = if en {
        (
            format!(
                "Your day in {}: {count} notification{}",
                domain::PRODUCT_NAME,
                if count > 1 { "s" } else { "" }
            ),
            format!("Hello {name},"),
            "This digest replaces one email per notification, as you chose; change it in the application.",
        )
    } else {
        (
            format!(
                "Votre journée sur {} : {count} notification{}",
                domain::PRODUCT_NAME,
                if count > 1 { "s" } else { "" }
            ),
            format!("Bonjour {name},"),
            "Ce résumé remplace un e-mail par notification, comme vous l’avez choisi ; modifiez-le dans l’application.",
        )
    };
    let mut text = format!("{hello}\n");
    let mut html = format!(
        "<!doctype html><html lang=\"{lang}\"><body><p>{hello}</p><ul>",
        lang = if en { "en" } else { "fr" },
        hello = escape_html(&hello)
    );
    for entry in lines {
        let parts = notification_lines(&entry.line, locale);
        text.push_str(&format!("\n[{}] {}\n", entry.tenant_name, parts.join(" · ")));
        html.push_str(&format!(
            "<li><strong>{}</strong> · {}",
            escape_html(&entry.tenant_name),
            escape_html(&parts.join(" · "))
        ));
        if let Some(link) = &entry.link {
            text.push_str(&format!("{link}\n"));
            html.push_str(&format!(" · <a href=\"{0}\">{0}</a>", escape_html(link)));
        }
        html.push_str("</li>");
    }
    text.push_str(&format!("\n{settings}\n"));
    html.push_str(&format!("</ul><p>{}</p></body></html>", escape_html(settings)));
    RenderedEmail { subject, text, html }
}

fn invitation(
    locale: &str,
    inviter: &str,
    tenant: &str,
    link: &str,
    days: u64,
    external: bool,
) -> RenderedEmail {
    let (subject, hello, body, button, ignore) = if locale == "en" {
        let place = if external { "request portal" } else { "workspace" };
        (
            format!("{inviter} invites you to {tenant}"),
            "Hello,".to_owned(),
            format!(
                "{inviter} invites you to join the {place} of {tenant} on {product}. \
                 This link is valid for {days} days:",
                product = domain::PRODUCT_NAME
            ),
            "Accept the invitation".to_owned(),
            "If you do not expect this invitation, ignore this email.".to_owned(),
        )
    } else {
        let place = if external {
            "le portail de demandes"
        } else {
            "l’espace de travail"
        };
        (
            format!("{inviter} vous invite à rejoindre {tenant}"),
            "Bonjour,".to_owned(),
            format!(
                "{inviter} vous invite à rejoindre {place} de {tenant} sur {product}. \
                 Ce lien est valable {days} jours :",
                product = domain::PRODUCT_NAME
            ),
            "Accepter l’invitation".to_owned(),
            "Si vous n’attendiez pas cette invitation, ignorez cet e-mail.".to_owned(),
        )
    };
    message(locale, subject, &hello, &body, link, &button, &ignore)
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
    message(locale, subject, &hello, &body, link, &button, &ignore)
}

/// An email with a greeting, a paragraph, a link as button and a closing line.
fn message(
    locale: &str,
    subject: String,
    hello: &str,
    body: &str,
    link: &str,
    button: &str,
    ignore: &str,
) -> RenderedEmail {
    let text = format!("{hello}\n\n{body}\n\n{link}\n\n{ignore}\n");
    let html = format!(
        "<!doctype html><html lang=\"{lang}\"><body>\
         <p>{hello}</p><p>{body}</p>\
         <p><a href=\"{link_attr}\">{button}</a></p>\
         <p>{ignore}</p></body></html>",
        lang = if locale == "en" { "en" } else { "fr" },
        hello = escape_html(hello),
        body = escape_html(body),
        link_attr = escape_html(link),
        button = escape_html(button),
        ignore = escape_html(ignore),
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

    fn invitation(external: bool) -> EmailMessage {
        EmailMessage::Invitation {
            inviter_name: "Lucas <i>".to_owned(),
            tenant_name: "Acme".to_owned(),
            link: "https://acme.example.org/accept-invitation?token=abc".to_owned(),
            valid_days: 7,
            external,
        }
    }

    #[test]
    fn invitations_name_the_inviter_the_tenant_and_the_deadline() {
        let email = render(&invitation(false), "fr");
        assert_eq!(email.subject, "Lucas <i> vous invite à rejoindre Acme");
        assert!(
            email.text.contains("rejoindre l’espace de travail de Acme"),
            "{}",
            email.text
        );
        assert!(email.text.contains("valable 7 jours"), "{}", email.text);
        assert!(
            email
                .text
                .contains("https://acme.example.org/accept-invitation?token=abc")
        );
        assert!(email.html.contains("Lucas &lt;i&gt;"), "{}", email.html);
        assert!(email.html.contains("Accepter l’invitation"));

        let email = render(&invitation(true), "en");
        assert_eq!(email.subject, "Lucas <i> invites you to Acme");
        assert!(
            email.text.contains("join the request portal of Acme"),
            "{}",
            email.text
        );
        assert!(email.text.contains("valid for 7 days"), "{}", email.text);
        assert!(email.html.contains("Accept the invitation"));
    }

    fn line(event: &str) -> NotificationLine {
        NotificationLine {
            event: event.to_owned(),
            actor: "Inès Moreau".to_owned(),
            key: "PLAT-151".to_owned(),
            summary: "Politique MFA <v2>".to_owned(),
            status: [
                ("fr".to_owned(), "En cours".to_owned()),
                ("en".to_owned(), "In progress".to_owned()),
            ]
            .into(),
            excerpt: Some("@Lucas peux-tu valider ?".to_owned()),
            conflict: None,
        }
    }

    #[test]
    fn an_absence_conflict_tells_the_hours_to_reschedule() {
        let conflict = NotificationLine {
            excerpt: None,
            conflict: Some(ConflictLine {
                resource_name: "Nathan Roux".to_owned(),
                minutes: 450,
            }),
            ..line("absence_conflict")
        };
        assert_eq!(
            notification_sentence(&conflict, "fr"),
            "7 h 30 à replanifier sur PLAT-151 (absence de Nathan Roux)"
        );
        assert_eq!(
            notification_sentence(&conflict, "en"),
            "7 h 30 min to reschedule on PLAT-151 (absence of Nathan Roux)"
        );
        let whole = NotificationLine {
            conflict: Some(ConflictLine {
                resource_name: "Nathan Roux".to_owned(),
                minutes: 420,
            }),
            ..conflict
        };
        assert!(notification_sentence(&whole, "en").starts_with("7 h to reschedule"));
    }

    #[test]
    fn a_notification_tells_what_happened_in_the_members_language() {
        let message = EmailMessage::Notification {
            tenant_name: "Acme".to_owned(),
            line: line("mentioned"),
            link: Some("https://acme.example.org/issues/PLAT-151".to_owned()),
        };
        let fr = render(&message, "fr");
        assert_eq!(fr.subject, "Inès Moreau vous a mentionné sur PLAT-151 · Acme");
        assert!(fr.text.contains("« @Lucas peux-tu valider ? »"));
        assert!(
            fr.html.contains("Politique MFA &lt;v2&gt;"),
            "escaped: {}",
            fr.html
        );
        assert!(
            fr.html
                .contains("href=\"https://acme.example.org/issues/PLAT-151\"")
        );
        let en = render(&message, "en");
        assert_eq!(en.subject, "Inès Moreau mentioned you on PLAT-151 · Acme");
        assert_eq!(
            notification_sentence(&line("status_changed"), "en"),
            "Inès Moreau moved PLAT-151 to In progress"
        );
        assert_eq!(
            notification_sentence(&line("status_changed"), "fr"),
            "Inès Moreau a passé PLAT-151 à En cours"
        );
        assert_eq!(
            notification_sentence(&line("assigned"), "fr"),
            "Inès Moreau vous a assigné PLAT-151"
        );
        assert_eq!(
            notification_sentence(&line("created"), "en"),
            "Inès Moreau created PLAT-151"
        );
    }

    #[test]
    fn a_digest_gathers_the_notifications_of_every_organization() {
        let lines = vec![
            DigestLine {
                tenant_name: "Acme".to_owned(),
                line: line("commented"),
                link: Some("https://acme.example.org/issues/PLAT-151".to_owned()),
            },
            DigestLine {
                tenant_name: "Acme Services".to_owned(),
                line: line("assigned"),
                link: None,
            },
        ];
        let message = EmailMessage::Digest {
            display_name: "Lucas".to_owned(),
            lines,
        };
        let en = render(&message, "en");
        assert!(en.subject.ends_with(": 2 notifications"), "{}", en.subject);
        assert!(en.text.contains("[Acme] Inès Moreau commented on PLAT-151"));
        assert!(
            en.text
                .contains("[Acme Services] Inès Moreau assigned PLAT-151 to you")
        );
        assert_eq!(en.html.matches("<li>").count(), 2);
        assert!(render(&message, "fr").subject.starts_with("Votre journée"));
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

    #[test]
    fn an_export_ready_email_links_to_the_export_in_the_members_language() {
        let message = EmailMessage::ExportReady {
            tenant_name: "Acme <Industrie>".to_owned(),
            file_name: "search-2026-11-18.csv".to_owned(),
            rows: 20_000,
            link: Some("https://acme.example.org/exports/e1".to_owned()),
            valid_hours: 24,
        };
        let en = render(&message, "en");
        assert_eq!(
            en.subject,
            "Your export search-2026-11-18.csv is ready · Acme <Industrie>"
        );
        assert!(en.text.starts_with("Your export search-2026-11-18.csv is ready: 20000 rows.\nIt can be downloaded for 24 hours, by you only.\n\nhttps://acme.example.org/exports/e1"));
        assert!(
            en.html
                .contains("<a href=\"https://acme.example.org/exports/e1\">Download the export</a>")
        );
        let fr = render(&message, "fr");
        assert_eq!(
            fr.subject,
            "Votre export search-2026-11-18.csv est prêt · Acme <Industrie>"
        );
        assert!(fr.html.contains("Télécharger l’export"));
        assert!(fr.text.contains("pendant 24 heures"));
        let serialized = serde_json::to_value(&message).unwrap();
        assert_eq!(serialized["type"], "export_ready");
        assert_eq!(
            serde_json::from_value::<EmailMessage>(serialized).unwrap(),
            message
        );
    }

    #[test]
    fn a_bulk_done_email_links_to_its_report_in_the_members_language() {
        let message = EmailMessage::BulkDone {
            tenant_name: "Acme".to_owned(),
            kind: "transition".to_owned(),
            total: 120,
            changed: 117,
            refused: 3,
            link: Some("https://acme.example.org/bulk-actions/b1".to_owned()),
        };
        let en = render(&message, "en");
        assert_eq!(en.subject, "Your bulk status change is done · Acme");
        assert!(en.text.starts_with(
            "Your bulk status change of 120 tickets is done: 117 changed, 3 left unchanged for a reason."
        ));
        assert!(
            en.html
                .contains("<a href=\"https://acme.example.org/bulk-actions/b1\">See the report</a>")
        );
        let fr = render(&message, "fr");
        assert_eq!(fr.subject, "Votre changement de statut groupé est terminé · Acme");
        let serialized = serde_json::to_value(&message).unwrap();
        assert_eq!(serialized["type"], "bulk_done");
        assert_eq!(
            serde_json::from_value::<EmailMessage>(serialized).unwrap(),
            message
        );
    }
}
