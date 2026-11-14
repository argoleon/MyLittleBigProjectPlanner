//! Rich text as the editor (Tiptap) sends it, read into and written from
//! `domain::rich_text::Doc` (L05-US01). Only the nodes and marks of the
//! allow-list are read; anything else refuses the document, so that what
//! is stored is what is rendered.

use domain::rich_text::{Block, Doc, Inline, Mark, is_allowed_href};
use serde_json::{Map, Value, json};

/// Why a document is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

type Read<T> = Result<T, Invalid>;

fn invalid<T>(what: impl Into<String>) -> Read<T> {
    Err(Invalid(what.into()))
}

fn kind(node: &Value) -> Read<&str> {
    node.get("type")
        .and_then(Value::as_str)
        .map_or_else(|| invalid("a node without type"), Ok)
}

fn content(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn attr<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("attrs").and_then(|a| a.get(name))
}

/// The document `json` (`{"type": "doc", "content": […]}`).
pub fn parse(json: &Value) -> Read<Doc> {
    if kind(json)? != "doc" {
        return invalid("not a document");
    }
    content(json).iter().map(block).collect::<Read<Vec<_>>>().map(Doc)
}

fn blocks(nodes: &[Value]) -> Read<Vec<Block>> {
    nodes.iter().map(block).collect()
}

fn block(node: &Value) -> Read<Block> {
    Ok(match kind(node)? {
        "paragraph" => Block::Paragraph(inlines(content(node))?),
        "heading" => Block::Heading {
            level: attr(node, "level")
                .and_then(Value::as_u64)
                .and_then(|l| u8::try_from(l).ok())
                .filter(|l| (1..=4).contains(l))
                .unwrap_or(2),
            content: inlines(content(node))?,
        },
        "bulletList" => Block::BulletList(items(content(node))?),
        "orderedList" => Block::OrderedList {
            start: attr(node, "start")
                .and_then(Value::as_u64)
                .and_then(|s| u32::try_from(s).ok())
                .unwrap_or(1),
            items: items(content(node))?,
        },
        "blockquote" => Block::Blockquote(blocks(content(node))?),
        "codeBlock" => Block::CodeBlock(
            content(node)
                .iter()
                .map(|t| t.get("text").and_then(Value::as_str).unwrap_or_default())
                .collect(),
        ),
        "horizontalRule" => Block::HorizontalRule,
        other => return invalid(format!("unsupported block {other}")),
    })
}

fn items(nodes: &[Value]) -> Read<Vec<Vec<Block>>> {
    nodes
        .iter()
        .map(|item| match kind(item)? {
            "listItem" => blocks(content(item)),
            other => invalid(format!("unsupported list item {other}")),
        })
        .collect()
}

fn inlines(nodes: &[Value]) -> Read<Vec<Inline>> {
    nodes
        .iter()
        .map(|node| match kind(node)? {
            "hardBreak" => Ok(Inline::HardBreak),
            "text" => Ok(Inline::Text {
                text: node
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                marks: node
                    .get("marks")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .map(mark)
                    .collect::<Read<_>>()?,
            }),
            other => invalid(format!("unsupported inline {other}")),
        })
        .collect()
}

fn mark(node: &Value) -> Read<Mark> {
    Ok(match kind(node)? {
        "bold" => Mark::Bold,
        "italic" => Mark::Italic,
        "strike" => Mark::Strike,
        "code" => Mark::Code,
        "link" => {
            let href = attr(node, "href").and_then(Value::as_str).unwrap_or_default();
            if !is_allowed_href(href) {
                return invalid("a link that is not http(s) or mailto");
            }
            Mark::Link(href.to_owned())
        }
        other => return invalid(format!("unsupported mark {other}")),
    })
}

/// The JSON of a document, as the editor reads it.
pub fn to_json(doc: &Doc) -> Value {
    json!({"type": "doc", "content": doc.0.iter().map(block_json).collect::<Vec<_>>()})
}

fn node(kind: &str, content: Vec<Value>) -> Value {
    let mut node = Map::new();
    node.insert("type".to_owned(), Value::String(kind.to_owned()));
    if !content.is_empty() {
        node.insert("content".to_owned(), Value::Array(content));
    }
    Value::Object(node)
}

fn with_attrs(mut value: Value, attrs: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.insert("attrs".to_owned(), attrs);
    }
    value
}

fn block_json(block: &Block) -> Value {
    match block {
        Block::Paragraph(content) => node("paragraph", inlines_json(content)),
        Block::Heading { level, content } => {
            with_attrs(node("heading", inlines_json(content)), json!({"level": level}))
        }
        Block::BulletList(items) => node("bulletList", items_json(items)),
        Block::OrderedList { start, items } => {
            with_attrs(node("orderedList", items_json(items)), json!({"start": start}))
        }
        Block::Blockquote(blocks) => node("blockquote", blocks.iter().map(block_json).collect()),
        Block::CodeBlock(code) if code.is_empty() => node("codeBlock", Vec::new()),
        Block::CodeBlock(code) => node("codeBlock", vec![json!({"type": "text", "text": code})]),
        Block::HorizontalRule => node("horizontalRule", Vec::new()),
    }
}

fn items_json(items: &[Vec<Block>]) -> Vec<Value> {
    items
        .iter()
        .map(|item| node("listItem", item.iter().map(block_json).collect()))
        .collect()
}

fn inlines_json(inlines: &[Inline]) -> Vec<Value> {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::HardBreak => json!({"type": "hardBreak"}),
            Inline::Text { text, marks } if marks.is_empty() => json!({"type": "text", "text": text}),
            Inline::Text { text, marks } => json!({
                "type": "text",
                "text": text,
                "marks": marks.iter().map(|m| match m {
                    Mark::Bold => json!({"type": "bold"}),
                    Mark::Italic => json!({"type": "italic"}),
                    Mark::Strike => json!({"type": "strike"}),
                    Mark::Code => json!({"type": "code"}),
                    Mark::Link(href) => json!({"type": "link", "attrs": {"href": href}}),
                }).collect::<Vec<_>>(),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_editor_document_reads_and_writes_back_the_same() {
        let json = json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [
                {"type": "text", "text": "Replace by "},
                {"type": "text", "text": "OIDC", "marks": [{"type": "bold"}, {"type": "link", "attrs": {"href": "https://example.org"}}]},
                {"type": "hardBreak"}
            ]},
            {"type": "heading", "attrs": {"level": 3}, "content": [{"type": "text", "text": "Criteria"}]},
            {"type": "bulletList", "content": [{"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Entra ID"}]}]}]},
            {"type": "orderedList", "attrs": {"start": 2}, "content": [{"type": "listItem", "content": [{"type": "paragraph"}]}]},
            {"type": "blockquote", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "q", "marks": [{"type": "italic"}, {"type": "strike"}, {"type": "code"}]}]}]},
            {"type": "codeBlock", "content": [{"type": "text", "text": "x < 1"}]},
            {"type": "horizontalRule"}
        ]});
        let doc = parse(&json).unwrap();
        assert_eq!(doc.0.len(), 7);
        assert_eq!(to_json(&doc), json);
        assert!(
            doc.to_html()
                .starts_with("<p>Replace by <strong><a href=\"https://example.org\"")
        );
    }

    #[test]
    fn what_is_not_allowed_refuses_the_document() {
        for hostile in [
            json!({"type": "paragraph"}),
            json!({"type": "doc", "content": [{"type": "iframe", "attrs": {"src": "https://x"}}]}),
            json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "image", "attrs": {"src": "x"}}]}]}),
            json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": [{"type": "link", "attrs": {"href": "javascript:alert(1)"}}]}]}]}),
            json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": [{"type": "textStyle", "attrs": {"color": "red"}}]}]}]}),
            json!({"type": "doc", "content": [{"content": []}]}),
        ] {
            assert!(parse(&hostile).is_err(), "{hostile}");
        }
        assert_eq!(parse(&json!({"type": "doc"})).unwrap(), Doc::default());
    }
}
