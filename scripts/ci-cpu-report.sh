#!/usr/bin/env bash
#
# Report how much CPU a CI job had and how long it waited for CPU, as a
# Markdown table for the job summary.
#
#   scripts/ci-cpu-report.sh start                                  # first step
#   scripts/ci-cpu-report.sh report | tee -a "$GITHUB_STEP_SUMMARY" # last step
#
# Two kinds of slowness look alike from inside a job: the job's own CPU limit
# throttling it, and other work on the same node keeping it waiting. The
# cgroup's `cpu.stat` shows the first. Pressure stall information (PSI) shows
# the second: the share of time some task was ready to run but had no CPU.
# The runner container's PSI covers this job (the pod is ephemeral). The
# node's PSI (/proc/pressure/cpu, not namespaced) covers everything on the
# node, so it is taken as the difference between `start` and `report`. The
# browsers of the web end-to-end suite run in the Docker sidecar, whose cgroup
# a job can't read, so the node's pressure is what shows their contention.
#
# Called from .github/workflows/quality-checks.yml (web-e2e). A counter it
# can't read shows as "unavailable" rather than failing the job.
set -uo pipefail

mode="${1:-report}"
snapshot="${RUNNER_TEMP:-/tmp}/ci-cpu-start"

# Microseconds that some task waited for CPU, from a PSI file.
psi_total() {
    awk '$1=="some" { for (i = 2; i <= NF; i++) if ($i ~ /^total=/) { sub("total=", "", $i); print $i } }' "$1" 2>/dev/null
}

if [ "$mode" = "start" ]; then
    { date +%s; psi_total /proc/pressure/cpu; } >"$snapshot"
    exit 0
fi

started="$(sed -n 1p "$snapshot" 2>/dev/null)"
node_waited_before="$(sed -n 2p "$snapshot" 2>/dev/null)"
now="$(date +%s)"
elapsed=$((now - ${started:-$now}))

stat="$(cat /sys/fs/cgroup/cpu.stat 2>/dev/null)"
max="$(cat /sys/fs/cgroup/cpu.max 2>/dev/null)"
field() { awk -v k="$1" '$1==k {print $2}' <<<"$stat"; }

limit="$(awk '{ if ($1=="max") print "none"; else if ($2 > 0) printf "%.1f CPU", $1/$2 }' <<<"$max")"
used_s=$(($(field usage_usec || echo 0) / 1000000))
periods="$(field nr_periods)"
throttled_pct="$(awk -v t="$(field nr_throttled)" -v p="${periods:-0}" 'BEGIN{ if (p > 0) printf "%.1f%%", 100*t/p; else print "n/a" }')"
runner_waited_s=$(($(psi_total /sys/fs/cgroup/cpu.pressure || echo 0) / 1000000))
node_waited_after="$(psi_total /proc/pressure/cpu)"
if [ -n "$node_waited_before" ] && [ -n "$node_waited_after" ] && [ "$elapsed" -gt 0 ]; then
    node_pct="$(awk -v a="$node_waited_after" -v b="$node_waited_before" -v e="$elapsed" 'BEGIN{ printf "%.0f%%", 100*(a-b)/1000000/e }')"
else
    node_pct="unavailable"
fi

echo '### CPU'
echo ''
echo "Node: $(nproc) CPUs, $(awk -F': ' '/model name/{print $2; exit}' /proc/cpuinfo). Job ran ${elapsed}s."
echo ''
echo '| measure | value |'
echo '|---|---|'
echo "| runner limit | ${limit:-unknown} |"
echo "| runner CPU used | ${used_s}s |"
echo "| runner periods throttled by its limit | ${throttled_pct} |"
echo "| runner time waiting for CPU (PSI) | ${runner_waited_s}s |"
echo "| node time with a task waiting for CPU (PSI) | ${node_pct} of the job |"
