import { useMemo, useState } from "react";
import { CategorySelect } from "@/components/CategorySelect";
import { TransferPairMenu } from "@/components/TransferPairMenu";
import { transferCategoryIds } from "@/lib/categories";
import type { Account, Category, FinancialItem, Transaction } from "@/types/api";

export function TransferAwareCategorySelect({
  txn,
  categories,
  accounts,
  items = [],
  className,
  value,
  onChange,
}: {
  txn: Transaction;
  categories: Category[];
  accounts: Account[];
  items?: FinancialItem[];
  className?: string;
  value: number | null | "";
  onChange: (id: number | null) => void;
}) {
  const [askDest, setAskDest] = useState(false);
  const transferIds = useMemo(
    () => transferCategoryIds(categories),
    [categories]
  );
  const transferRootId =
    categories.find(
      (c) => c.parent_id == null && c.name.toLowerCase() === "transfer"
    )?.id ?? null;

  return (
    <div className="min-w-0">
      <CategorySelect
        className={className}
        categories={categories}
        value={askDest ? transferRootId ?? value : value}
        onChange={(cat) => {
          if (cat != null && transferIds.has(cat)) {
            setAskDest(true);
            return;
          }
          setAskDest(false);
          onChange(cat);
        }}
      />
      {askDest && (
        <div className="mt-2" onClick={(e) => e.stopPropagation()}>
          <p className="text-[11px] text-mid mb-1">
            Which account is the other leg on?
          </p>
          <TransferPairMenu
            txn={txn}
            accounts={accounts}
            items={items}
            variant="panel"
            autoOpen
          />
          <button
            type="button"
            className="btn-ghost text-[11px] mt-1"
            onClick={() => setAskDest(false)}
          >
            Cancel
          </button>
        </div>
      )}
    </div>
  );
}
