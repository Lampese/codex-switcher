import { useEffect, useId, useRef, useState } from "react";
import type { AccountResetCredits } from "../types";
import { formatResetCreditDateTime, getAvailableResetCredits } from "../lib/resetCredits";
import { useI18n } from "../lib/i18n";
import type { AppLocale } from "../lib/dateFormat";

function getResetCreditsTone(resetCredits: AccountResetCredits | null): {
  container: string;
  badge: string;
  text: string;
} {
  const fallback = {
    container: "border-sky-200 bg-sky-50/70 dark:border-sky-800 dark:bg-sky-950/30",
    badge: "border-sky-200 bg-sky-100 text-sky-700 dark:border-sky-700 dark:bg-sky-900/50 dark:text-sky-300",
    text: "text-sky-700/80 dark:text-sky-300/80",
  };

  if (!resetCredits?.next_expires_at) return fallback;

  const expiry = new Date(resetCredits.next_expires_at);
  if (Number.isNaN(expiry.getTime())) return fallback;

  const remainingMs = expiry.getTime() - Date.now();
  const dayMs = 24 * 60 * 60 * 1000;

  if (remainingMs <= 3 * dayMs) {
    return {
      container: "border-red-200 bg-red-50/70 dark:border-red-800 dark:bg-red-950/30",
      badge: "border-red-200 bg-red-100 text-red-700 dark:border-red-700 dark:bg-red-900/50 dark:text-red-300",
      text: "text-red-700/80 dark:text-red-300/80",
    };
  }

  if (remainingMs <= 10 * dayMs) {
    return {
      container: "border-amber-200 bg-amber-50/70 dark:border-amber-800 dark:bg-amber-950/30",
      badge: "border-amber-200 bg-amber-100 text-amber-700 dark:border-amber-700 dark:bg-amber-900/50 dark:text-amber-300",
      text: "text-amber-700/80 dark:text-amber-300/80",
    };
  }

  return fallback;
}

type Translate = (key: string, values?: Record<string, string | number>) => string;

function formatExpiryDetail(
  expiresAt: string | null,
  t: Translate,
  locale: AppLocale,
): string {
  const expiry = formatResetCreditDateTime(expiresAt, { locale });
  if (expiry === "No expiry") return t("noExpiry");
  if (expiry === "Expiry unavailable") return t("expiryUnavailable");
  return t("expires", { value: expiry });
}

export function ResetCreditsMenu({
  compact,
  resetCredits,
}: {
  compact: boolean;
  resetCredits: AccountResetCredits | null;
}) {
  const { t, locale } = useI18n();
  const [isOpen, setIsOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popupId = useId();
  const availableCredits = getAvailableResetCredits(resetCredits);
  const count = availableCredits.length;
  const countLabel = count === 1 ? t("oneReset") : t("manyResets", { count });
  const nextExpiry = formatResetCreditDateTime(
    availableCredits[0]?.expires_at ?? null,
    { compact, locale },
  );
  const nextExpiryLabel =
    nextExpiry === "No expiry"
      ? t("noExpiry")
      : nextExpiry === "Expiry unavailable"
        ? t("expiryUnavailable")
        : t("closest", { value: nextExpiry });
  const tone = getResetCreditsTone(resetCredits);

  useEffect(() => {
    if (!isOpen) return;

    const handlePointerDown = (event: MouseEvent) => {
      if (!wrapperRef.current?.contains(event.target as Node)) {
        setIsOpen(false);
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setIsOpen(false);
      buttonRef.current?.focus();
    };

    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [isOpen]);

  useEffect(() => {
    setIsOpen(false);
  }, [resetCredits]);

  if (count === 0) return null;

  return (
    <div ref={wrapperRef} className="relative min-w-0 max-w-full">
      <button
        ref={buttonRef}
        type="button"
        aria-expanded={isOpen}
        aria-controls={popupId}
        aria-haspopup="dialog"
        onClick={() => setIsOpen((open) => !open)}
        className={
          compact
            ? `flex min-w-0 max-w-full items-center gap-1.5 rounded-full border px-2 py-1 text-[11px] leading-none transition-colors hover:brightness-95 focus:outline-none focus:ring-2 focus:ring-sky-400/60 ${tone.container} ${tone.text}`
            : `flex max-w-full items-center gap-2 rounded-lg border px-2 py-1.5 text-xs transition-colors hover:brightness-95 focus:outline-none focus:ring-2 focus:ring-sky-400/60 ${tone.container}`
        }
        title={`${countLabel} · ${nextExpiryLabel} · ${t("clickExpiry")}`}
      >
        <span
          className={
            compact
              ? "shrink-0 whitespace-nowrap font-semibold"
              : `whitespace-nowrap rounded-full border px-2.5 py-0.5 font-medium ${tone.badge}`
          }
        >
          {countLabel}
        </span>
        <span className={`truncate ${compact ? "" : tone.text}`}>
          · {nextExpiryLabel}
        </span>
        <svg
          className={`h-3 w-3 shrink-0 transition-transform ${isOpen ? "rotate-180" : ""}`}
          viewBox="0 0 12 12"
          fill="none"
          aria-hidden="true"
        >
          <path
            d="m3 4.5 3 3 3-3"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="1.5"
          />
        </svg>
      </button>

      {isOpen && (
        <div
          id={popupId}
          role="dialog"
          aria-label={t("clickExpiry")}
          className="absolute right-0 top-full z-30 mt-2 w-80 max-w-[calc(100vw-3rem)] overflow-hidden rounded-xl border border-gray-200 bg-white text-left shadow-xl dark:border-gray-700 dark:bg-gray-900"
        >
          <div className="flex items-center justify-between border-b border-gray-100 px-3 py-2.5 dark:border-gray-800">
            <span className="text-xs font-semibold text-gray-900 dark:text-gray-100">
              {t("availableResets")}
            </span>
            <span className="text-[11px] text-gray-500 dark:text-gray-400">
              {count}
            </span>
          </div>
          <div className="max-h-64 overflow-y-auto py-1">
            {availableCredits.map((credit, index) => (
              <div
                key={credit.id}
                className="flex items-start gap-2.5 px-3 py-2.5 text-xs"
              >
                <span className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-full font-semibold ${tone.badge}`}>
                  {index + 1}
                </span>
                <div className="min-w-0">
                  <div className="truncate font-medium text-gray-800 dark:text-gray-200">
                    {credit.title?.trim() || t("resetDefault", { index: index + 1 })}
                  </div>
                  <div className="mt-0.5 text-[11px] text-gray-500 dark:text-gray-400">
                    {formatExpiryDetail(credit.expires_at, t, locale)}
                  </div>
                </div>
              </div>
            ))}
          </div>
          <div className="border-t border-gray-100 px-3 py-2 text-[10px] text-gray-400 dark:border-gray-800 dark:text-gray-500">
            {t("localTime")}
          </div>
        </div>
      )}
    </div>
  );
}
