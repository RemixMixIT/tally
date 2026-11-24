import { createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { formatGbp } from "@/lib/format";
import type {
  ExpenditureCountedLine,
  ExpenditureReport,
  NamedAmount,
} from "@/types/api";
import {
  ResponsiveContainer,
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  CartesianGrid,
  Cell,
} from "recharts";
import {
  chartAxisProps,
  chartTooltipStyle,
  categoricalPalette,
} from "@/lib/chart-theme";

export const Route = createFileRoute("/expenditure")({
  component: ExpenditurePage,
});

function ExpenditurePage() {
  const q = useQuery({
    queryKey: ["expenditure"],
    queryFn: () => api.get<{ expenditure: ExpenditureReport }>("/api/expenditure"),
  });
  const r = q.data?.expenditure;
  const catHeight = Math.max(280, (r?.categories.length ?? 0) * 32 + 40);

  return (
    <div className="p-8 md:p-12 space-y-8 max-w-[1100px]">
      <header className="fade-in">
        <p className="text-xs uppercase tracking-widest text-mid mb-3">Personal</p>
        <h1 className="text-4xl md:text-5xl mb-2">
          <em>Expenditure</em>
        </h1>
        <p className="text-mid text-sm max-w-2xl">
          {r?.caption ?? "Personal only. Loading complete-month averages…"}
        </p>
      </header>

      <section className="card p-6 fade-in-1">
        <p className="text-[10px] uppercase tracking-widest text-mid mb-2">
          £ per month
        </p>
        <p className="font-extrabold text-4xl md:text-5xl mono tracking-tight">
          {formatGbp(r?.total_gbp_per_month ?? 0)}
        </p>
        {r && r.month_count > 0 && (
          <p className="text-xs text-mid mt-2">
            Average of {r.month_count} complete month
            {r.month_count === 1 ? "" : "s"}
            {r.is_rolling_12 ? " · 12-month rolling window" : ""}
          </p>
        )}
      </section>

      {q.isError && (
        <p className="text-sm text-danger">Could not load expenditure.</p>
      )}

      {r && r.month_count > 0 && (
        <>
          <section className="card p-6 fade-in-2">
            <h2 className="text-lg font-semibold mb-1">Groups</h2>
            <p className="text-[11px] text-mid mb-4">
              Household, leisure, car, and travel. Every included category maps to one group.
            </p>
            <SpendBars data={r.groups} height={220} yWidth={168} />
          </section>

          <section className="card p-6 fade-in-3">
            <h2 className="text-lg font-semibold mb-1">Categories</h2>
            <p className="text-[11px] text-mid mb-4">
              Named leaves plus Insurance and Mortgage. Remaining household spend is Household
              bills. Bars sum to the grouped total.
            </p>
            <SpendBars data={r.categories} height={catHeight} yWidth={140} />
          </section>
        </>
      )}

      {r && r.skipped_fx.length > 0 && (
        <p className="text-sm text-mid">
          Skipped non-USD foreign currency:{" "}
          {r.skipped_fx
            .map(
              (f) =>
                `${f.currency} (${f.transaction_count} txn${f.transaction_count === 1 ? "" : "s"})`
            )
            .join(", ")}
          .
        </p>
      )}

      {r && (
        <section className="grid lg:grid-cols-2 gap-4">
          <div className="card overflow-x-auto max-h-[640px] overflow-y-auto">
            <div className="px-6 pt-5 pb-2 sticky top-0 bg-card">
              <h2 className="text-lg font-semibold">Counted</h2>
              <p className="text-[11px] text-mid">
                In the average. Accounts first, then Tally categories (leaf names).
              </p>
            </div>
            <CountedList lines={r.counted ?? []} />
          </div>
          <div className="card overflow-x-auto max-h-[640px] overflow-y-auto">
            <div className="px-6 pt-5 pb-2 sticky top-0 bg-card">
              <h2 className="text-lg font-semibold">Excluded</h2>
              <p className="text-[11px] text-mid">
                Left out of the average, with the reason. Amounts in the window are £/month;
                incomplete months are the £ that would have been counted that period.
              </p>
            </div>
            {(r.excluded ?? []).length ? (
              <ul className="divide-y divide-[rgb(var(--warm-border))]">
                {r.excluded.map((line, i) => (
                  <li
                    key={`${line.name}-${line.reason}-${i}`}
                    className="px-6 py-3 text-sm flex justify-between gap-3"
                  >
                    <div>
                      <p className="font-medium">{line.name}</p>
                      <p className="text-[11px] text-mid">
                        {line.reason}
                        {line.transaction_count
                          ? ` · ${line.transaction_count} txn${line.transaction_count === 1 ? "" : "s"}`
                          : ""}
                      </p>
                    </div>
                    {line.gbp_value != null && (
                      <p className="mono font-semibold shrink-0">
                        {formatGbp(line.gbp_value)}
                        {line.per_month ? "/month" : ""}
                      </p>
                    )}
                  </li>
                ))}
              </ul>
            ) : (
              <p className="px-6 py-3 text-sm text-mid">Nothing excluded.</p>
            )}
          </div>
        </section>
      )}
    </div>
  );
}

function CountedList({ lines }: { lines: ExpenditureCountedLine[] }) {
  const accounts = lines.filter((l) => l.detail.startsWith("Account"));
  const cats = lines.filter((l) => !l.detail.startsWith("Account"));
  if (!lines.length) {
    return <p className="px-6 py-3 text-sm text-mid">Nothing counted yet.</p>;
  }
  return (
    <>
      {accounts.length > 0 && (
        <>
          <p className="px-6 pt-2 text-[11px] uppercase tracking-widest text-mid">
            Accounts
          </p>
          <SenseRows lines={accounts} />
        </>
      )}
      {cats.length > 0 && (
        <>
          <p className="px-6 pt-3 text-[11px] uppercase tracking-widest text-mid">
            Categories
          </p>
          <SenseRows lines={cats} />
        </>
      )}
    </>
  );
}

function SenseRows({ lines }: { lines: ExpenditureCountedLine[] }) {
  return (
    <ul className="divide-y divide-[rgb(var(--warm-border))]">
      {lines.map((line, i) => (
        <li
          key={`${line.name}-${line.detail}-${i}`}
          className="px-6 py-3 text-sm flex justify-between gap-3"
        >
          <div>
            <p className="font-medium">{line.name}</p>
            <p className="text-[11px] text-mid">{line.detail}</p>
          </div>
          <p className="mono font-semibold shrink-0">
            {formatGbp(line.gbp_per_month)}/month
          </p>
        </li>
      ))}
    </ul>
  );
}

function SpendBars({
  data,
  height,
  yWidth,
}: {
  data: NamedAmount[];
  height: number;
  yWidth: number;
}) {
  return (
    <div style={{ height }}>
      <ResponsiveContainer width="100%" height="100%">
        <BarChart
          data={data}
          layout="vertical"
          margin={{ top: 4, right: 16, left: 4, bottom: 4 }}
        >
          <CartesianGrid strokeDasharray="3 3" stroke="rgba(42,31,26,0.08)" />
          <XAxis
            type="number"
            {...chartAxisProps}
            tickFormatter={(v: number) =>
              `£${Math.round(v).toLocaleString("en-GB")}`
            }
          />
          <YAxis
            type="category"
            dataKey="name"
            width={yWidth}
            {...chartAxisProps}
          />
          <Tooltip
            formatter={(v: number) => [formatGbp(v), "£/month"]}
            contentStyle={chartTooltipStyle}
            cursor={{ fill: "rgba(80, 94, 77, 0.06)" }}
          />
          <Bar dataKey="gbp_per_month" maxBarSize={28}>
            {data.map((_, i) => (
              <Cell
                key={i}
                fill={categoricalPalette[i % categoricalPalette.length]}
              />
            ))}
          </Bar>
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}
