//! Rich text (specification §12): ticket descriptions, and comments from
//! L05-US04. The editor (Tiptap) sends a structured document; it is stored
//! as such and rendered to HTML here, from an allow-list of nodes and
//! marks: every text is escaped and every link checked, so that no markup
//! nor script can come through.

/// A mark on a run of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    Bold,
    Italic,
    Strike,
    Code,
    /// An http(s) or mailto link; others are refused when parsed.
    Link(String),
}

/// What a paragraph or a heading holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text {
        text: String,
        marks: Vec<Mark>,
    },
    HardBreak,
    /// `@person` (L05-US04): an account, with its name when written.
    Mention {
        id: u128,
        label: String,
    },
    /// A ticket picked by its key (L05-US04, `#` in the editor).
    IssueRef {
        key: String,
    },
}

/// A block of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    /// Levels 1 to 4.
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    BulletList(Vec<Vec<Block>>),
    OrderedList {
        start: u32,
        items: Vec<Vec<Block>>,
    },
    Blockquote(Vec<Block>),
    CodeBlock(String),
    HorizontalRule,
}

/// A document: its blocks in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc(pub Vec<Block>);

/// Most characters of text in a document.
pub const MAX_TEXT_CHARS: usize = 100_000;
/// Deepest nesting of lists and quotes.
pub const MAX_DEPTH: usize = 10;

/// Why a document is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    TooLong,
    TooDeep,
}

impl Refusal {
    pub const fn code(self) -> &'static str {
        match self {
            Self::TooLong => "too_long",
            Self::TooDeep => "too_deep",
        }
    }
}

/// Whether a link target is allowed: http, https or mailto, without spaces
/// or control characters.
pub fn is_allowed_href(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
        && href.len() <= 2048
        && !href.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl Doc {
    /// Whether it holds no text (blank paragraphs only).
    pub fn is_empty(&self) -> bool {
        self.plain_text().trim().is_empty() && !self.0.iter().any(|b| matches!(b, Block::HorizontalRule))
    }

    /// The document within its limits.
    pub fn validate(&self) -> Result<(), Refusal> {
        if self.plain_text().chars().count() > MAX_TEXT_CHARS {
            return Err(Refusal::TooLong);
        }
        if self.0.iter().any(|b| depth(b) > MAX_DEPTH) {
            return Err(Refusal::TooDeep);
        }
        Ok(())
    }

    /// Its text, blocks on their own lines: for search and notifications.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.0 {
            plain_block(block, &mut out);
        }
        out.trim_end().to_owned()
    }

    /// The accounts it mentions, once each, in order.
    pub fn mentions(&self) -> Vec<u128> {
        let mut found = Vec::new();
        for_each_inline(&self.0, &mut |inline| {
            if let Inline::Mention { id, .. } = inline
                && !found.contains(id)
            {
                found.push(*id);
            }
        });
        found
    }

    /// The ticket keys it refers to, picked or typed (`PLAT-120`, not in
    /// code nor in a link), once each, in order.
    pub fn issue_refs(&self) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let mut add = |key: &str| {
            if !found.iter().any(|k| k == key) {
                found.push(key.to_owned());
            }
        };
        for_each_inline(&self.0, &mut |inline| match inline {
            Inline::IssueRef { key } => add(key),
            Inline::Text { text, marks } if links_keys(marks) => {
                for (start, end) in key_spans(text) {
                    add(&text[start..end]);
                }
            }
            _ => {}
        });
        found
    }

    /// Its HTML, safe to show: escaped text, allowed elements only.
    pub fn to_html(&self) -> String {
        let mut out = String::new();
        for block in &self.0 {
            html_block(block, &mut out);
        }
        out
    }
}

fn depth(block: &Block) -> usize {
    match block {
        Block::BulletList(items) | Block::OrderedList { items, .. } => {
            1 + items.iter().flatten().map(depth).max().unwrap_or(0)
        }
        Block::Blockquote(blocks) => 1 + blocks.iter().map(depth).max().unwrap_or(0),
        _ => 1,
    }
}

fn plain_inlines(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { text, .. } => out.push_str(text),
            Inline::HardBreak => out.push('\n'),
            Inline::Mention { label, .. } => {
                out.push('@');
                out.push_str(label);
            }
            Inline::IssueRef { key } => out.push_str(key),
        }
    }
}

/// Calls `f` with every inline of `blocks`, in order.
fn for_each_inline(blocks: &[Block], f: &mut impl FnMut(&Inline)) {
    for block in blocks {
        match block {
            Block::Paragraph(content) | Block::Heading { content, .. } => content.iter().for_each(&mut *f),
            Block::BulletList(items) | Block::OrderedList { items, .. } => {
                for item in items {
                    for_each_inline(item, f);
                }
            }
            Block::Blockquote(blocks) => for_each_inline(blocks, f),
            Block::CodeBlock(_) | Block::HorizontalRule => {}
        }
    }
}

/// Whether ticket keys typed in text with `marks` become links: not in
/// code, not already in a link.
fn links_keys(marks: &[Mark]) -> bool {
    !marks.iter().any(|m| matches!(m, Mark::Code | Mark::Link(_)))
}

/// Where `text` holds ticket keys (`PLAT-120`), whole words only: byte
/// ranges.
pub fn key_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !word(bytes[i - 1]);
        if starts_word && bytes[i].is_ascii_uppercase() {
            let mut end = i;
            while end < bytes.len() && word(bytes[end]) {
                end += 1;
            }
            if crate::issues::parse_key(&text[i..end]).is_some() {
                spans.push((i, end));
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    spans
}

/// A link to the ticket `key` in the application.
fn issue_link(key: &str, out: &mut String) {
    out.push_str("<a class=\"issue-ref\" href=\"/issues/");
    out.push_str(&escape(key));
    out.push_str("\">");
    out.push_str(&escape(key));
    out.push_str("</a>");
}

fn plain_block(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(content) | Block::Heading { content, .. } => {
            plain_inlines(content, out);
            out.push('\n');
        }
        Block::BulletList(items) | Block::OrderedList { items, .. } => {
            for item in items {
                for block in item {
                    plain_block(block, out);
                }
            }
        }
        Block::Blockquote(blocks) => {
            for block in blocks {
                plain_block(block, out);
            }
        }
        Block::CodeBlock(code) => {
            out.push_str(code);
            out.push('\n');
        }
        Block::HorizontalRule => {}
    }
}

/// `text` escaped for HTML text and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

fn html_inlines(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::HardBreak => out.push_str("<br>"),
            Inline::Mention { id, label } => {
                out.push_str("<span class=\"mention\" data-id=\"");
                out.push_str(&crate::fields::format_uuid(*id));
                out.push_str("\">@");
                out.push_str(&escape(label));
                out.push_str("</span>");
            }
            Inline::IssueRef { key } => issue_link(key, out),
            Inline::Text { text, marks } => {
                let mut closing = Vec::new();
                for mark in marks {
                    match mark {
                        Mark::Bold => {
                            out.push_str("<strong>");
                            closing.push("</strong>");
                        }
                        Mark::Italic => {
                            out.push_str("<em>");
                            closing.push("</em>");
                        }
                        Mark::Strike => {
                            out.push_str("<s>");
                            closing.push("</s>");
                        }
                        Mark::Code => {
                            out.push_str("<code>");
                            closing.push("</code>");
                        }
                        Mark::Link(href) if is_allowed_href(href) => {
                            out.push_str("<a href=\"");
                            out.push_str(&escape(href));
                            out.push_str("\" rel=\"noopener noreferrer nofollow\" target=\"_blank\">");
                            closing.push("</a>");
                        }
                        Mark::Link(_) => {}
                    }
                }
                if links_keys(marks) {
                    let mut from = 0;
                    for (start, end) in key_spans(text) {
                        out.push_str(&escape(&text[from..start]));
                        issue_link(&text[start..end], out);
                        from = end;
                    }
                    out.push_str(&escape(&text[from..]));
                } else {
                    out.push_str(&escape(text));
                }
                for close in closing.iter().rev() {
                    out.push_str(close);
                }
            }
        }
    }
}

fn html_block(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(content) => {
            out.push_str("<p>");
            html_inlines(content, out);
            out.push_str("</p>");
        }
        Block::Heading { level, content } => {
            let level = (*level).clamp(1, 4);
            out.push_str(&format!("<h{level}>"));
            html_inlines(content, out);
            out.push_str(&format!("</h{level}>"));
        }
        Block::BulletList(items) => html_list("ul", None, items, out),
        Block::OrderedList { start, items } => html_list("ol", Some(*start), items, out),
        Block::Blockquote(blocks) => {
            out.push_str("<blockquote>");
            for block in blocks {
                html_block(block, out);
            }
            out.push_str("</blockquote>");
        }
        Block::CodeBlock(code) => {
            out.push_str("<pre><code>");
            out.push_str(&escape(code));
            out.push_str("</code></pre>");
        }
        Block::HorizontalRule => out.push_str("<hr>"),
    }
}

fn html_list(tag: &str, start: Option<u32>, items: &[Vec<Block>], out: &mut String) {
    match start {
        Some(start) if start != 1 => out.push_str(&format!("<{tag} start=\"{start}\">")),
        _ => out.push_str(&format!("<{tag}>")),
    }
    for item in items {
        out.push_str("<li>");
        for block in item {
            html_block(block, out);
        }
        out.push_str("</li>");
    }
    out.push_str(&format!("</{tag}>"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks: Vec::new(),
        }
    }

    fn marked(text: &str, marks: Vec<Mark>) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            marks,
        }
    }

    /// The description of mockup 03.
    fn description() -> Doc {
        Doc(vec![
            Block::Paragraph(vec![
                text("Replace the local authentication by "),
                marked("OIDC", vec![Mark::Bold]),
            ]),
            Block::Heading {
                level: 3,
                content: vec![text("Acceptance criteria")],
            },
            Block::BulletList(vec![
                vec![Block::Paragraph(vec![text("Sign in with Entra ID")])],
                vec![Block::Paragraph(vec![marked(
                    "the docs",
                    vec![
                        Mark::Link("https://example.org/a?b=1&c=2".to_owned()),
                        Mark::Italic,
                    ],
                )])],
            ]),
        ])
    }

    #[test]
    fn a_document_renders_to_html_from_its_allowed_elements() {
        assert_eq!(
            description().to_html(),
            "<p>Replace the local authentication by <strong>OIDC</strong></p>\
             <h3>Acceptance criteria</h3>\
             <ul><li><p>Sign in with Entra ID</p></li>\
             <li><p><a href=\"https://example.org/a?b=1&amp;c=2\" rel=\"noopener noreferrer nofollow\" target=\"_blank\"><em>the docs</em></a></p></li></ul>"
        );
        let more = Doc(vec![
            Block::OrderedList {
                start: 3,
                items: vec![vec![Block::Paragraph(vec![text("third")])]],
            },
            Block::Blockquote(vec![Block::Paragraph(vec![
                text("a"),
                Inline::HardBreak,
                text("b"),
            ])]),
            Block::CodeBlock("let x = 1 < 2;".to_owned()),
            Block::HorizontalRule,
        ]);
        assert_eq!(
            more.to_html(),
            "<ol start=\"3\"><li><p>third</p></li></ol><blockquote><p>a<br>b</p></blockquote>\
             <pre><code>let x = 1 &lt; 2;</code></pre><hr>"
        );
    }

    #[test]
    fn no_markup_nor_script_comes_through() {
        let hostile = Doc(vec![Block::Paragraph(vec![
            text("<script>alert('x')</script>"),
            marked("click", vec![Mark::Link("javascript:alert(1)".to_owned())]),
            marked("data", vec![Mark::Link("data:text/html,<b>".to_owned())]),
            marked(
                "quote",
                vec![Mark::Link("https://x.org/\"onmouseover=alert(1)".to_owned())],
            ),
        ])]);
        let html = hostile.to_html();
        assert_eq!(
            html,
            "<p>&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;clickdata\
             <a href=\"https://x.org/&quot;onmouseover=alert(1)\" rel=\"noopener noreferrer nofollow\" target=\"_blank\">quote</a></p>"
        );
        assert!(!html.contains("<script") && !html.contains("javascript:") && !html.contains("data:"));
        assert!(is_allowed_href("mailto:someone@example.org"));
        assert!(!is_allowed_href("https://exa mple.org"));
        assert!(!is_allowed_href("ftp://example.org"));
    }

    #[test]
    fn a_document_has_its_plain_text_and_limits() {
        assert_eq!(
            description().plain_text(),
            "Replace the local authentication by OIDC\nAcceptance criteria\nSign in with Entra ID\nthe docs"
        );
        assert!(Doc::default().is_empty());
        assert!(Doc(vec![Block::Paragraph(vec![text("  ")])]).is_empty());
        assert!(!description().is_empty());
        assert_eq!(description().validate(), Ok(()));
        let long = Doc(vec![Block::CodeBlock("a".repeat(MAX_TEXT_CHARS + 1))]);
        assert_eq!(long.validate(), Err(Refusal::TooLong));
        let mut deep = Block::Paragraph(vec![text("x")]);
        for _ in 0..=MAX_DEPTH {
            deep = Block::Blockquote(vec![deep]);
        }
        assert_eq!(Doc(vec![deep]).validate(), Err(Refusal::TooDeep));
        assert_eq!(Refusal::TooDeep.code(), "too_deep");
    }

    #[test]
    fn mentions_and_ticket_references_are_found_once_each() {
        let doc = Doc(vec![
            Block::Paragraph(vec![
                Inline::Mention {
                    id: 7,
                    label: "Camille Martin".into(),
                },
                text(" see PLAT-120 and CRM-2, and PLAT-120 again"),
                Inline::IssueRef { key: "OPS-9".into() },
            ]),
            Block::BulletList(vec![vec![Block::Paragraph(vec![
                Inline::Mention {
                    id: 7,
                    label: "Camille".into(),
                },
                Inline::Mention {
                    id: 8,
                    label: "Hugo".into(),
                },
                Inline::Text {
                    text: "not CODE-1".into(),
                    marks: vec![Mark::Code],
                },
            ])]]),
            Block::CodeBlock("LOG-3 in code".into()),
        ]);
        assert_eq!(doc.mentions(), [7, 8]);
        assert_eq!(doc.issue_refs(), ["PLAT-120", "CRM-2", "OPS-9"]);
        assert_eq!(
            doc.plain_text(),
            "@Camille Martin see PLAT-120 and CRM-2, and PLAT-120 againOPS-9\n@Camille@Hugonot CODE-1\nLOG-3 in code"
        );
    }

    #[test]
    fn ticket_keys_are_whole_words() {
        let keys = |text: &str| {
            key_spans(text)
                .into_iter()
                .map(|(s, e)| text[s..e].to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys("PLAT-120, (CRM-2) and AB-1."), ["PLAT-120", "CRM-2", "AB-1"]);
        for none in [
            "xPLAT-120",
            "PLAT-120x",
            "plat-120",
            "PLAT-0",
            "P-1",
            "PLAT_1",
            "PLAT-120-3",
            "été PLAT-",
        ] {
            assert_eq!(keys(none), Vec::<String>::new(), "{none}");
        }
        assert_eq!(keys("é PLAT-1 à"), ["PLAT-1"]);
    }

    #[test]
    fn mentions_and_keys_are_rendered_escaped_and_linked() {
        let doc = Doc(vec![Block::Paragraph(vec![
            Inline::Mention {
                id: 0x1f,
                label: "<img src=x onerror=alert(1)>".into(),
            },
            text(" fixed in PLAT-120 <script>"),
            Inline::Text {
                text: "PLAT-121".into(),
                marks: vec![Mark::Code],
            },
            Inline::Text {
                text: "PLAT-122".into(),
                marks: vec![Mark::Link("https://example.org".into())],
            },
            Inline::IssueRef { key: "OPS-9".into() },
        ])]);
        assert_eq!(
            doc.to_html(),
            "<p><span class=\"mention\" data-id=\"00000000-0000-0000-0000-00000000001f\">@&lt;img src=x onerror=alert(1)&gt;</span> \
             fixed in <a class=\"issue-ref\" href=\"/issues/PLAT-120\">PLAT-120</a> &lt;script&gt;\
             <code>PLAT-121</code>\
             <a href=\"https://example.org\" rel=\"noopener noreferrer nofollow\" target=\"_blank\">PLAT-122</a>\
             <a class=\"issue-ref\" href=\"/issues/OPS-9\">OPS-9</a></p>"
        );
    }
}
