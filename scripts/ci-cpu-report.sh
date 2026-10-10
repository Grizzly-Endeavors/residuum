#!/usr/bin/env bash
#
# Report how much CPU a CI job had and how much of its time the CPU limit
# throttled away, as a Markdown table for the job summary.
#
# Runners are ephemeral pods, so the cgroup counters cover this job alone:
# run it once, at the end. It reads the runner container's cgroup and, when a
# Docker daemon is beside the runner, the daemon's (the browsers of the web
# end-to-end suite run there, in the Playwright container).
#
#   scripts/ci-cpu-report.sh [image-for-the-docker-probe] >> "$GITHUB_STEP_SUMMARY"
#
# Without an image argument it probes with the Playwright image already pulled.
#
# Called from .github/workflows/quality-checks.yml (web-e2e). A counter it
# can't read shows as "unavailable" rather than failing the job.
set -uo pipefail

# Any image already on the daemon will do for the probe; the Playwright one is.
probe_image="${1:-$(docker images --format '{{.Repository}}:{{.Tag}}' 2>/dev/null | grep -m1 playwright)}"

# One row per cgroup: CPU limit, time used, and throttling.
row() {
    local label="$1" stat="$2" max="$3"
    if [ -z "$stat" ]; then
        echo "| $label | unavailable | | | |"
        return
    fi
    local usage periods throttled throttled_usec limit
    usage="$(awk '$1=="usage_usec"{print $2}' <<<"$stat")"
    periods="$(awk '$1=="nr_periods"{print $2}' <<<"$stat")"
    throttled="$(awk '$1=="nr_throttled"{print $2}' <<<"$stat")"
    throttled_usec="$(awk '$1=="throttled_usec"{print $2}' <<<"$stat")"
    limit="$(awk '{ if ($1=="max") print "none"; else if ($2 > 0) printf "%.1f CPU", $1/$2 }' <<<"$max")"
    limit="${limit:-unknown}"
    local pct="0"
    if [ "${periods:-0}" -gt 0 ]; then
        pct="$(awk -v t="$throttled" -v p="$periods" 'BEGIN{printf "%.1f", 100*t/p}')"
    fi
    printf '| %s | %s | %.0f s | %s%% of %s periods | %.0f s |\n' \
        "$label" "$limit" "$((${usage:-0} / 1000000))" "$pct" "${periods:-0}" "$((${throttled_usec:-0} / 1000000))"
}

echo '### CPU'
echo ''
echo "Host: $(nproc) CPUs visible, $(awk -F': ' '/model name/{print $2; exit}' /proc/cpuinfo)${NODE_NAME:+, node $NODE_NAME}"
echo ''
echo '| cgroup | limit | CPU used | throttled | time throttled |'
echo '|---|---|---|---|---|'
row "runner" "$(cat /sys/fs/cgroup/cpu.stat 2>/dev/null)" "$(cat /sys/fs/cgroup/cpu.max 2>/dev/null)"
if [ -n "$probe_image" ] && command -v docker >/dev/null 2>&1; then
    # With the daemon's cgroup namespace, the probe's cgroup root is the
    # Docker sidecar's own cgroup, which holds every container the job ran.
    dind_stat="$(docker run --rm --cgroupns=host -v /sys/fs/cgroup:/cg:ro --entrypoint cat "$probe_image" /cg/cpu.stat 2>/dev/null)"
    dind_max="$(docker run --rm --cgroupns=host -v /sys/fs/cgroup:/cg:ro --entrypoint cat "$probe_image" /cg/cpu.max 2>/dev/null)"
    row "docker (browsers)" "$dind_stat" "$dind_max"
fi
