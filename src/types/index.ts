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

export type SwitchReason = "manual" | "threshold";
export type SwitchPolicy = "next_session_only";
export type SwitchOutcome =
  | "switched_now"
  | "queued_for_next_session"
  | "applied_queued"
  | "cleared_queued"
  | "noop";

export interface SwitchState {
  active_account_id: string | null;
  queued_account_id: string | null;
  queued_reason: SwitchReason | null;
  queued_at: string | null;
  switch_policy: SwitchPolicy;
}

export interface SwitchActionResult {
  outcome: SwitchOutcome;
  account_id: string | null;
  state: SwitchState;
}

export interface AutoSwitchConfig {
  enabled: boolean;
  threshold_percent: number;
  check_interval_seconds: number;
  respect_weekly_limit: boolean;
  excluded_account_ids: string[];
  priority_order: string[];
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

export type AutoSwitchReason =
  | "PrimaryLimitReached"
  | "WeeklyLimitReached"
  | "BothLimitsReached";

export interface AutoSwitchEvent {
  timestamp: number;
  from_account_id: string;
  to_account_id: string;
  reason: AutoSwitchReason;
  triggered_at_percent: number;
}
