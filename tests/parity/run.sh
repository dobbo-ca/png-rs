#!/bin/sh
# Parity gates of png-rs (docs/design.md, "Oracles and parity"). CI only, called by
# .github/ci/Dockerfile; needs the oracle binaries on PATH.
#   run.sh gather <dst> <root> <sub>...   copy the corpus files of <root>/<sub> to <dst>, each named
#                                         by its path relative to <root> with / replaced by __
#   run.sh full <report-dir>              the three gates on tests/data + /data/x + mutations
#   run.sh ref <out-dir>                  png-oracle on PngSuite, both profiles, one thread
#   run.sh arm <ref-dir>                  the aarch64 oracle (native arm64) on PngSuite vs <ref-dir>
# Binaries: png-oracle (png-rs examples/oracle), pngoracle (libpng), byblos-oracle, and for `arm`
# png-oracle-arm64. Exit status 0 only when every gate is an empty diff.
set -u
W=/work

# flat <dst> <root>: copy the files named on stdin (paths under <root>) to <dst>, flattened.
flat() {
  while IFS= read -r f; do
    rel=${f#"$2"/}
    cp "$f" "$1/$(printf '%s' "$rel" | sed 's#/#__#g')"
  done
}

# Only .png files, except fuzz-grown/, whose files have no extension.
gather() {
  dst=$1 root=$2; shift 2
  mkdir -p "$dst"
  for sub in "$@"; do
    [ -d "$root/$sub" ] || continue
    find "$root/$sub" -type f ! -name '.*' \( -name '*.png' -o -path '*/fuzz-grown/*' \) | flat "$dst" "$root"
  done
}

count() { find "$1" -type f -name "$2" | wc -l; }

# kind <dir> <name>: the .err token of <name> in <dir>, "eight" if it decoded, else "none".
kind() {
  if [ -f "$1/$2.err" ]; then cat "$1/$2.err"
  elif [ -f "$1/$2.eight" ]; then echo eight
  else echo none; fi
}

# pairs <dirA> <labelA> <dirB> <labelB> [split]: one line "<name>: <labelA>=<token> <labelB>=<token>" for
# every name whose .err token differs (or is .err on one side and .eight on the other). With
# `split` only the ok/err split counts (libpng writes `error` as its token).
pairs() {
  { (cd "$1" && ls | sed -n "s/\\.err\$//p"); (cd "$3" && ls | sed -n "s/\\.err\$//p"); } | sort -u |
  while IFS= read -r n; do
    a=$(kind "$1" "$n"); b=$(kind "$3" "$n")
    if [ -n "${5:-}" ]; then [ "$a" = eight ] && x=eight || x=err; [ "$b" = eight ] && y=eight || y=err; [ $x = $y ] && continue; fi
    [ "$a" = "$b" ] || echo "$n: $2=$a $4=$b"
  done
}

# gate <label> <diff-file>: an empty diff passes.
gate() {
  lines=$(wc -l < "$2")
  if [ "$lines" -eq 0 ]; then
    echo "| $1 | ok |" >> "$REP/summary.md"; rm -f "$2"
  else
    echo "| $1 | FAIL, $lines diff lines |" >> "$REP/summary.md"; bad=1
    head -n 200 "$2" > "$2.t" && mv "$2.t" "$2"
  fi
}

run() { "$@" || { echo "FAILED: $*" >&2; bad=1; }; }

full() {
  REP=$1 bad=0
  rm -rf $W; mkdir -p "$REP" $W/corpus $W/all $W/mut $W/c $W/lp1 $W/lpn $W/go1 $W/gon $W/byb
  gather $W/corpus tests/data pngsuite byblos fuzz-grown
  find /data/x -type f -name '*.png' | flat $W/corpus /data/x
  n=$(ls $W/corpus | wc -l)
  # PngSuite 176 + byblos 272: a mount that did not arrive must fail, not shrink the gate.
  [ "$n" -ge 448 ] || { echo "corpus has $n files, want at least 448" >&2; bad=1; }
  run png-oracle gen $W/corpus $W/mut
  cp -al $W/corpus/. $W/all/ && cp -al $W/mut/. $W/all/ || bad=1

  run pngoracle $W/corpus $W/c
  run png-oracle --profile libpng --threads 1 $W/corpus $W/lp1
  run png-oracle --profile libpng $W/corpus $W/lpn
  run png-oracle --profile go --threads 1 $W/all $W/go1
  run png-oracle --profile go $W/all $W/gon
  run byblos-oracle $W/all $W/byb

  : > $W/d-libpng; : > $W/d-byblos; : > $W/d-lp-threads; : > $W/d-go-threads
  # libpng: the same inputs fail (the C tool writes only `error` in .err), and the rest is identical.
  (cd $W/c && ls | sed -n 's/\.err$//p' | sort) > $W/c.errs
  (cd $W/lp1 && ls | sed -n 's/\.err$//p' | sort) > $W/lp.errs
  diff $W/c.errs $W/lp.errs > $W/d-libpng
  diff -rq -x '*.err' $W/c $W/lp1 >> $W/d-libpng
  pairs $W/c libpng $W/lp1 png split > $W/p-libpng
  # byblos writes .eight and .err (with tokens) only.
  diff -rq -x '*.native' -x '*.meta' $W/byb $W/go1 > $W/d-byblos
  pairs $W/byb byblos $W/go1 png > $W/p-byblos
  for d in libpng byblos; do { cat $W/p-$d; cat $W/d-$d; } > $W/d.t && mv $W/d.t $W/d-$d; done
  diff -rq $W/lp1 $W/lpn > $W/d-lp-threads
  diff -rq $W/go1 $W/gon > $W/d-go-threads

  {
    echo "## png-rs parity"; echo
    echo "| input set | files |"; echo "|---|---|"
    echo "| corpus (PngSuite, byblos, fuzz-grown, public sets) | $n |"
    echo "| mutations | $(ls $W/mut | wc -l) |"
    echo "| libpng: ok / err | $(count $W/c '*.meta') / $(count $W/c '*.err') |"
    echo "| png-rs libpng profile: ok / err | $(count $W/lp1 '*.meta') / $(count $W/lp1 '*.err') |"
    echo "| png-rs go profile, corpus + mutations: ok / err | $(count $W/go1 '*.meta') / $(count $W/go1 '*.err') |"
    echo "| byblos, corpus + mutations: ok / err | $(count $W/byb '*.eight') / $(count $W/byb '*.err') |"
    echo "| files compared with libpng (.native .eight .meta) | $(find $W/lp1 -type f ! -name '*.err' | wc -l) |"
    echo "| files compared with byblos (.eight .err) | $(find $W/byb -type f | wc -l) |"
    echo; echo "| gate | result |"; echo "|---|---|"
  } > "$REP/summary.md"
  gate "libpng vs png-rs --profile libpng" $W/d-libpng
  gate "byblos vs png-rs --profile go" $W/d-byblos
  gate "libpng profile, 1 thread vs default threads" $W/d-lp-threads
  gate "go profile, 1 thread vs default threads" $W/d-go-threads
  for d in libpng byblos; do
    [ -s $W/p-$d ] && { echo; echo "token pairs, $d vs png-rs (first 50):"; echo; head -n 50 $W/p-$d | sed "s/^/    /"; } >> "$REP/summary.md"
  done
  for d in d-libpng d-byblos d-lp-threads d-go-threads; do [ -f $W/$d ] && cp $W/$d "$REP/$d.diff"; done
  rm -rf $W
  return $bad
}

# The PngSuite trees every aarch64 run is compared with.
ref() {
  rm -rf $W; gather $W/in tests/data pngsuite
  for p in go libpng; do mkdir -p "$1/$p"; run png-oracle --profile $p --threads 1 $W/in "$1/$p"; done
  rm -rf $W
  return "${bad:-0}"
}

arm() {
  REP=$1 bad=0
  rm -rf $W; gather $W/in tests/data pngsuite
  for p in go libpng; do
    for simd in default scalar; do
      if [ $simd = scalar ]; then export PNG_SIMD=scalar; else unset PNG_SIMD; fi
      mkdir -p $W/out
      run /usr/local/bin/png-oracle-arm64 --profile $p $W/in $W/out
      if diff -rq "$REP/$p" $W/out > $W/d; then echo "aarch64 $p $simd: identical to x86_64"
      else echo "aarch64 $p $simd: DIFF"; head -n 50 $W/d; bad=1; fi
      rm -rf $W/out
    done
  done
  rm -rf $W
  return $bad
}

cmd=${1:-}; [ $# -gt 0 ] && shift
case $cmd in
  gather) gather "$@" ;;
  full) full "$@" ;;
  ref) bad=0; ref "$@" ;;
  arm) arm "$@" ;;
  *) echo "usage: run.sh gather|full|ref|arm ..." >&2; exit 2 ;;
esac
