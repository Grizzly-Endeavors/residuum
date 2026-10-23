import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_UPLOAD_BYTES } from "../../lib/api";
import type { AgentState, AgentSummary } from "../../lib/hub-types";
import { hub } from "../../lib/hub.svelte";
import { mail } from "../../lib/mail.svelte";
import { toast } from "../../lib/toast.svelte";
import { render, screen } from "../../test/component";
import { jsonResponse, resetMail, serveMail, type SeenRequest } from "../../test/mail";
import { waitFor } from "../../test/wait";
import MailCompose from "./MailCompose.svelte";

function agent(name: string, state: AgentState = "running"): AgentSummary {
  return {
    name,
    display_name: name,
    state,
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
  };
}

let requests: SeenRequest[] = [];
let answer: (request: SeenRequest) => Response | Promise<Response> | undefined;

function sends(): SeenRequest[] {
  return requests.filter((request) => request.path === "/api/hub/mail/send");
}

function compose(to: string[] = []): {
  onsent: ReturnType<typeof vi.fn>;
  oncancel: ReturnType<typeof vi.fn>;
} {
  const onsent = vi.fn();
  const oncancel = vi.fn();
  render(MailCompose, { to, onsent, oncancel });
  return { onsent, oncancel };
}

function chosen(name: string): boolean {
  return (
    screen.getByRole("button", { name: new RegExp(`^${name}`) }).getAttribute("aria-pressed") ===
    "true"
  );
}

beforeEach(() => {
  resetMail();
  hub.agents = [agent("atlas"), agent("drifter", "stopped"), agent("scout")];
  answer = () => undefined;
  requests = serveMail((request) => {
    const own = answer(request);
    if (own !== undefined) return own;
    if (request.path === "/api/hub/mail/send") {
      return jsonResponse({ thread_id: "atlas/agent/20260314_plan" });
    }
    if (request.path.startsWith("/api/hub/mail/threads?")) {
      return jsonResponse({ threads: [], next: null });
    }
    return undefined;
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
  hub.agents = [];
});

describe("New message", () => {
  it("starts addressed to the agents it was opened for, and names a stopped one", () => {
    compose(["scout"]);
    expect(screen.getByRole("heading", { name: "New message", level: 2 })).toBeTruthy();
    expect(chosen("scout")).toBe(true);
    expect(chosen("atlas")).toBe(false);
    expect(screen.getByRole("button", { name: /^drifter/ }).textContent).toContain("stopped");
  });

  it("says it is a new thread that is not sent yet", () => {
    compose(["atlas"]);
    expect(screen.getByText("New thread · not sent yet")).toBeTruthy();
  });

  it("states the upload limit with or without a recipient, and says who will see it", async () => {
    compose();
    const form = screen.getByRole("form", { name: "New message" });
    expect(form).toHaveTextContent("Any file, up to 50 MB each. Pick at least one agent.");

    await userEvent.click(screen.getByRole("button", { name: /^atlas/ }));
    expect(form).toHaveTextContent(
      "Any file, up to 50 MB each. atlas will see this next time it checks its inbox.",
    );
    await userEvent.click(screen.getByRole("button", { name: /^scout/ }));
    expect(form).toHaveTextContent(
      "Any file, up to 50 MB each. They’ll see this next time they check their inbox.",
    );
  });

  it.each(["Control", "Meta"])(
    "sends from the message on %s+Enter, as the reply does",
    async (modifier) => {
      vi.spyOn(toast, "success").mockImplementation(() => 0);
      const { onsent } = compose(["atlas"]);
      await userEvent.type(
        screen.getByRole("textbox", { name: /^Message/ }),
        `Quick one.{${modifier}>}{Enter}{/${modifier}}`,
      );
      await waitFor(() => {
        expect(onsent).toHaveBeenCalledOnce();
      });
      expect(sends()).toHaveLength(1);
      expect(sends()[0]?.body).toMatchObject({ body: "Quick one.", to: ["atlas"] });
    },
  );

  it("moves from the subject to the message on Enter instead of sending", async () => {
    const { onsent } = compose(["atlas"]);
    await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Ready to go.");
    const subject = screen.getByRole("textbox", { name: "Subject" });
    await userEvent.type(subject, "Weekly plan{Enter}");
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: /^Message/ }));
    expect(sends()).toHaveLength(0);
    expect(onsent).not.toHaveBeenCalled();
  });

  it("still sends from the subject on Ctrl+Enter", async () => {
    vi.spyOn(toast, "success").mockImplementation(() => 0);
    const { onsent } = compose(["atlas"]);
    await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Ready to go.");
    await userEvent.type(screen.getByRole("textbox", { name: "Subject" }), "Plan");
    await userEvent.keyboard("{Control>}{Enter}{/Control}");
    await waitFor(() => {
      expect(onsent).toHaveBeenCalledOnce();
    });
    expect(sends()[0]?.body).toMatchObject({ subject: "Plan", body: "Ready to go." });
  });

  it("says what is missing when Ctrl+Enter is pressed on a blank message", async () => {
    compose(["atlas"]);
    screen.getByRole("textbox", { name: /^Message/ }).focus();
    await userEvent.keyboard("{Control>}{Enter}{/Control}");
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Write a message before sending it.",
    );
    expect(sends()).toHaveLength(0);
  });

  it("titles itself with the subject as it is written", async () => {
    compose(["atlas"]);
    await userEvent.type(screen.getByRole("textbox", { name: "Subject" }), "Weekly plan");
    expect(screen.getByRole("heading", { name: "Weekly plan" })).toBeTruthy();
  });

  it("says to choose an agent before sending, and sends to the ones chosen", async () => {
    const success = vi.spyOn(toast, "success").mockImplementation(() => 0);
    const { onsent } = compose();
    await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Plan the *week*.");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Add at least one agent to send it to.",
    );
    expect(sends()).toHaveLength(0);

    await userEvent.click(screen.getByRole("button", { name: /^atlas/ }));
    await userEvent.click(screen.getByRole("button", { name: /^scout/ }));
    expect(screen.queryByRole("alert")).toBeNull();
    await userEvent.type(screen.getByRole("textbox", { name: "Subject" }), "  Weekly plan ");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => {
      expect(onsent).toHaveBeenCalledWith("atlas/agent/20260314_plan");
    });
    expect(sends()[0]?.body).toEqual({
      to: ["atlas", "scout"],
      subject: "Weekly plan",
      body: "Plan the *week*.",
      attachments: [],
    });
    expect(success).toHaveBeenCalledWith("Sent to atlas and scout.");
    expect(mail.newMessage).toBeNull();
  });

  it("says to write something before sending a blank message", async () => {
    compose(["atlas"]);
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Write a message before sending it.",
    );
    expect(sends()).toHaveLength(0);
  });

  it("sends with no subject", async () => {
    vi.spyOn(toast, "success").mockImplementation(() => 0);
    const { onsent } = compose(["atlas"]);
    await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Quick one.");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => {
      expect(onsent).toHaveBeenCalledOnce();
    });
    expect(sends()[0]?.body).toMatchObject({ subject: "", to: ["atlas"] });
  });

  it("keeps everything and says why when the send fails", async () => {
    answer = (request) =>
      request.path === "/api/hub/mail/send"
        ? jsonResponse({ error: "There is no agent named 'atlas'." }, 400)
        : undefined;
    const { onsent } = compose(["atlas"]);
    await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Hello?");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Couldn't send your message.");
    expect(onsent).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: /^Message/ })).toHaveProperty("value", "Hello?");
  });

  it("asks the place to close it from Cancel and from Back, keeping the draft for it to decide", async () => {
    const { oncancel } = compose(["atlas"]);
    await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Draft");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await userEvent.click(screen.getByRole("button", { name: "Back to list" }));
    expect(oncancel).toHaveBeenCalledTimes(2);
    expect(mail.newMessage?.body).toBe("Draft");
  });

  it("keeps a draft already being written, and adds the recipients it is opened for", () => {
    const draft = mail.openNewMessage(["scout"]);
    draft.body = "Started earlier";
    compose(["atlas"]);
    expect(chosen("scout")).toBe(true);
    expect(chosen("atlas")).toBe(true);
    expect(screen.getByRole("textbox", { name: /^Message/ })).toHaveProperty(
      "value",
      "Started earlier",
    );
  });

  it("attaches files, refusing one over the limit, and sends with the uploaded ones", async () => {
    vi.spyOn(toast, "success").mockImplementation(() => 0);
    answer = (request) =>
      request.path.startsWith("/api/hub/uploads")
        ? jsonResponse({ id: "upload_0001", filename: "notes.md", size: 5 })
        : undefined;
    const { onsent } = compose(["atlas"]);
    const huge = new File(["x"], "huge.iso");
    Object.defineProperty(huge, "size", { value: MAX_UPLOAD_BYTES + 1 });
    await userEvent.upload(screen.getByLabelText("Files to attach"), [
      new File(["notes"], "notes.md", { type: "text/markdown" }),
      huge,
    ]);
    expect(
      screen.getByText("huge.iso is larger than the 50 MB upload limit, so it wasn't attached."),
    ).toBeTruthy();
    await waitFor(() => {
      expect(screen.queryByRole("progressbar", { name: "Uploading notes.md" })).toBeNull();
    });
    expect(screen.getByRole("button", { name: "Remove notes.md" })).toBeTruthy();
    await userEvent.type(screen.getByRole("textbox", { name: /^Message/ }), "Notes attached.");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => {
      expect(onsent).toHaveBeenCalledOnce();
    });
    expect(sends()[0]?.body).toMatchObject({ attachments: ["upload_0001"] });
  });
});
