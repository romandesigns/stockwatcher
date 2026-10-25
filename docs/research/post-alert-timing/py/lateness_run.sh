#!/usr/bin/env bash
# RESEARCH ONLY (post-alert-timing protocol §9): run the lateness export per day.
# usage: lateness_run.sh EXE ALERTDIR OUTROOT day...   (archive chosen by date; CONFIRM-PAST
# days come from LAT_ARCHIVE_CONFIRM, default C:\research\stockwatcher\archive-confirm)
EXE=$1; AL=$2; OUT=$3; shift 3
CONF=${LAT_ARCHIVE_CONFIRM:-'C:\research\stockwatcher\archive-confirm'}
for d in "$@"; do
  if [[ "$d" < "2026-03-23" ]]; then R="$CONF"
  elif [[ "$d" < "2026-09-22" ]]; then R='C:\research\stockwatcher\archive-hist'
  else R='C:\research\stockwatcher\archive-vps'; fi
  [ -f "$OUT/$d/done" ] && continue
  mkdir -p "$OUT/$d"
  A=""; [ -f "$AL/$d.jsonl" ] && A="$(cygpath -w "$AL/$d.jsonl")"
  LAT_ROOT="$R" LAT_DAY=$d LAT_OUT="$(cygpath -w "$OUT/$d")" LAT_ALERTS="$A" LAT_THREADS=${LAT_THREADS:-6} \
    "$EXE" running::lateness --ignored --nocapture > "$OUT/$d/log.txt" 2>&1 && touch "$OUT/$d/done"
done
