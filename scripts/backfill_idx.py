#!/usr/bin/env python3
"""One-off backfill: generate .idx.jsonl files for episodes that predate the pipeline.

Mirrors the interaction-pair extraction algorithm in src/memory/chunk_extractor.rs:
1. Walk messages in order
2. On each user message, start a new pending pair
3. Skip assistant messages with empty/whitespace-only content (tool-call-only)
4. Skip tool and system messages
5. On the first assistant message with non-empty text, close the pair
6. If a new user message arrives before the pair closes, discard the incomplete pair

Usage:
    python3 scripts/backfill_idx.py <memory_dir>

Example:
    python3 scripts/backfill_idx.py ~/.residuum/workspace/memory
"""

import json
import sys
from pathlib import Path


def extract_chunks(messages: list[dict], episode_id: str, date: str, context: str) -> list[dict]:
    """Extract interaction-pair chunks from parsed JSONL messages (excluding meta line)."""
    chunks = []
    # (line_number, content, context)
    pending_user: tuple[int, str, str] | None = None
    line_offset = 2  # line 1 is meta

    for i, msg in enumerate(messages):
        line_num = line_offset + i
        role = msg.get("role", "")

        if role == "user":
            # per-message context falls back to episode-level context
            msg_ctx = msg.get("project_context", context)
            pending_user = (line_num, msg["content"], msg_ctx)

        elif role == "assistant":
            text = (msg.get("content") or "").strip()
            if not text:
                # tool-call-only assistant message — skip, keep pending user
                continue
            if pending_user is not None:
                user_line, user_content, user_ctx = pending_user
                chunk_id = f"{episode_id}-c{len(chunks)}"
                chunks.append({
                    "chunk_id": chunk_id,
                    "episode_id": episode_id,
                    "date": date,
                    "context": user_ctx,
                    "line_start": user_line,
                    "line_end": line_num,
                    "content": f"user: {user_content}\nassistant: {text}",
                })
                pending_user = None
            # else: orphaned assistant message, skip

        # tool / system messages: skip entirely

    return chunks


def process_episode(jsonl_path: Path) -> list[dict]:
    """Parse an episode .jsonl and return extracted chunks."""
    lines = jsonl_path.read_text().strip().split("\n")
    if not lines:
        return []

    meta = json.loads(lines[0])
    if meta.get("type") != "meta":
        print(f"  WARN: first line is not meta, skipping: {jsonl_path}", file=sys.stderr)
        return []

    episode_id = meta["id"]
    date = meta.get("date", "")
    context = meta.get("context", "workspace")

    messages = []
    for line in lines[1:]:
        line = line.strip()
        if not line:
            continue
        messages.append(json.loads(line))

    return extract_chunks(messages, episode_id, date, context)


def main():
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <memory_dir>", file=sys.stderr)
        sys.exit(1)

    memory_dir = Path(sys.argv[1])
    episodes_dir = memory_dir / "episodes"
    if not episodes_dir.is_dir():
        print(f"Episodes directory not found: {episodes_dir}", file=sys.stderr)
        sys.exit(1)

    jsonl_files = sorted(episodes_dir.rglob("ep-*.jsonl"))
    # exclude already-generated idx files
    jsonl_files = [f for f in jsonl_files if not f.name.endswith(".idx.jsonl")]

    created = 0
    skipped = 0

    for jsonl_path in jsonl_files:
        idx_path = jsonl_path.with_suffix("").with_suffix(".idx.jsonl")
        if idx_path.exists():
            print(f"  SKIP (exists): {idx_path.relative_to(memory_dir)}")
            skipped += 1
            continue

        chunks = process_episode(jsonl_path)
        if not chunks:
            print(f"  SKIP (no chunks): {jsonl_path.relative_to(memory_dir)}")
            skipped += 1
            continue

        lines = [json.dumps(c, ensure_ascii=False) for c in chunks]
        idx_path.write_text("\n".join(lines) + "\n")
        print(f"  WROTE {idx_path.relative_to(memory_dir)} ({len(chunks)} chunks)")
        created += 1

    print(f"\nDone: {created} files created, {skipped} skipped")


if __name__ == "__main__":
    main()
