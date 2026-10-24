# Contributing to the Residuum Web UI

This guide covers the Residuum web app, the Svelte 5 single-page app the Rust binary embeds: how to run it without the backend, the checks every change passes, and how it is tested. [ARCHITECTURE.md](./ARCHITECTURE.md) explains how the app is put together, and [AESTHETIC.md](./AESTHETIC.md) covers the visual system.

## Prerequisites

- **Node.js** in the `engines` range of `package.json` (`^22.13.0 || ^24.0.0 || >=26.0.0`). Check with `node --version`.
- **npm**, which comes with Node.js.
- **Docker**, for the visual comparisons and the WebKit project, and as the fallback browser when Chromium can't be installed locally (see [Running the end-to-end suite](#running-the-end-to-end-suite)).

## Getting Started

```bash
cd web
npm install
npm run dev:mock
```

Open <http://localhost:5173>. No backend is needed.

With [`just`](https://github.com/casey/just) from the repo root:

| Recipe                         | Does                                                                                                              |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------- |
| `just web-mock`                | `dev:mock`, installing dependencies first if needed. Extra arguments go to Vite (`just web-mock --port 5199`)     |
| `just web-mock-setup`          | The same, starting in the setup wizard                                                                            |
| `just web-mock-serve [port]`   | The deterministic mock on the dev server, headless                                                                |
| `just web-mock-preview [port]` | Builds, then serves the production build with the deterministic mock                                              |
| `just web-dev`                 | The dev server against a real backend (see [Running Against the Real Backend](#running-against-the-real-backend)) |

<http://localhost:5173/dev/gallery> shows every primitive control in every state. The dev server and mock builds serve it; production builds leave it out.

## Mock Mode

`npm run dev:mock` starts the Vite dev server with a mock backend (`mock/`) that serves the whole API and every WebSocket: the hub, four agents and the artifacts listener on a second port. It is the same contract the backend has, including scoping (`/api/agents/{name}/…`, `/api/hub/…`, `/api/team/…`), so a route that 404s or 409s on the backend does the same here.

The scenario has four agents: `scout` and `atlas` running, `drifter` stopped, and `brittle` failed (its main model doesn't exist, so it fails every start until Settings or `POST /api/mock/fix-agent?agent=brittle` fixes it). They come with sample chat, sessions, inbox, mail, checkpoints, files and a workbench artifact. Config files load from `../assets/*.example.*`.

Chat replies are scripted by how a message starts (`mock/chat.ts`, `mock/chat-scenarios.ts`):

| Message starts with | The agent                                                                                       |
| ------------------- | ----------------------------------------------------------------------------------------------- |
| anything else       | Thinks, searches memory, reads two files, replies                                               |
| `fail`              | Fails to read the second file                                                                   |
| `error`             | Fails the turn with an `error` frame                                                            |
| `retry`             | Streams part of a reply, restarts the stream, streams it again                                  |
| `think`             | Reasons at length before a short answer                                                         |
| `segments`          | Works in several rounds of tools and text                                                       |
| `remember`          | Follows the reply with two seconds of memory work                                               |
| `spawn`             | Starts a spawned session that reports back to the chat                                          |
| `drop`              | Loses the connection mid-turn (`drop finish …`, `drop compress …` vary what happens while away) |

A message sent to a session containing "busy" fails to deliver.

### Test controls

Routes under `/api/mock/` drive the mock from a spec or by hand: simulate a teammate message, a Telegram message, a session messaging the main chat, a team or agent file changing, the hub socket going down, a pulse firing, a Cloud sign-in, and more. They live in `mock/controls.ts` (plus `mock/scheduled.ts`, `mock/cloud.ts`, `mock/team-events.ts` and a few others), each with a comment saying what it does. The ones specs use most:

| Control                                              | Does                                                                                                                                                                                      |
| ---------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `POST /api/mock/reset`                               | Restores the scenario: clock, delays, ids, timers, every agent and the hub's state. Closes every socket. `{ "setup": true }` resets to a hub with no agents, which opens the setup wizard |
| `POST /api/mock/time`, `/time/advance`, `/time/step` | Manual time (below)                                                                                                                                                                       |
| `POST /api/mock/delays`                              | Sets the delay scale until the next reset                                                                                                                                                 |
| `POST /api/mock/turn-hold`                           | Holds chat turns at their last step (`true`), mid-reply (`"reply"`) or earlier (`"steps"`)                                                                                                |
| `POST /api/mock/hub-socket`                          | `{ "online": false }` takes the hub socket down                                                                                                                                           |
| `POST /api/mock/agent-file`, `/team-file`            | Changes a file and sends the live-update frames the backend would                                                                                                                         |
| `GET /api/mock/watches?agent=`                       | The paths that agent's sockets watch right now                                                                                                                                            |

### Setup wizard mode

`VITE_MOCK_SETUP=1 npm run dev:mock` starts with no agents, so the app opens the first-run wizard. A running mock gets there with `POST /api/mock/reset` and `{ "setup": true }`, which is how specs reach it.

### Deterministic mode

`MOCK_DETERMINISTIC=1 npm run dev:mock -- --port 5173 --strictPort` (or `just web-mock-serve 5173`) makes the same steps give the same responses. The end-to-end and visual tests run this way.

- **One fixed clock** at 2026-03-14 12:00 UTC, read in UTC, until a test moves it with `POST /api/mock/clock/advance` (`{ "ms": 3600000 }`). Ids that would come from time or chance come from a counter that reset restarts.
- **No delays.** Every simulated step takes no time and runs in order. `MOCK_DELAY_SCALE` (or `POST /api/mock/delays`) brings them back: `1` is the natural pace.
- **Manual time.** `POST /api/mock/time` with `{ "mode": "manual" }` stops simulated time. `/time/advance` (`{ "ms": 1590 }`) runs every timer due in that span, in order, and `/time/step` runs the next one. `{ "mode": "scaled" }` or a reset returns to wall-clock time. Specs use the fixture wrappers (see [Waiting](#waiting)).
- **A fixed artifacts port**, 5180 (or `MOCK_ARTIFACTS_PORT`).

### Preview mode

`just web-mock-preview 4173` builds, then runs `MOCK_DETERMINISTIC=1 npm run preview:mock -- --port 4173 --strictPort`. It serves the production build in `dist/` with the whole mock. Use it for anything the dev server can't show: the service worker, installability, the bundle as shipped.

### What the mock doesn't do

- No model calls; responses are canned.
- State lives in memory and is lost when the server stops.
- Rate limits and network errors aren't simulated.
- The workspace routes don't block the paths the backend blocks. Moves and raw writes take no checkpoint. Only the `agent-file` and `team-file` controls send `workspace_changed`; writes through the workspace routes don't announce themselves.
- `POST /api/secrets` accepts any value, including the `secret:` and `${ENV_VAR}` references the backend rejects.

### Changing the mock

- Everything under `mock/` is formatted, linted and type-checked like `src/`, and its tests (`mock/**/*.test.ts`) run in `npm test`.
- Handlers live in route tables (`Route` in `mock/routes.ts`, collected in `mock/api-routes.ts`). Type response bodies with the generated protocol types in `src/lib/generated/` wherever one exists.
- **Add the mock route with the client function.** `mock/route-parity.test.ts` calls every function `src/lib/api.ts` exports and fails when one has no sample call or hits a route the mock doesn't serve. When you add an API client function, add its sample call there and its route to the mock in the same change.
- `mock/test-support.ts` has two harnesses: route tables over HTTP against a stub hub, and `startMockServer`, the whole mock with its sockets. Pass `{ deterministic: true }` when a test asserts on times or ids.

## Code Quality

Before submitting changes, run:

```bash
npm run lint          # ESLint, then Stylelint
npm run format        # Prettier, rewriting files
npm run format:check  # Prettier check, as CI runs it
npm run check         # svelte-check and the service worker's tsc; warnings fail it
npm test              # Vitest: unit and component tests
npm run test:coverage # The same, with a coverage report in coverage/
npm run e2e:fast      # Playwright, without the visual comparisons
```

`just web-check` runs format check, lint, type check, tests and `just web-size` (the initial route's size, raw and gzipped, which CI also puts in the Web Checks job summary; it has no threshold).

- **ESLint.** Every `.ts` module under `src/`, `e2e/`, `mock/`, `build/` and `scripts/`, rune stores (`*.svelte.ts`) included, gets the strict type-aware rules; `.svelte` files get a set that fits runes. When a rule is wrong for one line, use `// eslint-disable-next-line <rule> -- <reason>`, never a blanket disable.
- **Stylelint.** Outside `src/styles/tokens.css`, stylesheets and `<style>` blocks may not use literal colors, raw `font-size`/`font`, raw `z-index`, literal durations or easings, or `transition: all`; use a `var(--…)` token. Viewport media queries may use only the shell breakpoints; a component with its own responsive rule uses a container query. When no token fits, add one to `tokens.css`.
- **svelte-check** fails on every warning, accessibility ones included. Fix the markup rather than suppressing the warning.
- **Generated types.** `src/lib/generated/` comes from the Rust types. After changing an exported Rust type, run `just types` and commit the result; `just types-check` and CI fail when it is stale.
- **Stores don't navigate.** ESLint rejects importing the router from `src/lib/` (see [ARCHITECTURE.md](./ARCHITECTURE.md#project-structure)).

## Testing

Tests sit in five layers. Use the lowest one that can show the behavior: lower layers are faster and break for fewer unrelated reasons.

| Layer         | Runs in                                            | Use it for                                                                   | Lives in                                                        |
| ------------- | -------------------------------------------------- | ---------------------------------------------------------------------------- | --------------------------------------------------------------- |
| Unit          | Vitest in Node                                     | Stores, routing, formatters, parsers, the mock's logic, the CI report script | `src/**/*.test.ts`, `mock/**/*.test.ts`, `scripts/**/*.test.ts` |
| Component     | Vitest in jsdom with Testing Library               | One component's empty, loading, error, populated and live states             | `src/components/**/*.test.ts`, `src/**/*.component.test.ts`     |
| End-to-end    | Playwright in Chromium, against the mock           | User flows: navigation, sockets, focus, touch, real layout                   | `e2e/**/*.spec.ts`                                              |
| Accessibility | axe-core inside an end-to-end spec                 | Every place and overlay a change touches                                     | `expectNoAxeViolations` in `e2e/support/axe.ts`                 |
| Visual        | Playwright screenshots in the Playwright container | One shot per distinct surface or component state, as a design reference      | `e2e/visual/`                                                   |

`npm test` runs the unit and component layers. The pre-push hook runs the tests that import a changed file (`vitest related`), or all of them when config or test setup changes. The Playwright layers aren't in the hooks: run `just web-e2e-fast` before pushing a UI change.

### Unit and component tests

- Mount with `render` and mock `fetch` with `src/test/component.ts`. Drive input with `@testing-library/user-event`, which presses keys the way a browser does.
- `src/test/snippets.ts` turns markup into a snippet for `children`; `src/test/ui/` holds harnesses for components that need a parent.
- **Wait on the condition, never on time.** Use `findBy*` for an element that appears, and `waitFor` from `src/test/wait.ts` for anything else. Under fake timers use `waitFor`, which steps the faked clock (`findBy*` polls on `setInterval`, which fake timers freeze).
- A test that needs time to pass uses `vi.useFakeTimers()` and `vi.advanceTimersByTimeAsync`.
- ESLint rejects `vi.waitFor` (no hang guard), `shouldAdvanceTime` (lets real time drive fake timers), and `setTimeout` with a non-zero delay. A zero-delay `setTimeout` is allowed for flushing a macrotask.
- The timeouts in `src/test/guard.ts` are hang guards, set far above what a loaded machine needs. Never raise them for a slow machine; a wait that ends on its condition never reaches them.

### Running the end-to-end suite

| Recipe                | Runs                                                                                 | Needs                                       |
| --------------------- | ------------------------------------------------------------------------------------ | ------------------------------------------- |
| `just web-e2e-fast`   | Everything but the visual comparisons                                                | Chromium (installed on first use) or Docker |
| `just web-e2e`        | Everything, with the visual comparisons in the container                             | Docker                                      |
| `just web-e2e-update` | Regenerates visual baselines (`-g "<title>"` for some)                               | Docker                                      |
| `just web-e2e-webkit` | The specs in a WebKit iPhone, in the container (local only)                          | Docker                                      |
| `just web-stress`     | The specs this branch adds or changes, repeated under CPU throttling (see [CI](#ci)) | Chromium or Docker                          |

Extra arguments go to Playwright: `just web-e2e-fast e2e/smoke/chat.spec.ts`, `--grep`, `--headed`, `--debug`. The recipes name their projects, so `--project` adds rather than narrows; to run one project, call Playwright directly: `cd web && ../scripts/with-loopback-network.sh npx playwright test --project=phone e2e/smoke`. `npm run e2e:report` opens the last HTML report.

How the recipes find a browser (`scripts/with-e2e-browser.sh`):

- They install Chromium with a time limit (`E2E_BROWSER_INSTALL_TIMEOUT`, default 240 seconds). If the install fails, they fall back to the Playwright container and say so. `E2E_ALL_IN_CONTAINER=1` skips straight to the container.
- A fresh Linux machine also needs Chromium's system libraries: `cd web && npx playwright install --with-deps chromium`.
- Local Chromium runs inside a loopback-only network namespace (`scripts/with-loopback-network.sh`). Chromium on Linux aborts in-flight loads with `net::ERR_NETWORK_CHANGED` whenever a host interface comes or goes, which Docker does on every container start, so specs would fail at random on a busy machine. Where the namespace can't be made (macOS, or restricted user namespaces), or with `E2E_HOST_NETWORK=1`, the run uses the host network and prints why. A bare `npx playwright test` uses the host network.
- The first Docker run pulls the Playwright image, about a gigabyte.

### Projects and tags

| Project                            | Browser                                                  | Runs                                                                                                           |
| ---------------------------------- | -------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `desktop`, `phone`                 | Chromium at 1440×900, or 390×844 with touch              | Untagged specs, against the dev server locally and the production build on CI (or with `E2E_SMOKE_ON_BUILD=1`) |
| `dev-desktop`, `dev-phone`         | The same                                                 | `@dev` specs, always against the dev server                                                                    |
| `preview-desktop`, `preview-phone` | The same                                                 | `@preview` specs, against the production build                                                                 |
| `visual-desktop`, `visual-phone`   | The same, in the container, clock frozen, reduced motion | `@visual` specs                                                                                                |
| `webkit-phone`                     | WebKit as an iPhone 13                                   | Untagged and `@dev` specs, local only                                                                          |

A spec runs in every project matching its tag, so one spec covers both sizes; branch on `isMobile` only when behavior differs. Put tags on a test or describe block: `test("installs", { tag: "@preview" }, async ({ page }) => { … })`. Use `@preview` for what only a build has (service worker, installability), `@dev` for what only the dev server has (the gallery), and `@visual` for screenshots. Give a spec at most one of them. In `webkit-phone`, branch on `browserName === "webkit"` where iPhone behavior differs, and `test.skip` with a reason where WebKit can't run the spec.

CI runs untagged specs against the build because the dev server serves each module as a separate request, which on a shared runner makes page loads take seconds.

### Servers and state

- Playwright starts a deterministic mock on the dev server and one on the production build for every worker. Each binds a free port on 127.0.0.1 itself, so parallel suites (two worktrees, say) never collide and a mock you started by hand can keep running. `E2E_DEV_PORT` and `E2E_PREVIEW_PORT` pin the first worker's ports.
- Tests reach the servers at 127.0.0.1, never `localhost`, which can resolve to `::1` where another process holds the port.
- Every test starts with `POST /api/mock/reset` on its worker's mock, and tests run fully parallel, so never rely on another test's state.
- `E2E_WORKERS` sets the worker count (default: one per two CPUs, at most six). Use it instead of `--workers`, because servers start per worker.
- Requests to anything but this machine are aborted, so the internet can't change a run.
- Global setup (`e2e/support/warmup.ts`) compiles every dev-server module before the first test, so no spec pays for the first compile.

### Writing a spec

Import `test` and `expect` from `e2e/support/fixtures`, not `@playwright/test`. The fixtures reset the mock before each test, abort non-loopback requests, fail on any violation of the app shell's Content-Security-Policy, and add the `mock` fixture:

```ts
import { expect, test } from "../support/fixtures";

test("a teammate's message shows in the chat", async ({ page, mock }) => {
  await mock.post("/api/mock/teammate-message", { params: { agent: "atlas" } });
  await page.goto("/agent/atlas");
  await expect(page.getByText("scout asked me to check the wiki index")).toBeVisible();
});
```

`mock.post(path, { params, data })` calls a [test control](#test-controls) and fails on any non-2xx answer. Locate elements by role and accessible name, and assert with web-first assertions (`expect(locator).toBeVisible()`).

A test that loads a page for each of many cases (a table of redirects) loops over the table around `test(...)`, one test per case, so a failure names its case.

#### Waiting

The suite must pass when other suites share the machine and every page load takes seconds longer. A spec that passes only because nothing was slow will fail there. So every step waits for the condition it depends on:

- **Navigate with `page.goto` and `page.reload`.** The fixtures make them return only once the app is usable (`waitForApp` in `e2e/support/app.ts`): shell drawn, hub socket connected, overview loaded, fonts loaded, and the code behind a `?settings=` or `?panel=file:` URL mounted. You never need to wait for the shell before the first action. A test that cuts the hub off changes what "ready" means: later loads expect the "Can't reach Residuum" banner until the test restores it.
- **Open lazy parts through `e2e/support/lazy.ts`**: `expectSettingsOpen`, `expectPaletteOpen`, `expectFileOpen`, `expectFileRendered` and `expectSetupOpen`. They wait with the longer bound code loading needs and return the part's locator.
- **No fixed sleeps.** ESLint rejects `waitForTimeout` and any `setTimeout` in `e2e/`. Wait on an element, an attribute, a URL (`expect.poll`), or a response (`page.waitForResponse`, registered before the click that causes it).
- **Act only on a ready trigger.** A control that opens a menu or dialog is ready once it is visible, enabled and closed (`aria-expanded="false"`), with anything that was closing over it gone.
- **Socket-delivered changes need the watch in place.** A change made through `agent-file` or `team-file` reaches the page only if its watch already reached the mock. Poll `GET /api/mock/watches?agent=…` until the path is watched and then change the file once (as `agent-file-panel.spec.ts` does), or make the same change inside `expect.poll` until the page shows it (as `schedule.spec.ts` does).
- **Never raise the test timeout or mark a test `test.slow()`.** The three-minute timeout in `playwright.config.ts` is a hang guard, not a speed budget.

**Manual time.** A spec about a moment that lasts only a while (a reply mid-stream, an agent stopping, a turn retrying) holds the mock at that moment with manual time. Stretching it with the delay scale doesn't work, because a stretched window still closes on a slow enough browser.

1. Call `mock.manualTime()` before starting what you watch.
2. Wait on the page for evidence the mock started it, such as the turn head's "Working" or an agent's "Stopping" card.
3. `await mock.advance(ms)` to a time inside the moment (the schedule is in the mock: in `mock/chat.ts` a turn's last call starts writing at 1.5s, then a piece every `PIECE_MS`), and assert.

Time stands still between moves, for the page as well as the mock. The fixtures install Playwright's clock on every page, so in manual time the page's own timers (toast dismissal, reconnect backoff in `src/lib/reconnect.ts`) also run only when a move passes them, and durations the page measures are exact. A move runs only timers already set, so wait for the sign that the page set one (the composer's "Reconnecting" line) before the move that should fire it. Wait for what one move shows before making the next. `mock.stepUntil((timeout) => expect(x).toBeVisible({ timeout }))` runs one timer at a time until the expectation holds, for an outcome that stays once it shows. Never call `page.clock` directly (ESLint rejects it): the fixtures keep the page and mock clocks together. Animation frames stay on the browser's own time, so drawing and scrolling continue while time stands still. `chat-turns.spec.ts` and `state-cards.spec.ts` show the pattern.

`POST /api/mock/turn-hold` is the simpler tool when a spec only needs a turn to keep running while it looks.

#### Accessibility

`expectNoAxeViolations(page)` scans the page as it stands and fails on serious and critical violations, naming the rule and elements. Put the page in the state under test first (open the menu, then scan), and scan each place and overlay a change touches. `{ within: "[role=dialog]" }` limits the scan to part of the page. Minor and moderate findings don't fail; they're in the attached result.

### Visual

The visual suite is a small design reference: one shot per distinct surface or component state that defines the look (Home, Settings, the shell's navigation, chat turns, the composer, prose, the Inbox, toasts, gallery controls, and so on), with a phone variant only where the layout differs. Add a shot only for a new surface or state that defines the look; behavior belongs in the smoke specs.

- Take a shot with `expectScreenshot(locator, "name", options)` from `e2e/support/screenshot.ts`. It captures one element once the clock and motion are fixed, fonts and images have loaded, and the caret is hidden. `options.mask` paints over anything nondeterministic. Use `expectClipScreenshot([locators], "name")` only when no single element holds the component.
- Wait for the element to reach the state under test first, and tag the test `@visual`.
- Outside the container a `@visual` spec skips itself, so `just web-e2e-fast` never compares screenshots.

**Baselines.** Text rendering differs between machines, so baselines are made and compared only inside the official Playwright container (`mcr.microsoft.com/playwright`, the `-noble` image matching the pinned `@playwright/test`, as `linux/amd64` on any host), via `scripts/with-playwright-container.sh`. The mock stays on your machine.

- `just web-e2e-update` rewrites the baselines; `-g "<part of a test title>"` limits it. Look at every changed image under `e2e/__screenshots__/` before committing: a baseline records whatever the page showed, bugs included.
- Bumping `@playwright/test` changes the image, so rerun `just web-e2e-update` and review every image. Also regenerate the visual-report fixtures with `node scripts/fixtures/visual-report/capture/capture.mjs` from `web/`, run `npx vitest run scripts/visual-report.test.ts`, and commit the changed fixtures.

**On a pull request,** the visual comparisons run in their own job (`Web Visual Comparisons`) that never fails the PR, and the `Visual Report` job posts one comment, updated in place on each run, listing changed, new and errored shots with the re-shoot command. If a change is intended, re-shoot and commit the baselines; if not, fix the page. `web/scripts/visual-report.mjs` builds the comment and is tested by `scripts/visual-report.test.ts` in `npm test`. The merge queue and release workflow don't run the comparisons.

### When a spec fails

- Failed tests leave a trace and screenshot under `web/test-results/`, and the HTML report is in `web/playwright-report/`. Open a trace with `npx playwright show-trace test-results/<test>/trace.zip` to step through actions, DOM, console and network.
- A failed visual comparison leaves the actual, expected and diff images beside it.
- When `page.goto`'s readiness wait times out, its message lists the page's failed requests, so an aborted load reads as one rather than as slowness.
- In CI, each failing end-to-end shard uploads `playwright-report-shard-<n>`, and the visual job uploads `playwright-visual-report` when any shot doesn't match.
- A spec that fails only under load is waiting on time instead of the event it checks. Fix the wait (see [Waiting](#waiting)); never rerun past it.

### CI

| Where                                                                        | Web checks that run                                                                                                            |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| Pull request                                                                 | Only the visual comparisons, which never fail it                                                                               |
| Merge queue (batches that change `web/`, Markdown excepted) and release tags | `Web Checks` (format, lint, types, unit tests, build), `Web Tests (Node 22)` and `(Node 24)`, and four `Web End-to-End` shards |
| Nightly on `main`                                                            | Web Stress: every spec repeated under CPU throttling                                                                           |

All of these are in `.github/workflows/web-checks.yml`, called from `ci.yml` and `release.yml`; it can also be run by hand from the Actions tab. The end-to-end shards run every non-visual project in the Playwright container (`E2E_ALL_IN_CONTAINER=1 scripts/with-playwright-container.sh npm --prefix web run e2e:fast -- --shard=<n>/4`), because the runners have no Chromium. The same command works locally. Node 26+ is allowed by `engines` but not run in CI; reproduce a Node-specific failure with your version manager.

**Stress.** The nightly Web Stress jobs (`.github/workflows/nightly-stress.yml`) repeat every spec with each page's CPU throttled four times, over four shards. Each failing test becomes an `investigate` issue titled `Nightly stress: <spec> › <test>`, or a comment on the open one. `scripts/web-stress.sh` holds the logic:

- `just web-stress` repeats the spec files your branch adds or changes relative to `origin/main` (five times each, within a budget) with CPU throttling. Run it on every spec you add or change before you push. Harness, mock and config changes select nothing, since the regular run covers them.
- `WEB_STRESS_ALL=1 just web-stress e2e/smoke/<spec>.spec.ts` repeats one spec regardless of the branch, which is how to reproduce a nightly failure.
- `WEB_STRESS_REPEAT`, `WEB_STRESS_CPU_THROTTLE`, `WEB_STRESS_WORKERS` and `WEB_STRESS_BUDGET` tune it; the nightly values are in `nightly-stress.yml`. Extra arguments go to Playwright (`just web-stress --grep "tab is titled"`).

## Running Against the Real Backend

With the Rust backend running on port 7700 (`just serve` from the repo root):

```bash
npm run dev   # or: just web-dev
```

Vite proxies `/api`, WebSockets included, to `localhost:7700`.
