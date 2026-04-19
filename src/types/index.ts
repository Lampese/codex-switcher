// Types matching the Rust backend

export type AuthMode = "api_key" | "chat_gpt";

export interface AccountInfo {
  id: string;
  name: string;
  email: string | null;
  plan_type: string | null;
  auth_mode: AuthMode;
  is_active: boolean;
  created_at: string;
  last_used_at: string | null;
}

export interface UsageInfo {
  account_id: string;
  plan_type: string | null;
  primary_used_percent: number | null;
  primary_window_minutes: number | null;
  primary_resets_at: number | null;
  secondary_used_percent: number | null;
  secondary_window_minutes: number | null;
  secondary_resets_at: number | null;
  has_credits: boolean | null;
  unlimited_credits: boolean | null;
  credits_balance: string | null;
  error: string | null;
}

export interface OAuthLoginInfo {
  auth_url: string;
  callback_port: number;
}

export interface AccountWithUsage extends AccountInfo {
  usage?: UsageInfo;
  usageLoading?: boolean;
}

export interface CodexProcessInfo {
  count: number;
  background_count: number;
  can_switch: boolean;
  pids: number[];
}

export interface WarmupSummary {
  total_accounts: number;
  warmed_accounts: number;
  failed_account_ids: string[];
}

export interface ImportAccountsSummary {
  total_in_payload: number;
  imported_count: number;
  skipped_count: number;
}

// ── Codex usage stats ────────────────────────────────────────────────────────

export interface ModelTokenBreakdown {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
}

export interface HeatmapDay {
  date: string; // "YYYY-MM-DD"
  count: number;
}

export interface DailyModelData {
  date: string; // "YYYY-MM-DD"
  models: Record<string, number>; // model → total tokens
  details: Record<string, ModelTokenBreakdown>; // model → exact token split
}

export interface ModelTotals {
  model: string;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  percentage: number;
}

export interface CodexStats {
  sessions: number;
  messages: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  active_days: number;
  current_streak: number;
  longest_streak: number;
  peak_hour: number | null;
  favorite_model: string | null;
  heatmap: HeatmapDay[];
  daily_model_data: DailyModelData[];
  model_totals: ModelTotals[];
  fun_fact: string | null;
}
