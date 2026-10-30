import type { BrokerStatus, LiveAccount } from "@stockwatcher/app-state";
import type { SettingsSheetSection } from "@stockwatcher/chart-native";

import { TradingAccountPanel } from "./TradingAccountPanel";

/**
 * Settings → Trading (owner, 2026-10-29): the live market's account, the simulator or the owner's
 * Alpaca paper account. Synced (app-state `tradingAccount`), the web's Trading tab too.
 */
export function tradingAccountSection(account: LiveAccount, status: BrokerStatus, onChange: (a: LiveAccount) => void): SettingsSheetSection {
  return {
    id: "trading",
    label: "Trading",
    icon: "trade",
    description: "Which account the live market's chart orders go to.",
    render: () => <TradingAccountPanel account={account} status={status} onChange={onChange} />,
  };
}
