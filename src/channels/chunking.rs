//! Code-fence-aware text chunking for channels with message size limits.

/// Split text into chunks that fit within `max_chars`, preserving code fence integrity.
///
/// - Prefers splitting at newline or whitespace boundaries over mid-word
/// - If a `` ``` `` code fence spans a chunk boundary, the fence is closed at the
///   end of the current chunk and reopened at the start of the next
/// - Returns a single-element vec if the text fits within the limit
/// - Returns an empty vec for empty input
#[must_use]
pub fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() || max_chars == 0 {
        return Vec::new();
    }
    if text.len() <= max_chars {
        return vec![text.to_string()];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut pos = 0;
    let mut in_fence = false;
    let mut fence_header = String::new();

    while pos < text.len() {
        let remaining = text.get(pos..).unwrap_or("");

        // Build prefix for this chunk (reopen fence if needed)
        let prefix = if in_fence {
            format!("{fence_header}\n")
        } else {
            String::new()
        };

        // Suffix we may need to append (close fence if still open at end of chunk)
        let suffix_reserve: usize = if in_fence { 4 } else { 0 }; // "\n```"

        let budget = max_chars
            .saturating_sub(prefix.len())
            .saturating_sub(suffix_reserve);

        if budget == 0 {
            break;
        }

        if remaining.len() <= budget {
            // Last chunk — fits entirely
            let mut chunk = prefix;
            chunk.push_str(remaining);
            update_fence_state(remaining, &mut in_fence, &mut fence_header);
            if in_fence {
                chunk.push_str("\n```");
            }
            chunks.push(chunk);
            break;
        }

        let split_at = find_split_point(remaining, budget);

        let slice = remaining.get(..split_at).unwrap_or("");

        update_fence_state(slice, &mut in_fence, &mut fence_header);

        let mut chunk = prefix;
        chunk.push_str(slice);

        if in_fence {
            chunk.push_str("\n```");
        }

        chunks.push(chunk);

        // Advance past the split, skipping a leading newline
        pos += split_at;
        if text.get(pos..pos + 1) == Some("\n") {
            pos += 1;
        } else if text.get(pos..pos + 2) == Some("\r\n") {
            pos += 2;
        }
    }

    chunks
}

/// Update fence tracking state by scanning lines in `text`.
fn update_fence_state(text: &str, in_fence: &mut bool, fence_header: &mut String) {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if *in_fence {
                *in_fence = false;
                fence_header.clear();
            } else {
                *in_fence = true;
                *fence_header = trimmed.to_string();
            }
        }
    }
}

/// Find the best byte offset to split at, preferring newline > whitespace > hard cut.
fn find_split_point(text: &str, max: usize) -> usize {
    if max >= text.len() {
        return text.len();
    }

    // Clamp to a char boundary
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    if end == 0 {
        // Find first char boundary > 0
        let mut first = 1;
        while first < text.len() && !text.is_char_boundary(first) {
            first += 1;
        }
        return first;
    }

    let search_region = text.get(..end).unwrap_or("");

    // Prefer splitting at a newline
    if let Some(pos) = search_region.rfind('\n')
        && pos > 0
    {
        return pos;
    }

    // Fall back to whitespace
    if let Some(pos) = search_region.rfind(char::is_whitespace)
        && pos > 0
    {
        return pos;
    }

    // Hard cut at char boundary
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        let result = chunk_text("", 100);
        assert!(result.is_empty(), "empty input should return empty vec");
    }

    #[test]
    fn short_text_no_split() {
        let result = chunk_text("hello world", 100);
        assert_eq!(result.len(), 1, "short text should not be split");
        assert_eq!(
            result.first().map(String::as_str),
            Some("hello world"),
            "content should be preserved"
        );
    }

    #[test]
    fn exact_boundary_no_split() {
        let text = "abcde";
        let result = chunk_text(text, 5);
        assert_eq!(result.len(), 1, "text exactly at limit should not be split");
    }

    #[test]
    fn splits_at_newline() {
        let text = "line one\nline two\nline three";
        let result = chunk_text(text, 15);
        assert!(result.len() >= 2, "should split into multiple chunks");
        for chunk in &result {
            assert!(
                chunk.len() <= 15,
                "chunk should fit in limit, got len {}",
                chunk.len()
            );
        }
    }

    #[test]
    fn splits_at_whitespace() {
        let text = "word1 word2 word3 word4 word5";
        let result = chunk_text(text, 12);
        assert!(result.len() >= 2, "should split into multiple chunks");
    }

    #[test]
    fn long_single_line_hard_split() {
        let text = "a".repeat(50);
        let result = chunk_text(&text, 20);
        assert!(
            result.len() >= 3,
            "should split long line into multiple chunks"
        );
        let combined: String = result.join("");
        assert_eq!(combined, text, "recombined chunks should equal original");
    }

    #[test]
    fn code_fence_closed_and_reopened() {
        let text = "before\n```rust\nline 1\nline 2\nline 3\nline 4\n```\nafter";
        let result = chunk_text(text, 30);
        assert!(result.len() >= 2, "should split into multiple chunks");

        for (i, chunk) in result.iter().enumerate() {
            let opens: usize = chunk
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    t.starts_with("```") && t.len() > 3
                })
                .count();
            let closes: usize = chunk.lines().filter(|l| l.trim() == "```").count();
            assert!(
                opens.abs_diff(closes) <= 1,
                "chunk {i} has unbalanced fences: opens={opens} closes={closes}\n---\n{chunk}\n---"
            );
        }
    }

    #[test]
    fn unicode_boundary_respect() {
        let text = "hello 🌍 world 🌍 test";
        let result = chunk_text(text, 10);
        assert!(result.len() >= 2, "should split unicode text");
    }

    #[test]
    fn zero_max_chars() {
        let result = chunk_text("hello", 0);
        assert!(result.is_empty(), "zero max should return empty vec");
    }

    #[test]
    fn single_char_input() {
        let result = chunk_text("a", 100);
        assert_eq!(result.len(), 1, "single char should not be split");
        assert_eq!(
            result.first().map(String::as_str),
            Some("a"),
            "content should be preserved"
        );
    }

    #[test]
    fn no_fence_just_text_splits_cleanly() {
        let text = "aaa\nbbb\nccc\nddd\neee";
        let result = chunk_text(text, 8);
        assert!(result.len() >= 2, "should split into multiple chunks");
        for chunk in &result {
            assert!(
                chunk.len() <= 8,
                "chunk should fit in limit, got len {} for: {chunk:?}",
                chunk.len()
            );
        }
    }
}
