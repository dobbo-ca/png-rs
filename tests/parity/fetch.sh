#!/bin/sh
# fetch.sh <corpus.toml> <dir>: download every [[file]] of corpus.toml into <dir>/<path> once, at
# most 20 MB/s, check its size and sha256, and extract the .png files under its `extract` subpaths
# into <dir>/x/<name>/. A file that is already there with its stamp (<path>.ok, written after the
# sum matched) is not downloaded again, and an extraction with its stamp (<dir>/x/<name>.ok, which
# holds the `extract` value) is not repeated, so a second run is a no-op.
# Run it inside a BuildKit cache mount, id=png-rs-data, sharing=locked.
set -eu
[ $# -eq 2 ] || { echo "usage: fetch.sh <corpus.toml> <dir>" >&2; exit 2; }
toml=$1 dir=$2
tab=$(printf '\t')
mkdir -p "$dir/x"
# One "name path url size sha256 extract" line per [[file]], tab separated (extract has spaces).
awk -F' = ' -v OFS='\t' '
  function emit() { if (p != "") print n, p, u, s, h, e; n = p = u = s = h = e = "" }
  /^\[\[file\]\]/ { emit() }
  $1 == "name" { n = $2 } $1 == "path" { p = $2 } $1 == "url" { u = $2 } $1 == "size" { s = $2 }
  $1 == "sha256" { h = $2 } $1 == "extract" { e = $2 }
  END { emit() }' "$toml" | tr -d '"' |
while IFS=$tab read -r name path url size sum extract; do
  f=$dir/$path x=$dir/x/$name
  if [ -f "$f.ok" ] && [ -f "$f" ] && [ "$(stat -c %s "$f")" = "$size" ]; then
    :
  else
    echo "fetch $path ($size bytes)"
    rm -f "$f.ok" "$x.ok"
    # -C - resumes a .part left by an interrupted run.
    if [ ! -f "$f.part" ] || [ "$(stat -c %s "$f.part")" != "$size" ]; then
      curl -fsSL --retry 3 --limit-rate 20M -C - -o "$f.part" "$url"
    fi
    got=$(stat -c %s "$f.part")
    [ "$got" = "$size" ] || { echo "$path: size $got, want $size" >&2; rm -f "$f.part"; exit 1; }
    echo "$sum  $f.part" | sha256sum -c --quiet - || { echo "$path: sha256 mismatch" >&2; rm -f "$f.part"; exit 1; }
    mv "$f.part" "$f"
    touch "$f.ok"
  fi
  if [ "$(cat "$x.ok" 2>/dev/null || true)" != "$extract" ]; then
    rm -rf "$x" "$x.ok"
    mkdir -p "$x"
    for sub in $extract; do
      tar -xzf "$f" -C "$x" --strip-components=1 --wildcards "$name/$sub/*"
    done
    find "$x" -type f ! -name '*.png' -delete
    printf '%s' "$extract" > "$x.ok"
  fi
  echo "$name: $(find "$x" -type f -name '*.png' | wc -l) png files"
done
echo "png-rs-data: $(du -sh "$dir" | cut -f1) in $dir"
