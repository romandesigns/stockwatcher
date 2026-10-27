/** Synthetic measured evidence, shared by client acceptance tests. */
export const breakoutTestFixture = {
  v: 1, rule: "post_flag_exact_high_v1", policy_id: "rocket-momentum-15s-exact-after-v1",
  timeframe_secs: 15, event_time_basis: "bar_end", volume_scope: "whole_15s_bar",
  source: "accepted_original_ohlc", exact_state: "complete",
  bar_start_ms: 1793017410000, bar_end_ms: 1793017425000, event_at_ms: 1793017425000,
  evaluated_at_ms: 1793017425250, flag_at_ms: 1793017380000, flag_known_at_ms: 1793017381000,
  high_exec_ms: 1793017420000, reference_known_at_ms: 1793017381000,
  reference_basis: "flag_price", reference_bar_start_ms: null, reference_bar_end_ms: null,
  reference_price_micros: 2030000, high_price_micros: 2040001, close_price_micros: 2020000,
  high_receive_ms: 1793017420050,
  flag_known_ns: "1793017381000000000", evaluated_ns: "1793017425250000000",
  high_exec_ns: "1793017420000000001", high_receive_ns: "1793017420050000000",
  reference_known_ns: "1793017381000000000",
  flag_creation_admission: "10", decision_admission: "30", high_admission: "25",
  volume: 800, pace_slot: 300, required_multiple: 2, volume_ratio: 800 / 300,
  zero_base_fallback: false,
} as const;
