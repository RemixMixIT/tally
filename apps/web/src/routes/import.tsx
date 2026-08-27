import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { api } from "@/lib/api";
import type { Account } from "@/types/api";

export const Route = createFileRoute("/import")({ component: ImportCsvPage });

type Preview = {
  headers: string[];
  sample_rows: string[][];
  row_count: number;
  suggested: {
    date_column: string | null;
    description_column: string | null;
    amount_column: string | null;
    credit_column: string | null;
    debit_column: string | null;
  };
};

const DATE_FORMATS: { label: string; value: string }[] = [
  { label: "Auto-detect", value: "" },
  { label: "YYYY-MM-DD", value: "%Y-%m-%d" },
  { label: "DD/MM/YYYY", value: "%d/%m/%Y" },
  { label: "MM/DD/YYYY", value: "%m/%d/%Y" },
  { label: "DD-MM-YYYY", value: "%d-%m-%Y" },
  { label: "DD Mon YYYY", value: "%d %b %Y" },
];

function accountLabel(a: Account) {
  const name = a.custom_display_name ?? a.display_name;
  const bank = a.consent_nickname ?? "manual";
  return `${name} (${bank})`;
}

function ImportCsvPage() {
  const accounts = useQuery({ queryKey: ["accounts"], queryFn: () => api.get<{ accounts: Account[] }>("/api/accounts") });
  const [content, setContent] = useState("");
  const [preview, setPreview] = useState<Preview | null>(null);
  const [mapping, setMapping] = useState({
    account_id: 0,
    date_column: "",
    description_column: "",
    amount_column: "",
    credit_column: "",
    debit_column: "",
    date_format: "",
  });

  const previewMut = useMutation({
    mutationFn: (text: string) => api.post<Preview>("/api/csv/preview", { content: text }),
    onSuccess: (p) => {
      setPreview(p);
      const split = !!(p.suggested.credit_column || p.suggested.debit_column);
      setMapping((m) => ({
        ...m,
        date_column: p.suggested.date_column ?? "",
        description_column: p.suggested.description_column ?? "",
        amount_column: split ? "" : (p.suggested.amount_column ?? ""),
        credit_column: p.suggested.credit_column ?? "",
        debit_column: p.suggested.debit_column ?? "",
      }));
    },
  });
  const commit = useMutation({
    mutationFn: () => api.post<{ imported: number; skipped: number; errors: string[] }>("/api/csv/commit", {
      content,
      account_id: mapping.account_id,
      mapping: {
        date_column: mapping.date_column,
        description_column: mapping.description_column,
        amount_column: mapping.amount_column || null,
        credit_column: mapping.credit_column || null,
        debit_column: mapping.debit_column || null,
        date_format: mapping.date_format || null,
      },
    }),
  });

  const accountList = accounts.data?.accounts ?? [];

  return (
    <div className="p-6 md:p-8 max-w-3xl space-y-4">
      <h1 className="text-3xl font-bold tracking-tight">Import CSV</h1>
      <p className="text-sm text-muted">
        Drop in a CSV from your bank. Useful for accounts that aren't covered by TrueLayer (e.g.
        broker statements, niche credit cards). Server auto-detects columns; you confirm before commit.
      </p>

      <label className="card p-4 block cursor-pointer">
        <input
          type="file"
          accept=".csv,text/csv"
          onChange={async (e) => {
            const f = e.target.files?.[0];
            if (!f) return;
            const text = await f.text();
            setContent(text);
            previewMut.mutate(text);
          }}
        />
      </label>
      {previewMut.isError && (
        <p className="text-sm text-danger">Could not parse CSV. Check the file is comma-separated.</p>
      )}

      {preview && (
        <div className="card p-4 space-y-3">
          <p className="text-sm font-medium">{preview.row_count} rows detected.</p>

          <div className="grid grid-cols-2 gap-2">
            <select className="input" value={mapping.account_id || ""} onChange={(e) => setMapping((m) => ({ ...m, account_id: Number(e.target.value) }))}>
              <option value="">— pick account —</option>
              {accountList.map((a) => (
                <option key={a.id} value={a.id}>{accountLabel(a)}</option>
              ))}
            </select>
            <ColPicker headers={preview.headers} label="Date" value={mapping.date_column} onChange={(v) => setMapping((m) => ({ ...m, date_column: v }))} />
            <ColPicker headers={preview.headers} label="Description" value={mapping.description_column} onChange={(v) => setMapping((m) => ({ ...m, description_column: v }))} />
            <ColPicker headers={preview.headers} label="Amount" value={mapping.amount_column} onChange={(v) => setMapping((m) => ({ ...m, amount_column: v }))} />
            <ColPicker headers={preview.headers} label="Credit (in)" value={mapping.credit_column} onChange={(v) => setMapping((m) => ({ ...m, credit_column: v }))} />
            <ColPicker headers={preview.headers} label="Debit (out)" value={mapping.debit_column} onChange={(v) => setMapping((m) => ({ ...m, debit_column: v }))} />
            <select className="input" value={mapping.date_format} onChange={(e) => setMapping((m) => ({ ...m, date_format: e.target.value }))}>
              {DATE_FORMATS.map((f) => (
                <option key={f.value || "auto"} value={f.value}>{f.label}</option>
              ))}
            </select>
          </div>
          {accountList.length === 0 && (
            <p className="text-xs text-mid">
              No accounts yet. Create a manual account on the Banks page, then come back to import.
            </p>
          )}

          <table className="text-xs w-full">
            <thead>
              <tr>{preview.headers.map((h) => <th key={h} className="text-left p-1 border-b border-border">{h}</th>)}</tr>
            </thead>
            <tbody>
              {preview.sample_rows.map((r, i) => (
                <tr key={i} className="border-b border-border">
                  {r.map((c, j) => <td key={j} className="p-1 truncate">{c}</td>)}
                </tr>
              ))}
            </tbody>
          </table>

          <button className="btn-primary" disabled={commit.isPending || !mapping.account_id} onClick={() => commit.mutate()}>
            {commit.isPending ? "importing…" : `Import ${preview.row_count} rows`}
          </button>
          {commit.data && (
            <p className="text-sm text-success">
              Imported {commit.data.imported}, skipped {commit.data.skipped} (already seen). {commit.data.errors.length} errors.
            </p>
          )}
          {commit.isError && (
            <p className="text-sm text-danger">Import failed. Check the column mapping and date format.</p>
          )}
        </div>
      )}
    </div>
  );
}

function ColPicker({ headers, label, value, onChange }: { headers: string[]; label: string; value: string; onChange: (v: string) => void }) {
  return (
    <select className="input" value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">— {label} —</option>
      {headers.map((h) => <option key={h} value={h}>{h}</option>)}
    </select>
  );
}
