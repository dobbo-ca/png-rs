#!/bin/sh
# bench.sh <report-dir>: libpng (pngoracle bench) and png-rs (examples/bench, 1 thread and default
# threads) decode the same corpus REPS times (default 5); summary.md has ms per MB of output by
# file size class, lower is better. Information only, no gate: the status is 0 unless a tool fails
# or no file was timed by all three. Tools print TSV `name<TAB>bytes<TAB>ns`; examples/bench takes
# `<dir> <reps> [max_threads]`.
set -u
export LC_ALL=C
REP=$1 reps=${REPS:-5} W=/work tab=$(printf '\t')
rm -rf $W; mkdir -p "$REP" $W/corpus
sh tests/parity/run.sh gather $W/corpus tests/data pngsuite byblos
find /data/x -type f -name '*.png' | while IFS= read -r f; do
  cp "$f" "$W/corpus/$(printf '%s' "${f#/data/x/}" | sed 's#/#__#g')"
done
bad=0
pngoracle bench $W/corpus $reps > $W/c.raw || bad=1
png-bench $W/corpus $reps 1 > $W/r1.raw || bad=1
png-bench $W/corpus $reps > $W/rn.raw || bad=1
for t in c r1 rn; do sort -t "$tab" -k1,1 $W/$t.raw > $W/$t.tsv; done
# name bytes ns(libpng) bytes ns(1 thread) bytes ns(default)
join -t "$tab" $W/c.tsv $W/r1.tsv | join -t "$tab" - $W/rn.tsv > $W/all.tsv
files=$(wc -l < $W/all.tsv)
[ "$files" -gt 0 ] || bad=1
{
  echo "## png-rs vs libpng: decode time, ms per MB of output (lower is better)"; echo
  echo "$files files timed by all three tools, $reps repetitions. Information only, no gate."; echo
  echo "| size class (output) | files | libpng | png-rs 1 thread | png-rs default | 1 thread / libpng | default / libpng |"
  echo "|---|---|---|---|---|---|---|"
  awk -F'\t' '
    { b = $2; k = (b <= 4096) ? 1 : (b <= 65536) ? 2 : (b <= 1048576) ? 3 : (b <= 16777216) ? 4 : 5
      n[k]++; c[k] += $3; r1[k] += $5; rn[k] += $7; by[k] += b
      N++; C += $3; R1 += $5; RN += $7; BY += b }
    function row(l, n, by, c, r1, rn) {
      if (by > 0) printf "| %s | %d | %.2f | %.2f | %.2f | %.2f | %.2f |\n", l, n, c/by, r1/by, rn/by, r1/c, rn/c }
    END {
      split("<= 4 KiB|<= 64 KiB|<= 1 MiB|<= 16 MiB|> 16 MiB", l, "|")
      for (k = 1; k <= 5; k++) row(l[k], n[k], by[k], c[k], r1[k], rn[k])
      row("all", N, BY, C, R1, RN) }' $W/all.tsv
} > "$REP/summary.md"
cp $W/all.tsv "$REP/all.tsv"
rm -rf $W
exit $bad
