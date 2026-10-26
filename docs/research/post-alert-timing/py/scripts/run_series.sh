#!/usr/bin/env bash
# usage: run_series.sh SYMS_DIR OUT_ROOT ARCHIVE_ROOT_FOR_PAST day...   (archive chosen by date)
SD=$1; OUT=$2; PAST=$3; shift 3
for d in "$@"; do
  if [[ "$d" < "2026-03-23" ]]; then R="$PAST"; elif [[ "$d" < "2026-09-22" ]]; then R='C:\research\stockwatcher\archive-hist'; else R='C:\research\stockwatcher\archive-vps'; fi
  [ -f "$OUT/$d/done" ] && continue
  mkdir -p "$OUT/$d"
  S=$(cat "$SD/$d.txt"); [ -z "$S" ] && { touch "$OUT/$d/done"; continue; }
  PAT_ROOT="$R" PAT_DAY=$d PAT_OUT="$(cygpath -w $OUT/$d)" PAT_THREADS=6 PAT_SYMS="$S" ./pat.exe running::pat --ignored --nocapture > "$OUT/$d/log.txt" 2>&1 && touch "$OUT/$d/done"
done
