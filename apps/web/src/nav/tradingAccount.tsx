import type { BrokerStatus, LiveAccount } from "@stockwatcher/app-state";
import type { SettingsDialogSection } from "@stockwatcher/chart-web";
import { TradingAccountPanel } from "./TradingAccountPanel.tsx";

/**
 * Settings → Trading (owner, 2026-10-29): the live market's account, the simulator or the owner's
 * Alpaca paper account. Synced (app-state `tradingAccount`), so every device trades the same
 * account; the server reports whether Alpaca paper is configured (`broker.status`).
 */
export function tradingAccountSection(account: LiveAccount, status: BrokerStatus, onChange: (a: LiveAccount) => void): SettingsDialogSection {
  return {
    id: "trading",
    label: "Trading",
    icon: "trade",
    description: "Which account the live market's chart orders go to.",
    render: () => <TradingAccountPanel account={account} status={status} onChange={onChange} />,
  };
}
