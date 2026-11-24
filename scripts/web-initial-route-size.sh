#!/usr/bin/env bash
set -euo pipefail

dist="${1:-web/dist}"
index="$dist/index.html"

if [ ! -f "$index" ]; then
  echo "$index not found: run npm run build in web/ first" >&2
  exit 1
fi

assets="$(grep -oE '(src|href)="/assets/[^"]+\.(js|css)"' "$index" | sed -E 's/^[a-z]+="\/(.*)"$/\1/' | sort -u)"

kilobytes() {
  awk -v bytes="$1" 'BEGIN { printf "%.1f kB", bytes / 1000 }'
}

echo '| Initial route | Files | Raw | Gzipped |'
echo '|---------------|-------|-----|---------|'

total_raw=0
total_gzip=0
for extension in js css; do
  files=0
  raw=0
  gzipped=0
  for asset in $(grep -E "\.${extension}\$" <<<"$assets" || true); do
    files=$((files + 1))
    raw=$((raw + $(wc -c <"$dist/$asset")))
    gzipped=$((gzipped + $(gzip -c "$dist/$asset" | wc -c)))
  done
  total_raw=$((total_raw + raw))
  total_gzip=$((total_gzip + gzipped))
  label="JavaScript"
  [ "$extension" = css ] && label="CSS"
  echo "| $label | $files | $(kilobytes "$raw") | $(kilobytes "$gzipped") |"
done
echo "| Total | | $(kilobytes "$total_raw") | $(kilobytes "$total_gzip") |"
