# sccache and workspace crates: measurements

Recorded with sccache 0.18.0. These measurements are the basis for the caching choices in [docs/runbooks/ci.md](../runbooks/ci.md#build-caching-on-the-runners).

## Findings

What sccache does with workspace crates, measured with sccache 0.18.0 on a two-crate workspace built in two directories:

- **It does not cache an incrementally compiled crate.** Under the default dev profile Cargo compiles workspace crates with `-C incremental`, and sccache reports them `CannotCache(incremental)`, so locally sccache serves only registry dependencies. It never caches test harnesses or binaries (`CannotCache(crate-type)`).
- **With `CARGO_INCREMENTAL=0`, it caches a workspace crate's library, but only at the same absolute path.** The same workspace built in a second directory missed on both workspace crates and on the registry crates with build-script output (`serde`, `serde_core`), and hit on the registry crates without one (`proc_macro2`, `quote`, `syn`, `unicode_ident`).
- **`SCCACHE_BASEDIRS` does not change that for Rust.** With both checkouts listed in it, set in the server's environment when it started and shown by `sccache --show-stats` under "Base directories", the hits and misses were the same. sccache hashes the values of the `CARGO_*` environment variables rustc inherits: with the same sources at the same path, changing only `CARGO_TARGET_DIR` misses every crate, registry ones included. `CARGO_MANIFEST_DIR` and `OUT_DIR` are absolute paths in that set, which a base directory does not rewrite; that they cause the misses is inferred, not proved.

## Repeating the check

To check again, for example after an sccache upgrade (it uses a private server and leaves the shared one alone):

```bash
(
R="$(mktemp -d)"
# Workspace p1: root package topc depending on crates/lowc by path, lowc depending on serde with derive.
cargo new --quiet --lib "$R/p1" --name topc
cargo new --quiet --lib "$R/p1/crates/lowc"
printf '\n[workspace]\nmembers = ["crates/lowc"]\n' >>"$R/p1/Cargo.toml"
(cd "$R/p1" && cargo add --quiet lowc --path crates/lowc && cd crates/lowc && cargo add --quiet serde --features derive)
(cd "$R/p1" && cargo generate-lockfile --quiet)
cp -a "$R/p1" "$R/p2"

export SCCACHE_SERVER_PORT=4299 SCCACHE_DIR="$R/cache" SCCACHE_ERROR_LOG="$R/server.log" SCCACHE_LOG=debug
export SCCACHE_BASEDIRS="$R/p1:$R/p2" CARGO_INCREMENTAL=0 RUSTC_WRAPPER=sccache
unset CARGO_TARGET_DIR
sccache --start-server
sccache --show-stats | grep -A2 'Base directories'    # must list p1 and p2
(cd "$R/p1" && cargo build --quiet)
sccache --zero-stats
(cd "$R/p2" && cargo build --quiet)
sccache --show-stats                                   # hits and misses for p2
grep 'compile result' "$R/server.log" | grep -E '\[(lowc|topc|serde|serde_core)\]'
sccache --stop-server
)
```

The subshell keeps the variables out of your shell.

`SCCACHE_BASEDIRS` takes absolute paths separated by `:` on Unix, and a running server reads it only when it starts. The check passes when the p2 build shows cache hits for `lowc` and `topc`; with sccache 0.18.0 both miss. The same reading works on a real build: `[residuum_memory]: compile result: cache hit` in the server log. While the check fails, the choices in the runbook stand.
