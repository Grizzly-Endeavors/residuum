import type { UserInboxAttachment } from "../../src/lib/types";
import { PNG_PIXEL, storeAttachment } from "../inbox";
import type { MockAgent, MockHub } from "../state";

/**
 * The mail the scenario starts with, on top of each agent's sample inbox
 * (`data/inbox.ts`): `atlas` and `scout` each hold an unread "Deploy tomorrow",
 * a read "Daily Digest" and an archived "Last week's digest", and each is a
 * thread of its own. These threads are added:
 *
 * - "Wiki audit: stale pages": you wrote to atlas and scout, both replied, and
 *   you answered them both; scout hasn't seen that last message.
 * - "Research: local-first sync libraries": a long Markdown message from scout
 *   with a Markdown file, a text file and an image attached.
 * - "Pick up the weekly changelog": you wrote to drifter, which is stopped, and
 *   nobody answered.
 * - "Backup rotation": you asked atlas, it answered, and the thread is archived.
 *
 * Every message an agent sent here is read, so the unread counts stay those of
 * the sample inboxes.
 */

/** The threads the scenario starts with, by the name a test uses for them: their ids, which are the refs of their first messages. */
export const MAIL_SEED = {
  /** An agent's own unread message, one per agent. */
  deployTomorrow: {
    subject: "Deploy tomorrow",
    threadIds: { atlas: "atlas/user/mock_1", scout: "scout/user/mock_1" },
  },
  dailyDigest: {
    subject: "Daily Digest",
    threadIds: { atlas: "atlas/user/mock_2", scout: "scout/user/mock_2" },
  },
  lastWeeksDigest: {
    subject: "Last week's digest",
    threadIds: { atlas: "atlas/user/mock_archived_1", scout: "scout/user/mock_archived_1" },
  },
  /** Sent to atlas and scout; the presented message is atlas's copy. Its replies and your answer follow. */
  wikiAudit: {
    subject: "Wiki audit: stale pages",
    threadId: "atlas/agent/20260311_wiki_audit_stale_pages",
    sentId: "20260311_wiki_audit_stale_pages",
    replyId: "20260311_re_wiki_audit_stale_pages",
    answerId: "20260312_re_wiki_audit_stale_pages",
  },
  /** From scout, with Markdown headings, lists, a code block, links and three attachments. */
  syncResearch: {
    subject: "Research: local-first sync libraries",
    threadId: "scout/user/20260313_research_local_first_sync_libraries",
    id: "20260313_research_local_first_sync_libraries",
    attachments: ["findings.md", "benchmark-log.txt", "merge-times.png"],
  },
  /** Sent to drifter, which is stopped, and unanswered. */
  changelog: {
    subject: "Pick up the weekly changelog",
    threadId: "drifter/agent/20260312_pick_up_the_weekly_changelog",
  },
  /** Asked and answered, then archived. */
  backupRotation: {
    subject: "Backup rotation",
    threadId: "atlas/agent/20260308_backup_rotation",
    sentId: "20260308_backup_rotation",
    replyId: "20260308_re_backup_rotation",
  },
} as const;

const AUDIT_SCOPE = `# Wiki audit scope

## In scope

- \`team/wiki/projects/\` and everything below it
- \`team/wiki/index.md\` and \`team/wiki/log.md\`

## Out of scope

- \`team/workbench/\`
- Anything under \`memory/\`

## Output

One list, oldest page first, with the date each page was last edited.
`;

const STALE_PAGES_REPLY = `Done. Four pages haven't been edited since January:

1. \`team/wiki/projects/index.md\` (January 9)
2. \`team/wiki/projects/residuum.md\` (January 14)
3. \`team/wiki/log.md\` (January 21)
4. \`team/wiki/index.md\` (January 28)

I haven't changed anything. Tell me which ones to archive.`;

const SOURCE_NOTES_REPLY = `Checked the topics behind atlas's list against current sources.

- **Newer docs exist** for the two project pages; I've put the links in the attached notes.
- **Nothing has changed** for the log and the index.`;

const WIKI_ANSWER = `Thanks both. Atlas, please archive the two oldest pages. Scout, hold off on the project pages until atlas is done.`;

const FINDINGS = `# Local-first sync: findings

## Recommendation

Prototype **Yjs** first and keep **Loro** as the second option.

## Why

- Yjs merged the 10,000-edit run fastest and used the least memory.
- Automerge is solid but its documents were about three times larger.
- ElectricSQL assumes Postgres, which doesn't fit a workspace of plain files.
`;

const BENCHMARK_LOG = `library      merge_ms   doc_kb   peak_mb
yjs                41      212       38
loro               57      240       44
automerge         188      604       97

10,000 edits, 3 replicas, merged in random order.
Machine: 8 cores, 16 GB, Node 22.
`;

const SYNC_RESEARCH = `# Local-first sync: what I found

I compared four libraries against the constraints in \`team/wiki/projects/residuum.md\`. Short version: **two are worth a prototype**.

## Candidates

1. **Automerge**: mature CRDT, larger payloads.
2. **Yjs**: fastest in my runs, smallest memory footprint.
3. **Loro**: newest, good history support, API still moving.
4. **ElectricSQL**: built around Postgres, so it doesn't fit a file workspace.

## What I measured

Merging 10,000 edits across three replicas:

\`\`\`text
library      merge_ms   doc_kb
yjs                41      212
loro               57      240
automerge         188      604
\`\`\`

The full log is attached as \`benchmark-log.txt\`.

## Next steps

- Prototype Yjs against \`workspace/notes/\`
- Ask atlas to add these results to the team wiki
- Re-check Loro in a month, once its API settles

More background: [Yjs documentation](https://docs.yjs.dev), [Automerge](https://automerge.org/docs/hello/) and the [CRDT overview](https://crdt.tech).

> The raw numbers are attached, so you can check my arithmetic.
`;

const CHANGELOG_REQUEST = `When you're next awake: draft this week's changelog from the merged pull requests and add it to \`team/wiki/log.md\`.

Group it by area, and keep each entry to one line.`;

const BACKUP_QUESTION = `How many nightly backups are we keeping right now? I'd like to cut it to 14.`;

const BACKUP_REPLY = `We keep **30** nightly backups. Cutting to 14 frees about 40 GB.

I'll make the change after tonight's run unless you say otherwise.`;

/**
 * A small valid PDF with one page holding `lines`. Its offsets are real, so a
 * viewer opens it without repairing it.
 */
export function tinyPdf(lines: readonly string[]): Buffer {
  const escape = (line: string): string => line.replace(/[\\()]/g, (char) => `\\${char}`);
  const content = `BT /F1 16 Tf 72 720 Td 22 TL ${lines
    .map((line) => `(${escape(line)}) Tj T*`)
    .join(" ")} ET`;
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    `<< /Length ${String(content.length)} >>\nstream\n${content}\nendstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  let pdf = "%PDF-1.4\n";
  const offsets: number[] = [];
  objects.forEach((object, index) => {
    offsets.push(pdf.length);
    pdf += `${String(index + 1)} 0 obj\n${object}\nendobj\n`;
  });
  const xref = pdf.length;
  pdf += `xref\n0 ${String(objects.length + 1)}\n0000000000 65535 f \n`;
  for (const offset of offsets) pdf += `${String(offset).padStart(10, "0")} 00000 n \n`;
  pdf += `trailer\n<< /Size ${String(objects.length + 1)} /Root 1 0 R >>\nstartxref\n${String(xref)}\n%%EOF\n`;
  return Buffer.from(pdf, "latin1");
}

const SOURCE_NOTES_PDF = tinyPdf([
  "Source notes: wiki audit",
  "",
  "team/wiki/projects/index.md  newer docs: yes",
  "team/wiki/projects/residuum.md  newer docs: yes",
  "team/wiki/log.md  no change upstream",
  "team/wiki/index.md  no change upstream",
]);

/** A file to attach: its name and bytes. */
interface FileSpec {
  filename: string;
  bytes: Buffer;
}

function text(filename: string, content: string): FileSpec {
  return { filename, bytes: Buffer.from(content) };
}

function agentNamed(hub: MockHub, name: string): MockAgent {
  const agent = hub.agents.get(name);
  if (agent === undefined) throw new Error(`the scenario has no agent '${name}'`);
  return agent;
}

/** A message you sent: one copy in each recipient's own inbox, all with one id. */
function sentCopies(
  hub: MockHub,
  message: {
    id: string;
    to: Record<string, { read: boolean }>;
    title: string;
    body: string;
    timestamp: string;
    inReplyTo?: string;
    files?: FileSpec[];
  },
): void {
  for (const [name, { read }] of Object.entries(message.to)) {
    const agent = agentNamed(hub, name);
    const attachments: UserInboxAttachment[] = (message.files ?? []).map((file) =>
      storeAttachment(agent.state, "agent", message.id, file.filename, file.bytes),
    );
    agent.state.agentInbox.push({
      id: message.id,
      title: message.title,
      body: message.body,
      source: "web",
      timestamp: message.timestamp,
      attachments,
      ...(message.inReplyTo === undefined ? {} : { in_reply_to: message.inReplyTo }),
      ...(read ? { read: true } : {}),
    });
  }
}

/** A message an agent sent you, read, in its inbox for you or in the archive. */
function agentReply(
  hub: MockHub,
  message: {
    agent: string;
    id: string;
    title: string;
    body: string;
    timestamp: string;
    inReplyTo?: string;
    files?: FileSpec[];
    archived?: boolean;
  },
): void {
  const { state } = agentNamed(hub, message.agent);
  const attachments = (message.files ?? []).map((file) =>
    storeAttachment(
      state,
      "user",
      message.id,
      file.filename,
      file.bytes,
      message.archived === true,
    ),
  );
  const list = message.archived === true ? state.inboxArchive : state.inboxItems;
  list.push({
    id: message.id,
    title: message.title,
    body: message.body,
    source: "agent",
    timestamp: message.timestamp,
    read: true,
    attachments,
    ...(message.inReplyTo === undefined ? {} : { in_reply_to: message.inReplyTo }),
  });
}

/** Add the scenario's mail threads to the agents `hub` holds. `atlas`, `scout` and `drifter` must exist. */
export function seedMail(hub: MockHub): void {
  const { clock } = hub.env;
  const audit = MAIL_SEED.wikiAudit;
  const research = MAIL_SEED.syncResearch;
  const backup = MAIL_SEED.backupRotation;

  sentCopies(hub, {
    id: audit.sentId,
    to: { atlas: { read: true }, scout: { read: true } },
    title: audit.subject,
    body: `Could you both help me tidy the team wiki this week?

- **atlas**: list every page nobody has touched since January.
- **scout**: for each of those topics, check whether newer sources exist online.

The scope is in the attached note. Send me what you find.`,
    timestamp: clock.dayAt(3, 14, 5),
    files: [text("audit-scope.md", AUDIT_SCOPE)],
  });
  agentReply(hub, {
    agent: "atlas",
    id: audit.replyId,
    title: `Re: ${audit.subject}`,
    body: STALE_PAGES_REPLY,
    timestamp: clock.dayAt(3, 14, 32),
    inReplyTo: `atlas/agent/${audit.sentId}`,
  });
  agentReply(hub, {
    agent: "scout",
    id: audit.replyId,
    title: `Re: ${audit.subject}`,
    body: SOURCE_NOTES_REPLY,
    timestamp: clock.dayAt(3, 15, 10),
    inReplyTo: `scout/agent/${audit.sentId}`,
    files: [{ filename: "source-notes.pdf", bytes: SOURCE_NOTES_PDF }],
  });
  sentCopies(hub, {
    id: audit.answerId,
    to: { atlas: { read: true }, scout: { read: false } },
    title: `Re: ${audit.subject}`,
    body: WIKI_ANSWER,
    timestamp: clock.dayAt(2, 9, 20),
    inReplyTo: `scout/user/${audit.replyId}`,
  });

  agentReply(hub, {
    agent: "scout",
    id: research.id,
    title: research.subject,
    body: SYNC_RESEARCH,
    timestamp: clock.dayAt(1, 16, 40),
    files: [
      text("findings.md", FINDINGS),
      text("benchmark-log.txt", BENCHMARK_LOG),
      { filename: "merge-times.png", bytes: PNG_PIXEL },
    ],
  });

  sentCopies(hub, {
    id: "20260312_pick_up_the_weekly_changelog",
    to: { drifter: { read: false } },
    title: MAIL_SEED.changelog.subject,
    body: CHANGELOG_REQUEST,
    timestamp: clock.dayAt(2, 18, 30),
  });

  sentCopies(hub, {
    id: backup.sentId,
    to: { atlas: { read: true } },
    title: backup.subject,
    body: BACKUP_QUESTION,
    timestamp: clock.dayAt(6, 10, 15),
  });
  agentReply(hub, {
    agent: "atlas",
    id: backup.replyId,
    title: `Re: ${backup.subject}`,
    body: BACKUP_REPLY,
    timestamp: clock.dayAt(6, 10, 40),
    inReplyTo: `atlas/agent/${backup.sentId}`,
    archived: true,
  });
}
