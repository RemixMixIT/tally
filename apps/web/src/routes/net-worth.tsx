import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { formatGbp } from "@/lib/format";
import { classLabel } from "@/lib/finance";
import type { NetWorthReport } from "@/types/api";
import {
  ResponsiveContainer,
  AreaChart,
  Area,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
} from "recharts";
import { chartColors, chartAxisProps, chartTooltipStyle, chartGradients } from "@/lib/chart-theme";

export const Route = createFileRoute("/net-worth")({ component: NetWorthPage });

function NetWorthPage() {
  const nw = useQuery({
    queryKey: ["net-worth"],
    queryFn: () => api.get<{ net_worth: NetWorthReport }>("/api/net-worth"),
  });
  const expl = useQuery({
    queryKey: ["net-worth-explanation"],
    queryFn: () =>
      api.get<{ net_worth: NetWorthReport; rules: string[] }>("/api/net-worth/explanation"),
  });
  const hist = useQuery({
    queryKey: ["net-worth-history"],
    queryFn: () =>
      api.get<{
        points: { date: string; assets_gbp: number; liabilities_gbp: number; net_worth_gbp: number }[];
      }>("/api/net-worth/history"),
  });

  const r = nw.data?.net_worth;
  const rules = expl.data?.rules ?? [];

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1100px]">
      <header className="fade-in">
        <p className="text-xs uppercase tracking-widest text-mid mb-3">Actuals</p>
        <h1 className="text-4xl md:text-5xl mb-2">
          Net <em>worth</em>
        </h1>
        <p className="text-mid text-sm max-w-2xl">
          Assets minus liabilities: manual valuations, bank and card balances, and the
          investment portfolio. Forecasts, inactive items, and transactions are excluded.
          Bank and broker figures are the latest available balances.
        </p>
      </header>

      <section className="grid md:grid-cols-3 gap-4 fade-in-1">
        <div className="card p-6">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">Net worth</p>
          <p className="font-extrabold text-3xl mono">{formatGbp(r?.net_worth_gbp ?? 0)}</p>
        </div>
        <div className="card p-6">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">Assets</p>
          <p className="font-extrabold text-3xl mono text-green">{formatGbp(r?.assets_gbp ?? 0)}</p>
        </div>
        <div className="card p-6">
          <p className="text-[10px] uppercase tracking-widest text-mid mb-2">Liabilities</p>
          <p className="font-extrabold text-3xl mono text-danger">{formatGbp(r?.liabilities_gbp ?? 0)}</p>
        </div>
      </section>

      {hist.data?.points.length ? (
        <section className="card p-6">
          <p className="text-[11px] text-mid mb-3">
            Chart follows valuation dates. Bank and portfolio balances use the latest figures at
            every point.
          </p>
          <div className="h-64">
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={hist.data.points}>
              <defs>
                <linearGradient id={chartGradients.green.id + "-nw"} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={chartGradients.green.from} stopOpacity={0.35} />
                  <stop offset="100%" stopColor={chartGradients.green.to} stopOpacity={0} />
                </linearGradient>
              </defs>
              <CartesianGrid strokeDasharray="3 3" stroke="rgba(42,31,26,0.08)" />
              <XAxis dataKey="date" {...chartAxisProps} />
              <YAxis {...chartAxisProps} />
              <Tooltip
                formatter={(v: number) => [formatGbp(v), "net worth"]}
                contentStyle={chartTooltipStyle}
              />
              <Area
                type="monotone"
                dataKey="net_worth_gbp"
                stroke={chartColors.green}
                fill={`url(#${chartGradients.green.id}-nw)`}
              />
            </AreaChart>
          </ResponsiveContainer>
          </div>
        </section>
      ) : null}

      <section className="card overflow-x-auto">
        <div className="px-6 pt-5 pb-2">
          <h2 className="text-lg font-semibold">By class</h2>
        </div>
        <table className="w-full text-sm">
          <thead className="bg-cream">
            <tr className="text-[11px] uppercase tracking-widest text-mid">
              <th className="text-left px-4 py-2.5">Class</th>
              <th className="text-right px-4 py-2.5">Assets</th>
              <th className="text-right px-4 py-2.5">Liabilities</th>
            </tr>
          </thead>
          <tbody>
            {(r?.by_class ?? []).map((c) => (
              <tr key={c.class} className="border-b border-thin">
                <td className="px-4 py-2.5">{classLabel(c.class)}</td>
                <td className="px-4 py-2.5 text-right mono">{formatGbp(c.assets_gbp)}</td>
                <td className="px-4 py-2.5 text-right mono">{formatGbp(c.liabilities_gbp)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="grid lg:grid-cols-2 gap-4">
        <div className="card overflow-x-auto">
          <div className="px-6 pt-5 pb-2">
            <h2 className="text-lg font-semibold">Counted</h2>
          </div>
          <ul className="divide-y divide-[rgb(var(--warm-border))]">
            {(r?.counted ?? []).map((line, i) => (
              <li key={`${line.source}-${line.financial_item_id ?? line.name}-${i}`} className="px-6 py-3 text-sm flex justify-between gap-3">
                <div>
                  {line.financial_item_id != null ? (
                    <Link to="/valuations/$id" params={{ id: String(line.financial_item_id) }} className="font-medium hover:text-green">
                      {line.name}
                    </Link>
                  ) : line.source === "bank_or_manual_account" ? (
                    <Link to="/banks" className="font-medium hover:text-green">
                      {line.name}
                    </Link>
                  ) : line.source === "broker_holdings" ? (
                    <Link to="/investments" className="font-medium hover:text-green">
                      {line.name}
                    </Link>
                  ) : (
                    <span className="font-medium">{line.name}</span>
                  )}
                  <p className="text-[11px] text-mid">
                    {line.role} · {line.source}
                    {line.valuation_date ? ` · ${line.valuation_date}` : ""}
                  </p>
                </div>
                <p className="mono font-semibold">{formatGbp(line.gbp_value)}</p>
              </li>
            ))}
          </ul>
        </div>
        <div className="card overflow-x-auto">
          <div className="px-6 pt-5 pb-2">
            <h2 className="text-lg font-semibold">Excluded</h2>
          </div>
          <ul className="divide-y divide-[rgb(var(--warm-border))]">
            {(r?.excluded ?? []).map((line, i) => (
              <li key={i} className="px-6 py-3 text-sm">
                <p className="font-medium">{line.name}</p>
                <p className="text-[11px] text-mid">{line.reason}</p>
              </li>
            ))}
          </ul>
        </div>
      </section>

      {(r?.unreconciled ?? []).length > 0 && (
        <section className="card p-6">
          <h2 className="text-lg font-semibold mb-3">Unreconciled</h2>
          {r!.unreconciled.map((u) => (
            <p key={u.financial_item_id} className="text-sm mb-2">
              {u.message}
            </p>
          ))}
        </section>
      )}

      <section className="card p-6">
        <h2 className="text-lg font-semibold mb-3">How this is calculated</h2>
        <ul className="list-disc pl-5 space-y-1 text-sm text-mid">
          {rules.map((rule) => (
            <li key={rule}>{rule}</li>
          ))}
        </ul>
      </section>
    </div>
  );
}
