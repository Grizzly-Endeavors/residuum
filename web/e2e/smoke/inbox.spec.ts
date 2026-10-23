import type { Locator, Page } from "@playwright/test";
import { MAIL_SEED } from "../../mock/data/mail";
import { MAX_UPLOAD_BYTES } from "../../mock/mail";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectFileOpen, expectFileRendered } from "../support/lazy";

/**
 * The Inbox as mail: conversations between you and your agents, in three
 * folders (Inbox, Sent, Archived), for every agent or one, with the open
 * thread beside the list. The scenario's mail (`mock/data/mail.ts`) gives
 * atlas and scout each an unread "Deploy tomorrow" and a read "Daily Digest",
 * and an archived "Last week's digest"; scout's "Research: local-first sync
 * libraries" carries a Markdown file, a text file and an image; the "Wiki
 * audit: stale pages" thread (sent to atlas and scout, with their replies and
 * your answer) carries a Markdown file and a PDF; "Pick up the weekly
 * changelog" went to drifter, which is stopped, and nobody answered; "Backup
 * rotation" was asked, answered and archived. Opening a thread marks it read,
 * which is what the rail's count follows.
 */

const AUDIT = MAIL_SEED.wikiAudit;
const SYNC = MAIL_SEED.syncResearch;
const CHANGELOG = MAIL_SEED.changelog;

/** Where scout's copy of the research message keeps its attachments, in scout's workspace. */
const SYNC_FILES = `inbox/user/attachments/${SYNC.id}`;

/** Path and query of the page's URL, decoded, so a test can compare them exactly. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

/** The conversations of the folder shown. On a phone it hides while a thread or New message is open. */
function conversations(page: Page): Locator {
  return page.getByRole("region", { name: "Conversations", exact: true });
}

/** The open thread. */
function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation", exact: true });
}

function rows(page: Page): Locator {
  return conversations(page).getByRole("listitem");
}

function matching(text: string): RegExp {
  return new RegExp(text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
}

/** A conversation's row by its subject, and by who it is with when two share a subject. */
function row(page: Page, subject: string, who?: string): Locator {
  const name =
    who === undefined
      ? matching(subject)
      : new RegExp(
          `^${who.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\b.*${matching(subject).source}`,
        );
  return conversations(page).getByRole("link", { name });
}

function heading(page: Page, subject: string): Locator {
  return conversation(page).getByRole("heading", { name: subject, level: 2 });
}

function folder(page: Page, name: "Inbox" | "Sent" | "Archived"): Locator {
  return page
    .getByRole("navigation", { name: "Folders" })
    .getByRole("link", { name: matching(name) });
}

/** The Inbox link with its count: in the rail, or in the phone's bottom bar. */
function inboxLink(page: Page, isMobile: boolean): Locator {
  const nav = isMobile
    ? page.getByRole("navigation", { name: "Main" })
    : page.getByRole("navigation", { name: "Places and agents" });
  return nav.getByRole("link", { name: /^Inbox/ });
}

/** The context panel showing `name`: a column beside the main region, or on a phone a full-screen sheet. */
function filePanel(page: Page, isMobile: boolean, name: string): Locator {
  return page.getByRole(isMobile ? "dialog" : "complementary", { name });
}

/** The panel's way out: Close beside the main region, Back on a phone's sheet. */
function closePanel(panel: Locator, isMobile: boolean): Locator {
  return panel.getByRole("button", { name: isMobile ? "Back" : "Close panel", exact: true });
}

/** The composer's file picker, which a button opens. */
function picker(page: Page): Locator {
  return page.getByLabel("Files to attach");
}

/** What the mock has on disk for one of scout's files. */
async function onDisk(page: Page, agent: string, path: string): Promise<string> {
  const response = await page.request.get(
    `/api/agents/${agent}/workspace/file?path=${encodeURIComponent(path)}`,
  );
  return response.text();
}

/** The toast saying `text`, with its Undo. An earlier toast stays up while manual time holds the clock, so it is found by its words. */
function toastWith(page: Page, text: string): Locator {
  return page.locator(".ui-toast", { hasText: text });
}

/** The phone shows the list or the thread, so it goes back to the list to see a row. */
async function toList(page: Page, isMobile: boolean): Promise<void> {
  if (isMobile) await conversation(page).getByRole("button", { name: "Back to list" }).click();
  await expect(conversations(page)).toBeVisible();
}

test.describe("the folders and the agent filter", () => {
  test("list each folder's conversations, with who they are with", async ({ page, isMobile }) => {
    await page.goto("/inbox");
    await expect(rows(page)).toHaveCount(6);
    await expect(row(page, "Deploy tomorrow", "atlas")).toHaveAccessibleName(/, unread/);
    await expect(row(page, "Deploy tomorrow", "scout")).toHaveAccessibleName(/, unread/);
    await expect(row(page, "Daily Digest", "atlas")).not.toHaveAccessibleName(/unread/);
    await expect(row(page, "Research: local-first sync libraries", "scout")).toHaveAccessibleName(
      /has attachments/,
    );
    await expect(row(page, "Wiki audit: stale pages", "atlas, scout")).toHaveAccessibleName(
      /4 messages/,
    );
    await expect(folder(page, "Inbox")).toHaveAccessibleName("Inbox 2 unread");
    await expect(folder(page, "Inbox")).toHaveAttribute("aria-current", "page");
    await expect(inboxLink(page, isMobile)).toHaveAccessibleName("Inbox 2 unread");

    // Sent holds what you wrote, worded by who it went to.
    await folder(page, "Sent").click();
    await expect.poll(() => address(page)).toBe("/inbox?folder=sent");
    await expect(folder(page, "Sent")).toHaveAttribute("aria-current", "page");
    await expect(rows(page)).toHaveCount(3);
    await expect(row(page, CHANGELOG.subject, "To drifter")).toBeVisible();
    await expect(row(page, AUDIT.subject, "To atlas, scout")).toBeVisible();
    await expect(row(page, "Backup rotation", "To atlas")).toBeVisible();

    await folder(page, "Archived").click();
    await expect.poll(() => address(page)).toBe("/inbox?folder=archived");
    await expect(rows(page)).toHaveCount(3);
    await expect(row(page, "Backup rotation", "atlas")).toBeVisible();
    await expect(row(page, "Last week's digest", "atlas")).toBeVisible();
    await expect(row(page, "Last week's digest", "scout")).toBeVisible();

    await folder(page, "Inbox").click();
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(rows(page)).toHaveCount(6);
  });

  test("the filter shows one agent's conversations, in whichever folder is open", async ({
    page,
  }) => {
    await page.goto("/inbox");
    const filter = conversations(page).getByRole("combobox", { name: "Show conversations with" });
    await expect(filter.getByRole("option", { name: "atlas (1 unread)" })).toBeAttached();

    await filter.selectOption("scout");
    await expect.poll(() => address(page)).toBe("/inbox?agent=scout");
    await expect(rows(page)).toHaveCount(4);
    await expect(row(page, "Deploy tomorrow", "atlas")).toHaveCount(0);
    await expect(row(page, "Research: local-first sync libraries")).toBeVisible();

    // The folder keeps the filter, and the Sent rows are the ones that include scout.
    await folder(page, "Sent").click();
    await expect.poll(() => address(page)).toBe("/inbox?folder=sent&agent=scout");
    await expect(rows(page)).toHaveCount(1);
    await expect(row(page, AUDIT.subject)).toBeVisible();

    await folder(page, "Inbox").click();
    await filter.selectOption("drifter");
    await expect(
      conversations(page).getByText("Nothing from drifter in your inbox."),
    ).toBeVisible();

    await filter.selectOption("");
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(rows(page)).toHaveCount(6);
  });

  test("an empty folder says so, and an empty Sent offers New message", async ({ page }) => {
    await page.goto("/inbox?folder=archived&agent=drifter");
    await expect(conversations(page).getByText("Nothing with drifter is archived.")).toBeVisible();

    await folder(page, "Sent").click();
    await conversations(page)
      .getByRole("combobox", { name: "Show conversations with" })
      .selectOption("brittle");
    await expect(
      conversations(page).getByText("You haven't written to brittle yet."),
    ).toBeVisible();
    await conversations(page).getByRole("button", { name: "New message" }).click();
    await expect
      .poll(() => address(page))
      .toBe("/inbox?folder=sent&agent=brittle&compose=1&to=brittle");
    await expect(page.getByRole("form", { name: "New message" })).toBeVisible();
  });
});

test.describe("reading a thread", () => {
  test("opening one marks it read, and the unread count drops", async ({ page, isMobile }) => {
    await page.goto("/inbox");
    const count = inboxLink(page, isMobile);
    await expect(count).toHaveAccessibleName("Inbox 2 unread");

    await row(page, "Deploy tomorrow", "atlas").click();
    await expect.poll(() => address(page)).toBe("/inbox?thread=atlas/user/mock_1");
    await expect(heading(page, "Deploy tomorrow")).toBeVisible();
    await expect(
      conversation(page).getByText("Reminder to trigger the deployment pipeline tomorrow morning."),
    ).toBeVisible();
    await expect(count).toHaveAccessibleName("Inbox 1 unread");
    if (!isMobile) {
      // The list is beside the thread: the row is the open one, and no longer unread.
      await expect(row(page, "Deploy tomorrow", "atlas")).toHaveAttribute("aria-current", "true");
      await expect(row(page, "Deploy tomorrow", "atlas")).not.toHaveAccessibleName(/unread/);
      await expect(folder(page, "Inbox")).toHaveAccessibleName("Inbox 1 unread");
    }

    // A link opens it too, and reading the last unread thread clears the count.
    await page.goto("/inbox?thread=scout/user/mock_1");
    await expect(heading(page, "Deploy tomorrow")).toBeVisible();
    await expect(count).toHaveAccessibleName("Inbox");
  });

  test("a thread with Markdown and attachments shows them, and passes the accessibility scan", async ({
    page,
  }) => {
    await page.goto(`/inbox?thread=${SYNC.threadId}`);
    const thread = conversation(page);
    await expect(heading(page, SYNC.subject)).toBeVisible();
    await expect(thread.getByText("You and scout · 1 message")).toBeVisible();
    await expect(thread.getByRole("heading", { name: "Candidates", level: 4 })).toBeVisible();
    await expect(thread.getByRole("link", { name: "Yjs documentation" })).toBeVisible();

    const files = thread.getByRole("list", { name: "Attachments" });
    await expect(files.getByRole("button", { name: "Open findings.md" })).toBeVisible();
    await expect(files.getByRole("button", { name: "Open benchmark-log.txt" })).toBeVisible();
    await expect(files.getByRole("link", { name: "Download merge-times.png" })).toBeVisible();
    await expectNoAxeViolations(page);
  });

  test("Back leaves the open thread before it leaves the Inbox", async ({ page, isMobile }) => {
    await page.goto("/home");
    await page.goto("/inbox");
    await row(page, "Daily Digest", "scout").click();
    await expect.poll(() => address(page)).toBe("/inbox?thread=scout/user/mock_2");

    if (isMobile) {
      // The thread has the screen to itself; Back to list returns to the row it came from.
      await conversation(page).getByRole("button", { name: "Back to list" }).click();
      await expect(row(page, "Daily Digest", "scout")).toBeFocused();
    } else {
      // Another thread takes its place without adding to the history.
      await row(page, "Daily Digest", "atlas").click();
      await expect.poll(() => address(page)).toBe("/inbox?thread=atlas/user/mock_2");
      await page.goBack();
    }
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(conversation(page)).toHaveCount(0);

    await page.goBack();
    await expect.poll(() => address(page)).toBe("/home");
  });

  test("a new message from an agent shows in the list while the Inbox is open", async ({
    page,
    mock,
    isMobile,
  }) => {
    await page.goto("/inbox");
    await expect(rows(page)).toHaveCount(6);

    await mock.post("/api/mock/user-inbox-add", {
      params: { agent: "scout" },
      data: { title: "Build is green" },
    });

    await expect(row(page, "Build is green", "scout")).toBeVisible();
    await expect(inboxLink(page, isMobile)).toHaveAccessibleName("Inbox 3 unread");
  });

  test("an agent's reply appears in the thread that is open", async ({ page, mock }) => {
    await page.goto(`/inbox?thread=${AUDIT.threadId}`);
    await expect(conversation(page).getByRole("article")).toHaveCount(4);

    await mock.post("/api/mock/user-inbox-add", {
      params: { agent: "atlas" },
      data: {
        body: "The two oldest pages are archived.",
        in_reply_to: `atlas/agent/${AUDIT.sentId}`,
      },
    });

    await expect(conversation(page).getByRole("article")).toHaveCount(5);
    await expect(conversation(page).getByText("The two oldest pages are archived.")).toBeVisible();
  });
});

test.describe("Archive and Move to Inbox", () => {
  test("archive a thread, undo it, then archive it and move it back from Archived", async ({
    page,
    mock,
  }) => {
    await page.goto("/inbox?thread=atlas/user/mock_2");
    await expect(heading(page, "Daily Digest")).toBeVisible();
    // Manual time holds the toast's own dismissal while the spec clicks Undo.
    await mock.manualTime();

    await conversation(page).getByRole("button", { name: "Archive" }).click();
    const archived = toastWith(page, "Archived “Daily Digest”.");
    await expect(archived).toBeVisible();
    // The thread closes and the list is back, without it.
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(row(page, "Daily Digest", "atlas")).toHaveCount(0);
    await expect(rows(page)).toHaveCount(5);

    await archived.getByRole("button", { name: "Undo" }).click();
    await expect(row(page, "Daily Digest", "atlas")).toBeVisible();
    await expect(rows(page)).toHaveCount(6);

    // Archive it again and find it in Archived, where Move to Inbox brings it back.
    await row(page, "Daily Digest", "atlas").click();
    await conversation(page).getByRole("button", { name: "Archive" }).click();
    await expect(row(page, "Daily Digest", "atlas")).toHaveCount(0);
    await folder(page, "Archived").click();
    await expect.poll(() => address(page)).toBe("/inbox?folder=archived");
    await expect(rows(page)).toHaveCount(4);

    await row(page, "Daily Digest", "atlas").click();
    await expect(conversation(page).getByRole("button", { name: "Archive" })).toHaveCount(0);
    await conversation(page).getByRole("button", { name: "Move to Inbox" }).click();
    const moved = toastWith(page, "Moved “Daily Digest” back to the inbox.");
    await expect(moved).toBeVisible();
    await expect(row(page, "Daily Digest", "atlas")).toHaveCount(0);

    await moved.getByRole("button", { name: "Undo" }).click();
    await expect(row(page, "Daily Digest", "atlas")).toBeVisible();
    await folder(page, "Inbox").click();
    await expect(row(page, "Daily Digest", "atlas")).toHaveCount(0);
  });

  test("archiving a thread that was unread leaves the other unread thread counted", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/inbox?thread=atlas/user/mock_1");
    await expect(heading(page, "Deploy tomorrow")).toBeVisible();
    await conversation(page).getByRole("button", { name: "Archive" }).click();
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(inboxLink(page, isMobile)).toHaveAccessibleName("Inbox 1 unread");
    await expect(row(page, "Deploy tomorrow", "atlas")).toHaveCount(0);
    await expect(row(page, "Deploy tomorrow", "scout")).toBeVisible();
  });
});

test.describe("Sent, and replying", () => {
  test("a thread only you wrote in shows who has seen it, with neither Archive nor Move to Inbox", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/inbox?folder=sent");
    await row(page, CHANGELOG.subject, "To drifter").click();
    await expect.poll(() => address(page)).toBe(`/inbox?folder=sent&thread=${CHANGELOG.threadId}`);

    const thread = conversation(page);
    await expect(heading(page, CHANGELOG.subject)).toBeVisible();
    await expect(thread.getByRole("article")).toHaveCount(1);
    await expect(thread.getByText("to drifter")).toBeVisible();
    await expect(thread.getByText(/draft this week's changelog/)).toBeVisible();
    await expect(thread.getByText(/Not seen yet . waiting in drifter.s inbox/)).toBeVisible();
    await expect(thread.getByRole("button", { name: "Archive" })).toHaveCount(0);
    await expect(thread.getByRole("button", { name: "Move to Inbox" })).toHaveCount(0);
    await expect(thread.getByRole("form", { name: "Reply to the thread" })).toContainText(
      "To: drifter",
    );

    // Sent doesn't count toward the unread mail.
    await expect(inboxLink(page, isMobile)).toHaveAccessibleName("Inbox 2 unread");
  });

  test("a reply goes to everyone in the thread, and appears in it", async ({ page, mock }) => {
    await page.goto(`/inbox?thread=${AUDIT.threadId}`);
    const thread = conversation(page);
    const reply = thread.getByRole("form", { name: "Reply to the thread" });
    await expect(thread.getByRole("article")).toHaveCount(4);
    await expect(reply).toContainText("To: atlas and scout");
    await expect(reply).toContainText("They’ll see this next time they check their inbox.");
    await expect(reply.getByRole("button", { name: "Send" })).toBeDisabled();
    await mock.manualTime();

    await reply.getByRole("textbox", { name: "Reply" }).fill("Thanks, that is enough for now.");
    await reply.getByRole("button", { name: "Send" }).click();

    await expect(page.getByText("Sent to atlas and scout.")).toBeVisible();
    await expect(thread.getByRole("article")).toHaveCount(5);
    const latest = thread.getByRole("article").last();
    await expect(latest).toContainText("Thanks, that is enough for now.");
    await expect(latest).toContainText("to atlas and scout");
    await expect(latest).toContainText("Not seen yet by atlas or scout");
    await expect(reply.getByRole("textbox", { name: "Reply" })).toHaveValue("");

    // The answer is in the thread for good: reopening it shows five messages.
    await page.reload();
    await expect(thread.getByRole("article")).toHaveCount(5);
  });

  test("Ctrl+Enter sends a reply", async ({ page }) => {
    await page.goto(`/inbox?thread=${AUDIT.threadId}`);
    const thread = conversation(page);
    await expect(thread.getByRole("article")).toHaveCount(4);
    await thread.getByRole("textbox", { name: "Reply" }).fill("Sent from the keyboard.");
    await thread.getByRole("textbox", { name: "Reply" }).press("Control+Enter");
    await expect(thread.getByRole("article")).toHaveCount(5);
  });
});

test.describe("New message", () => {
  test("goes to two agents with a subject, a body and a file, and opens in Sent", async ({
    page,
    mock,
    isMobile,
  }) => {
    await page.goto("/inbox");
    await page.getByRole("button", { name: "New message" }).click();
    await expect.poll(() => address(page)).toBe("/inbox?compose=1");

    const compose = page.getByRole("form", { name: "New message" });
    await expect(compose).toBeVisible();
    await expect(compose.getByRole("button", { name: /^drifter\W+stopped$/ })).toBeVisible();
    await expect(compose).toContainText("Pick at least one agent.");
    await compose.getByRole("button", { name: "atlas", exact: true }).click();
    await compose.getByRole("button", { name: "scout", exact: true }).click();
    await expect(compose.getByRole("button", { name: "atlas", exact: true })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(compose.getByRole("button", { name: "scout", exact: true })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    // Enter in the Subject moves on to the message instead of sending.
    await compose.getByRole("textbox", { name: "Subject" }).fill("Release checklist");
    await compose.getByRole("textbox", { name: "Subject" }).press("Enter");
    await expect(compose.getByRole("textbox", { name: /^Message/ })).toBeFocused();
    await expect(compose).toBeVisible();
    expect(address(page)).toBe("/inbox?compose=1");
    await expect(compose.getByRole("heading", { name: "Release checklist" })).toBeVisible();

    await compose
      .getByRole("textbox", { name: /^Message/ })
      .fill("Please **review** the checklist before Friday.");
    await picker(page).setInputFiles({
      name: "checklist.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("1. Tag the release\n2. Publish the notes\n"),
    });
    const staged = compose.getByRole("list", { name: "Attached files" });
    await expect(staged.getByRole("button", { name: "Remove checklist.txt" })).toBeVisible();
    await expectNoAxeViolations(page);

    await mock.manualTime();
    await compose.getByRole("button", { name: "Send" }).click();
    await expect(page.getByText("Sent to atlas and scout.")).toBeVisible();
    await expect
      .poll(() => address(page))
      .toBe("/inbox?folder=sent&thread=atlas/agent/20260314_release_checklist");

    const thread = conversation(page);
    await expect(heading(page, "Release checklist")).toBeVisible();
    await expect(thread.getByText("to atlas and scout")).toBeVisible();
    await expect(thread.getByText("review", { exact: true })).toBeVisible();
    await expect(thread.getByRole("button", { name: "Open checklist.txt" })).toBeVisible();

    await toList(page, isMobile);
    await expect(folder(page, "Sent")).toHaveAttribute("aria-current", "page");
    await expect(row(page, "Release checklist", "To atlas, scout")).toBeVisible();
  });

  test("a message needs a body and at least one agent to be sent", async ({ page }) => {
    await page.goto("/inbox?compose=1&to=atlas");
    const compose = page.getByRole("form", { name: "New message" });
    await expect(compose.getByRole("button", { name: "atlas", exact: true })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    await compose.getByRole("button", { name: "Send" }).click();
    await expect(compose.getByRole("alert")).toHaveText("Write a message before sending it.");

    await compose.getByRole("button", { name: "atlas", exact: true }).click();
    await compose.getByRole("textbox", { name: /^Message/ }).fill("Hello.");
    await compose.getByRole("button", { name: "Send" }).click();
    await expect(compose.getByRole("alert")).toHaveText("Add at least one agent to send it to.");
    expect(address(page)).toBe("/inbox?compose=1&to=atlas");
  });

  test("a file over 50 MB is refused before anything uploads, and the rest still attach", async ({
    page,
  }) => {
    const uploads: string[] = [];
    page.on("request", (request) => {
      if (request.url().includes("/api/hub/uploads")) uploads.push(request.url());
    });
    await page.goto("/inbox?compose=1&to=atlas");
    const compose = page.getByRole("form", { name: "New message" });
    await expect(compose).toContainText("Any file, up to 50 MB each.");

    // Playwright won't hand a browser a buffer this big, so the page makes the file.
    await picker(page).evaluate((input: HTMLInputElement, bytes: number) => {
      const files = new DataTransfer();
      files.items.add(
        new File([new Uint8Array(bytes)], "huge-video.mov", { type: "video/quicktime" }),
      );
      input.files = files.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
    }, MAX_UPLOAD_BYTES + 1);

    await expect(
      compose.getByText(
        "huge-video.mov is larger than the 50 MB upload limit, so it wasn't attached.",
      ),
    ).toBeVisible();
    await expect(compose.getByRole("list", { name: "Attached files" })).toHaveCount(0);
    expect(uploads).toEqual([]);

    await compose.getByRole("button", { name: "Dismiss" }).click();
    await expect(compose.getByText(/larger than the 50 MB upload limit/)).toHaveCount(0);

    // A file within the limit attaches as usual, and uploads.
    await picker(page).setInputFiles({
      name: "notes.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("small enough"),
    });
    await expect(compose.getByRole("button", { name: "Remove notes.txt" })).toBeVisible();
    await expect.poll(() => uploads.length).toBe(1);
  });

  test("leaving with text written asks first", async ({ page, isMobile }) => {
    await page.goto("/inbox?compose=1&to=atlas");
    const compose = page.getByRole("form", { name: "New message" });
    await compose.getByRole("textbox", { name: /^Message/ }).fill("Half a thought");
    const ask = page.getByRole("alertdialog", { name: "Discard unsaved changes?" });

    const nav = isMobile
      ? page.getByRole("navigation", { name: "Main" })
      : page.getByRole("navigation", { name: "Places and agents" });
    await nav.getByRole("link", { name: /^Home/ }).click();
    await expect(ask).toContainText("An unsent new message");
    await ask.getByRole("button", { name: "Keep editing" }).click();
    await expect(compose.getByRole("textbox", { name: /^Message/ })).toHaveValue("Half a thought");
    expect(address(page)).toBe("/inbox?compose=1&to=atlas");

    await nav.getByRole("link", { name: /^Home/ }).click();
    await ask.getByRole("button", { name: "Discard and leave" }).click();
    await expect.poll(() => address(page)).toBe("/home");
  });
});

test.describe("attachments", () => {
  test("a Markdown file opens rendered in the Files panel beside the thread, and Edit saves it", async ({
    page,
    isMobile,
  }) => {
    await page.goto(`/inbox?thread=${SYNC.threadId}`);
    await conversation(page).getByRole("button", { name: "Open findings.md" }).click();

    await expect
      .poll(() => address(page))
      .toBe(`/inbox?thread=${SYNC.threadId}&panel=agent-file:scout:${SYNC_FILES}/findings.md`);
    const panel = filePanel(page, isMobile, "findings.md");
    await expect(panel).toBeVisible();
    await expectFileRendered(page, "Local-first sync: findings");
    await expect(
      panel.getByText(
        "ElectricSQL assumes Postgres, which doesn't fit a workspace of plain files.",
      ),
    ).toBeVisible();
    // The thread it came from stays open beside the panel.
    await expect(heading(page, SYNC.subject)).toBeVisible();
    if (isMobile) await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
    else await expectNoAxeViolations(page);

    await panel.getByRole("button", { name: "Edit" }).click();
    const editor = await expectFileOpen(page, "findings.md");
    // The file has to be in the editor before the edit starts, or loading it replaces the edit.
    await expect(editor).toHaveValue(/Prototype \*\*Yjs\*\* first/);
    await editor.press("Control+End");
    await editor.pressSequentially("- Added in the panel.\n");
    await expect(panel.getByText("Unsaved changes")).toBeVisible();

    await panel.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByText("Saved findings.md.")).toBeVisible();
    expect(await onDisk(page, "scout", `${SYNC_FILES}/findings.md`)).toMatch(
      /- Added in the panel\.\n$/,
    );

    // Back to the rendered view, which shows the saved line.
    await panel.getByRole("button", { name: "Edit" }).click();
    await expect(panel.getByText("Added in the panel.")).toBeVisible();

    await closePanel(panel, isMobile).click();
    await expect(panel).toBeHidden();
    await expect.poll(() => address(page)).toBe(`/inbox?thread=${SYNC.threadId}`);
    await expect(heading(page, SYNC.subject)).toBeVisible();
  });

  test("the panel belongs to the thread, so Back to list closes both", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "A phone's file sheet covers the thread; its Back closes just the sheet.");
    await page.goto(`/inbox?thread=${SYNC.threadId}`);
    await conversation(page).getByRole("button", { name: "Open findings.md" }).click();
    const panel = filePanel(page, isMobile, "findings.md");
    await expect(panel).toBeVisible();
    await expectFileRendered(page, "Local-first sync: findings");

    // Room for the panel and the thread takes the list's place, so the thread offers Back to list.
    await conversation(page).getByRole("button", { name: "Back to list" }).click();
    await expect(panel).toBeHidden();
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(conversations(page)).toBeVisible();
  });

  test("a text file opens in the panel as it was written", async ({ page, isMobile }) => {
    await page.goto(`/inbox?thread=${SYNC.threadId}`);
    await conversation(page).getByRole("button", { name: "Open benchmark-log.txt" }).click();

    const panel = filePanel(page, isMobile, "benchmark-log.txt");
    await expect(panel).toBeVisible();
    await expect(
      panel.getByText("10,000 edits, 3 replicas, merged in random order."),
    ).toBeVisible();
    await expect(panel.getByRole("heading", { name: "library" })).toHaveCount(0);
    expect(address(page)).toBe(
      `/inbox?thread=${SYNC.threadId}&panel=agent-file:scout:${SYNC_FILES}/benchmark-log.txt`,
    );
  });

  test("a PDF downloads", async ({ page }) => {
    await page.goto(`/inbox?thread=${AUDIT.threadId}`);
    const file = conversation(page).getByRole("link", { name: "Download source-notes.pdf" });
    await expect(file).toBeVisible();

    const download = page.waitForEvent("download");
    await file.click();
    expect((await download).suggestedFilename()).toBe("source-notes.pdf");
    const served = await page.request.get((await file.getAttribute("href")) ?? "");
    expect(served.headers()["content-type"]).toBe("application/pdf");
    expect((await served.body()).subarray(0, 5).toString("latin1")).toBe("%PDF-");
  });

  test("a stopped agent's thread opens, and its attachments download and open", async ({
    page,
    isMobile,
  }) => {
    const stopped = await page.request.post("/api/hub/agents/scout/stop");
    expect(stopped.ok()).toBe(true);

    await page.goto(`/inbox?thread=${SYNC.threadId}`);
    await expect(heading(page, SYNC.subject)).toBeVisible();
    const image = conversation(page).getByRole("link", { name: "Download merge-times.png" });
    const download = page.waitForEvent("download");
    await image.click();
    expect((await download).suggestedFilename()).toBe("merge-times.png");
    const served = await page.request.get((await image.getAttribute("href")) ?? "");
    expect(served.headers()["content-type"]).toBe("image/png");
    expect((await served.body()).subarray(1, 4).toString("latin1")).toBe("PNG");

    // The Markdown file loads from the stopped agent's workspace too.
    await conversation(page).getByRole("button", { name: "Open findings.md" }).click();
    await expect(filePanel(page, isMobile, "findings.md")).toBeVisible();
    await expectFileRendered(page, "Local-first sync: findings");
  });
});

test.describe("links", () => {
  test("a link from before threads still lands on its conversation", async ({ page }) => {
    await page.goto("/inbox?item=atlas:mock_1");
    await expect(heading(page, "Deploy tomorrow")).toBeVisible();
    await expect.poll(() => address(page)).toBe("/inbox?thread=atlas/user/mock_1");

    await page.goto("/inbox?agent=scout&item=scout:mock_2");
    await expect(heading(page, "Daily Digest")).toBeVisible();
    await expect.poll(() => address(page)).toBe("/inbox?agent=scout&thread=scout/user/mock_2");
  });

  test("a link to the archive tab lands in the Archived folder", async ({ page }) => {
    await page.goto("/inbox?tab=archived");
    await expect.poll(() => address(page)).toBe("/inbox?folder=archived");
    await expect(folder(page, "Archived")).toHaveAttribute("aria-current", "page");
    await expect(row(page, "Last week's digest", "atlas")).toBeVisible();
    await expect(rows(page)).toHaveCount(3);
  });

  test("any message's address opens its thread under the thread's own", async ({ page }) => {
    // The Wiki audit thread began with the copy sent to atlas; atlas's reply names a message in it.
    await page.goto(`/inbox?thread=atlas/user/${AUDIT.replyId}`);
    await expect(heading(page, AUDIT.subject)).toBeVisible();
    await expect.poll(() => address(page)).toBe(`/inbox?thread=${AUDIT.threadId}`);
    await expect(conversation(page).getByRole("article")).toHaveCount(4);
  });

  test("a link to a conversation that is gone says so, and leads back to the list", async ({
    page,
  }) => {
    await page.goto("/inbox?thread=atlas/user/no_such_message");
    const gone = page.getByRole("region", { name: "Conversation not found" });
    await expect(
      gone.getByRole("heading", { name: "That conversation isn't there any more", level: 2 }),
    ).toBeVisible();
    await expectNoAxeViolations(page);

    await gone.getByRole("button", { name: "Back to list" }).click();
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(rows(page)).toHaveCount(6);

    await page.goto("/inbox?item=atlas:no_such_item");
    await expect(
      page.getByRole("heading", { name: "That conversation isn't there any more", level: 2 }),
    ).toBeVisible();
  });
});

test.describe("when something fails", () => {
  test("a list that can't load says so, and Try again loads it", async ({ page }) => {
    const listing = /\/api\/hub\/mail\/threads\?/;
    await page.route(listing, (route) =>
      route.fulfill({ status: 500, contentType: "application/json", body: '{"error":"boom"}' }),
    );
    await page.goto("/inbox");
    const problem = conversations(page).getByRole("alert").filter({
      hasText: "Couldn't load your inbox.",
    });
    await expect(problem).toBeVisible();
    await expectNoAxeViolations(page);

    await page.unroute(listing);
    await problem.getByRole("button", { name: "Try again" }).click();
    await expect(rows(page)).toHaveCount(6);
    await expect(problem).toHaveCount(0);
  });

  test("an archive that fails keeps the thread open and can be tried again", async ({ page }) => {
    const archiving = /\/api\/hub\/mail\/threads\/[^/]+\/archive$/;
    await page.route(archiving, (route) =>
      route.fulfill({ status: 503, contentType: "application/json", body: '{"error":"busy"}' }),
    );
    await page.goto("/inbox?thread=atlas/user/mock_2");
    const thread = conversation(page);
    await expect(heading(page, "Daily Digest")).toBeVisible();
    await thread.getByRole("button", { name: "Archive" }).click();

    const problem = thread.getByRole("alert").filter({ hasText: "Couldn't archive it." });
    await expect(problem).toBeVisible();
    expect(address(page)).toBe("/inbox?thread=atlas/user/mock_2");

    await page.unroute(archiving);
    await problem.getByRole("button", { name: "Try again" }).click();
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(row(page, "Daily Digest", "atlas")).toHaveCount(0);
  });

  test("a thread that can't load says so, and Try again opens it", async ({ page }) => {
    const opening = /\/api\/hub\/mail\/threads\/[^/?]+$/;
    await page.route(opening, (route) =>
      route.fulfill({ status: 500, contentType: "application/json", body: '{"error":"boom"}' }),
    );
    await page.goto("/inbox?thread=atlas/user/mock_2");
    const problem = conversation(page).getByRole("alert").filter({
      hasText: "Couldn't open that conversation.",
    });
    await expect(problem).toBeVisible();

    await page.unroute(opening);
    await problem.getByRole("button", { name: "Try again" }).click();
    await expect(heading(page, "Daily Digest")).toBeVisible();
  });

  test("a reply that fails keeps its text, and sends when tried again", async ({ page }) => {
    const replying = /\/api\/hub\/mail\/threads\/[^/]+\/reply$/;
    await page.route(replying, (route) =>
      route.fulfill({ status: 503, contentType: "application/json", body: '{"error":"busy"}' }),
    );
    await page.goto(`/inbox?thread=${AUDIT.threadId}`);
    const reply = conversation(page).getByRole("form", { name: "Reply to the thread" });
    await reply.getByRole("textbox", { name: "Reply" }).fill("Please go ahead.");
    await reply.getByRole("button", { name: "Send" }).click();
    await expect(reply.getByRole("alert")).toContainText("Couldn't send your reply.");
    await expect(reply.getByRole("textbox", { name: "Reply" })).toHaveValue("Please go ahead.");

    await page.unroute(replying);
    await reply.getByRole("button", { name: "Send" }).click();
    await expect(conversation(page).getByRole("article")).toHaveCount(5);
  });
});

test.describe("the layout", () => {
  test("on a phone one pane shows at a time, with Back to list", async ({ page, isMobile }) => {
    test.skip(!isMobile, "The phone shows the list or the open thread, never both.");
    await page.goto("/inbox");
    await expect(conversations(page)).toBeVisible();
    await expect(page.getByText("Choose a conversation to read it here.")).toBeHidden();

    await row(page, "Daily Digest", "atlas").click();
    await expect(conversation(page)).toBeVisible();
    await expect(conversations(page)).toBeHidden();
    await expect(conversation(page).getByRole("button", { name: "Back to list" })).toBeVisible();

    await conversation(page).getByRole("button", { name: "Back to list" }).click();
    await expect(conversations(page)).toBeVisible();
    await expect(conversation(page)).toHaveCount(0);

    await page.getByRole("button", { name: "New message" }).click();
    await expect(page.getByRole("form", { name: "New message" })).toBeVisible();
    await expect(conversations(page)).toBeHidden();
    await page
      .getByRole("form", { name: "New message" })
      .getByRole("button", { name: "Cancel" })
      .click();
    await expect(conversations(page)).toBeVisible();
    await expect.poll(() => address(page)).toBe("/inbox");
  });

  test("on a desktop the list sits beside the thread, which needs no Back to list", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "The desktop shows the list and the thread together.");
    await page.goto("/inbox?thread=atlas/user/mock_2");
    await expect(heading(page, "Daily Digest")).toBeVisible();
    await expect(conversations(page)).toBeVisible();
    await expect(conversation(page).getByRole("button", { name: "Back to list" })).toBeHidden();

    const list = await conversations(page).boundingBox();
    const thread = await conversation(page).boundingBox();
    expect(list).not.toBeNull();
    expect(thread).not.toBeNull();
    expect((list?.x ?? 0) + (list?.width ?? 0)).toBeLessThanOrEqual(thread?.x ?? 0);
  });
});
