import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { MailAttachment, MailThread, OverviewResponse } from "../../src/lib/hub-types";
import { PNG_PIXEL } from "../inbox";
import { startMockServer, type MockServerHarness } from "../test-support";
import { MAIL_SEED, tinyPdf } from "./mail";

describe("tinyPdf", () => {
  const pdf = tinyPdf(["Hello (world)", "Second line"]).toString("latin1");

  it("is a PDF whose cross-reference table points at its objects", () => {
    expect(pdf.startsWith("%PDF-1.4\n")).toBe(true);
    expect(pdf.endsWith("%%EOF\n")).toBe(true);
    const xrefAt = Number(/startxref\n(\d+)\n/.exec(pdf)?.[1]);
    expect(pdf.slice(xrefAt, xrefAt + 5)).toBe("xref\n");
    const entries = [...pdf.slice(xrefAt).matchAll(/^(\d{10}) 00000 n $/gm)].map((m) =>
      Number(m[1]),
    );
    expect(entries).toHaveLength(5);
    entries.forEach((offset, index) => {
      expect(pdf.slice(offset, offset + 7), `object ${String(index + 1)}`).toBe(
        `${String(index + 1)} 0 obj`,
      );
    });
  });

  it("states the length of its page stream and escapes the text it shows", () => {
    const stream = /stream\n([^]*)\nendstream/.exec(pdf)?.[1] ?? "";
    expect(Number(/\/Length (\d+)/.exec(pdf)?.[1])).toBe(stream.length);
    expect(stream).toContain("(Hello \\(world\\)) Tj");
  });
});

describe("the scenario's mail", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await mock.close();
  });

  const url = (path: string): string => `${mock.baseUrl}${path}`;
  const thread = async (id: string): Promise<MailThread> => {
    const res = await fetch(url(`/api/hub/mail/threads/${encodeURIComponent(id)}`));
    expect(res.status, id).toBe(200);
    return (await res.json()) as MailThread;
  };
  const attachmentsOf = async (id: string): Promise<MailAttachment[]> =>
    (await thread(id)).messages.flatMap((message) => message.attachments);

  it("holds every thread MAIL_SEED names", async () => {
    const ids = [
      ...Object.values(MAIL_SEED.deployTomorrow.threadIds),
      ...Object.values(MAIL_SEED.dailyDigest.threadIds),
      ...Object.values(MAIL_SEED.lastWeeksDigest.threadIds),
      MAIL_SEED.wikiAudit.threadId,
      MAIL_SEED.syncResearch.threadId,
      MAIL_SEED.changelog.threadId,
      MAIL_SEED.backupRotation.threadId,
    ];
    for (const id of ids) expect((await thread(id)).thread_id).toBe(id);
  });

  it("keeps the sample inboxes' unread counts: only each agent's 'Deploy tomorrow' is unread", async () => {
    const overview = (await (await fetch(url("/api/hub/overview"))).json()) as OverviewResponse;
    expect(overview.agents.map((agent) => [agent.name, agent.inbox_unread])).toEqual([
      ["atlas", 1],
      ["brittle", 0],
      ["drifter", 0],
      ["scout", 1],
    ]);
  });

  it("has attachments of the kinds a message can carry: .md, .txt, .pdf and .png", async () => {
    const wiki = await attachmentsOf(MAIL_SEED.wikiAudit.threadId);
    const research = await attachmentsOf(MAIL_SEED.syncResearch.threadId);
    const all = [...wiki, ...research];
    expect(new Set(all.map((attachment) => attachment.content_type))).toEqual(
      new Set(["text/markdown", "text/plain", "application/pdf", "image/png"]),
    );
    expect(research.map((attachment) => attachment.filename)).toEqual(
      MAIL_SEED.syncResearch.attachments,
    );
  });

  it("serves each attachment's real bytes at its url, in the size it states", async () => {
    const all = [
      ...(await attachmentsOf(MAIL_SEED.wikiAudit.threadId)),
      ...(await attachmentsOf(MAIL_SEED.syncResearch.threadId)),
    ];
    expect(all.length).toBeGreaterThan(0);
    for (const attachment of all) {
      const res = await fetch(url(attachment.url));
      expect(res.status, attachment.filename).toBe(200);
      expect(res.headers.get("content-type"), attachment.filename).toBe(attachment.content_type);
      const bytes = Buffer.from(await res.arrayBuffer());
      expect(bytes.length, attachment.filename).toBe(attachment.size);
      if (attachment.content_type === "application/pdf") {
        expect(bytes.subarray(0, 8).toString(), attachment.filename).toBe("%PDF-1.4");
      }
      if (attachment.content_type === "image/png") {
        expect(bytes, attachment.filename).toEqual(PNG_PIXEL);
      }
      expect(bytes.toString("latin1")).not.toContain("Mock attachment");
    }
  });

  it("opens each text attachment in the file panel by its path, in its agent's workspace", async () => {
    const all = [
      ...(await attachmentsOf(MAIL_SEED.wikiAudit.threadId)),
      ...(await attachmentsOf(MAIL_SEED.syncResearch.threadId)),
    ].filter((attachment) => attachment.content_type.startsWith("text/"));
    expect(all.map((attachment) => attachment.filename)).toEqual([
      "audit-scope.md",
      "findings.md",
      "benchmark-log.txt",
    ]);
    for (const attachment of all) {
      const file = await fetch(
        url(
          `/api/agents/${attachment.agent}/workspace/file?path=${encodeURIComponent(attachment.path)}`,
        ),
      );
      expect(file.status, attachment.path).toBe(200);
      const viaAttachment = await (await fetch(url(attachment.url))).text();
      expect(await file.text(), attachment.path).toBe(viaAttachment);
    }
  });

  it("writes the Markdown message with headings, lists, a code block and links", async () => {
    const [message] = (await thread(MAIL_SEED.syncResearch.threadId)).messages;
    expect(message?.body).toMatch(/^# /m);
    expect(message?.body).toMatch(/^## /m);
    expect(message?.body).toMatch(/^\d+\. /m);
    expect(message?.body).toMatch(/^- /m);
    expect(message?.body).toMatch(/^```text$/m);
    expect(message?.body).toMatch(/\[[^\]]+\]\(https:\/\/[^)]+\)/);
  });

  it("starts over with the scenario on a reset", async () => {
    const sent = await fetch(url("/api/hub/mail/send"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ to: ["scout"], subject: "Temp", body: "Temp", attachments: [] }),
    });
    expect(sent.status).toBe(200);
    await fetch(url("/api/mock/reset"), { method: "POST" });
    const page = (await (await fetch(url("/api/hub/mail/threads?folder=sent"))).json()) as {
      threads: { subject: string }[];
    };
    expect(page.threads.map((entry) => entry.subject)).toEqual([
      MAIL_SEED.changelog.subject,
      MAIL_SEED.wikiAudit.subject,
      MAIL_SEED.backupRotation.subject,
    ]);
  });
});
