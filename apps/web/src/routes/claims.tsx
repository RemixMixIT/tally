import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Flag } from "lucide-react";
import { ClaimFlagToggles } from "@/components/ClaimFlagToggles";
import { api } from "@/lib/api";
import { formatDate, formatMoney } from "@/lib/format";

export const Route = createFileRoute("/claims")({
  component: ClaimsPage,
});

interface PendingReceipt {
  id: number;
  photo_id: string;
  extracted_amount: number | null;
  extracted_date: number | null;
  status: string;
  linked_transaction_id: number | null;
  claim_flags: string[];
  dual: boolean;
}

function ClaimsPage() {
  const qc = useQueryClient();
  const receipts = useQuery({
    queryKey: ["pending-receipts"],
    queryFn: () =>
      api.get<{ pending_receipts: PendingReceipt[] }>("/api/pending-receipts"),
  });
  const save = useMutation({
    mutationFn: ({ id, claim_flags }: { id: number; claim_flags: string[] }) =>
      api.patch(`/api/pending-receipts/${id}`, { claim_flags }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["pending-receipts"] }),
  });

  const rows = receipts.data?.pending_receipts ?? [];

  return (
    <div className="p-6 max-w-4xl">
      <header className="mb-6">
        <p className="text-[10px] uppercase tracking-widest text-mid mb-1 flex items-center gap-1.5">
          <Flag className="size-3.5" /> Claims
        </p>
        <h1 className="text-2xl font-extrabold tracking-tight">Claim flags</h1>
        <p className="text-sm text-mid mt-2 max-w-xl">
          Mark RemixMix, Client-X, or both. A bank transaction is marked from{" "}
          <Link to="/transactions" className="underline">
            Transactions
          </Link>
          . A receipt that is still waiting for a statement is marked here.
        </p>
      </header>

      {receipts.isLoading && <p className="text-sm text-mid">Loading</p>}
      {receipts.isError && (
        <p className="text-sm text-orange">Could not load pending receipts.</p>
      )}
      {!receipts.isLoading && rows.length === 0 && (
        <div className="card p-5 text-sm text-mid">
          No pending receipts yet. When one is waiting, its flags go here.
        </div>
      )}

      <ul className="space-y-3">
        {rows.map((row) => (
          <li key={row.id} className="card p-4">
            <div className="flex flex-wrap items-baseline justify-between gap-2 mb-3">
              <div>
                <p className="font-semibold">Receipt #{row.id}</p>
                <p className="text-xs text-mid mono">{row.photo_id}</p>
              </div>
              <p className="text-sm">
                <span className="pill-green">{row.status}</span>
                {row.extracted_amount != null && (
                  <span className="mono ml-2">
                    {formatMoney(row.extracted_amount, "GBP")}
                  </span>
                )}
                {row.extracted_date != null && (
                  <span className="text-mid ml-2">{formatDate(row.extracted_date)}</span>
                )}
              </p>
            </div>
            <ClaimFlagToggles
              flags={row.claim_flags}
              disabled={save.isPending}
              onChange={(claim_flags) => save.mutate({ id: row.id, claim_flags })}
            />
            {row.linked_transaction_id != null && (
              <p className="text-xs text-mid mt-2">
                Linked to transaction #{row.linked_transaction_id}
              </p>
            )}
          </li>
        ))}
      </ul>
      {save.isError && (
        <p className="text-xs text-orange mt-3">Could not save those flags.</p>
      )}
      <ClaimPacks />
    </div>
  );
}

interface PackLine {
  source_kind: string;
  source_id: number;
  amount: string;
  date: string;
  supplier: string;
  category: string;
  receipt_link: string;
}

interface ClaimPackRow {
  id: string;
  party: string;
  total: string;
  lines: PackLine[];
}

function ClaimPacks() {
  const qc = useQueryClient();
  const packs = useQuery({
    queryKey: ["claim-packs"],
    queryFn: () =>
      api.get<{ packs: ClaimPackRow[]; source_total: string }>("/api/claim-packs"),
  });
  const generate = useMutation({
    mutationFn: () =>
      api.post<{ packs: ClaimPackRow[]; source_total: string }>("/api/claim-packs"),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["claim-packs"] }),
  });
  const rows = packs.data?.packs ?? [];

  return (
    <section className="mt-10">
      <div className="flex flex-wrap items-end justify-between gap-3 mb-4">
        <div>
          <h2 className="text-lg font-extrabold">Claim packs</h2>
          <p className="text-sm text-mid mt-1 max-w-xl">
            Prepares amount, date, supplier, category and the receipt link.
            RemixMix is one pack. Client-X is another. A dual claim is on both,
            with two pack ids, and the source total still counts it once.
          </p>
        </div>
        <button
          type="button"
          className="btn-cta"
          disabled={generate.isPending}
          onClick={() => generate.mutate()}
        >
          {generate.isPending ? "Preparing" : "Prepare packs"}
        </button>
      </div>
      {packs.data && (
        <p className="text-sm mb-4">
          Source total {packs.data.source_total}. This is each flagged item once,
          not the two pack totals added together.
        </p>
      )}
      {generate.isError && (
        <p className="text-sm text-orange mb-3">Could not prepare the packs.</p>
      )}
      {!packs.isLoading && rows.length === 0 && (
        <div className="card p-5 text-sm text-mid">
          No packs yet. Mark RemixMix, Client-X, or both, then prepare packs.
        </div>
      )}
      <ul className="space-y-4">
        {rows.map((pack) => (
          <li key={pack.id} className="card p-4">
            <div className="flex flex-wrap items-baseline justify-between gap-2 mb-3">
              <div>
                <span className={pack.party === "RemixMix" ? "pill-green" : "pill-orange"}>
                  {pack.party}
                </span>
                <p className="text-xs text-mid mono mt-2">{pack.id}</p>
              </div>
              <p className="font-semibold mono">{pack.total}</p>
            </div>
            <p className="text-xs mb-3">
              <a className="underline" href={`/api/claim-packs/${encodeURIComponent(pack.id)}?format=csv`}>
                CSV
              </a>
              <a className="underline ml-3" href={`/api/claim-packs/${encodeURIComponent(pack.id)}?format=md`}>
                Markdown
              </a>
            </p>
            <table className="w-full text-sm">
              <thead>
                <tr className="text-left text-xs text-mid">
                  <th className="py-1 pr-2">Amount</th>
                  <th className="py-1 pr-2">Date</th>
                  <th className="py-1 pr-2">Supplier</th>
                  <th className="py-1 pr-2">Category</th>
                  <th className="py-1">Receipt</th>
                </tr>
              </thead>
              <tbody>
                {pack.lines.map((line) => (
                  <tr key={`${line.source_kind}-${line.source_id}`}>
                    <td className="py-1 pr-2 mono">{line.amount}</td>
                    <td className="py-1 pr-2">{line.date}</td>
                    <td className="py-1 pr-2">{line.supplier}</td>
                    <td className="py-1 pr-2">{line.category}</td>
                    <td className="py-1 mono text-xs">{line.receipt_link}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </li>
        ))}
      </ul>
    </section>
  );
}
