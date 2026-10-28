#!/usr/bin/env bash
# Usage: scripts/next-version.sh <branch> <last-version>
# Prints the release version a merged branch gets, or nothing when that branch does not release.
#   major/ breaking/ -> X+1.0.0   feature/ -> X.Y+1.0   enhance/ fix/ -> X.Y.Z+1   anything else -> no release
set -eu
[ $# -eq 2 ] || { echo "usage: $0 <branch> <last-version>" >&2; exit 2; }
branch=$1 last=${2#v}
case $last in
  *[!0-9.]*|.*|*.|*..*) echo "bad version: $2" >&2; exit 2 ;;
esac
IFS=. read -r major minor patch rest <<EOF
$last
EOF
[ -n "$major" ] && [ -n "$minor" ] && [ -n "$patch" ] && [ -z "$rest" ] || { echo "bad version: $2" >&2; exit 2; }
case $branch in
  major/*|breaking/*) echo "$((major + 1)).0.0" ;;
  feature/*) echo "$major.$((minor + 1)).0" ;;
  enhance/*|fix/*) echo "$major.$minor.$((patch + 1))" ;;
esac
