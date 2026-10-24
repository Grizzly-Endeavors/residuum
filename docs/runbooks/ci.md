# CI maintenance

## The `main` ruleset

`.github/rulesets/main.json` is the ruleset as it should be: it refuses pushes, force pushes and deletion of `main`, requires a PR, and merges only through the merge queue (up to five PRs per batch) once `CI gate` passes. A change made in the repository settings is lost at the next apply, so change the file.

Apply a change after its PR merges:

```bash
id=$(gh api repos/Grizzly-Endeavors/residuum/rulesets --jq '.[] | select(.name == "main") | .id')
gh api --method PUT "repos/Grizzly-Endeavors/residuum/rulesets/$id" --input .github/rulesets/main.json
```

A repository without the ruleset gets it with `gh api --method POST repos/Grizzly-Endeavors/residuum/rulesets --input .github/rulesets/main.json`. The flow also needs auto-merge allowed and head branches deleted on merge:

```bash
gh api --method PATCH repos/Grizzly-Endeavors/residuum -F allow_auto_merge=true -F delete_branch_on_merge=true
```

## Build caching on the runners

- The lab runners' `github-runner` image sets `RUSTC_WRAPPER=sccache` with an S3-compatible shared store. That configuration lives with the runner pools in grizzly-platform.
- `rust-checks.yml`, the `rust-stress` job of `nightly-stress.yml` and the lab-runner jobs of `cross-compile.yml` set `CARGO_INCREMENTAL=0`, because sccache does not cache incrementally compiled crates and CI checkouts are fresh anyway.
- Checkouts sit at the fixed path `/home/runner/_work/residuum/residuum`. **No CI job may set `CARGO_TARGET_DIR`**: sccache hashes its value, so a per-job target dir misses every crate.
- Local builds keep incremental compilation; reuse there comes from the build-dir pool ([build-dirs.md](./build-dirs.md)), not sccache.

The measurements behind these choices, and how to repeat them after an sccache upgrade, are in [docs/archive/sccache-workspace-caching.md](../archive/sccache-workspace-caching.md).

## Reading CI metrics

The `rust-lint` and `rust-test` job summaries print `sccache --show-stats`; `rust-test` and the web end-to-end and stress shards also print a CPU table (`scripts/ci-cpu-report.sh`). Job summaries are not readable through `gh` or the API, so each table is also written to the job log between `===== BEGIN CI METRICS (<job>) =====` and `===== END CI METRICS (<job>) =====`:

```bash
gh run view <run-id> --log | sed -n '/BEGIN CI METRICS/,/END CI METRICS/p'
gh api repos/Grizzly-Endeavors/residuum/actions/jobs/<job-id>/logs   # one job
```

## `just ci-failures`

`just ci-failures [pr|branch|tag|commit]` lists every failure of the latest runs, grouped by job and step, including runs still in progress and, for a PR, its latest merge queue batch. Each failure is flagged:

- `touched` — the change edits a file the failure names, or a package the failing `tests/it` module covers.
- `seen elsewhere` — the same failure appeared on another branch or tag in the last 14 days.
- `likely flake` — not touched but seen elsewhere: check `main` before treating it as this change's.

Failed jobs' logs are cached under `target/ci-failures-cache/`. `just ci-failures-test` checks the log parser and batch selection against `scripts/ci-failures-fixtures/`.
