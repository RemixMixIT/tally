import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { formatGbp } from "@/lib/format";
import type { RetirementReport, RetirementTrackerPoint } from "@/types/api";
import {
  ResponsiveContainer,
  LineChart,
  Line,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
  Legend,
  ReferenceLine,
} from "recharts";
import { chartAxisProps, chartTooltipStyle, chartColors } from "@/lib/chart-theme";

export const Route = createFileRoute("/retirement")({ component: RetirementPage });

function dashGbp(n: number | null | undefined): string {
  return n == null ? "—" : formatGbp(n);
}

function impliedAnnual(pnl: number | null, capital: number | null): string {
  if (pnl == null || capital == null || capital === 0) return "—";
  return `${(((pnl / capital) * 12) * 100).toFixed(0)}%`;
}

function planRate(r: number | null | undefined): string {
  return r == null ? "—" : `${Math.round(r * 100)}%`;
}

function axisGbp(n: number): string {
  if (Math.abs(n) >= 1000) return `£${(n / 1000).toFixed(n % 1000 === 0 ? 0 : 1)}k`;
  return `£${Math.round(n)}`;
}

function pathTick(date: string): string {
  const [y, m, d] = date.split("-").map(Number);
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  if (y === 2026 && m === 9 && d === 7) return "Start";
  return `${months[m - 1]} ${String(y).slice(2)}`;
}

function tooltipGbp(value: number | string | undefined, name: string) {
  if (value == null || value === "") return ["—", name];
  return [formatGbp(Number(value)), name];
}

function RetirementPage() {
  const q = useQuery({
    queryKey: ["retirement"],
    queryFn: () => api.get<{ retirement: RetirementReport }>("/api/retirement"),
  });
  const r = q.data?.retirement;

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1100px]">
      <header className="fade-in">
        <p className="text-xs uppercase tracking-widest text-mid mb-3">2030 test</p>
        <h1 className="text-4xl md:text-5xl mb-2">
          <em>Retirement</em>
        </h1>
        <p className="text-mid text-sm max-w-2xl">
          Trailing six-month average net P&amp;L across SIPP, Global Prime, and ISA versus the
          path to £9,000 a month by December 2030. Deposits are not profit. Implied is this
          month annualised against the SIPP ~13% and FX ~30% planning rates. Forecasts on the
          other page compound a pot — that is a different model.
        </p>
      </header>

      {q.isError && (
        <p className="text-sm text-danger">Could not load the retirement report.</p>
      )}

      <section className="grid md:grid-cols-4 gap-4 fade-in-1">
        <div className="card p-6">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">TMS capital</p>
          <p className="font-extrabold text-3xl mono">{dashGbp(r?.tms_capital_gbp)}</p>
        </div>
        <div className="card p-6">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">This month P&amp;L</p>
          <p className="font-extrabold text-3xl mono">{dashGbp(r?.this_month_pnl_gbp)}</p>
          {r && !r.this_month_cashflow_complete && (
            <p className="text-[11px] text-mid mt-2">Cashflow incomplete — not yet adjusted.</p>
          )}
        </div>
        <div className="card p-6">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">Trailing 6m avg</p>
          <p className="font-extrabold text-3xl mono">{dashGbp(r?.trailing_6m_avg_gbp)}</p>
          <p className="text-[11px] text-mid mt-2">
            {r?.trailing_months_ready ?? 0} of 6 complete months
          </p>
        </div>
        <div className="card p-6">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">Path today</p>
          <p className="font-extrabold text-3xl mono">{dashGbp(r?.path_target_gbp)}</p>
          <p className="text-[11px] text-mid mt-2">
            Off-track below {dashGbp(r?.off_track_below_gbp)} (70%)
            {r?.on_track == null ? "" : r.on_track ? " · on track" : " · off track"}
          </p>
        </div>
      </section>

      {r?.tracker.length ? <TrackerChart asOf={r.as_of} points={r.tracker} /> : null}

      <section className="card overflow-x-auto">
        <table className="w-full text-sm min-w-[720px]">
          <thead className="bg-cream">
            <tr className="text-[11px] uppercase tracking-widest text-mid">
              <th className="text-left px-4 py-2.5">Book</th>
              <th className="text-right px-4 py-2.5">Capital</th>
              <th className="text-right px-4 py-2.5">Equity Δ</th>
              <th className="text-right px-4 py-2.5">In</th>
              <th className="text-right px-4 py-2.5">Out</th>
              <th className="text-right px-4 py-2.5">P&amp;L</th>
              <th className="text-right px-4 py-2.5">Implied</th>
              <th className="text-right px-4 py-2.5">Plan</th>
            </tr>
          </thead>
          <tbody>
            {(r?.books ?? []).map((b) => (
              <tr key={b.id} className="border-t border-thin">
                <td className="px-4 py-3">
                  <Link
                    to="/valuations/$id"
                    params={{ id: String(b.id) }}
                    className="font-medium hover:text-green"
                  >
                    {b.name}
                  </Link>
                  <p className="text-[11px] text-mid">{b.valuation_date ?? "no valuation"}</p>
                </td>
                <td className="px-4 py-3 text-right mono">{dashGbp(b.capital_gbp)}</td>
                <td className="px-4 py-3 text-right mono">
                  {dashGbp(b.this_month.equity_change_gbp)}
                </td>
                <td className="px-4 py-3 text-right mono">
                  {dashGbp(b.this_month.contributions_gbp)}
                </td>
                <td className="px-4 py-3 text-right mono">
                  {dashGbp(b.this_month.withdrawals_gbp)}
                </td>
                <td className="px-4 py-3 text-right mono font-semibold">
                  {dashGbp(b.this_month.pnl_gbp)}
                </td>
                <td className="px-4 py-3 text-right mono text-mid">
                  {impliedAnnual(b.this_month.pnl_gbp, b.capital_gbp)}
                </td>
                <td className="px-4 py-3 text-right text-mid">{planRate(b.planning_annual_return)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      {(r?.months ?? []).length > 0 && (
        <section className="card overflow-x-auto">
          <p className="text-[11px] uppercase tracking-widest text-mid px-4 pt-4">Months</p>
          <table className="w-full text-sm min-w-[520px]">
            <thead>
              <tr className="text-[11px] uppercase tracking-widest text-mid">
                <th className="text-left px-4 py-2.5">Month</th>
                <th className="text-right px-4 py-2.5">Equity Δ</th>
                <th className="text-right px-4 py-2.5">Net P&amp;L</th>
                <th className="text-left px-4 py-2.5">Cashflow</th>
              </tr>
            </thead>
            <tbody>
              {r?.months.map((m) => (
                <tr key={m.year_month} className="border-t border-thin">
                  <td className="px-4 py-2.5 mono">{m.year_month}</td>
                  <td className="px-4 py-2.5 text-right mono">{dashGbp(m.equity_change_gbp)}</td>
                  <td className="px-4 py-2.5 text-right mono">{dashGbp(m.pnl_gbp)}</td>
                  <td className="px-4 py-2.5 text-mid text-xs">
                    {m.cashflow_complete ? "complete" : "incomplete"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {r?.notes.length ? (
        <ul className="text-sm text-mid space-y-1">
          {r.notes.map((n) => (
            <li key={n}>· {n}</li>
          ))}
        </ul>
      ) : null}

      <p className="text-xs text-mid">
        Pot compounding lives on{" "}
        <Link to="/forecasts" className="text-green">
          forecasts
        </Link>
        . Valuations on{" "}
        <Link to="/valuations" className="text-green">
          assets
        </Link>
        .
      </p>
    </div>
  );
}

function TrackerChart({
  asOf,
  points,
}: {
  asOf: string;
  points: RetirementTrackerPoint[];
}) {
  const ticks = points.filter((p) => p.is_checkpoint).map((p) => p.date);
  const checkpoints = points.filter((p) => p.is_checkpoint);

  return (
    <section className="card p-6 space-y-6">
      <div>
        <h2 className="text-lg font-semibold mb-1">Tracker</h2>
        <p className="text-[11px] text-mid max-w-2xl">
          Wiki path from £0 in September 2026 to £9,000 a month by December 2030. Month P&amp;L
          and the trailing six-month average (the success test) fill in as cashflow months
          close. Off-track is 70% of the path.
        </p>
      </div>
      <div className="h-80">
        <ResponsiveContainer width="100%" height="100%">
          <LineChart data={points} margin={{ top: 8, right: 12, left: 4, bottom: 0 }}>
            <CartesianGrid strokeDasharray="3 3" stroke="rgba(42,31,26,0.08)" />
            <XAxis
              dataKey="date"
              ticks={ticks}
              tickFormatter={pathTick}
              {...chartAxisProps}
            />
            <YAxis
              {...chartAxisProps}
              tickFormatter={axisGbp}
              width={52}
              domain={[
                (min: number) => Math.min(0, min),
                (max: number) => Math.max(9000, max),
              ]}
            />
            <Tooltip
              formatter={tooltipGbp}
              labelFormatter={(label) => pathTick(String(label))}
              contentStyle={chartTooltipStyle}
            />
            <Legend wrapperStyle={{ fontSize: 12, fontFamily: "Inter" }} />
            <ReferenceLine
              x={asOf}
              stroke={chartColors.ink}
              strokeDasharray="2 4"
              label={{ value: "now", fill: chartColors.muted, fontSize: 11, position: "insideTopLeft" }}
            />
            <Line
              type="monotone"
              dataKey="estimate_gbp"
              name="Estimate"
              stroke={chartColors.green}
              strokeWidth={2}
              dot={false}
            />
            <Line
              type="monotone"
              dataKey="off_track_below_gbp"
              name="Off-track (70%)"
              stroke={chartColors.muted}
              strokeDasharray="4 4"
              strokeWidth={1.5}
              dot={false}
            />
            <Line
              type="monotone"
              dataKey="month_pnl_gbp"
              name="Month P&L"
              stroke={chartColors.orange}
              strokeWidth={2}
              connectNulls={false}
              dot={{ r: 3, fill: chartColors.orange }}
            />
            <Line
              type="monotone"
              dataKey="trailing_6m_gbp"
              name="Trailing 6m"
              stroke={chartColors.ink}
              strokeWidth={2}
              connectNulls={false}
              dot={{ r: 3, fill: chartColors.ink }}
            />
          </LineChart>
        </ResponsiveContainer>
      </div>
      <div className="overflow-x-auto">
        <p className="text-[11px] uppercase tracking-widest text-mid mb-2">Checkpoints</p>
        <table className="w-full text-sm min-w-[640px]">
          <thead>
            <tr className="text-[11px] uppercase tracking-widest text-mid">
              <th className="text-left py-2 pr-4">When</th>
              <th className="text-right py-2 px-4">Estimate</th>
              <th className="text-right py-2 px-4">Off-track below</th>
              <th className="text-right py-2 px-4">Month P&amp;L</th>
              <th className="text-right py-2 pl-4">Trailing 6m</th>
            </tr>
          </thead>
          <tbody>
            {checkpoints.map((p) => (
              <tr key={p.date} className="border-t border-thin">
                <td className="py-2 pr-4 mono text-xs">{p.date}</td>
                <td className="py-2 px-4 text-right mono">{formatGbp(p.estimate_gbp)}</td>
                <td className="py-2 px-4 text-right mono text-mid">
                  {formatGbp(p.off_track_below_gbp)}
                </td>
                <td className="py-2 px-4 text-right mono">{dashGbp(p.month_pnl_gbp)}</td>
                <td className="py-2 pl-4 text-right mono">{dashGbp(p.trailing_6m_gbp)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
