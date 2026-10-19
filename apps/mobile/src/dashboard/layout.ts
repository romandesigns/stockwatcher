/**
 * How many charts this screen can show (owner, 2026-10-18, on a Galaxy Z Fold): a phone-size
 * screen (the Fold's cover screen, any ordinary phone) has room for ONE readable chart, a
 * tablet-size one (the Fold unfolded) for two; four is the web's. Tablet-size is Android's own
 * rule, a smallest side of at least 600 dp (sw600dp), so it does not change on rotation.
 */
export function maxChartsFor(width: number, height: number): number {
  return Math.min(width, height) >= 600 ? 2 : 1;
}
