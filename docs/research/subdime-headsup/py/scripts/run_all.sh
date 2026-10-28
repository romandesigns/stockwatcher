#!/usr/bin/env bash
# Sub-dime heads-up export (docs/research/subdime-headsup/protocol.md): every post-2026-03-30 day,
# both variants (prod = $0.10 floor, sub = $0.05 floor), on both roots (the day's in-band archive
# and archive-sd, the A1 sub-dime closes). Idempotent: a finished (variant, root, day) is skipped.
W=/c/research/stockwatcher/work-sd
WT=/h/wavystack/stockwatcher/.claude/worktrees/subdime
cd "$WT" || exit 1
days() {
  ls /c/research/stockwatcher/archive-hist | grep '^2026-' | awk '$0>="2026-03-30"' | sed 's/$/ archive-hist/'
  ls /c/research/stockwatcher/archive-vps | grep -E '^2026-[0-9]{2}-[0-9]{2}$' | sed 's/$/ archive-vps/'
}
# $1 = DEV or TEST (protocol split); TEST runs only after frozen.json is committed.
case "$1" in DEV) LO=2026-03-30; HI=2026-07-31;; TEST) LO=2026-08-03; HI=2026-10-23;; *) echo "usage: run_all.sh DEV|TEST"; exit 2;; esac
days | awk -v lo=$LO -v hi=$HI '$1>=lo && $1<=hi' | while read -r d root; do
  for v in prod sub; do
    F=""; [ "$v" = sub ] && F="--features subdime"
    for r in "$root" archive-sd; do
      out="$W/out/$v/$r/$d"
      [ -f "$out/done" ] && continue
      [ -d "/c/research/stockwatcher/$r/$d" ] || continue
      mkdir -p "$out"
      SD_ROOT="C:/research/stockwatcher/$r" SD_DAY="$d" SD_OUT="C:/research/stockwatcher/work-sd/out/$v/$r/$d" \
        SD_RANDOM=1 SD_THREADS=14 CARGO_TARGET_DIR="$W/target-$v" \
        cargo test -q -p stockwatcher-server --release $F --lib running::subdime -- --ignored --nocapture \
        > "$out/log.txt" 2>&1 && grep -q "done in" "$out/log.txt" && touch "$out/done"
      echo "$v $r $d $( [ -f "$out/done" ] && echo ok || echo FAIL )"
    done
  done
done
echo RUN-ALL-END
