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

export interface ScheduledWarmupSettings {
  enabled: boolean;
  local_time: string;
  account_ids: string[];
  last_run_local_date: string | null;
  last_missed_prompt_local_date: string | null;
}

export interface ScheduledWarmupStatus {
  schedule: ScheduledWarmupSettings | null;
  valid_account_ids: string[];
  missed_run_today: boolean;
  next_run_local_iso: string | null;
}

export interface ScheduledWarmupEvent {
  summary: WarmupSummary;
  trigger: string;
}

export interface ImportAccountsSummary {
  total_in_payload: number;
  imported_count: number;
  skipped_count: number;
}

export type ExportSecurityMode = "less_secure" | "passphrase" | "keychain";

export interface AppSettings {
  version: number;
  export_security_mode: ExportSecurityMode | null;
  scheduled_warmup: ScheduledWarmupSettings | null;
}
