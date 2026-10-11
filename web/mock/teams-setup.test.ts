import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { TeamsSetupJob, TeamsSetupPrereqs } from "../src/lib/generated/protocol";
import { fetchJson, fetchText, startMockServer, type MockServerHarness } from "./test-support";

describe("the Teams setup wizard mock routes", () => {
  let harness: MockServerHarness;
  const agent = "atlas";

  const call = (method: string, path: string, body?: unknown): ReturnType<typeof fetchJson> =>
    fetchJson(`${harness.baseUrl}/api/agents/${agent}/teams-setup${path}`, {
      method,
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });

  const getPrereqs = async (): Promise<TeamsSetupPrereqs> =>
    (await call("GET", "/prereqs")).body as TeamsSetupPrereqs;

  const getJob = async (logSince?: number): Promise<{ status: number; job: TeamsSetupJob }> => {
    const query = logSince !== undefined ? `?log_since=${logSince}` : "";
    const res = await call("GET", `/job${query}`);
    return { status: res.status, job: res.body as TeamsSetupJob };
  };

  const validForm = {
    bot_name: "Atlas Bot",
    short_description: "Helpful agent in Teams",
    long_description: "A full description of what Atlas does in Microsoft Teams.",
    developer_name: "Residuum Team",
    developer_url: "https://residuum.example.com",
    privacy_url: "https://residuum.example.com/privacy",
    terms_url: "https://residuum.example.com/terms",
    messaging_endpoint: "https://my-hub.residuum.cloud/api/teams/messages",
    color_icon_png_base64: null,
    outline_icon_png_base64: null,
  };

  beforeEach(async () => {
    harness = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await harness.close();
  });

  it("serves prerequisites with node/npm and pinned ATK version", async () => {
    const prereqs = await getPrereqs();
    expect(prereqs.node.found).toBe(true);
    expect(prereqs.npm.found).toBe(true);
    expect(prereqs.atk.pinned_version).toBe("1.1.17");
    expect(prereqs.manual_guide_url).toContain("teams-setup");
  });

  it("answers 404 for job when no setup has run", async () => {
    const res = await call("GET", "/job");
    expect(res.status).toBe(404);
  });

  it("validates consent when ATK is not installed", async () => {
    const res = await call("POST", "/job", {
      form: validForm,
      consent_install_cli: false,
      replace_existing: false,
    });
    expect(res.status).toBe(400);
    expect((res.body as { error: string }).error).toContain("consent");
  });

  it("validates replace_existing when Teams is already configured", async () => {
    // Configure teams in agent's config
    const state = harness.hub.agents.get(agent)?.state;
    expect(state).toBeDefined();
    if (!state) return;
    state.configToml += '\n[teams]\napp_id = "existing-app-id"\n';

    const res = await call("POST", "/job", {
      form: validForm,
      consent_install_cli: true,
      replace_existing: false,
    });
    expect(res.status).toBe(400);
    expect((res.body as { error: string }).error).toContain("replac");
  });

  it("validates required form fields and length bounds", async () => {
    let res = await call("POST", "/job", {
      form: { ...validForm, bot_name: "" },
      consent_install_cli: true,
      replace_existing: true,
    });
    expect(res.status).toBe(400);
    expect((res.body as { error: string }).error).toContain("Bot name");

    res = await call("POST", "/job", {
      form: { ...validForm, short_description: "a".repeat(81) },
      consent_install_cli: true,
      replace_existing: true,
    });
    expect(res.status).toBe(400);
    expect((res.body as { error: string }).error).toContain("Short description");

    res = await call("POST", "/job", {
      form: { ...validForm, developer_url: "" },
      consent_install_cli: true,
      replace_existing: true,
    });
    expect(res.status).toBe(400);
    expect((res.body as { error: string }).error).toContain("Developer URL");
  });

  it("simulates happy path: prereqs -> install_cli -> pauses at sign_in -> redirect -> provision -> import -> succeeded", async () => {
    const startRes = await call("POST", "/job", {
      form: validForm,
      consent_install_cli: true,
      replace_existing: false,
    });
    expect(startRes.status).toBe(200);
    let job = startRes.body as TeamsSetupJob;

    // After start, it moves through prereqs and install_cli to sign_in
    expect(job.phase).toBe("sign_in");
    expect(job.state).toBe("waiting_for_user");
    expect(job.sign_in).not.toBeNull();
    expect(job.sign_in?.login_url).toBe("http://localhost:4321/auth");

    // Invalid redirect URL rejected with 400
    const badRedirect = await call("POST", "/job/redirect", {
      url: "https://login.microsoft.com/bad",
    });
    expect(badRedirect.status).toBe(400);
    expect((badRedirect.body as { error: string }).error).toContain("http://localhost:4321/");

    // Valid redirect URL accepted, job enters scaffold
    const goodRedirect = await call("POST", "/job/redirect", {
      url: "http://localhost:4321/auth?code=mock-code-123",
    });
    expect(goodRedirect.status).toBe(200);
    job = goodRedirect.body as TeamsSetupJob;
    expect(job.phase).toBe("scaffold");
    expect(job.state).toBe("running");

    // Next poll advances through scaffold to provision
    const poll1 = await getJob();
    expect(poll1.status).toBe(200);
    job = poll1.job;
    expect(job.phase).toBe("provision");
    expect(job.state).toBe("running");

    // Next poll advances through provision to import
    const poll2 = await getJob();
    expect(poll2.status).toBe(200);
    job = poll2.job;
    expect(job.phase).toBe("import");

    // Next poll advances through import to succeeded
    const poll3 = await getJob();
    expect(poll3.status).toBe(200);
    job = poll3.job;
    expect(job.state).toBe("succeeded");
    expect(job.result).not.toBeNull();
    expect(job.result?.bot_id).toBe("28374619-abcd-4ef0-9123-abcdef012345");
    expect(job.created.bot_id).toBe("28374619-abcd-4ef0-9123-abcdef012345");
    expect(job.created.teams_app_id).toBe("teams-app-777888999");

    // App installation
    const installRes = await call("POST", "/job/install-app");
    expect(installRes.status).toBe(200);
    expect((installRes.body as TeamsSetupJob).app_installed).toBe(true);

    // Package download
    const pkgRes = await fetch(`${harness.baseUrl}/api/agents/${agent}/teams-setup/job/package`);
    expect(pkgRes.status).toBe(200);
    expect(pkgRes.headers.get("Content-Type")).toBe("application/zip");

    // Delete completed job
    const delStatus = (
      await fetchText(`${harness.baseUrl}/api/agents/${agent}/teams-setup/job`, {
        method: "DELETE",
      })
    ).status;
    expect(delStatus).toBe(204);

    // After delete, job is 404
    const afterDel = await call("GET", "/job");
    expect(afterDel.status).toBe(404);
  });

  it("can be driven to failure during provision, preserving created bot_id without teams_app_id", async () => {
    // Bot name containing "fail" causes provision to fail
    const startRes = await call("POST", "/job", {
      form: { ...validForm, bot_name: "fail_provision_bot" },
      consent_install_cli: true,
      replace_existing: false,
    });
    expect(startRes.status).toBe(200);

    // Complete sign-in
    await call("POST", "/job/redirect", { url: "http://localhost:4321/auth?code=mock" });

    // Poll scaffold -> provision
    await getJob();

    // Poll provision -> fails!
    const failedPoll = await getJob();
    const job = failedPoll.job;
    expect(job.state).toBe("failed");
    expect(job.error?.phase).toBe("provision");
    expect(job.created.bot_id).not.toBeNull();
    expect(job.created.teams_app_id).toBeNull(); // Only bot_id created before failure!

    // Retrying restarts from failed phase
    const retryRes = await call("POST", "/job/retry");
    expect(retryRes.status).toBe(200);
    const retriedJob = retryRes.body as TeamsSetupJob;
    expect(retriedJob.state).toBe("running");
  });

  it("can be cancelled while running and kills execution", async () => {
    await call("POST", "/job", {
      form: validForm,
      consent_install_cli: true,
      replace_existing: false,
    });

    const cancelRes = await call("POST", "/job/cancel");
    expect(cancelRes.status).toBe(200);
    const job = cancelRes.body as TeamsSetupJob;
    expect(job.state).toBe("cancelled");

    // Can delete cancelled job
    const delStatus = (
      await fetchText(`${harness.baseUrl}/api/agents/${agent}/teams-setup/job`, {
        method: "DELETE",
      })
    ).status;
    expect(delStatus).toBe(204);
  });

  it("handles cleanup requests", async () => {
    const res = await call("POST", "/cleanup", {
      project_files: true,
      cli: true,
      sign_out: false,
    });
    expect(res.status).toBe(200);
    expect(res.body).toEqual({
      removed: ["project_files", "cli"],
      failed: [],
    });
  });

  it("returns 409 with the running job on second start attempt", async () => {
    const start1 = await call("POST", "/job", {
      form: validForm,
      consent_install_cli: true,
      replace_existing: false,
    });
    expect(start1.status).toBe(200);

    const start2 = await call("POST", "/job", {
      form: validForm,
      consent_install_cli: true,
      replace_existing: false,
    });
    expect(start2.status).toBe(409);
    const conflictJob = start2.body as TeamsSetupJob;
    expect(conflictJob.agent).toBe(agent);
    expect(conflictJob.state).toBe("waiting_for_user");
  });

  it("refuses DELETE with 409 while job is running or waiting for user", async () => {
    await call("POST", "/job", {
      form: validForm,
      consent_install_cli: true,
      replace_existing: false,
    });

    const delStatus = (
      await fetchText(`${harness.baseUrl}/api/agents/${agent}/teams-setup/job`, {
        method: "DELETE",
      })
    ).status;
    expect(delStatus).toBe(409);
  });

  it("validates redirect port and host per contract", async () => {
    await call("POST", "/job", {
      form: validForm,
      consent_install_cli: true,
      replace_existing: false,
    });

    // Non-localhost URL
    const external = await call("POST", "/job/redirect", {
      url: "https://example.com:4321/auth?code=123",
    });
    expect(external.status).toBe(400);
    expect((external.body as { error: string }).error).toContain("localhost");

    // Wrong port
    const wrongPort = await call("POST", "/job/redirect", {
      url: "http://localhost:9999/auth?code=123",
    });
    expect(wrongPort.status).toBe(400);
    expect((wrongPort.body as { error: string }).error).toContain("wrong port");
  });

  it("filters log lines incrementally when log_since is passed", async () => {
    await call("POST", "/job", {
      form: validForm,
      consent_install_cli: true,
      replace_existing: false,
    });

    const allLogs = await getJob();
    expect(allLogs.status).toBe(200);
    expect(allLogs.job.log.length).toBeGreaterThan(0);
    const lastSeq = allLogs.job.last_seq;

    const noNewLogs = await getJob(lastSeq);
    expect(noNewLogs.status).toBe(200);
    expect(noNewLogs.job.log).toHaveLength(0);

    const partialLogs = await getJob(1);
    expect(partialLogs.status).toBe(200);
    expect(partialLogs.job.log.every((l) => l.seq > 1)).toBe(true);
  });
});
