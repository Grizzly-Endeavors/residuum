# Build dirs

Cargo builds of this repository share a pool of warm target dirs, `target/pool/0`, `target/pool/1` and so on, under the main checkout (the one that holds `.git`). A build leases one dir for as long as it runs, with `CARGO_TARGET_DIR` set to it, so a fresh worktree starts warm instead of compiling every dependency. A tree that builds again gets the dir it used last when that dir is free. `scripts/target-pool path` prints the pool's location from any worktree, and `scripts/target-pool status` lists each dir with its size, its holder and the worktree that used it last.

## What leases

- The pre-push hook, once per pushed commit.
- Every `just` recipe that builds Rust (`scripts/target-pool run`).
- In Claude Code sessions with the `rust-build` plugin (`Grizzly-Endeavors/rust-toolkit`, from the `grizzly-plugins` marketplace), a plain `cargo build`/`check`/`clippy`/`test`/`run`/`doc`. The plugin also deletes the `target/` of an agent worktree under `.claude/worktrees/` once it has gone a day without changes.
- A `cargo` you run by hand without the plugin builds in the checkout's own `target/`, unless you run it as `scripts/target-pool run -- cargo …`.

Setting `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR` or `CARGO_BUILD_BUILD_DIR`, or passing `--target-dir`, turns leasing off for that build. A `build.target-dir` in a Cargo config file does not: the leased `CARGO_TARGET_DIR` overrides it. The pre-push hook always leases when it checks a commit other than the checkout's HEAD.

## Behavior

- **Hand-offs.** When a dir passes from one worktree to another, `target-pool` deletes the workspace members' fingerprints in it, because Cargo would otherwise take another worktree's build of a member as fresh. Members rebuild; dependencies stay warm.
- **All dirs busy.** The run prints one line saying it is waiting and blocks until a dir frees. It never builds cold or in a shared dir.
- **Size.** A dir that ends a run above the cap is emptied, with a line on stderr saying so.
- **Binaries.** A build leaves its binary in the pool dir it leased, where the next lease can replace it. `just build` and `just release` copy it out to `target/debug/residuum` and `target/release/residuum`, which `just serve` and `just run` start, so a running hub holds no pool dir.
- **Cleaning.** `just clean` empties `target/` except the pool, removes `web/dist`, and empties every pool dir no build holds. Plain `cargo clean` in the main checkout deletes the pool and its locks under any running build.

## Settings

| Variable | Default | Meaning |
| --- | --- | --- |
| `TARGET_POOL_SIZE` | `4` | Dirs in the pool. Builds beyond this many at once wait. |
| `TARGET_POOL_CAP_GB` | `40` | Size in GB above which a dir is emptied after a run. |
| `TARGET_POOL_DISABLE` | unset | `1` turns leasing off everywhere. |

## Updating the script

`scripts/target-pool` is a vendored copy of `plugin/bin/target-pool` from `Grizzly-Endeavors/rust-toolkit`, so the hook and the recipes work without the plugin. The copies share one layout and lock protocol, so they lease from the same pool. Update it by copying the upstream file unchanged, in a PR; never edit the copy here.
