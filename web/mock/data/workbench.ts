/** The sample artifact's name, which is its file's name in the team workbench folder. */
export const MOCK_WORKBENCH_ARTIFACT_NAME = "tip-splitter";

/** The agent the sample artifact's model calls and sessions run on. */
const ARTIFACT_AGENT = "atlas";

/** The sample artifact: a tip splitter that exercises `residuum.ask` and `residuum.sessions.start`. */
export const MOCK_WORKBENCH_ARTIFACT = `<!doctype html>
<html><head><title>Tip Splitter</title>
<style>
  body { margin: 0; padding: 32px; background: #14181f; color: #e6e8ec; font: 16px system-ui; }
  label { display: block; margin: 12px 0 4px; color: #9aa3b2; }
  input { font: inherit; padding: 6px 8px; width: 160px; }
  output { display: block; margin-top: 20px; font-size: 28px; }
  button { margin-top: 20px; font: inherit; }
</style></head>
<body>
  <h1>Tip splitter</h1>
  <label for="bill">Bill</label><input id="bill" type="number" value="84">
  <label for="people">People</label><input id="people" type="number" value="3">
  <output id="each"></output>
  <button id="ask">Ask Residuum about this split</button>
  <button id="burst">Fire 3 calls at once</button>
  <button id="spawn">Start a background session</button>
  <script>
    // Model calls and sessions run on one agent's models, so each names it.
    const agent = ${JSON.stringify(ARTIFACT_AGENT)};
    const each = document.getElementById("each");
    const update = () => {
      const bill = Number(document.getElementById("bill").value);
      const people = Math.max(1, Number(document.getElementById("people").value));
      each.textContent = (bill * 1.2 / people).toFixed(2) + " each, with 20% tip";
    };
    document.querySelectorAll("input").forEach((i) => i.addEventListener("input", update));
    update();
    document.getElementById("ask").addEventListener("click", () =>
      residuum
        .ask("Is " + each.textContent + " right? Answer in one short sentence.", { agent })
        .then((r) => alert(r.content))
        .catch((e) => alert(e.message)),
    );
    // For exercising the activity panel's "Cancel calls": three calls in
    // flight at once, long enough to see and cancel before they resolve.
    document.getElementById("burst").addEventListener("click", () => {
      for (let i = 0; i < 3; i++) {
        residuum
          .ask("Sanity check #" + (i + 1) + " on " + each.textContent, { agent })
          .catch(() => {});
      }
    });
    // For exercising the activity panel's session list and stop buttons.
    document.getElementById("spawn").addEventListener("click", () =>
      residuum.sessions
        .start({
          agent,
          prompt: "Double check this tip split against last month's dinner out.",
        })
        .catch((e) => alert(e.message)),
    );
  </script>
</body></html>`;
