# Scripts

Claude-powered codebase audit and remediation tools. Both scripts use the Claude CLI (`claude -p`) under the hood.

## audit-modules.sh

Audits every Rust module in `src/` in parallel, then runs a review pass to filter out non-actionable findings.

```bash
./scripts/audit-modules.sh [OPTIONS]
```

| Flag | Description | Default |
|---|---|---|
| `-p, --prompt PROMPT` | Audit instruction (inline text) | *required (or use -f)* |
| `-f, --file FILE` | Read prompt from a file (`-` for stdin) | — |
| `-m, --model MODEL` | Claude model to use | `sonnet` |
| `-j, --jobs N` | Max parallel jobs | `4` |
| `-o, --output DIR` | Output directory | `audit-results/` |

**How it works:**

1. Discovers all modules under `src/` (directories + standalone `.rs` files, excluding `lib.rs`)
2. Feeds each module's source files to Claude with your prompt, writing per-module markdown to the output directory
3. Runs a review pass that removes findings requiring cross-module/architectural changes, deletes empty files, and writes a `SUMMARY.md`

```bash
# Inline prompt
./scripts/audit-modules.sh -p "Review for error handling issues"

# Prompt from file, using opus
./scripts/audit-modules.sh -f audits/no-silent-failures.txt -m opus

# Prompt from stdin
cat prompt.txt | ./scripts/audit-modules.sh -f -
```

## apply-audits.sh

Takes audit results from `audit-modules.sh` and applies fixes one module at a time, each as its own commit on a dedicated branch.

```bash
./scripts/apply-audits.sh [OPTIONS]
```

| Flag | Description | Default |
|---|---|---|
| `-i, --input DIR` | Audit results directory | `audit-results/` |
| `-m, --model MODEL` | Claude model to use | `sonnet` |
| `-b, --branch NAME` | Branch name | `audit/apply-fixes` |
| `--dry-run` | Show what would be done without doing it | — |

**How it works:**

1. Requires a clean working tree; skips failed/empty audit files
2. Creates a branch from `main` (or resumes an existing one)
3. For each audit file, maps it back to its source module, feeds Claude the findings + source with strict instructions (only fix what's called out), and commits the result
4. Successfully applied audits are moved to `applied/`; failures are cleaned up

```bash
# Apply with defaults
./scripts/apply-audits.sh

# Specify model and branch
./scripts/apply-audits.sh -m opus -b audit/error-handling

# Preview without making changes
./scripts/apply-audits.sh --dry-run
```

## Typical Workflow

```bash
# 1. Run the audit
./scripts/audit-modules.sh -p "Review for error handling issues" -m opus

# 2. Review audit-results/SUMMARY.md and per-module findings

# 3. Apply the fixes
./scripts/apply-audits.sh -m opus -b audit/error-handling

# 4. Review the branch
git log --oneline main..audit/error-handling
git diff main...audit/error-handling

# 5. Merge
git checkout main && git merge --no-ff audit/error-handling
```
