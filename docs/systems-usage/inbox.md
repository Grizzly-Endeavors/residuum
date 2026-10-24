# Inbox

The inbox is the mail between the user and the agents. Every agent has two inbox folders on disk: the **agent inbox** holds what arrives for the agent (the user's messages, and the results and files the agent's background work files for later triage), and the **user inbox** holds what the agent has written to the user. The web UI shows both as one mailbox: threads of messages with Inbox, Sent and Archived folders, and the agent tools read the same files, so the agent and the user see one definition of a thread.

Mail is only part of what the two folders hold. Each item records its own source, and only some sources are mail (see [What is mail](#what-is-mail)).

## The Two Inboxes

| | Agent inbox | User inbox |
|---|---|---|
| Path | `inbox/agent/` | `inbox/user/` |
| Archive path | `archive/inbox/agent/` | `archive/inbox/user/` |
| Attachments path | `inbox/agent/attachments/{item id}/` | `inbox/user/attachments/{item id}/` |
| Written by | The user's messages (web UI, CLI, the `/inbox` chat command), the notification router for `scheduled` and webhook-triggered `external` results, `POST /api/agents/{name}/agent-inbox`, the companion item of each file a Discord, Telegram, Teams or A2A message carries, and each file an A2A task's remote agent returns as an artifact | `user_inbox_add` |
| Read and manage | The agent: `inbox_list`, `inbox_read`, `inbox_archive`, `inbox_restore`. The user: the web UI | Every agent can list and read it (`user_inbox_list`, `inbox_read` with a ref) but never changes it. The user reads, archives, restores and replies in the web UI |

The user inbox is a delivery channel to the user. An agent (most often a background sub-agent such as `introspection`) writes findings there with `user_inbox_add`, and the user answers them in the web UI. Because the files are read straight from disk, the user can list, read, archive and restore a stopped agent's items too.

Archiving is a soft delete in both folders. An item's JSON file and its attachment directory move together under `archive/inbox/`, and restoring moves them back. The user archives and restores user-inbox items in the web UI, and the agent restores its own agent-inbox items with `inbox_restore`.

## What Is Mail

The web UI's Inbox, Sent and Archived folders show mail only:

- **Messages the user sent an agent** are agent-inbox items whose source is `web` or `cli`. A message sent to several agents is one copy per recipient (see [Copies of one message](#copies-of-one-message)).
- **Messages an agent sent the user** are user-inbox items. Every user-inbox item is mail.

Every other agent-inbox item is not mail and never appears in a thread: pulse, action, `scheduled` and webhook results, `system` items, `artifact:<name>` and `api` items, and the companion items of chat and A2A files. These stay in the agent inbox for the agent to triage, and the agent's `inbox_list` and `inbox_read` still show them.

## Refs

An item is named by a **ref**, the string `<agent>/<kind>/<id>`:

- `<agent>` is the agent's folder name (the name `/api/agents/{name}` and the attachment URLs use), not its display name.
- `<kind>` is `agent` for the agent inbox or `user` for the user inbox.
- `<id>` is the item's ID, the filename stem.

Example: `atlas/agent/20260227_deploy-status` is the agent inbox item `20260227_deploy-status` of the agent `atlas`. A ref names one item wherever it sits, active or archived. The agent tools accept a full ref (`inbox_read`), a bare ID in the agent's own agent inbox, or a bare ID for `user_inbox_add`'s `in_reply_to`, which means that agent's user inbox. The API takes refs in percent-encoded form inside URLs.

## Item Format

Each item is a JSON file. There is **no `id` field in the JSON body**. The ID is the filename stem, so `20260227_deploy-completed.json` has the ID `20260227_deploy-completed`.

```json
{
  "title": "Deploy completed",
  "body": "Production deploy v2.3.1 finished successfully.",
  "source": "deploy-watcher",
  "timestamp": "2026-02-27T14:30",
  "read": false,
  "attachments": [],
  "in_reply_to": "atlas/agent/20260227_deploy-status"
}
```

- `timestamp` is a naive local time to the minute, in the hub's timezone. The API reports it as RFC 3339 with an offset.
- `source` names where the item came from: `web` or `cli` for the user's messages, `artifact:<name>` for a workbench artifact, `api` for another client of the agent-inbox endpoint, and the producing label otherwise (a job, a webhook, a platform).
- `in_reply_to` is present only on an item that answers a message. It holds that message's ref.
- `attachments` lists each copied file's path, relative to the workspace root. Only the filename is authoritative: the directory can go stale once an item is archived, so consumers resolve each attachment by filename against the item's current location.

Filenames are generated from the date and the sanitized title (`{YYYYMMDD}_{title}`, at most 60 title characters). Titles that are blank fall back to the first line of the body.

IDs are unique within an agent across both its inbox and its archive. When the generated stem is taken, `_2`, `_3`, ... is appended, so two same-titled items on one day each keep their own file and attachment directory, and an archived item's ID is never reused. The ID is claimed on disk before the item is written, by creating the item's `attachments/{id}/` directory, so writers running at the same moment never get the same ID. An item saved with no attachments has that directory removed, and a claim that is abandoned (a cancelled task, a failed save) removes it too. Every writer claims its ID this way: mail sends and replies, the `/inbox` chat command, `user_inbox_add`, the notification router, `POST /api/agents/{name}/agent-inbox`, and the companion items of chat and A2A files. Archiving and restoring never replace a different item that has the same ID. If the destination already holds that ID, the move fails with an error naming the conflict, and both items stay put. A thread archive or restore that hits such a conflict answers `500` naming each message that failed, and `inbox_archive`/`inbox_restore` report it in their result.

## Threads

A **thread** is a root message and every message whose `in_reply_to` chain reaches it. The thread's ID is the root's ref, and its subject is the root's title.

- A reply's `in_reply_to` names the message it answers, so a thread can cross agents and the archive. An agent that answers the user's message, and the user's follow-up, are one conversation.
- A message whose parent is missing, unreadable, or itself makes it the root of its own thread.
- If the chain loops, the oldest message in the loop is the root.
- Messages are ordered by time. Where two messages share a stored minute, the one that is a parent of the other comes first, then the ref breaks any remaining tie. The root always comes first in the thread view.
- Any ref of a message in a thread, not only the root, finds the whole thread and returns its canonical thread ID.

### Copies of one message

A message the user sends to several agents is written as one agent-inbox item per recipient, all with the same ID. Those copies are shown as one message. Copies count as the same message when they share the ID, title, body and absolute time. The canonical ref of the message is the copy whose agent name sorts first, and a ref to any copy resolves to the message.

Each recipient's copy has its own read and archived state. The message's **Seen** list reports, for each recipient, whether that agent has read or archived its copy.

## Folders and State

| Folder | Holds a thread when |
|---|---|
| Inbox | The thread has at least one active user-inbox item |
| Archived | The thread has user-inbox items and every one is archived |
| Sent | The thread contains at least one message the user wrote |

A thread can be in Sent and Inbox at once. A thread the user started that no agent has answered shows only in Sent. A thread with unread user-inbox items is unread. Opening a thread marks all of its user-inbox items read. Archiving and restoring change only the user-inbox items of the thread, so an agent's copy of the user's message is never moved. A new agent reply to an archived thread adds an active user-inbox item, so the thread returns to Inbox.

A folder that cannot be read is logged and skipped, so one unreadable agent does not hide the others. A missing folder counts as empty.

Participants are every agent that wrote in the thread plus every agent a message of the user's went to.

## Sending and Replying

**New message.** The web UI's New message composes an empty thread with `to` (one or more agents), a subject, a body and attachments. The subject becomes each copy's title, and a blank subject takes the first line of the body. The send is all or nothing. One ID is claimed in every recipient's agent inbox at once, free there and in each archive, as an item and as an attachment directory, so two sends with the same subject at the same moment get different IDs. If any copy or attachment fails, the claims are released and the copies already written are removed.

**The `/inbox` chat command** sends its note as a message from the user to the agent it was typed to, with source `cli`. The note is titled with its first line and shows in Sent like any other message.

**Reply.** A reply carries a body and attachments. Its recipients are computed on the server as reply-all: every agent that wrote in the thread, plus every agent a message of the user's in it went to. The reply answers the latest message, and its title is `Re: <subject>`, with repeated `Re:` prefixes collapsed rather than stacked.

An agent's reply goes to the user only. Agents coordinate with one another through `message_agent` and can read the thread with the inbox tools.

## Uploads and Attachments

Files reach a message in two steps. The client first stages each file with `POST /api/hub/uploads?filename=<name>`, which takes the raw bytes and returns an upload ID. The send or reply then names those IDs in its `attachments`.

- Uploads are at most 50 MB. A larger body is refused with `413`.
- Staged uploads live in the hub directory under `uploads/`, not in an agent's folder, so one upload can go to several recipients and the staging works while every agent is stopped.
- Staged uploads are removed 24 hours after they were staged. The store prunes at hub startup and before each new upload.
- A send that names an unknown upload ID is refused with `400`.
- Each recipient's copy receives its own copy of every attachment, in that copy's `attachments/{item id}/` directory.

Attachments of a user-inbox item are served by `GET /api/agents/{name}/inbox/{id}/attachments/{index}`, and those of an agent-inbox item by `GET /api/agents/{name}/inbox/agent/{id}/attachments/{index}`, whether the item is active or archived. Each attachment in a thread carries its URL and its workspace-relative path. Both routes confine every path to the item's own attachment directory and answers `404` for a missing file. In the web UI, a `.md` or `.txt` attachment opens in the Files panel instead of downloading (see [Web Inbox](#web-inbox)). See [hub-http.md](hub-http.md#staged-uploads) for the upload route and the rest of the hub's surface.

For the agent, `user_inbox_add`'s `attachments` parameter takes paths to files the agent has already written. Each file is copied into the item's own directory, so the original can be moved or deleted afterward. There is no size cap, since these files are already local. A file that fails to copy is skipped and named in the result, and the item is still created with the others.

## Agent Tools

| Tool | Parameters | Description |
|------|-----------|-------------|
| `inbox_list` | `unread_only` (bool, optional), `archived` (bool, optional), `query` (text, optional), `limit` (integer, optional), `before` (cursor, optional) | Lists the agent's own agent-inbox items newest first, a page at a time. `archived: true` lists `archive/inbox/agent/` and ignores `unread_only`. See [Paging and search](#paging-and-search). |
| `inbox_read` | `id` (a filename stem of the agent's own agent inbox, or any item's ref) | Reads one item, shows its ref, and for mail shows the thread's earlier messages oldest first, with each message's ref, and the refs of later messages. Marks the item read only when it is an active item of the agent's own agent inbox. |
| `inbox_archive` | `ids` (filename stems) | Moves items from `inbox/agent/` to `archive/inbox/agent/`. |
| `inbox_restore` | `ids` (filename stems) | Moves items from `archive/inbox/agent/` back. Find the stems with `inbox_list` and `archived: true`. |
| `user_inbox_add` | `title` (optional when `in_reply_to` is set), `body`, `in_reply_to` (ref, optional), `attachments` (paths, optional) | Writes an item to the user inbox. With `in_reply_to`, the item answers that message in its thread, titled `Re: <subject>` unless a title is given. A ref that is not in any thread is refused, and nothing is written. Without `in_reply_to`, a title is required. |
| `user_inbox_list` | `agent` (folder name, optional), `include_archived` (bool, optional), `query` (text, optional), `limit` (integer, optional), `before` (cursor, optional) | Lists every agent's user-inbox items newest first, a page at a time, with ref, title, agent, time, read and archived state, and the message each one answers. Read-only. See [Paging and search](#paging-and-search). |

Neither `user_inbox_list` nor `inbox_read` changes the user's read or archived state. Archiving and restoring user-inbox items belong to the user in the web UI.

### Paging and search

`inbox_list` and `user_inbox_list` list one page at a time, newest first, with items of the same time ordered by their ref. `limit` sets the page size (50 by default, at most 200: a larger value counts as 200; 0 or anything that is not a whole number is refused). When more items follow the page, the listing ends with `Showing {n} of {total}. For the next page, call again with before="{cursor}" and the same query, agent and include_archived.`, and the next call passes that cursor as `before`. The listing's header counts the items from the start of the page, so a later page counts only the items that remain. A cursor is `{unix ms}:{ref}`, the same form the hub's mail thread list returns as `next`: `inbox_list` reads an item's stored local time in the agent's timezone, and a `before` that isn't a cursor a listing returned is refused.

`query` keeps the items whose title, body or attachment filenames contain the text, ignoring case; a blank `query` is ignored. It combines with the other parameters, so `query` with `agent`, `include_archived`, `unread_only`, `archived` and `before` narrows the same list a page at a time.

## Notifications and Other Writers

The notification router files each substantive `scheduled` result and each substantive webhook-triggered `external` result as an agent-inbox item, with the result's summary as the body and its source label as the source (see [notifications.md](notifications.md)). A conversation-triggered `external` result (A2A, or a non-owner Discord, Telegram or Teams chat) files nothing: its output already went back to the conversation it came from, and its observations are merged into memory as an episode. `artifact` and `spawned` session results are relayed elsewhere (see [background-tasks.md](background-tasks.md#result-routing)).

A workbench artifact can file an agent-inbox item with `POST /api/agents/{name}/agent-inbox` (body `{ title?, body }`). The title defaults to the body's first line, and a blank body is refused with `400`. The item's source is `artifact:<name>` when the request carries the `X-Residuum-Artifact` header that the workbench SDK sends, and `api` otherwise. Neither is mail: the user's own messages go through the hub's mail routes. An artifact cannot write to the user inbox.

A file that a Discord, Telegram, Teams or A2A message carries gets its own companion agent-inbox item, and each file an A2A task's remote agent returns gets one as well, titled `A2A artifact: <filename>`. The file sits in that item's attachment directory, so archiving the item moves the file with it (see [a2a.md](a2a.md)).

A file sent with a message in the web chat gets the same kind of item, titled `Chat attachment: <filename>` with source `chat` and the author line `Web chat`. The chat sends the upload ids from [Staged uploads](hub-http.md#staged-uploads), and the gateway copies each staged file into its item's attachment directory before it publishes the message. The message text gets an `[Attachment: <filename> (<size> bytes) → <path>]` line for each file, and a supported image is also given to the model inline. A message naming an upload that is no longer staged, or a file that fails to copy, is refused whole with a plain-language reason (a `message_refused` frame), and no item is left behind. The staged uploads are removed once the message is published. Withdrawing a waiting message doesn't remove the companion items of the files it came with; they stay in the agent inbox.

## Web Inbox

The Inbox place (`/inbox`, **Inbox** at the top of the left-hand rail) is a mail client over every agent's mail. It reads and writes through the hub's `/api/hub/mail/*` routes, which work from the agents' files, so a stopped or failed agent's mail is listed, read, archived and answered like a running agent's. What counts as a conversation is in [Threads](#threads) and [Folders and State](#folders-and-state), and how files reach a message is in [Uploads and Attachments](#uploads-and-attachments).

- **Folders.** **Inbox** holds the conversations with at least one message from an agent that isn't archived, **Sent** the conversations with a message you wrote, and **Archived** the conversations whose agent messages are all archived. A conversation can be in Inbox and Sent at once.
- **Agent filter.** The filter beside the folders is **All agents** or one agent. In the Inbox folder it shows each agent's unread count.
- **List.** Conversations are newest first, in pages (**Show older** pages older ones in). A row names who the conversation is with (in Sent, who it went to: **To atlas, wren**), how many messages it holds, when it last moved, its subject (**(no subject)** when it has none) and the first line of the latest message. An unread conversation has a dot and a bold subject, and a conversation with files has a paperclip.
- **Unread counts.** The counts on the rail, in the bottom bar, in the tab title and on the app badge are each agent's `inbox_unread` from the [team overview](hub.md#team-overview): the unread messages from agents, not conversations. They recount when mail is read, archived, restored or arrives, and the open list and conversation refresh with them.
- **Address.** The folder, the filter, the open conversation and a New message are in the URL: `/inbox?folder=sent&agent=atlas&thread=<ref>` and `/inbox?compose=1&to=atlas,scout`, with `thread` percent-encoded. A link naming any message of a conversation opens the whole conversation. `?item=<agent>:<id>` opens that agent's message and `?tab=archived` the Archived folder. An agent in the URL that doesn't exist is dropped, with a notice.

### Reading and filing

Opening a conversation shows its messages oldest first, each marked as yours or the agent's, with the time, the Markdown body, its attachments and, on a message you wrote, whether each agent it went to has seen it.

- **Read.** Opening a conversation marks it read at once. If the hub refuses, it goes back to unread and the reason is shown.
- **Archive and restore.** Archiving moves the whole conversation to Archived, and restoring moves it back to Inbox. Each shows a toast with **Undo**.
- **Reply.** The box under the messages is reply-all: it goes to every agent in the conversation, listed on its **To:** line, and says that each agent will see it the next time it checks its inbox. Ctrl+Enter or Cmd+Enter sends it. A reply draft stays while you switch folders, and is kept in memory only.
- **Attachments.** A `.md` or `.txt` attachment opens in the Files panel beside the conversation, rendered, with **Edit** to change it and save it in the agent's workspace. Any other attachment downloads. Closing the conversation, opening another or changing folder closes a panel the conversation opened, and leaves a Files panel opened some other way.

### Writing

**New message** (in the place header, and in an agent's chat as **New message to** that agent) opens a message in the pane beside the list. **To** takes one or more agents (the filtered agent is filled in), and the message has a subject, a Markdown body and attachments. The subject may be empty, and the first line of the body is then the subject. The body may not be empty. Sending opens the conversation in Sent; the hub files one copy in each recipient's inbox. Cancelling drops the draft. Leaving a New message that has text or files asks first, unless it is already being sent (the send carries on, and a notice says whether it went), and closing or reloading the page with any unsent draft asks too.

A file attached to a message or reply uploads when it is chosen, shows its progress on its chip, and can be removed first. A failed upload shows **Try again** on its chip, and **Send** waits for uploads still running and won't go while one has failed. Both boxes say under the text what a file may weigh (**Any file, up to 50 MB each.**) and when the recipients will see the message, and a file over that is refused before it uploads, with a message naming the file and the limit. A message waiting on uploads says so under the text. In a New message, Ctrl+Enter or Cmd+Enter sends from the message box or the subject, as in a reply; a plain Enter in the subject moves to the message box and never sends.

### Layout

The conversation list, the conversation and a Files panel share the main region, and the conversation narrows to a minimum width before anything else gives way. When the list and the conversation no longer fit side by side, as when a Files panel opens beside them, the place shows one at a time: the list, or the open conversation or New message with **Back to list**. On a phone it is always one at a time. Focus follows what opens: opening a conversation or New message while the list is hidden, and sending a New message, put focus on the conversation's subject (or **New message**), and **Back to list** or **Cancel** put it back on the conversation's row or the list. **Show older** keeps focus when its last page takes the button away, moving it to the first new row.

## Intended Usage

The agent inbox is for items that do not need immediate attention. Background results that are informational rather than actionable belong there rather than on a push channel. The agent triages it periodically, reads what needs follow-up, and archives what is resolved. A heartbeat pulse drives this.

The user inbox is for findings the agent wants to hand the user without interrupting a conversation. The built-in `reflection` pulse delivers its suggestions there, and `memory_tending` writes to the team wiki and `team/USER.md` directly rather than to the user inbox. Attach a file with `attachments` when the finding is easier to review as a file than as inline text, such as an export, a screenshot or a generated report. Before adding an item, the agent checks what every agent has already sent, including the archive, so it does not repeat a suggestion. A reply to the user's mail names the message it answers in `in_reply_to`.

## See Also

- [hub-http.md](hub-http.md): the hub's route table, the staged-upload route, and the browser landing for `/inbox?thread=<ref>`
- [notifications.md](notifications.md): how results reach the agent inbox
- [background-tasks.md](background-tasks.md): how session results are routed
- [a2a.md](a2a.md): the companion items of A2A artifacts
