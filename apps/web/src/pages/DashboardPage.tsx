/**
 * The main page (owner request 2026-10-18). A placeholder until the live dashboard is built: it
 * will consume only live data (docs/CONVENTIONS.md "Product direction"; the constraints to plan
 * around are in docs/architecture/chart-packages.md).
 */
export function DashboardPage() {
  return (
    <main className="flex min-h-full flex-col gap-1 p-4 text-foreground">
      <h1 className="m-0 text-xl font-semibold tracking-tight">Dashboard</h1>
      <p className="m-0 text-sm text-muted-foreground">Main page. The live market dashboard will be built here.</p>
    </main>
  );
}
