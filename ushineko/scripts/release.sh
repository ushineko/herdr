#!/usr/bin/env bash
# Build release binaries for this fork and publish them as a GitHub Release.
#
# Uses upstream's own manual artifact workflow (build-artifacts-manual.yml)
# with the settings herdr's releases use, so the binaries match an official
# build of the same source. Usage:
#
#   ushineko/scripts/release.sh ushineko-v0.9.5.1 [ref]
#
# ref defaults to ushineko/features. The tag must start with "ushineko-v":
# upstream's release.yml (disabled in this fork) is triggered by "v*" tags.
set -euo pipefail

repo=ushineko/herdr
tag=${1:?usage: release.sh <ushineko-vX.Y.Z.N> [ref]}
ref=${2:-ushineko/features}

case "$tag" in
  ushineko-v*) ;;
  *) echo "error: tag must start with ushineko-v" >&2; exit 2 ;;
esac
if gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
  echo "error: release $tag already exists" >&2; exit 2
fi

git fetch -q origin "$ref"
git tag -a "$tag" -m "$tag" "origin/$ref"
git push -q origin "$tag"

gh workflow run build-artifacts-manual.yml --repo "$repo" --ref "$tag" \
  -f build_group=all -f libghostty_optimize=ReleaseFast -f libghostty_simd=true

run_id=""
for _ in $(seq 1 30); do
  run_id=$(gh run list --repo "$repo" --workflow build-artifacts-manual.yml \
    --branch "$tag" --limit 1 --json databaseId -q '.[0].databaseId // empty')
  [ -n "$run_id" ] && break
  sleep 2
done
[ -n "$run_id" ] || { echo "error: build run for $tag did not start" >&2; exit 1; }
echo "building: https://github.com/$repo/actions/runs/$run_id"
gh run watch "$run_id" --repo "$repo" --exit-status >/dev/null

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh run download "$run_id" --repo "$repo" --dir "$work/artifacts"
mkdir "$work/release"
for dir in "$work"/artifacts/*/; do
  for file in "$dir"*; do
    name=$(basename "$file")
    [ "$name" = BUILD_INFO.txt ] && continue
    cp "$file" "$work/release/$name"
    cp "$dir/BUILD_INFO.txt" "$work/release/$name.BUILD_INFO.txt"
  done
done
(cd "$work/release" && sha256sum herdr-* | grep -v BUILD_INFO > SHA256SUMS)

base=$(git describe --tags --abbrev=0 --match 'v*' "origin/$ref")
notes="Built from \`$ref\` at $(git rev-parse --short "origin/$ref") on herdr $base.

Commits on top of herdr $base:

$(git log --no-merges --format='- %s' "$base..origin/$ref")"
gh release create "$tag" --repo "$repo" --title "$tag" --notes "$notes" "$work"/release/*
echo "released: https://github.com/$repo/releases/tag/$tag"
