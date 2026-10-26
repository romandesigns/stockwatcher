#!/usr/bin/env bash
# CONFIRM-PAST import v3: day by day at <= 8,000 requests/min; each day's sha256 verify runs in the
# background while the next day imports; a day is marked imported only when its verify passes.
cd /c/research/stockwatcher/work-pat
verify() {
  d=$1
  ARCHIVE_ROOT='C:\research\stockwatcher\archive-past' ./target/release/swarchive.exe verify $d > confirm/logs2/verify-$d.log 2>&1
  rc=$?
  if [ $rc -eq 0 ] && ! grep -q '^FAIL' confirm/logs2/verify-$d.log; then touch confirm/imported/$d; echo "imported $d $(grep -c '^ok' confirm/logs2/verify-$d.log) files"; else echo "verify failed $d"; fi
}
for d in $(cat confirm_days.txt); do
  [ -f confirm/imported/$d ] && continue
  ok=0
  for try in 1 2 3; do
    ./sw.sh alpaca-batch --from $d --to $d --jobs 20 --per-minute 8000 >> confirm/logs2/import-$d.log 2>&1
    if tail -3 confirm/logs2/import-$d.log | grep -q ', 0 failed'; then ok=1; break; fi
    echo "retry $d ($try)"
  done
  [ $ok -eq 1 ] && verify $d &
done
wait
echo IMPORT-LOOP-END
