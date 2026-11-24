import { nextFeedId } from "./feed-id";
import { localTimestamp } from "./time";
import type { AssistantFeedItem, FeedItem, ThinkingFeedItem } from "./types";

export type FrameScheduler = (flush: () => void) => void;

export function nextAnimationFrame(flush: () => void): void {
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(flush);
  else setTimeout(flush, 16);
}

export class MessageIds {
  private readonly ids = new Set<string>();

  add(id: string): void {
    this.ids.add(id);
  }

  has(id: string): boolean {
    return this.ids.has(id);
  }

  delete(id: string): void {
    this.ids.delete(id);
  }
}

export type StreamEnding = "finished" | "stopped" | "interrupted";

type TurnTag = { turnId?: string };

function keyOf(turnId: string, call: number): string {
  return `${turnId}#${String(call)}`;
}

export class LiveStreams {
  private readonly text = new Map<string, AssistantFeedItem>();
  private readonly thoughts = new Map<string, ThinkingFeedItem>();
  private readonly heldText = new Map<string, string>();
  private readonly heldThoughts = new Map<string, string>();
  private readonly anchors = new Map<string, number | null>();
  private scheduled = false;

  constructor(
    private readonly feed: FeedItem[],
    private readonly schedule: FrameScheduler = nextAnimationFrame,
    private readonly stamp: () => string = localTimestamp,
    private readonly now: () => number = Date.now,
  ) {}

  appendText(turnId: string, call: number, piece: string, tag: TurnTag): void {
    const key = keyOf(turnId, call);
    const draft = this.text.get(key);
    if (draft !== undefined) {
      this.hold(this.heldText, key, piece);
      return;
    }
    this.noteCall(key);
    const created = this.pushItem<AssistantFeedItem>({
      id: nextFeedId(),
      kind: "assistant",
      content: piece,
      call,
      streaming: true,
      timestamp: this.stamp(),
      ...tag,
    });
    this.text.set(key, created);
  }

  completeText(
    turnId: string,
    call: number | undefined,
    content: string,
    tag: TurnTag,
    deliveredTo?: string,
  ): void {
    const key = call === undefined ? null : keyOf(turnId, call);
    const draft = key === null ? undefined : this.text.get(key);
    if (key !== null) {
      this.heldText.delete(key);
      this.text.delete(key);
    }
    if (draft === undefined) {
      if (content === "") return;
      if (key !== null) this.noteCall(key);
      this.pushItem<AssistantFeedItem>({
        id: nextFeedId(),
        kind: "assistant",
        content,
        ...(call === undefined ? {} : { call }),
        ...(deliveredTo === undefined ? {} : { deliveredTo }),
        timestamp: this.stamp(),
        ...tag,
      });
      return;
    }
    if (content === "") {
      this.remove(draft.id);
      return;
    }
    draft.content = content;
    draft.streaming = false;
    draft.timestamp = this.stamp();
    if (deliveredTo !== undefined) draft.deliveredTo = deliveredTo;
  }

  appendThought(turnId: string, call: number, piece: string, tag: TurnTag): void {
    const key = keyOf(turnId, call);
    if (this.thoughts.has(key)) {
      this.hold(this.heldThoughts, key, piece);
      return;
    }
    this.noteCall(key);
    const created = this.pushItem<ThinkingFeedItem>({
      id: nextFeedId(),
      kind: "thinking",
      content: piece,
      call,
      streaming: true,
      startedAt: this.now(),
      ...tag,
    });
    this.thoughts.set(key, created);
  }

  completeThought(turnId: string, call: number, content: string, tag: TurnTag): void {
    const key = keyOf(turnId, call);
    const draft = this.thoughts.get(key);
    this.heldThoughts.delete(key);
    if (draft !== undefined) {
      if (content !== "") draft.content = content;
      draft.streaming = false;
      draft.endedAt ??= this.now();
      return;
    }
    if (content.trim() === "") return;
    const item: ThinkingFeedItem = {
      id: nextFeedId(),
      kind: "thinking",
      content,
      call,
      ...tag,
    };
    const after = this.anchors.get(key);
    const at = after === undefined ? -1 : this.feed.findIndex((entry) => entry.id === after) + 1;
    if (after === undefined || at === 0) this.pushItem(item);
    else this.feed.splice(at, 0, item);
    const stored = this.feed.find((entry) => entry.id === item.id);
    if (stored?.kind === "thinking") this.thoughts.set(key, stored);
  }

  endThought(turnId: string, call: number): void {
    const key = keyOf(turnId, call);
    const draft = this.thoughts.get(key);
    if (draft?.streaming !== true) return;
    this.flushKey(key);
    draft.streaming = false;
    draft.endedAt ??= this.now();
  }

  restart(turnId: string, call: number): void {
    const key = keyOf(turnId, call);
    for (const draft of [this.text.get(key), this.thoughts.get(key)]) {
      if (draft !== undefined) this.remove(draft.id);
    }
    this.text.delete(key);
    this.thoughts.delete(key);
    this.heldText.delete(key);
    this.heldThoughts.delete(key);
    this.anchors.delete(key);
  }

  finish(turnId: string, how: StreamEnding): void {
    this.flush();
    const prefix = `${turnId}#`;
    for (const [key, draft] of [...this.text]) {
      if (!key.startsWith(prefix)) continue;
      draft.streaming = false;
      if (how !== "finished") draft.cut = how;
      this.text.delete(key);
    }
    for (const [key, draft] of [...this.thoughts]) {
      if (!key.startsWith(prefix)) continue;
      draft.streaming = false;
      draft.endedAt ??= this.now();
      this.thoughts.delete(key);
    }
    for (const key of [...this.anchors.keys()])
      if (key.startsWith(prefix)) this.anchors.delete(key);
  }

  clear(): void {
    this.text.clear();
    this.thoughts.clear();
    this.heldText.clear();
    this.heldThoughts.clear();
    this.anchors.clear();
  }

  flush(): void {
    this.flushHeld(this.heldText, this.text);
    this.flushHeld(this.heldThoughts, this.thoughts);
  }

  private flushKey(key: string): void {
    const text = this.heldText.get(key);
    const draft = this.text.get(key);
    if (text !== undefined && draft !== undefined) draft.content += text;
    this.heldText.delete(key);
    const thought = this.heldThoughts.get(key);
    const thinking = this.thoughts.get(key);
    if (thought !== undefined && thinking !== undefined) thinking.content += thought;
    this.heldThoughts.delete(key);
  }

  private flushHeld(
    held: Map<string, string>,
    drafts: Map<string, AssistantFeedItem | ThinkingFeedItem>,
  ): void {
    for (const [key, piece] of held) {
      const draft = drafts.get(key);
      if (draft !== undefined) draft.content += piece;
    }
    held.clear();
  }

  private hold(held: Map<string, string>, key: string, piece: string): void {
    held.set(key, (held.get(key) ?? "") + piece);
    if (this.scheduled) return;
    this.scheduled = true;
    this.schedule(() => {
      this.scheduled = false;
      this.flush();
    });
  }

  private noteCall(key: string): void {
    if (!this.anchors.has(key)) this.anchors.set(key, this.feed.at(-1)?.id ?? null);
  }

  private pushItem<T extends FeedItem>(item: T): T {
    this.feed.push(item);
    return this.feed[this.feed.length - 1] as T;
  }

  private remove(id: number): void {
    const at = this.feed.findIndex((entry) => entry.id === id);
    if (at >= 0) this.feed.splice(at, 1);
  }
}
