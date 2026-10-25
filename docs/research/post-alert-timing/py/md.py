"""RESEARCH ONLY: render agg.py JSON tables as markdown.  python -I md.py TABLES_JSON OUT_MD"""
from __future__ import annotations

import json
import sys


def f(x):
    if x is None:
        return "—"
    if isinstance(x, list):
        if x[0] is None:
            return "—"
        return f"{x[0]:+.2f} [{x[1]:+.2f}, {x[2]:+.2f}]"
    if isinstance(x, float):
        return f"{x:.3f}"
    return str(x)


def main():
    d = json.load(open(sys.argv[1]))
    W = d["frozen"]["winner"]
    L = [f"# {sys.argv[1]}", "", f"alerts {d['all_alerts']}, eligible {d['eligible_alerts']}, "
         f"excluded (no quote in window) {d['excluded_no_quotes']}; frozen dynamic = {W}", ""]
    L += ["## Strategies (P1, per-alert and per-trade net %, 95% day-clustered CI)", "",
          "| strategy | trades | coverage | per-alert net | per-trade net | win/loss/timeout | med delay s | med half-spread (quoted) | both fills quoted | B0 on skipped |",
          "|---|---|---|---|---|---|---|---|---|---|"]
    for st, s in d["strategies"].items():
        L.append(f"| {st} | {s['trades']} | {s['coverage']:.1%} | {f(s['per_alert_net'])} | {f(s['per_trade_net'])} | "
                 f"{s['win']:.1%}/{s['loss']:.1%}/{s['timeout']:.1%} | {f(s['median_delay_s'])} | {f(s['median_entry_half_spread_q'])} | "
                 f"{s['fills_both_quoted']:.1%} | {f(s['b0_on_skipped_mean'])} ({s['skipped']}) |")
    L += ["", "## Paired per-alert Δ (pp)", "", "| strategy | vs B0 primary | vs B0 quote-complete (n) | vs B0 gross (price) | vs D8 primary | vs D8 quote-complete |", "|---|---|---|---|---|---|"]
    for st in d["strategies"]:
        a = d["delta_vs_B0"].get(st)
        b = d["delta_vs_D8"].get(st)
        L.append(f"| {st} | {f(a['primary']['delta_pp']) if a else '—'} | "
                 f"{(f(a['quote_complete']['delta_pp']) + ' (' + str(a['quote_complete']['n']) + ')') if a else '—'} | "
                 f"{f(a['gross']['delta_pp']) if a else '—'} | {f(b['primary']['delta_pp']) if b else '—'} | "
                 f"{f(b['quote_complete']['delta_pp']) if b else '—'} |")
    L += ["", "## Matched per-trade Δ (pp; only alerts the strategy entered: timing without abstention)", "",
          "| strategy | n | vs B0 net | vs B0 gross | vs D8 net | vs D8 gross |", "|---|---|---|---|---|---|"]
    for st, m in d.get("matched_per_trade", {}).items():
        L.append(f"| {st} | {m['vs_B0']['n']} | {f(m['vs_B0']['net_pp'])} | {f(m['vs_B0']['gross_pp'])} | {f(m['vs_D8']['net_pp'])} | {f(m['vs_D8']['gross_pp'])} |")
    L += ["", "## Segments", "", "| segment | alerts | B0 per-alert | D8 per-alert | D8−B0 | D8−B0 gross | " + W + "−B0 | share of waiting gain |", "|---|---|---|---|---|---|---|---|"]
    for sg, e in d["segments"].items():
        L.append(f"| {sg}{' (thin)' if e['thin'] else ''} | {e['alerts']} | {f(e['B0']['per_alert_net'])} | {f(e['D8']['per_alert_net'])} | "
                 f"{f(e['D8']['delta_vs_B0'])} | {f(e['D8']['gross_delta_vs_B0'])} | {f(e[W]['delta_vs_B0'])} | {f(d['waiting_gain_share_by_segment'].get(sg))} |")
    L.append(f"\nD8−B0 without open_print: {f(d['waiting_gain_without_open_print']['delta_pp'])} (n {d['waiting_gain_without_open_print']['n']})")
    for name in [n for n in ("paths", "bands", "event_types") if n in d]:
        L += ["", f"## {name}", "", f"| {name} | alerts | B0 per-alert | D8−B0 | {W}−B0 | {W}−D8 approx |", "|---|---|---|---|---|---|"]
        for g, e in d[name].items():
            L.append(f"| {g}{' (thin)' if e['thin'] else ''} | {e['alerts']} | {f(e['B0']['per_alert_net'])} | {f(e['D8']['delta_vs_B0'])} | {f(e[W]['delta_vs_B0'])} | — |")
    if "decomposition_vs_D8" in d:
        L += ["", "## Per-alert Δ vs D8 split into timing and skip (A3.1)", "",
              "| strategy | per-alert | timing | skip | skip share | random-skip expected [band] | filter beyond random | skip on imputed D8 entry |", "|---|---|---|---|---|---|---|---|"]
        for st, x in d["decomposition_vs_D8"].items():
            L.append(f"| {st} | {x['per_alert_delta_pp']} | {x['timing_component_pp']} | {x['skip_component_pp']} | {x['skip_share']:.1%} | {x['random_skip_expected_pp']} {x['random_skip_band_pp']} | {x['filter_value_beyond_random_pp']} | {x['skip_component_on_imputed_ref_entry_pp']} |")
    L += ["", "## Robustness", "", "```", json.dumps({"quarters": d["quarters"], "drop_top1pct": d["drop_top1pct"],
                                                    "halts_excluded": d["halts_excluded"], "sibling": d["sibling"]}, indent=1), "```"]
    L += ["", "## Verdicts", "", "```", json.dumps(d["verdicts"], indent=1), "```", "",
          "## H0 per-trade net", "", "```", json.dumps({"per_trade": d["H0_per_trade_net"], "profitable": d["H0_profitable"]}, indent=1), "```"]
    open(sys.argv[2], "w", encoding="utf-8").write("\n".join(L) + "\n")


if __name__ == "__main__":
    main()
