#!/usr/bin/env bash
# Usage: scripts/npm-pack.sh <version> <archives-dir> <out-dir>
# Builds out/ai-launcher-<platform>-<arch>/ for each release archive found, plus out/ai-launcher/.
set -euo pipefail
[ $# -eq 3 ] || { echo "usage: $0 <version> <archives-dir> <out-dir>" >&2; exit 2; }
ver=$1 src=$(cd "$2" && pwd) out=$3
root=$(cd "$(dirname "$0")/.." && pwd)
rm -rf "$out"; mkdir -p "$out"; out=$(cd "$out" && pwd)

# target -> platform arch
targets="aarch64-apple-darwin:darwin:arm64 x86_64-apple-darwin:darwin:x64
x86_64-unknown-linux-gnu:linux:x64 aarch64-unknown-linux-gnu:linux:arm64
x86_64-pc-windows-msvc:win32:x64"

for t in $targets; do
  IFS=: read -r target plat arch <<<"$t"
  exe=ai-launcher; ext=tar.gz; [ "$plat" = win32 ] && { exe=ai-launcher.exe; ext=zip; }
  archive="$src/ai-launcher-$target.$ext"
  [ -f "$archive" ] || { echo "missing $archive" >&2; exit 1; }
  d="$out/ai-launcher-$plat-$arch"; mkdir -p "$d/bin"
  if [ $ext = zip ]; then unzip -qo "$archive" "$exe" -d "$d/bin"; else tar -xzf "$archive" -C "$d/bin" "$exe"; fi
  chmod +x "$d/bin/$exe"
  sed -e "s/__PLATFORM__/$plat/g" -e "s/__ARCH__/$arch/g" -e "s/__VERSION__/$ver/g" \
    "$root/npm/platform-template/package.json" > "$d/package.json"
done

cp -R "$root/npm/ai-launcher" "$out/ai-launcher"
node -e '
const fs=require("fs"),p=process.argv[1]+"/package.json",v=process.argv[2];
const j=JSON.parse(fs.readFileSync(p));j.version=v;
for(const k in j.optionalDependencies)j.optionalDependencies[k]=v;
fs.writeFileSync(p,JSON.stringify(j,null,2)+"\n")' "$out/ai-launcher" "$ver"
echo "packed v$ver into $out"
