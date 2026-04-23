import { useState, useEffect, useMemo, useRef } from "react";
import type {
  CodexStats,
  DailyModelData,
  ModelTokenBreakdown,
  ModelTotals,
} from "../types";
import { invokeBackend } from "../lib/platform";

// ── Colour palette (7 blues, darkest first) ───────────────────────────────────
const BLUES = [
  "#1d4ed8",
  "#2563eb",
  "#3b82f6",
  "#60a5fa",
  "#93c5fd",
  "#bfdbfe",
  "#dbeafe",
];

// ── Helpers ───────────────────────────────────────────────────────────────────

function fmtNum(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return n.toLocaleString();
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return n.toString();
}

function fmtHour(h: number | null): string {
  if (h === null) return "—";
  if (h === 0) return "12 AM";
  if (h < 12) return `${h} AM`;
  if (h === 12) return "12 PM";
  return `${h - 12} PM`;
}

function fmtModelName(model: string): string {
  if (model.startsWith("gpt-")) {
    return model.toUpperCase();
  }

  return model;
}

function startOfLocalDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

function accumulateBreakdowns(
  dailyData: DailyModelData[],
  cutoff: string | null
): Record<string, ModelTokenBreakdown> {
  const relevantDays = cutoff
    ? dailyData.filter((day) => day.date >= cutoff)
    : dailyData;
  const totals: Record<string, ModelTokenBreakdown> = {};

  for (const day of relevantDays) {
    for (const [model, detail] of Object.entries(day.details)) {
      if (!totals[model]) {
        totals[model] = { input_tokens: 0, output_tokens: 0, total_tokens: 0 };
      }
      totals[model].input_tokens += detail.input_tokens;
      totals[model].output_tokens += detail.output_tokens;
      totals[model].total_tokens += detail.total_tokens;
    }
  }

  return totals;
}

function isoDate(d: Date): string {
  const year = d.getFullYear();
  const month = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function cutoffDate(range: TimeRange): string | null {
  if (range === "all") return null;
  const d = new Date();
  d.setDate(d.getDate() - (range === "7d" ? 7 : 30));
  return isoDate(d);
}

function longestStreakForDates(sortedDates: string[]): number {
  let longestStreak = 0;
  let streak = 0;
  let previous = "";

  for (const date of sortedDates) {
    if (previous) {
      const diff = (new Date(date).getTime() - new Date(previous).getTime()) / 86400000;
      streak = diff === 1 ? streak + 1 : 1;
    } else {
      streak = 1;
    }

    if (streak > longestStreak) {
      longestStreak = streak;
    }

    previous = date;
  }

  return longestStreak;
}

function currentStreakForDates(activeDaysSet: Set<string>): number {
  const today = isoDate(new Date());
  const yesterday = isoDate(new Date(Date.now() - 86400000));
  let streak = 0;
  let checkDay = activeDaysSet.has(today)
    ? today
    : activeDaysSet.has(yesterday)
      ? yesterday
      : null;

  while (checkDay && activeDaysSet.has(checkDay)) {
    streak++;
    checkDay = isoDate(new Date(new Date(checkDay).getTime() - 86400000));
  }

  return streak;
}

function peakHourForOverviewDays(
  overviewDays: CodexStats["daily_overview_data"]
): number | null {
  const hourlyCounts = Array.from({ length: 24 }, () => 0);

  for (const day of overviewDays) {
    day.hourly_messages.forEach((count, hour) => {
      hourlyCounts[hour] += count ?? 0;
    });
  }

  let bestHour: number | null = null;
  let bestCount = 0;
  hourlyCounts.forEach((count, hour) => {
    if (count > bestCount) {
      bestCount = count;
      bestHour = hour;
    }
  });

  return bestHour;
}

type TimeRange = "all" | "30d" | "7d";
type TabId = "overview" | "models";

// ── Heatmap calendar ─────────────────────────────────────────────────────────

function HeatmapCalendar({ heatmap }: { heatmap: CodexStats["heatmap"] }) {
  const today = startOfLocalDay(new Date());
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // Start from the Sunday 52 full weeks back
  const start = new Date(today);
  start.setDate(start.getDate() - start.getDay() - 52 * 7);

  const map = useMemo(
    () => new Map(heatmap.map((d) => [d.date, d.count])),
    [heatmap]
  );

  // Compute quartile thresholds for colour intensity
  const sorted = useMemo(() => {
    const vals = heatmap.map((d) => d.count).filter((c) => c > 0);
    return vals.sort((a, b) => a - b);
  }, [heatmap]);

  const q1 = sorted[Math.floor(sorted.length * 0.25)] ?? 1;
  const q2 = sorted[Math.floor(sorted.length * 0.5)] ?? 1;
  const q3 = sorted[Math.floor(sorted.length * 0.75)] ?? 1;

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollLeft = el.scrollWidth - el.clientWidth;
  }, [heatmap]);

  function level(count: number): number {
    if (count === 0) return 0;
    if (count <= q1) return 1;
    if (count <= q2) return 2;
    if (count <= q3) return 3;
    return 4;
  }

  const levelClass = [
    "bg-gray-100 dark:bg-gray-800",
    "bg-blue-100 dark:bg-blue-950",
    "bg-blue-300 dark:bg-blue-800",
    "bg-blue-500 dark:bg-blue-500",
    "bg-blue-700 dark:bg-blue-300",
  ];

  // Build 53 columns × 7 rows (lv === -1 means future/hidden)
  const weeks: Array<Array<{ date: string; lv: number }>> = [];
  const cur = new Date(start);
  for (let w = 0; w < 53; w++) {
    const week: (typeof weeks)[0] = [];
    for (let d = 0; d < 7; d++) {
      const ds = isoDate(cur);
      const future = cur > today;
      week.push({ date: ds, lv: future ? -1 : level(map.get(ds) ?? 0) });
      cur.setDate(cur.getDate() + 1);
    }
    weeks.push(week);
  }

  return (
    <div ref={scrollRef} className="overflow-x-auto">
      <div className="flex gap-[3px]">
        {weeks.map((week, wi) => (
          <div key={wi} className="flex flex-col gap-[3px]">
            {week.map((cell, di) => (
              <div
                key={di}
                title={
                  cell.lv >= 0
                    ? `${cell.date}: ${fmtTokens(map.get(cell.date) ?? 0)} tokens`
                    : undefined
                }
                className={`w-[11px] h-[11px] rounded-[2px] ${
                  cell.lv === -1
                    ? "opacity-0"
                    : levelClass[cell.lv]
                }`}
              />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Models bar chart (SVG) ────────────────────────────────────────────────────

function ModelsChart({
  data,
  models,
  range,
}: {
  data: DailyModelData[];
  models: ModelTotals[];
  range: TimeRange;
}) {
  const cutoff = cutoffDate(range);
  const filtered = cutoff ? data.filter((d) => d.date >= cutoff) : data;

  const modelColor = useMemo(() => {
    const map: Record<string, string> = {};
    models.forEach((m, i) => {
      map[m.model] = BLUES[i % BLUES.length];
    });
    return map;
  }, [models]);

  if (filtered.length === 0) {
    return (
      <div className="flex items-center justify-center h-40 text-sm text-gray-400 dark:text-gray-500">
        No data for this period
      </div>
    );
  }

  // Chart geometry
  const W = 560;
  const H = 180;
  const L = 58; // left margin (y-axis labels)
  const B = 32; // bottom margin (x-axis labels)
  const T = 8;  // top margin
  const cW = W - L - 8;
  const cH = H - B - T;

  const maxTokens = Math.max(
    ...filtered.map((d) => Object.values(d.models).reduce((a, b) => a + b, 0)),
    1
  );

  // Nice round y-axis ceiling
  const mag = Math.pow(10, Math.floor(Math.log10(maxTokens)));
  const yMax = Math.ceil(maxTokens / mag) * mag;
  const yTicks = [0, 0.25, 0.5, 0.75, 1].map((f) => Math.round(yMax * f));

  const barW = Math.max(2, Math.min(18, cW / filtered.length - 1));
  const gap = cW / filtered.length;

  // X-axis label indices: show at most 8, prefer month boundaries
  const labelSet = new Set<number>();
  if (filtered.length <= 8) {
    filtered.forEach((_, i) => labelSet.add(i));
  } else {
    let prevMonth = "";
    filtered.forEach((d, i) => {
      const mo = d.date.slice(0, 7);
      if (mo !== prevMonth) {
        labelSet.add(i);
        prevMonth = mo;
      }
    });
    // Always show first and last
    labelSet.add(0);
    labelSet.add(filtered.length - 1);
  }

  function barX(i: number) {
    return L + i * gap + gap / 2 - barW / 2;
  }
  function toY(tokens: number) {
    return T + cH * (1 - tokens / yMax);
  }

  const modelOrder = models.map((m) => m.model);

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      className="w-full"
      style={{ height: `${H}px` }}
    >
      {/* Y-axis grid lines + labels */}
      {yTicks.map((v) => {
        const y = toY(v);
        return (
          <g key={v}>
            <line
              x1={L}
              y1={y}
              x2={W - 8}
              y2={y}
              stroke="currentColor"
              strokeWidth={0.5}
              className="text-gray-200 dark:text-gray-700"
            />
            <text
              x={L - 4}
              y={y + 4}
              textAnchor="end"
              fontSize={9}
              className="fill-gray-400 dark:fill-gray-500"
            >
              {fmtTokens(v)}
            </text>
          </g>
        );
      })}

      {/* Stacked bars */}
      {filtered.map((day, i) => {
        let yOffset = toY(0);
        return (
          <g key={day.date}>
            {modelOrder.map((model) => {
              const tokens = day.models[model] ?? 0;
              if (tokens === 0) return null;
              const barH = (tokens / yMax) * cH;
              const x = barX(i);
              const y = yOffset - barH;
              yOffset = y;
              return (
                <rect
                  key={model}
                  x={x}
                  y={y}
                  width={barW}
                  height={barH}
                  fill={modelColor[model] ?? BLUES[6]}
                  rx={1}
                >
                  <title>
                    {day.date} · {fmtModelName(model)}: {fmtTokens(tokens)} tokens
                  </title>
                </rect>
              );
            })}
          </g>
        );
      })}

      {/* X-axis labels */}
      {filtered.map((day, i) => {
        if (!labelSet.has(i)) return null;
        const x = barX(i) + barW / 2;
        const label = day.date.slice(5).replace("-", " "); // "04 18"
        const [mo, dy] = label.split(" ");
        const months = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
        const moName = months[parseInt(mo, 10) - 1] ?? mo;
        return (
          <text
            key={i}
            x={x}
            y={H - 4}
            textAnchor="middle"
            fontSize={9}
            className="fill-gray-400 dark:fill-gray-500"
          >
            {moName} {dy}
          </text>
        );
      })}

      {/* Baseline */}
      <line
        x1={L}
        y1={toY(0)}
        x2={W - 8}
        y2={toY(0)}
        stroke="currentColor"
        strokeWidth={1}
        className="text-gray-300 dark:text-gray-600"
      />
    </svg>
  );
}

// ── Stat card ─────────────────────────────────────────────────────────────────

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="bg-gray-50 dark:bg-gray-800/60 rounded-xl px-4 py-3">
      <p className="text-xs text-gray-500 dark:text-gray-400 mb-1">{label}</p>
      <p className="text-lg font-semibold text-gray-900 dark:text-gray-100 leading-tight">
        {value}
      </p>
    </div>
  );
}

// ── Main modal ────────────────────────────────────────────────────────────────

interface StatsModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export function StatsModal({ isOpen, onClose }: StatsModalProps) {
  const [tab, setTab] = useState<TabId>("overview");
  const [range, setRange] = useState<TimeRange>("all");
  const [stats, setStats] = useState<CodexStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) return;
    setLoading(true);
    setError(null);
    invokeBackend<CodexStats>("get_codex_stats")
      .then(setStats)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [isOpen]);

  const filteredModelTotals = useMemo(() => {
    if (!stats) return [];
    const totals = accumulateBreakdowns(
      stats.daily_model_data,
      cutoffDate(range)
    );
    const grand = Object.values(totals).reduce(
      (sum, detail) => sum + detail.total_tokens,
      0
    );

    return Object.entries(totals)
      .map(([model, detail]) => ({
        model,
        input_tokens: detail.input_tokens,
        output_tokens: detail.output_tokens,
        total_tokens: detail.total_tokens,
        percentage: grand > 0 ? (detail.total_tokens / grand) * 100 : 0,
      }))
      .sort((a, b) => b.total_tokens - a.total_tokens);
  }, [stats, range]);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
      <div
        className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-3xl shadow-2xl flex flex-col"
        style={{ maxHeight: "90vh" }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-gray-100 dark:border-gray-800 shrink-0">
          <div className="flex items-center gap-3">
            {/* Tab bar */}
            <div className="flex gap-1 bg-gray-100 dark:bg-gray-800 rounded-lg p-0.5">
              {(["overview", "models"] as TabId[]).map((t) => (
                <button
                  key={t}
                  onClick={() => setTab(t)}
                  className={`px-3 py-1.5 text-sm font-medium rounded-md transition-colors capitalize ${
                    tab === t
                      ? "bg-white dark:bg-gray-900 text-gray-900 dark:text-gray-100 shadow-sm"
                      : "text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300"
                  }`}
                >
                  {t === "overview" ? "Overview" : "Models"}
                </button>
              ))}
            </div>
          </div>

          <div className="flex items-center gap-2">
            {/* Time range */}
            <div className="flex gap-1 bg-gray-100 dark:bg-gray-800 rounded-lg p-0.5">
              {(["all", "30d", "7d"] as TimeRange[]).map((r) => (
                <button
                  key={r}
                  onClick={() => setRange(r)}
                  className={`px-2.5 py-1 text-xs font-medium rounded-md transition-colors ${
                    range === r
                      ? "bg-white dark:bg-gray-900 text-gray-900 dark:text-gray-100 shadow-sm"
                      : "text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300"
                  }`}
                >
                  {r === "all" ? "All" : r}
                </button>
              ))}
            </div>
            <button
              onClick={onClose}
              className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 transition-colors p-1"
            >
              ✕
            </button>
          </div>
        </div>

        {/* Body */}
        <div className="overflow-y-auto p-6 space-y-5">
          {loading && (
            <div className="flex items-center justify-center py-16">
              <div className="animate-spin h-8 w-8 border-2 border-gray-900 dark:border-gray-100 border-t-transparent rounded-full" />
            </div>
          )}

          {error && (
            <div className="text-center py-10 text-red-500 text-sm">{error}</div>
          )}

          {!loading && !error && stats && (
            <>
              {tab === "overview" && (
                <OverviewTab stats={stats} range={range} />
              )}
              {tab === "models" && (
                <ModelsTab
                  stats={stats}
                  filteredTotals={filteredModelTotals}
                  range={range}
                />
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

// ── Overview tab ──────────────────────────────────────────────────────────────

function OverviewTab({ stats, range }: { stats: CodexStats; range: TimeRange }) {
  const cutoff = cutoffDate(range);
  const heatmapFiltered = cutoff
    ? stats.heatmap.filter((d) => d.date >= cutoff)
    : stats.heatmap;

  const filteredTotals = useMemo(() => {
    if (!cutoff) {
      return {
        sessions: stats.sessions,
        messages: stats.messages,
        total_tokens: stats.total_tokens,
        active_days: stats.active_days,
        current_streak: stats.current_streak,
        longest_streak: stats.longest_streak,
        peak_hour: stats.peak_hour,
        favorite_model: stats.favorite_model,
      };
    }

    const overviewDays = stats.daily_overview_data.filter((day) => day.date >= cutoff);
    const modelBreakdowns = accumulateBreakdowns(stats.daily_model_data, cutoff);
    const tokenDays = stats.daily_model_data.filter((d) => d.date >= cutoff);
    const activeDaysSet = new Set(
      heatmapFiltered
        .filter((day) => day.count > 0)
        .map((day) => day.date)
    );
    const total = tokenDays.reduce(
      (s, d) => s + Object.values(d.models).reduce((a, b) => a + b, 0),
      0
    );
    const favoriteModel = Object.entries(modelBreakdowns)
      .sort(([, a], [, b]) => b.total_tokens - a.total_tokens)[0]?.[0] ?? null;

    return {
      sessions: overviewDays.reduce((sum, day) => sum + day.sessions, 0),
      messages: overviewDays.reduce((sum, day) => sum + day.messages, 0),
      total_tokens: total,
      active_days: activeDaysSet.size,
      current_streak: currentStreakForDates(activeDaysSet),
      longest_streak: longestStreakForDates(Array.from(activeDaysSet).sort()),
      peak_hour: peakHourForOverviewDays(overviewDays),
      favorite_model: favoriteModel,
    };
  }, [stats, cutoff, heatmapFiltered]);

  const cards = [
    { label: "Sessions", value: fmtNum(filteredTotals.sessions) },
    { label: "Messages", value: fmtNum(filteredTotals.messages) },
    { label: "Total tokens", value: fmtNum(filteredTotals.total_tokens) },
    { label: "Active days", value: String(filteredTotals.active_days) },
    { label: "Current streak", value: `${filteredTotals.current_streak}d` },
    { label: "Longest streak", value: `${filteredTotals.longest_streak}d` },
    { label: "Peak hour", value: fmtHour(filteredTotals.peak_hour) },
    {
      label: "Favorite model",
      value: filteredTotals.favorite_model
        ? fmtModelName(filteredTotals.favorite_model)
        : "—",
    },
  ];

  return (
    <div className="space-y-5">
      {/* Stat cards grid */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
        {cards.map((c) => (
          <StatCard key={c.label} label={c.label} value={c.value} />
        ))}
      </div>

      {/* Heatmap */}
      <div className="bg-gray-50 dark:bg-gray-800/40 rounded-xl p-4">
        <HeatmapCalendar heatmap={heatmapFiltered} />
        {stats.fun_fact && (
          <p className="mt-3 text-xs text-gray-400 dark:text-gray-500">
            {stats.fun_fact}
          </p>
        )}
      </div>
    </div>
  );
}

// ── Models tab ────────────────────────────────────────────────────────────────

function ModelsTab({
  stats,
  filteredTotals,
  range,
}: {
  stats: CodexStats;
  filteredTotals: ModelTotals[];
  range: TimeRange;
}) {
  const modelColors = useMemo(() => {
    const map: Record<string, string> = {};
    filteredTotals.forEach((m, i) => {
      map[m.model] = BLUES[i % BLUES.length];
    });
    return map;
  }, [filteredTotals]);

  return (
    <div className="space-y-5">
      {/* Bar chart */}
      <div className="bg-gray-50 dark:bg-gray-800/40 rounded-xl p-4">
        <ModelsChart
          data={stats.daily_model_data}
          models={filteredTotals}
          range={range}
        />
      </div>

      {/* Model list */}
      <div className="space-y-2">
        {filteredTotals.map((m) => (
          <div
            key={m.model}
            className="flex items-center gap-3 bg-gray-50 dark:bg-gray-800/40 rounded-xl px-4 py-3"
          >
            <span
              className="inline-block w-3 h-3 rounded-sm shrink-0"
              style={{ background: modelColors[m.model] ?? BLUES[6] }}
            />
            <span className="text-sm font-medium text-gray-800 dark:text-gray-200 min-w-0 truncate flex-1">
              {fmtModelName(m.model)}
            </span>
            <span className="text-xs text-gray-500 dark:text-gray-400 shrink-0 whitespace-nowrap">
              {fmtTokens(m.input_tokens)} in · {fmtTokens(m.output_tokens)} out
            </span>
            <span className="text-sm font-semibold text-gray-700 dark:text-gray-300 shrink-0 w-12 text-right">
              {m.percentage.toFixed(1)}%
            </span>
          </div>
        ))}
        {filteredTotals.length === 0 && (
          <p className="text-center text-sm text-gray-400 dark:text-gray-500 py-6">
            No model data for this period
          </p>
        )}
      </div>
    </div>
  );
}
