#!/usr/bin/env bash
# For every imported CONFIRM day: replay the Runners triggers (the pullback study's binary),
# export the per-second series, build quote windows and fetch quotes. No outcome is computed here.
cd /c/research/stockwatcher/work-pat
PY=/c/research/stockwatcher/venv/Scripts/python.exe
P=/h/wavystack/stockwatcher/.claude/worktrees/timing/docs/research/post-alert-timing/py
PAST='C:\research\stockwatcher\archive-past'
while true; do
  left=0
  for d in $(cat confirm_days.txt); do
    [ -f confirm/processed/$d ] && continue
    left=1
    [ -f confirm/imported/$d ] || continue
    mkdir -p trig-confirm/$d
    if [ ! -f trig-confirm/$d/done ]; then
      PAA_ROOT="$PAST" PAA_DAY=$d PAA_OUT="$(cygpath -w $PWD/trig-confirm/$d)" PAA_THREADS=10 ../work-paa/paa2.exe running::paa --ignored --nocapture > trig-confirm/$d/log.txt 2>&1 && touch trig-confirm/$d/done || { echo "trigger replay failed $d"; continue; }
    fi
    rm -rf tmp1; mkdir -p tmp1/$d; cp trig-confirm/$d/triggers.jsonl tmp1/$d/
    $PY -I $P/trig.py syms tmp1 syms-confirm > /dev/null
    ./run_series.sh syms-confirm series-confirm "$PAST" $d
    [ -f series-confirm/$d/done ] || { echo "series failed $d"; continue; }
    rm -f confirm/windows-$d.csv
    $PY -I $P/trig.py windows tmp1 confirm/windows-$d.csv confirm > /dev/null
    AQ_RATE_PER_MIN=1000 $PY -I $P/fetch.py confirm/windows-$d.csv ../quotes-pat/confirm confirm >> confirm/logs2/fetch-$d.log 2>&1
    grep -q '^DONE' confirm/logs2/fetch-$d.log && touch confirm/processed/$d && echo "processed $d" || echo "fetch failed $d"
  done
  [ $left -eq 0 ] && break
  sleep 30
done
echo PROCESS-LOOP-END
