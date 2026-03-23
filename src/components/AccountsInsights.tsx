import { useMemo } from "react";
import { getAccountNextResetAt, getAccountRemainingPercent } from "../accountAnalytics";
import type { AccountWithUsage } from "../types";

interface AccountsInsightsProps {
  accounts: AccountWithUsage[];
  embedded?: boolean;
}

const BUCKETS = [
  {
    key: "depleted",
    label: "Depleted",
    fillClass: "bg-slate-500",
    dotClass: "bg-slate-500",
    pillClass: "border-slate-200 bg-slate-100 text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200",
  },
  {
    key: "critical",
    label: "Critical",
    fillClass: "bg-rose-500",
    dotClass: "bg-rose-500",
    pillClass: "border-rose-200 bg-rose-50 text-rose-700 dark:border-rose-900/70 dark:bg-rose-500/12 dark:text-rose-300",
  },
  {
    key: "watch",
    label: "Watch",
    fillClass: "bg-amber-500",
    dotClass: "bg-amber-500",
    pillClass: "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-900/70 dark:bg-amber-500/12 dark:text-amber-300",
  },
  {
    key: "healthy",
    label: "Healthy",
    fillClass: "bg-sky-500",
    dotClass: "bg-sky-500",
    pillClass: "border-sky-200 bg-sky-50 text-sky-700 dark:border-sky-900/70 dark:bg-sky-500/12 dark:text-sky-300",
  },
  {
    key: "ready",
    label: "Ready",
    fillClass: "bg-emerald-500",
    dotClass: "bg-emerald-500",
    pillClass: "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900/70 dark:bg-emerald-500/12 dark:text-emerald-300",
  },
] as const;

function formatTimeUntil(resetAt: number | null): string {
  if (!resetAt) {
    return "No reset data";
  }

  const diff = resetAt - Math.floor(Date.now() / 1000);

  if (diff <= 0) {
    return "now";
  }

  if (diff < 3600) {
    return `${Math.ceil(diff / 60)}m`;
  }

  if (diff < 86400) {
    const hours = Math.floor(diff / 3600);
    const minutes = Math.floor((diff % 3600) / 60);
    return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  }

  const days = Math.floor(diff / 86400);
  const hours = Math.floor((diff % 86400) / 3600);
  return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
}

export function AccountsInsights({ accounts, embedded = false }: AccountsInsightsProps) {
  const summary = useMemo(() => {
    const trackedAccounts = accounts
      .map((account) => ({
        remaining: getAccountRemainingPercent(account),
        nextResetAt: getAccountNextResetAt(account),
      }))
      .filter(
        (account): account is { remaining: number; nextResetAt: number | null } =>
          account.remaining !== null
      );

    const buckets = {
      depleted: 0,
      critical: 0,
      watch: 0,
      healthy: 0,
      ready: 0,
    };

    for (const account of trackedAccounts) {
      if (account.remaining <= 0) {
        buckets.depleted += 1;
      } else if (account.remaining < 25) {
        buckets.critical += 1;
      } else if (account.remaining < 50) {
        buckets.watch += 1;
      } else if (account.remaining < 70) {
        buckets.healthy += 1;
      } else {
        buckets.ready += 1;
      }
    }

    const trackedCount = trackedAccounts.length;
    const unknownCount = Math.max(0, accounts.length - trackedCount);
    const averageRemaining = trackedCount
      ? Math.round(
          trackedAccounts.reduce((sum, account) => sum + account.remaining, 0) / trackedCount
        )
      : null;

    const now = Math.floor(Date.now() / 1000);
    const nextResetAt =
      trackedAccounts
        .map((account) => account.nextResetAt)
        .filter((value): value is number => value !== null && value > now)
        .sort((a, b) => a - b)[0] ?? null;

    return {
      buckets,
      trackedCount,
      unknownCount,
      averageRemaining,
      lowReserveCount: buckets.critical + buckets.watch,
      nextResetAt,
      highReserveShare: trackedCount ? Math.round((buckets.ready / trackedCount) * 100) : 0,
      standbySummary:
        trackedCount === accounts.length
          ? "all standby tracked"
          : `${trackedCount} of ${accounts.length} standby tracked`,
    };
  }, [accounts]);

  const containerClass = embedded
    ? "relative rounded-[24px] border border-slate-200/80 bg-[linear-gradient(180deg,rgba(248,250,252,0.95),rgba(255,255,255,0.96))] p-4 shadow-[0_1px_2px_rgba(15,23,42,0.05),0_16px_32px_rgba(15,23,42,0.06)] ring-1 ring-white/70 dark:border-slate-800 dark:bg-[linear-gradient(180deg,rgba(15,23,42,0.96),rgba(15,23,42,0.92))] dark:ring-slate-900/60 dark:shadow-[0_1px_2px_rgba(2,6,23,0.45),0_16px_32px_rgba(2,6,23,0.35)]"
    : "relative overflow-hidden rounded-3xl border border-slate-200 bg-white shadow-[0_1px_2px_rgba(15,23,42,0.08),0_18px_40px_rgba(15,23,42,0.06)] dark:border-slate-800 dark:bg-slate-900 dark:shadow-[0_1px_2px_rgba(2,6,23,0.45),0_20px_40px_rgba(2,6,23,0.4)]";

  return (
    <div className={containerClass}>
      {!embedded && (
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_top_left,_rgba(16,185,129,0.18),_transparent_32%),radial-gradient(circle_at_bottom_right,_rgba(14,165,233,0.12),_transparent_30%)]" />
      )}

      <div className={embedded ? "relative" : "relative p-4 sm:p-5"}>
        <div className="flex flex-col gap-4 xl:flex-row xl:items-start xl:justify-between">
          <div className="min-w-0">
            <p className="text-[11px] font-semibold uppercase tracking-[0.26em] text-slate-500 dark:text-slate-400">
              Bench Overview
            </p>

            <div className="mt-3 flex items-end gap-3">
              <div className="text-4xl font-semibold tracking-tight text-slate-900 dark:text-white">
                {summary.trackedCount === 0 ? "--" : summary.buckets.ready}
              </div>
              <div className="pb-1 text-sm leading-5 text-slate-600 dark:text-slate-300">
                <p>ready now</p>
                <p>{summary.standbySummary}</p>
              </div>
            </div>
          </div>

          <div className="flex flex-wrap gap-2 text-xs">
            <span className="inline-flex items-center rounded-full border border-slate-200 bg-white px-3 py-1.5 font-medium text-slate-600 shadow-sm dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
              Avg reserve {summary.averageRemaining !== null ? `${summary.averageRemaining}%` : "--"}
            </span>
            <span className="inline-flex items-center rounded-full border border-emerald-100 bg-emerald-50 px-3 py-1.5 font-medium text-emerald-700 shadow-sm">
              Next reset {formatTimeUntil(summary.nextResetAt)}
            </span>
          </div>
        </div>

        <div className="mt-4 rounded-2xl border border-slate-200/80 bg-white/88 p-3.5 shadow-sm ring-1 ring-white/70 dark:border-slate-800 dark:bg-slate-900/85 dark:ring-slate-900/60">
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="text-xs font-medium text-slate-500 dark:text-slate-400">Availability mix</p>
              <p className="mt-0.5 text-sm text-slate-700 dark:text-slate-200">
                {summary.highReserveShare}% are strong switch candidates.
              </p>
            </div>
            <div className="flex flex-wrap gap-2 text-[11px]">
              <span className="inline-flex items-center rounded-full border border-amber-100 bg-amber-50 px-2.5 py-1 font-medium text-amber-700 dark:border-amber-900/70 dark:bg-amber-500/12 dark:text-amber-300">
                Low reserve {summary.lowReserveCount}
              </span>
              <span className="inline-flex items-center rounded-full border border-slate-200 bg-slate-100 px-2.5 py-1 font-medium text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200">
                Depleted {summary.buckets.depleted}
              </span>
              {summary.unknownCount > 0 && (
                <span className="inline-flex items-center rounded-full border border-slate-200 bg-white px-2.5 py-1 font-medium text-slate-600 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
                  Awaiting data {summary.unknownCount}
                </span>
              )}
            </div>
          </div>

          <div className="mt-3 flex h-3 overflow-hidden rounded-full bg-slate-100 dark:bg-slate-800">
            {summary.trackedCount === 0 ? (
              <div className="h-full w-full animate-pulse bg-slate-200 dark:bg-slate-700" />
            ) : (
              BUCKETS.map((bucket) => {
                const count = summary.buckets[bucket.key];

                if (count === 0) {
                  return null;
                }

                return (
                  <div
                    key={bucket.key}
                    className={`${bucket.fillClass} h-full transition-all duration-300`}
                    style={{ width: `${(count / summary.trackedCount) * 100}%` }}
                  />
                );
              })
            )}
          </div>

          <div className="mt-3 flex flex-wrap gap-2 text-[11px] leading-5 text-slate-500 dark:text-slate-300">
            {BUCKETS.map((bucket) => (
              <span
                key={bucket.key}
                className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 font-medium ${bucket.pillClass}`}
              >
                <span className={`h-2 w-2 rounded-full ${bucket.dotClass}`} />
                {bucket.label} {summary.buckets[bucket.key]}
              </span>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
