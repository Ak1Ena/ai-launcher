#!/usr/bin/env bash
# Usage: scripts/npm-pack.sh <version> <archives-dir> <out-dir>
# Builds out/ai-launcher/ with every platform's binary in bin/<platform>-<arch>/.
set -euo pipefail
[ $# -eq 3 ] || { echo "usage: $0 <version> <archives-dir> <out-dir>" >&2; exit 2; }
ver=$1 src=$(cd "$2" && pwd) out=$3
root=$(cd "$(dirname "$0")/.." && pwd)
rm -rf "$out"; mkdir -p "$out"; out=$(cd "$out" && pwd)
cp -R "$root/npm/ai-launcher" "$out/ai-launcher"
cp "$root/README.md" "$root/LICENSE" "$out/ai-launcher/"

# target -> platform arch
targets="aarch64-apple-darwin:darwin:arm64 x86_64-apple-darwin:darwin:x64
x86_64-unknown-linux-gnu:linux:x64 aarch64-unknown-linux-gnu:linux:arm64
x86_64-pc-windows-msvc:win32:x64"

for t in $targets; do
  IFS=: read -r target plat arch <<<"$t"
  exe=ai-launcher; ext=tar.gz; [ "$plat" = win32 ] && { exe=ai-launcher.exe; ext=zip; }
  archive="$src/ai-launcher-$target.$ext"
  [ -f "$archive" ] || { echo "missing $archive" >&2; exit 1; }
  d="$out/ai-launcher/bin/$plat-$arch"; mkdir -p "$d"
  if [ $ext = zip ]; then unzip -qo "$archive" "$exe" -d "$d"; else tar -xzf "$archive" -C "$d" "$exe"; fi
  chmod +x "$d/$exe"
done

node -e '
const fs=require("fs"),p=process.argv[1]+"/package.json";
const j=JSON.parse(fs.readFileSync(p));j.version=process.argv[2];
fs.writeFileSync(p,JSON.stringify(j,null,2)+"\n")' "$out/ai-launcher" "$ver"
echo "packed v$ver into $out"
