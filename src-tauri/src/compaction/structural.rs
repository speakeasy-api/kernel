use tracing::{debug, instrument};

use super::budget::Message;

pub struct StructuralFilter;

impl StructuralFilter {
    /// Applies all structural filters to the message list.
    /// The system prompt (role "system") is never modified.
    #[instrument(skip_all, fields(input_count = messages.len()))]
    pub fn apply(messages: &[Message]) -> Vec<Message> {
        debug!(input_count = messages.len(), "applying structural filters");
        let messages = Self::collapse_tool_outputs(messages);
        debug!(after_collapse = messages.len(), "tool outputs collapsed");
        let messages = Self::deduplicate_reads(&messages);
        debug!(after_dedup = messages.len(), "duplicate reads removed");

        let result: Vec<Message> = messages
            .into_iter()
            .filter_map(|mut msg| {
                if !msg.role.eq_ignore_ascii_case("system") {
                    msg.content = Self::strip_thinking_tags(&msg.content);
                    msg.content = Self::strip_redundant_whitespace(&msg.content);
                }

                let is_system = msg.role.eq_ignore_ascii_case("system");
                let is_user = msg.role.eq_ignore_ascii_case("user");
                if is_system || is_user || !msg.content.trim().is_empty() {
                    Some(msg)
                } else {
                    None
                }
            })
            .collect();
        debug!(output_count = result.len(), "structural filtering complete");
        result
    }

    fn strip_thinking_tags(content: &str) -> String {
        let mut cleaned = content.to_string();

        while let Some(open_tag) = find_next_open_thinking_tag(&cleaned, 0) {
            let mut depth = 1usize;
            let mut cursor = open_tag.end;
            let mut close_end = None;

            while let Some(tag) = find_next_thinking_tag(&cleaned, cursor) {
                cursor = tag.end;
                if tag.is_close {
                    depth -= 1;
                    if depth == 0 {
                        close_end = Some(tag.end);
                        break;
                    }
                } else {
                    depth += 1;
                }
            }

            if let Some(end) = close_end {
                cleaned.replace_range(open_tag.start..end, "");
            } else {
                // If an opening tag is malformed/unclosed, drop only the tag token.
                cleaned.replace_range(open_tag.start..open_tag.end, "");
            }
        }

        cleaned
    }

    fn collapse_tool_outputs(messages: &[Message]) -> Vec<Message> {
        messages
            .iter()
            .map(|msg| {
                if !msg.role.eq_ignore_ascii_case("tool") {
                    return msg.clone();
                }

                let original_len = msg.content.chars().count();
                if original_len <= 2000 {
                    return msg.clone();
                }

                let head = first_n_chars(&msg.content, 500);
                let tail = last_n_chars(&msg.content, 200);
                let content =
                    format!("{head}\n... [truncated, {original_len} chars total] ...\n{tail}");

                Message {
                    role: msg.role.clone(),
                    content,
                }
            })
            .collect()
    }

    fn deduplicate_reads(messages: &[Message]) -> Vec<Message> {
        let mut deduped = messages.to_vec();

        for i in 0..messages.len() {
            let msg = &messages[i];
            if !msg.role.eq_ignore_ascii_case("tool") {
                continue;
            }

            let Some(path) = extract_read_path(&msg.content) else {
                continue;
            };

            let mut latest_duplicate = i;
            let window_end = usize::min(messages.len(), i + 11);
            let mut j = i + 1;

            // Dedup only within consecutive tool-result runs and a sliding window.
            while j < window_end {
                let next = &messages[j];
                if !next.role.eq_ignore_ascii_case("tool") {
                    break;
                }

                if extract_read_path(&next.content)
                    .as_deref()
                    .is_some_and(|next_path| next_path == path)
                {
                    latest_duplicate = j;
                }

                j += 1;
            }

            if latest_duplicate > i {
                deduped[i].content = format!("[duplicate read of {path} — see later message]");
            }
        }

        deduped
    }

    fn strip_redundant_whitespace(content: &str) -> String {
        let trimmed_lines: Vec<String> = content
            .split('\n')
            .map(|line| line.trim_end_matches(char::is_whitespace).to_string())
            .collect();

        let collapsed_spaces: Vec<String> = trimmed_lines
            .into_iter()
            .map(|line| collapse_non_indentation_spaces(&line))
            .collect();

        collapse_newline_runs(&collapsed_spaces.join("\n"))
    }
}

fn first_n_chars(input: &str, n: usize) -> String {
    input.chars().take(n).collect()
}

fn last_n_chars(input: &str, n: usize) -> String {
    let total = input.chars().count();
    if total <= n {
        return input.to_string();
    }
    input.chars().skip(total - n).collect()
}

fn extract_read_path(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim_start();

        if let Some(raw_path) = trimmed.strip_prefix("File: ") {
            return normalize_path(raw_path);
        }

        if let Some(raw_path) = trimmed.strip_prefix("Contents of ") {
            let raw_path = raw_path.trim();
            let raw_path = raw_path.strip_suffix(':').unwrap_or(raw_path);
            return normalize_path(raw_path);
        }
    }

    None
}

fn normalize_path(raw_path: &str) -> Option<String> {
    let trimmed = raw_path
        .trim()
        .trim_matches(|c| matches!(c, '`' | '"' | '\''))
        .trim_end_matches(|c| matches!(c, ',' | ';'))
        .trim();

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn collapse_non_indentation_spaces(line: &str) -> String {
    let first_non_indent = line
        .char_indices()
        .find(|(_, ch)| !matches!(ch, ' ' | '\t'))
        .map(|(idx, _)| idx)
        .unwrap_or(line.len());

    let (indent, rest) = line.split_at(first_non_indent);
    let mut out = String::with_capacity(line.len());
    out.push_str(indent);

    let mut space_run = 0usize;
    for ch in rest.chars() {
        if ch == ' ' {
            space_run += 1;
            continue;
        }

        flush_space_run(&mut out, space_run);
        space_run = 0;
        out.push(ch);
    }
    flush_space_run(&mut out, space_run);

    out
}

fn flush_space_run(out: &mut String, count: usize) {
    if count == 0 {
        return;
    }

    if count >= 3 {
        out.push(' ');
    } else {
        for _ in 0..count {
            out.push(' ');
        }
    }
}

fn collapse_newline_runs(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut newline_run = 0usize;

    for ch in input.chars() {
        if ch == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                output.push('\n');
            }
        } else {
            newline_run = 0;
            output.push(ch);
        }
    }

    output
}

#[derive(Debug, Clone, Copy)]
struct ThinkingTag {
    start: usize,
    end: usize,
    is_close: bool,
}

fn find_next_open_thinking_tag(input: &str, from: usize) -> Option<ThinkingTag> {
    let mut cursor = from;
    while let Some(tag) = find_next_thinking_tag(input, cursor) {
        if !tag.is_close {
            return Some(tag);
        }
        cursor = tag.end;
    }
    None
}

fn find_next_thinking_tag(input: &str, from: usize) -> Option<ThinkingTag> {
    let bytes = input.as_bytes();
    let mut i = from;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }

        let mut j = i + 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }

        let mut is_close = false;
        if j < bytes.len() && bytes[j] == b'/' {
            is_close = true;
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
        }

        let name_start = j;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_')
        {
            j += 1;
        }

        if name_start == j {
            i += 1;
            continue;
        }

        let tag_name = &input[name_start..j];
        if !tag_name.eq_ignore_ascii_case("thinking") {
            i += 1;
            continue;
        }

        while j < bytes.len() && bytes[j] != b'>' {
            j += 1;
        }
        if j >= bytes.len() {
            return None;
        }

        return Some(ThinkingTag {
            start: i,
            end: j + 1,
            is_close,
        });
    }

    None
}
