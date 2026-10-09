// Text and reasoning as they stream into the main chat's feed. Each model call
// of a turn writes its text, and may think first, in pieces: the pieces make
// a draft item, which the authoritative message for the same call replaces
// once the call is done. Pieces are held back and added once per frame, so a
// fast stream re-renders the Markdown no more often than the screen draws.

import { nextFeedId } from "./feed-id";
import type { AssistantFeedItem, FeedItem, ThinkingFeedItem } from "./types";

/** Runs `flush` at the next frame the screen draws. */
export type FrameScheduler = (flush: () => void) => void;

/** The default scheduler: the next animation frame, or soon after where the page has none. */
export function nextAnimationFrame(flush: () => void): void {
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(flush);
  else setTimeout(flush, 16);
}

/** The ids of messages, remembered so a message isn't taken for another's. */
export class MessageIds {
  private readonly ids = new Set<string>();

  add(id: string): void {
    this.ids.add(id);
  }

  has(id: string): boolean {
    return this.ids.has(id);
  }
}

/** How a turn ended, for the text it left unfinished. */
export type StreamEnding = "finished" | "stopped" | "interrupted";

/** The fields that tie a new item to the turn in flight. */
type TurnTag = { turnId?: string };

function keyOf(turnId: string, call: number): string {
  return `${turnId}#${String(call)}`;
}

export class LiveStreams {
  // Bookkeeping only, never rendered.
  private readonly text = new Map<string, AssistantFeedItem>();
  private readonly thoughts = new Map<string, ThinkingFeedItem>();
  private readonly heldText = new Map<string, string>();
  private readonly heldThoughts = new Map<string, string>();
  /** The id of the item before a model call's first, so reasoning that arrives late goes back before it. */
  private readonly anchors = new Map<string, number | null>();
  private scheduled = false;

  /**
   * @param feed The feed's own array, so a draft's changes are seen.
   * @param schedule When held pieces are added to their drafts.
   */
  constructor(
    private readonly feed: FeedItem[],
    private readonly schedule: FrameScheduler = nextAnimationFrame,
  ) {}

  /** Add a piece of the text model call `call` is writing. */
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
      ...tag,
    });
    this.text.set(key, created);
  }

  /**
   * The text of model call `call` is complete: it replaces its draft, or is
   * added if there was none. Text of no model call (`call` absent) is added
   * as it is. `deliveredTo` is where the reply went when that wasn't here.
   */
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
    if (deliveredTo !== undefined) draft.deliveredTo = deliveredTo;
  }

  /** Add a piece of the reasoning model call `call` is doing. */
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
      startedAt: Date.now(),
      ...tag,
    });
    this.thoughts.set(key, created);
  }

  /**
   * The reasoning of model call `call` is complete. It replaces what streamed
   * in, or is added if nothing did, back before whatever the call has written.
   */
  completeThought(turnId: string, call: number, content: string, tag: TurnTag): void {
    const key = keyOf(turnId, call);
    const draft = this.thoughts.get(key);
    this.heldThoughts.delete(key);
    if (draft !== undefined) {
      if (content !== "") draft.content = content;
      draft.streaming = false;
      draft.endedAt ??= Date.now();
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

  /** The reasoning of model call `call` is over: its text, or its tools, began. */
  endThought(turnId: string, call: number): void {
    const key = keyOf(turnId, call);
    const draft = this.thoughts.get(key);
    if (draft?.streaming !== true) return;
    this.flushKey(key);
    draft.streaming = false;
    draft.endedAt ??= Date.now();
  }

  /** Model call `call` started over: what it had streamed is dropped. */
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

  /**
   * The turn is over. Drafts the authoritative message never replaced keep
   * what arrived, and say so when a stop or a failure cut them short.
   */
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
      draft.endedAt ??= Date.now();
      this.thoughts.delete(key);
    }
    for (const key of [...this.anchors.keys()])
      if (key.startsWith(prefix)) this.anchors.delete(key);
  }

  /** Forget every draft, as when the turn is settled from history. */
  clear(): void {
    this.text.clear();
    this.thoughts.clear();
    this.heldText.clear();
    this.heldThoughts.clear();
    this.anchors.clear();
  }

  /** Add every held piece to its draft now. */
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

  /** Remember where model call `key`'s first item goes. */
  private noteCall(key: string): void {
    if (!this.anchors.has(key)) this.anchors.set(key, this.feed.at(-1)?.id ?? null);
  }

  /** Add `item` to the feed and give back the feed's own, reactive copy of it. */
  private pushItem<T extends FeedItem>(item: T): T {
    this.feed.push(item);
    return this.feed[this.feed.length - 1] as T;
  }

  private remove(id: number): void {
    const at = this.feed.findIndex((entry) => entry.id === id);
    if (at >= 0) this.feed.splice(at, 1);
  }
}
