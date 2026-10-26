#!/bin/bash
# Loads ONLY the Alpaca key names from the repo .env (values never printed), then runs swimport.
while IFS='=' read -r k v; do
  k="${k#export }"; k="$(echo "$k" | tr -d ' ')"
  case "$k" in ALPACA_API_KEY|ALPACA_API_SECRET) v="${v%\"}"; v="${v#\"}"; v="${v%\'}"; v="${v#\'}"; v="$(echo "$v" | tr -d '\r')"; export "$k=$v";; esac
done < /h/wavystack/stockwatcher/.env
export ARCHIVE_ROOT='C:\research\stockwatcher\archive-past' PRICE_BAND_MIN=0.10 PRICE_BAND_MAX=25.00
exec /c/research/stockwatcher/work-pat/target/release/swimport.exe "$@"
