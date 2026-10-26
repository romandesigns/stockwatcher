#!/usr/bin/env bash
# Detached driver: one import loop + one processing loop, both idempotent per day (markers in
# confirm/imported and confirm/processed; a day without its marker is redone from scratch-safe steps).
cd /c/research/stockwatcher/work-pat
./import_loop3.sh >> confirm/import_loop5.out 2>&1 &
./process_loop.sh >> confirm/process_loop5.out 2>&1 &
wait
echo RUN-CONFIRM-END >> confirm/process_loop5.out
