//! The only Markdown parser for assistant replies. The browser renders these nodes verbatim.
use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatNode {
    Text { text: String },
    Paragraph { #[schema(no_recursion)] children: Vec<ChatNode> },
    Heading { level: u8, #[schema(no_recursion)] children: Vec<ChatNode> },
    List { start: Option<u64>, #[schema(no_recursion)] children: Vec<ChatNode> },
    Item { #[schema(no_recursion)] children: Vec<ChatNode> },
    Table { #[schema(no_recursion)] children: Vec<ChatNode> },
    TableHead { #[schema(no_recursion)] children: Vec<ChatNode> },
    TableRow { #[schema(no_recursion)] children: Vec<ChatNode> },
    TableCell { #[schema(no_recursion)] children: Vec<ChatNode> },
    Strong { #[schema(no_recursion)] children: Vec<ChatNode> },
    Em { #[schema(no_recursion)] children: Vec<ChatNode> },
    CodeSpan { #[schema(no_recursion)] children: Vec<ChatNode> },
    CodeBlock { language: String, #[schema(no_recursion)] children: Vec<ChatNode> },
    Link { href: String, title: String, autolink: bool, #[schema(no_recursion)] children: Vec<ChatNode> },
    LineBreak { #[schema(no_recursion)] children: Vec<ChatNode> },
}

impl ChatNode {
    pub(crate) fn children(&self) -> &[Self] {
        match self {
            Self::Text { .. } => &[],
            Self::Paragraph { children } | Self::Heading { children, .. } | Self::List { children, .. }
            | Self::Item { children } | Self::Table { children } | Self::TableHead { children }
            | Self::TableRow { children } | Self::TableCell { children } | Self::Strong { children }
            | Self::Em { children } | Self::CodeSpan { children } | Self::CodeBlock { children, .. }
            | Self::Link { children, .. } | Self::LineBreak { children } => children,
        }
    }
    pub(crate) fn children_mut(&mut self) -> &mut [Self] {
        match self {
            Self::Text { .. } => &mut [],
            Self::Paragraph { children } | Self::Heading { children, .. } | Self::List { children, .. }
            | Self::Item { children } | Self::Table { children } | Self::TableHead { children }
            | Self::TableRow { children } | Self::TableCell { children } | Self::Strong { children }
            | Self::Em { children } | Self::CodeSpan { children } | Self::CodeBlock { children, .. }
            | Self::Link { children, .. } | Self::LineBreak { children } => children,
        }
    }
    pub(crate) fn is_unit(&self) -> bool {
        matches!(self, Self::Paragraph { .. } | Self::Heading { .. } | Self::Item { .. }
            | Self::TableCell { .. } | Self::CodeBlock { .. })
    }
}

/// Exact DOM textContent, including the newline emitted beside a hard break.
/// Block boundaries are separate checking units even when textContent has no separator.
pub fn visible_text(nodes: &[ChatNode]) -> String {
    nodes.iter().map(|n| match n {
        ChatNode::Text { text } => text.clone(),
        _ => visible_text(n.children()),
    }).collect()
}

/// Readable plain text for exports and the model, with block boundaries intact.
pub(crate) fn plain_text(nodes: &[ChatNode]) -> String {
    fn append(nodes: &[ChatNode], out: &mut String) {
        for node in nodes {
            if let ChatNode::Text { text } = node { out.push_str(text); }
            else {
                append(node.children(), out);
                if node.is_unit() && !out.ends_with('\n') { out.push('\n'); }
            }
        }
    }
    let mut out = String::new();
    append(nodes, &mut out);
    out.trim_end_matches('\n').into()
}

pub(crate) fn parse(raw: &str) -> Vec<ChatNode> {
    fn nodes<'a>(events: &mut impl Iterator<Item = Event<'a>>) -> Vec<ChatNode> {
        let mut out = Vec::new();
        while let Some(event) = events.next() {
            let node = match event {
                Event::Start(tag) => {
                    let children = nodes(events);
                    match tag {
                        Tag::Paragraph => ChatNode::Paragraph { children },
                        Tag::Heading { level, .. } => ChatNode::Heading { level: level as u8, children },
                        Tag::List(start) => ChatNode::List { start, children },
                        Tag::Item => ChatNode::Item { children },
                        Tag::Table(_) => ChatNode::Table { children },
                        Tag::TableHead => ChatNode::TableHead { children },
                        Tag::TableRow => ChatNode::TableRow { children },
                        Tag::TableCell => ChatNode::TableCell { children },
                        Tag::Strong => ChatNode::Strong { children },
                        Tag::Emphasis => ChatNode::Em { children },
                        Tag::CodeBlock(kind) => ChatNode::CodeBlock {
                            language: match kind { CodeBlockKind::Fenced(s) => s.into_string(), CodeBlockKind::Indented => String::new() }, children,
                        },
                        Tag::Link { link_type, dest_url, title, .. } => ChatNode::Link {
                            href: dest_url.into_string(), title: title.into_string(),
                            autolink: matches!(link_type, LinkType::Autolink | LinkType::Email), children,
                        },
                        // Images, including their alt text, are absent from the visible tree.
                        Tag::Image { .. } => continue,
                        // Block quotes add no new visible text or checking unit.
                        _ => { out.extend(children); continue; }
                    }
                }
                Event::End(_) => break,
                Event::Text(s) | Event::Html(s) | Event::InlineHtml(s) => ChatNode::Text { text: s.into_string() },
                Event::Code(s) => ChatNode::CodeSpan { children: vec![ChatNode::Text { text: s.into_string() }] },
                Event::SoftBreak => ChatNode::Text { text: " ".into() },
                Event::HardBreak => ChatNode::LineBreak { children: vec![ChatNode::Text { text: "\n".into() }] },
                Event::Rule => ChatNode::LineBreak { children: vec![ChatNode::Text { text: "\n".into() }] },
                _ => continue,
            };
            out.push(node);
        }
        out
    }
    nodes(&mut Parser::new_ext(raw, Options::ENABLE_TABLES))
}

// Store the checked tree in the existing string column. No schema migration is needed.
const STORED_TREE: &str = "\u{001e}eunomia.chat.tree.v1:";
pub(crate) fn encode(nodes: &[ChatNode]) -> String {
    format!("{STORED_TREE}{}", serde_json::to_string(nodes).expect("chat nodes serialise"))
}
pub(crate) fn decode(content: &str) -> Option<Vec<ChatNode>> {
    serde_json::from_str(content.strip_prefix(STORED_TREE)?).ok()
}
pub(crate) fn stored_text(role: &str, content: &str) -> String {
    if role == "assistant" && let Some(nodes) = decode(content) { plain_text(&nodes) }
    else { content.into() }
}
