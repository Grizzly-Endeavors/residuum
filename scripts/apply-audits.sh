#!/usr/bin/env bash
set -euo pipefail

# Apply audit findings in parallel using git worktrees, one commit per module.
#
# Creates N worker worktrees (one per parallel job), each with its own cargo
# target directory. Modules are distributed round-robin across workers and
# processed sequentially within each worker, so each benefits from the
# previous module's warm build cache. Commits are then cherry-picked onto
# the audit branch sequentially, with Claude resolving any conflicts.
#
# Usage:
#   ./scripts/apply-audits.sh [OPTIONS]
#
# Options:
#   -i, --input DIR       Audit results directory (default: audit-results/)
#   -m, --model MODEL     Model to use (default: sonnet)
#   -j, --jobs N          Max parallel jobs (default: 4)
#   -b, --branch NAME     Branch name (default: audit/apply-fixes)
#   -t, --timeout SECS    Per-module timeout in seconds (default: 600)
#   --dry-run             Show what would be done without doing it
#   -h, --help            Show this help
#
# Examples:
#   ./scripts/apply-audits.sh
#   ./scripts/apply-audits.sh -i audit-results/ -m opus -j 6
#   ./scripts/apply-audits.sh -b audit/error-handling --dry-run

MODEL="sonnet"
JOBS=4
INPUT_DIR="audit-results"
BRANCH="audit/apply-fixes"
TIMEOUT=6000
DRY_RUN=false

usage() {
    sed -n '/^# Usage:/,/^[^#]/p' "$0" | head -n -1 | sed 's/^# \?//'
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -i|--input)   INPUT_DIR="$2"; shift 2 ;;
        -m|--model)   MODEL="$2"; shift 2 ;;
        -j|--jobs)    JOBS="$2"; shift 2 ;;
        -b|--branch)  BRANCH="$2"; shift 2 ;;
        -t|--timeout) TIMEOUT="$2"; shift 2 ;;
        --dry-run)    DRY_RUN=true; shift ;;
        -h|--help)    usage ;;
        *)            echo "Unknown option: $1"; usage ;;
    esac
done

if [[ ! -d "$INPUT_DIR" ]]; then
    echo "Error: audit results directory '$INPUT_DIR' not found"
    exit 1
fi

# Collect audit files, skipping failed/empty/summary
AUDIT_FILES=()
for f in "$INPUT_DIR"/*.md; do
    [[ ! -f "$f" ]] && continue
    [[ "$(basename "$f")" == "SUMMARY.md" ]] && continue
    if head -1 "$f" | grep -q "^# Audit failed"; then
        echo "[skip] $(basename "$f" .md) — audit had failed"
        continue
    fi
    if [[ $(wc -c < "$f") -lt 50 ]]; then
        echo "[skip] $(basename "$f" .md) — audit file too small"
        continue
    fi
    AUDIT_FILES+=("$f")
done

if [[ ${#AUDIT_FILES[@]} -eq 0 ]]; then
    echo "No valid audit results found in $INPUT_DIR/"
    exit 1
fi

echo "Found ${#AUDIT_FILES[@]} audit results to apply"
echo "Model: $MODEL | Jobs: $JOBS | Branch: $BRANCH | Timeout: ${TIMEOUT}s"
echo "---"

if [[ -n $(git status --porcelain) ]]; then
    echo "Error: working tree is dirty. Commit or stash changes first."
    exit 1
fi

if $DRY_RUN; then
    for f in "${AUDIT_FILES[@]}"; do
        echo "[dry-run] $(basename "$f" .md) — would apply in worktree"
    done
    exit 0
fi

# Setup temp directories
WORKTREE_BASE=$(mktemp -d "${TMPDIR:-/tmp}/apply-audits-wt-XXXXXX")
RESULTS_DIR=$(mktemp -d "${TMPDIR:-/tmp}/apply-audits-results-XXXXXX")
ORIGINAL_BRANCH=$(git rev-parse --abbrev-ref HEAD)

mark_picked() {
    touch "$RESULTS_DIR/${1}.picked"
}

cleanup() {
    echo ""
    echo "Cleaning up worktrees..."
    local preserved=0

    for ((w=0; w<JOBS; w++)); do
        local wt_path="$WORKTREE_BASE/worker-$w"
        local branch="wt/apply-audit-worker-$w"
        [[ -d "$wt_path" ]] || continue

        # Check if any commit on this branch is un-picked
        local has_unpicked=false
        while IFS= read -r sha; do
            [[ -n "$sha" ]] || continue
            for f in "$RESULTS_DIR"/*.result; do
                [[ -f "$f" ]] || continue
                if grep -q "commit:$sha" "$f"; then
                    local mod
                    mod=$(basename "$f" .result)
                    if [[ ! -f "$RESULTS_DIR/${mod}.picked" ]]; then
                        has_unpicked=true
                    fi
                    break
                fi
            done
            $has_unpicked && break
        done < <(git log --format='%H' "main..$branch" 2>/dev/null)

        if $has_unpicked; then
            echo "  [preserved] worker-$w — branch $branch has un-applied commits"
            preserved=$((preserved + 1))
            continue
        fi

        git worktree remove --force "$wt_path" 2>/dev/null || true
        git branch -D "$branch" 2>/dev/null || true
    done

    rmdir "$WORKTREE_BASE" 2>/dev/null || true

    if [[ $preserved -gt 0 ]]; then
        echo ""
        echo "WARNING: $preserved worker(s) preserved with un-applied commits."
        echo "To recover manually:"
        echo "  git branch --list 'wt/apply-audit-worker-*'"
        echo "  git log --oneline main..<branch>"
        echo "  git cherry-pick <sha>"
        echo ""
        echo "To clean up:"
        echo "  git worktree list | grep apply-audit"
        echo "  git worktree remove <path> && git branch -D <branch>"
    fi

    [[ -n "$RESULTS_DIR" ]] && rm -rf "$RESULTS_DIR"
}
trap cleanup EXIT

# Each worker gets nproc/2 build threads — enough to keep CPU busy without
# overwhelming the system when multiple workers compile simultaneously.
CARGO_BUILD_JOBS=$(( $(nproc) / 2 ))
[[ $CARGO_BUILD_JOBS -lt 1 ]] && CARGO_BUILD_JOBS=1
export CARGO_BUILD_JOBS

CLAUDE_TOOLS="Edit Read Glob Grep Bash(git:*) Bash(cargo:*)"
CLAUDE_DISALLOWED_TOOLS="Agent"

# =============================================================================
# Phase 1: Apply fixes in parallel workers
# =============================================================================

# Create worker worktrees, each with its own target directory
echo ""
echo "Creating $JOBS worker worktrees..."
for ((w=0; w<JOBS; w++)); do
    wt_path="$WORKTREE_BASE/worker-$w"
    wt_branch="wt/apply-audit-worker-$w"
    if ! git worktree add "$wt_path" -b "$wt_branch" main --quiet 2>/dev/null; then
        echo "FATAL: could not create worktree worker-$w"
        exit 1
    fi
done

# Distribute audit files round-robin across workers
for ((i=0; i<${#AUDIT_FILES[@]}; i++)); do
    echo "${AUDIT_FILES[$i]}" >> "$RESULTS_DIR/queue-$((i % JOBS))"
done

# Worker function: processes a queue of modules sequentially in one worktree
process_worker() {
    local worker_id="$1"
    local queue_file="$RESULTS_DIR/queue-$worker_id"
    local worktree_path="$WORKTREE_BASE/worker-$worker_id"

    [[ -f "$queue_file" ]] || return 0

    # Each worker gets its own target directory — no lock contention
    export CARGO_TARGET_DIR="$worktree_path/target"

    while IFS= read -r audit_file; do
        local module_name
        module_name=$(basename "$audit_file" .md)
        local result_file="$RESULTS_DIR/${module_name}.result"

        # Map module name to source path
        local module_path
        if [[ -f "src/${module_name}.rs" ]]; then
            module_path="src/${module_name}.rs"
        elif [[ -d "src/${module_name}" ]]; then
            module_path="src/${module_name}"
        else
            echo "[skip] $module_name — no matching source"
            echo "skip:no-source" > "$result_file"
            continue
        fi

        echo "[start] $module_name (worker $worker_id)"

        # Track which module is in-flight for recovery
        echo "$worker_id" > "$RESULTS_DIR/${module_name}.started"

        # Build prompt
        local tmpfile
        tmpfile=$(mktemp)

        local source_files
        if [[ -d "$worktree_path/$module_path" ]]; then
            source_files=$(find "$worktree_path/$module_path" -name '*.rs' -type f | sort)
        else
            source_files="$worktree_path/$module_path"
        fi

        {
            cat <<'INSTRUCTIONS'
You are fixing issues in a Rust module based on audit findings.

Rules:
- ONLY fix issues explicitly called out in the audit findings below
- Do NOT refactor, rename, or "improve" code beyond what the audit asks for
- Do NOT add comments explaining your fixes
- If an audit finding is vague or you're unsure how to fix it, skip it

After making ALL your changes, stage and commit them in a SINGLE commit using `git add` and `git commit -m "message"`.
Write a good commit message: concise summary (<=72 chars) on the first line.
Do NOT use `git commit -m "$(cat ...)"` or heredocs — just a plain `-m "message"` string.
Do NOT make multiple commits — everything goes in one commit.

If the pre-commit hooks fail, fix any issues that are surfaced, regardless of whether they are in this module or not.

INSTRUCTIONS
            echo "## Audit Findings"
            echo ""
            cat "$audit_file"
            echo ""
            echo "## Source Files"
            echo ""
            for f in $source_files; do
                local rel_path="${f#"$worktree_path"/}"
                echo "--- $rel_path ---"
                cat "$f"
                echo ""
            done
        } > "$tmpfile"

        # Run Claude in the worktree directory
        local pre_head
        pre_head=$(cd "$worktree_path" && git rev-parse HEAD)

        local claude_exit=0
        (cd "$worktree_path" && timeout "${TIMEOUT}s" claude -p --model "$MODEL" \
            --allowedTools "$CLAUDE_TOOLS" \
            --disallowedTools "$CLAUDE_DISALLOWED_TOOLS" \
            < "$tmpfile" > /dev/null 2>&1) || claude_exit=$?

        # 124 = timeout killed the process
        if [[ $claude_exit -eq 124 ]]; then
            echo "[timeout] $module_name — killed after ${TIMEOUT}s"
        fi

        # Always check HEAD — Claude may have committed before exiting non-zero
        local post_head
        post_head=$(cd "$worktree_path" && git rev-parse HEAD)

        if [[ "$post_head" != "$pre_head" ]]; then
            # Commit exists regardless of exit code — treat as success
            echo "[done]  $module_name ($post_head)"
            echo "commit:$post_head" > "$result_file"
            mkdir -p "$INPUT_DIR/applied"
            mv "$INPUT_DIR/${module_name}.md" "$INPUT_DIR/applied/"
        elif [[ $claude_exit -eq 0 ]]; then
            echo "[no-op] $module_name — no changes committed"
            echo "skip:no-changes" > "$result_file"
        else
            echo "[FAIL]  $module_name — Claude exited with error"
            echo "fail:claude" > "$result_file"
        fi

        rm -f "$RESULTS_DIR/${module_name}.started"
        rm -f "$tmpfile"
    done < "$queue_file"
}

echo ""
echo "Phase 1: Applying fixes in parallel (jobs=$JOBS)..."
echo ""

# Launch workers as background processes
for ((w=0; w<JOBS; w++)); do
    process_worker "$w" &
done
wait || true

# Recovery sweep: find modules that were started but never got a result
# (e.g. worker killed mid-run after Claude committed)
for f in "$RESULTS_DIR"/*.started; do
    [[ -f "$f" ]] || continue
    module_name=$(basename "$f" .started)
    result_file="$RESULTS_DIR/${module_name}.result"
    [[ -f "$result_file" ]] && continue

    worker_id=$(cat "$f")
    branch="wt/apply-audit-worker-$worker_id"
    branch_head=$(git rev-parse "$branch" 2>/dev/null) || continue

    # Check if this commit is already recorded for another module
    if ! grep -rql "commit:$branch_head" "$RESULTS_DIR"/*.result 2>/dev/null; then
        echo "[recovered] $module_name ($branch_head)"
        echo "commit:$branch_head" > "$result_file"
    else
        echo "[recovered] $module_name — no unrecorded commit found"
        echo "skip:no-changes" > "$result_file"
    fi
done

# =============================================================================
# Phase 2: Cherry-pick onto audit branch
# =============================================================================

echo ""
echo "Phase 2: Cherry-picking onto $BRANCH..."
echo ""

# Collect successful commits (sorted by module name for deterministic order)
COMMITS=()
COMMIT_MODULES=()
PHASE1_SKIPPED=0
PHASE1_FAILED=0

for f in $(printf '%s\n' "$RESULTS_DIR"/*.result | sort); do
    [[ -f "$f" ]] || continue
    module_name=$(basename "$f" .result)
    status=$(cat "$f")
    case "$status" in
        commit:*)
            COMMITS+=("${status#commit:}")
            COMMIT_MODULES+=("$module_name")
            ;;
        skip:*)
            PHASE1_SKIPPED=$((PHASE1_SKIPPED + 1))
            ;;
        fail:*)
            PHASE1_FAILED=$((PHASE1_FAILED + 1))
            ;;
    esac
done

if [[ ${#COMMITS[@]} -eq 0 ]]; then
    echo "No commits to cherry-pick."
    echo ""
    echo "---"
    echo "Applied: 0 | Skipped: $PHASE1_SKIPPED | Failed: $PHASE1_FAILED"
    exit 0
fi

echo "Collected ${#COMMITS[@]} commits to cherry-pick"

# Create or switch to the audit branch
if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    git checkout "$BRANCH" --quiet
    echo "Resuming on existing branch: $BRANCH"
else
    git checkout -b "$BRANCH" main --quiet
    echo "Created branch: $BRANCH"
fi

APPLIED=0
CHERRY_FAILED=0
CONFLICTS_RESOLVED=0

for i in "${!COMMITS[@]}"; do
    sha="${COMMITS[$i]}"
    module="${COMMIT_MODULES[$i]}"

    echo ""
    echo "=== $module ==="

    pre_head=$(git rev-parse HEAD)

    if git cherry-pick --no-commit "$sha" 2>/dev/null; then
        # Clean apply — commit with original message, skip hooks
        echo "  Cherry-picked cleanly, committing..."

        if git commit --no-verify -C "$sha" 2>/dev/null; then
            echo "  [done] Applied cleanly"
            mkdir -p "$INPUT_DIR/applied"
            [[ -f "$INPUT_DIR/${module}.md" ]] && mv "$INPUT_DIR/${module}.md" "$INPUT_DIR/applied/"
            APPLIED=$((APPLIED + 1))
            mark_picked "$module"
        else
            echo "  [FAIL] Could not commit"
            git reset --hard HEAD 2>/dev/null
            CHERRY_FAILED=$((CHERRY_FAILED + 1))
        fi
        continue
    fi

    # Cherry-pick failed — check if it's a merge conflict we can resolve
    conflicted_files=$(git diff --name-only --diff-filter=U 2>/dev/null || true)
    if [[ -z "$conflicted_files" ]]; then
        echo "  [FAIL] Cherry-pick failed (not a merge conflict)"
        git cherry-pick --abort 2>/dev/null || git reset --hard HEAD 2>/dev/null
        CHERRY_FAILED=$((CHERRY_FAILED + 1))
        continue
    fi

    echo "  [conflict] $(echo "$conflicted_files" | wc -l) file(s) — resolving with Claude..."

    # Build conflict resolution prompt
    resolve_prompt=$(mktemp)
    {
        cat <<'INSTRUCTIONS'
You are resolving git merge conflicts in Rust source files.

These conflicts arose from cherry-picking parallel audit fixes onto a branch that
already has other audit fixes applied. Both sides contain valid fixes for different
audit findings. Your job is to keep BOTH sets of changes.

Rules:
- Resolve ALL conflict markers (<<<<<<< / ======= / >>>>>>>)
- Keep changes from BOTH sides — do not discard either
- Do NOT make any changes beyond resolving the conflicts
- Do NOT make any changes beyond resolving the conflicts

INSTRUCTIONS
        cat <<COMMIT_INSTRUCTIONS
After resolving, stage and commit: git add -A && git commit -m "audit: fix $module (conflict resolved)"
Do NOT use \$() or heredocs — just a plain -m "message" string.

If the pre-commit hooks fail, read the error output, fix the issues, and retry the commit.
COMMIT_INSTRUCTIONS
        echo ""
        echo "## Conflicted Files"
        echo ""
        for cf in $conflicted_files; do
            echo "--- $cf ---"
            cat "$cf"
            echo ""
        done
        echo ""
        echo "## Audit Context (module: $module)"
        echo ""
        if [[ -f "$INPUT_DIR/${module}.md" ]]; then
            cat "$INPUT_DIR/${module}.md"
        fi
    } > "$resolve_prompt"

    if claude -p --model "$MODEL" \
        --allowedTools "$CLAUDE_TOOLS" \
        --disallowedTools "$CLAUDE_DISALLOWED_TOOLS" \
        < "$resolve_prompt" > /dev/null 2>&1; then

        if [[ "$(git rev-parse HEAD)" != "$pre_head" ]]; then
            echo "  [done] Conflict resolved and committed"
            mkdir -p "$INPUT_DIR/applied"
            [[ -f "$INPUT_DIR/${module}.md" ]] && mv "$INPUT_DIR/${module}.md" "$INPUT_DIR/applied/"
            APPLIED=$((APPLIED + 1))
            mark_picked "$module"
            CONFLICTS_RESOLVED=$((CONFLICTS_RESOLVED + 1))
        else
            echo "  [FAIL] Claude did not commit after resolution"
            git cherry-pick --abort 2>/dev/null || git reset --hard HEAD 2>/dev/null
            CHERRY_FAILED=$((CHERRY_FAILED + 1))
        fi
    else
        echo "  [FAIL] Claude could not resolve the conflict"
        git cherry-pick --abort 2>/dev/null || git reset --hard HEAD 2>/dev/null
        CHERRY_FAILED=$((CHERRY_FAILED + 1))
    fi

    rm -f "$resolve_prompt"
done

# Switch back to original branch if nothing was applied
if [[ $APPLIED -eq 0 ]]; then
    git checkout "$ORIGINAL_BRANCH" --quiet 2>/dev/null || true
fi

TOTAL_SKIPPED=$PHASE1_SKIPPED
TOTAL_FAILED=$((PHASE1_FAILED + CHERRY_FAILED))

echo ""
echo "---"
echo "Applied: $APPLIED | Skipped: $TOTAL_SKIPPED | Failed: $TOTAL_FAILED | Conflicts resolved: $CONFLICTS_RESOLVED"

if [[ $APPLIED -gt 0 ]]; then
    echo ""
    echo "Review the branch:"
    echo "  git log --oneline main..$BRANCH"
    echo "  git diff main...$BRANCH"
    echo ""
    echo "To merge:"
    echo "  git checkout main && git merge --no-ff $BRANCH"
fi
