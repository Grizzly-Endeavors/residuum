<script lang="ts">
  import type { WaitingMessage } from "../../lib/types";
  import { IconButton } from "../../lib/ui";

  // The messages the agent hasn't read yet, at the foot of the conversation
  // under the turn they wait on. Each is the user's bubble, a little faded
  // until the agent reads it, when it joins the conversation where it was
  // read. Beside it, Send now stops the turn in flight so the message starts
  // the next one, and Multitask sends it to a session forked from the
  // conversation instead. While one of them is under way, both wait.

  interface Props {
    messages: WaitingMessage[];
    /** The agent's name as shown. */
    agentName: string;
    /** The agent's connection is down: nothing can be asked of a message. */
    offline: boolean;
    onsendnow: (message: WaitingMessage) => void;
    onmultitask: (message: WaitingMessage) => void;
  }

  let { messages, agentName, offline, onsendnow, onmultitask }: Props = $props();

  const uid = $props.id();

  function senderLine(message: WaitingMessage): string | null {
    const sender = message.sender;
    if (sender === undefined) return null;
    return [sender.name, sender.interface, sender.location].filter(Boolean).join(" · ");
  }
</script>

{#if messages.length > 0}
  <section class="waiting" aria-labelledby="{uid}-note">
    <p class="waiting-note" id="{uid}-note">Waiting for {agentName} to read</p>
    <ol class="waiting-list">
      {#each messages as message, index (message.id)}
        {@const sender = senderLine(message)}
        {@const bubbleId = `${uid}-message-${String(index)}`}
        <li class="waiting-message">
          {#if sender !== null}
            <p class="waiting-sender">{sender}</p>
          {/if}
          <div class="waiting-row">
            <span class="waiting-actions">
              <IconButton
                size="sm"
                icon="send"
                label="Send now"
                tooltip="Send now: stop the reply and send this"
                aria-describedby={bubbleId}
                loading={message.asked === "send-now"}
                disabled={offline || message.asked === "withdraw"}
                onclick={() => {
                  onsendnow(message);
                }}
              />
              <IconButton
                size="sm"
                icon="fork"
                label="Multitask"
                tooltip="Multitask: work on this in a session forked from this chat"
                aria-describedby={bubbleId}
                loading={message.asked === "withdraw"}
                disabled={offline || message.asked === "send-now"}
                onclick={() => {
                  onmultitask(message);
                }}
              />
            </span>
            <div class="waiting-bubble" id={bubbleId}>
              {#if message.content}
                <p class="waiting-text">{message.content}</p>
              {/if}
              {#if message.images !== undefined && message.images.length > 0}
                <div class="waiting-images">
                  {#each message.images as image, imageIndex (imageIndex)}
                    <img
                      src="data:{image.media_type};base64,{image.data}"
                      alt="Attached image {String(imageIndex + 1)}"
                    />
                  {/each}
                </div>
              {/if}
            </div>
          </div>
        </li>
      {/each}
    </ol>
  </section>
{/if}

<style>
  .waiting {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--space-8);
  }

  .waiting-note {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .waiting-list {
    display: flex;
    flex-direction: column;
    align-self: stretch;
    gap: var(--space-12);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .waiting-message {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--space-4);
  }

  .waiting-sender {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  /* Send now and Multitask left of the bubble, on its last line, the way a sent message keeps its time. */
  .waiting-row {
    display: flex;
    align-items: flex-end;
    align-self: stretch;
    justify-content: flex-end;
    gap: var(--space-8);
  }

  .waiting-actions {
    display: flex;
    flex: none;
    gap: var(--space-2);
  }

  /* The user's bubble, faded a little: the agent hasn't read it yet. */
  .waiting-bubble {
    display: flex;
    flex: 0 1 auto;
    flex-direction: column;
    gap: var(--space-8);
    min-width: 0;
    max-width: min(82%, 560px);
    padding: var(--space-8) var(--space-14);
    border-radius: var(--corner-lg) var(--corner-lg) var(--corner-sm) var(--corner-lg);
    background: var(--color-moss-tint);
    color: var(--color-text);
    opacity: 0.72;
  }

  .waiting-text {
    font-size: var(--font-size-message);
    --leading: var(--line-height-ui);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .waiting-images {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);

    & img {
      display: block;
      max-width: 96px;
      max-height: 96px;
      border-radius: var(--corner-md);
      object-fit: cover;
    }
  }

  @media (max-width: 760px) {
    .waiting-bubble {
      max-width: 80%;
    }
  }
</style>
