use std::collections::HashMap;

use agentkit::core::{
    Item, ItemKind, MetadataMap, Part, TextPart, ToolCallId, ToolCallPart, ToolOutput,
    ToolResultPart,
};
use serde_json::Value;

use crate::anthropic::types::{ContentBlock, Message, Role};

/// Metadata key for the DB ordinal of a message that was loaded from the
/// conversation log. Stamped on every Item produced from a known DB row.
pub const META_ORDINAL: &str = "kernel.ordinal";
/// Metadata key indicating a pinned message (value: bool).
pub const META_PINNED: &str = "kernel.pinned";
/// Metadata key carrying the existing pinned context snippet (value: string).
/// Absent when the snippet hasn't been generated yet.
pub const META_CONTEXT_SNIPPET: &str = "kernel.context_snippet";

/// Per-message metadata used when seeding the agentkit transcript from the DB.
#[derive(Debug, Clone, Default)]
pub struct MessageMeta {
    pub ordinal: Option<i64>,
    pub pinned: bool,
    pub context_snippet: Option<String>,
}

/// Convert a kernel `Message` slice into agentkit `Item`s, stamping each
/// item with the supplied per-message metadata so that downstream compaction
/// strategies can identify pinned messages and the backend can compute
/// `up_to_ordinal` for snapshot persistence.
pub fn messages_to_items_with_meta(messages: &[Message], meta: &[MessageMeta]) -> Vec<Item> {
    messages
        .iter()
        .enumerate()
        .flat_map(|(i, msg)| {
            let m = meta.get(i).cloned().unwrap_or_default();
            message_to_items(msg, &m)
        })
        .collect()
}

fn message_to_items(message: &Message, meta: &MessageMeta) -> Vec<Item> {
    let has_tool_result = message
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

    if has_tool_result {
        let parts: Vec<Part> = message
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some(Part::ToolResult(ToolResultPart {
                    call_id: ToolCallId::new(tool_use_id.clone()),
                    output: ToolOutput::Text(content.clone()),
                    is_error: *is_error,
                    metadata: Default::default(),
                })),
                _ => None,
            })
            .collect();
        return vec![Item::new(ItemKind::Tool, parts).with_metadata(build_metadata(meta))];
    }

    let kind = match message.role {
        Role::User => ItemKind::User,
        Role::Assistant => ItemKind::Assistant,
    };

    let parts: Vec<Part> = message
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(Part::Text(TextPart {
                text: text.clone(),
                metadata: Default::default(),
            })),
            ContentBlock::ToolUse { id, name, input } => Some(Part::ToolCall(ToolCallPart {
                id: ToolCallId::new(id.clone()),
                name: name.clone(),
                input: input.clone(),
                metadata: Default::default(),
            })),
            ContentBlock::ToolResult { .. } => None,
        })
        .collect();

    vec![Item::new(kind, parts).with_metadata(build_metadata(meta))]
}

fn build_metadata(meta: &MessageMeta) -> MetadataMap {
    let mut map = MetadataMap::new();
    if let Some(ord) = meta.ordinal {
        map.insert(META_ORDINAL.to_string(), Value::from(ord));
    }
    if meta.pinned {
        map.insert(META_PINNED.to_string(), Value::Bool(true));
    }
    if let Some(snippet) = &meta.context_snippet {
        map.insert(META_CONTEXT_SNIPPET.to_string(), Value::String(snippet.clone()));
    }
    map
}

pub fn item_pinned(item: &Item) -> bool {
    item.metadata
        .get(META_PINNED)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn item_ordinal(item: &Item) -> Option<i64> {
    item.metadata.get(META_ORDINAL).and_then(Value::as_i64)
}

/// Build a meta vector aligned with `messages` using a `pinned_data` map
/// (index -> Option<context_snippet>) plus a list of optional DB ordinals.
/// `ordinals[i] == None` indicates a message that has no DB row (e.g. a
/// snapshot summary entry).
pub fn build_message_meta(
    message_count: usize,
    pinned_data: &HashMap<usize, Option<String>>,
    ordinals: &[Option<i64>],
) -> Vec<MessageMeta> {
    (0..message_count)
        .map(|i| {
            let ordinal = ordinals.get(i).and_then(|o| *o);
            let (pinned, context_snippet) = match pinned_data.get(&i) {
                Some(snippet) => (true, snippet.clone()),
                None => (false, None),
            };
            MessageMeta {
                ordinal,
                pinned,
                context_snippet,
            }
        })
        .collect()
}

pub fn items_to_messages(items: &[Item]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    for item in items {
        match item.kind {
            ItemKind::Tool => {
                let blocks: Vec<ContentBlock> = item
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::ToolResult(tr) => Some(ContentBlock::ToolResult {
                            tool_use_id: tr.call_id.0.clone(),
                            content: tool_output_to_text(&tr.output),
                            is_error: tr.is_error,
                        }),
                        _ => None,
                    })
                    .collect();
                if !blocks.is_empty() {
                    merge_or_push(
                        &mut out,
                        Message {
                            role: Role::User,
                            content: blocks,
                        },
                    );
                }
            }
            ItemKind::User => {
                let blocks = text_and_tool_blocks(&item.parts);
                if !blocks.is_empty() {
                    merge_or_push(
                        &mut out,
                        Message {
                            role: Role::User,
                            content: blocks,
                        },
                    );
                }
            }
            ItemKind::Assistant => {
                let blocks = text_and_tool_blocks(&item.parts);
                if !blocks.is_empty() {
                    merge_or_push(
                        &mut out,
                        Message {
                            role: Role::Assistant,
                            content: blocks,
                        },
                    );
                }
            }
            ItemKind::System | ItemKind::Developer | ItemKind::Context => {
                // System-level + Context (compaction summary) items are
                // surfaced via system_prompt / snapshot, not the conversation
                // message log. Skip in DB persistence.
            }
        }
    }
    out
}

fn text_and_tool_blocks(parts: &[Part]) -> Vec<ContentBlock> {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(ContentBlock::Text {
                text: t.text.clone(),
            }),
            Part::ToolCall(c) => Some(ContentBlock::ToolUse {
                id: c.id.0.clone(),
                name: c.name.clone(),
                input: c.input.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn merge_or_push(out: &mut Vec<Message>, msg: Message) {
    if let Some(last) = out.last_mut() {
        if last.role == msg.role {
            last.content.extend(msg.content);
            return;
        }
    }
    out.push(msg);
}

/// Lossy conversion to the textual `compaction::Message` form used by the
/// compaction prompt builder and token estimator. Tool calls and results
/// are flattened into bracketed prefixes so the LLM can still reason about
/// them.
pub fn messages_to_compaction(messages: &[Message]) -> Vec<crate::compaction::Message> {
    messages
        .iter()
        .map(|msg| {
            let has_tool_results = msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

            let role = if has_tool_results {
                "tool"
            } else {
                match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
            }
            .to_string();

            let content: String = msg
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::ToolUse { id, name, input } => {
                        format!(
                            "[tool_use:{id}] {name}({})",
                            serde_json::to_string(input).unwrap_or_default()
                        )
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let status = if *is_error { "error" } else { "ok" };
                        format!("[tool_result:{tool_use_id}:{status}]\n{content}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");

            crate::compaction::Message {
                role,
                content,
                pinned: false,
                context_snippet: None,
            }
        })
        .collect()
}

pub fn tool_output_to_text(output: &ToolOutput) -> String {
    match output {
        ToolOutput::Text(s) => s.clone(),
        ToolOutput::Structured(v) => v.to_string(),
        ToolOutput::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        ToolOutput::Files(_) => String::from("[files]"),
    }
}
