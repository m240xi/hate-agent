import { invoke } from '@tauri-apps/api/core';

export interface AppInfo {
  version: string;
  data_dir: string;
  platform: string;
  arch: string;
}

export async function appInfo(): Promise<AppInfo> {
  return invoke<AppInfo>('app_info');
}

export async function sessionSend(text: string): Promise<void> {
  return invoke<void>('session_send', { text });
}

export type ThinkingLevel = 'off' | 'low' | 'medium' | 'high';

export async function sessionSetOptions(
  model: string | null,
  thinking: ThinkingLevel | null,
  contextWindow?: number | null,
): Promise<void> {
  return invoke<void>('session_set_options', { model, thinking, contextWindow });
}

export async function sessionSetProvider(
  provider: string,
  model?: string | null,
): Promise<void> {
  return invoke<void>('session_set_provider', { provider, model: model ?? null });
}

export interface ProviderModel {
  id: string;
  name: string | null;
  provider?: string | null;
  context_tokens: number | null;
  price_in_mtok: number | null;
  price_out_mtok: number | null;
  price_cache_mtok: number | null;
  reasoning_efforts: string[];
  default_effort: string | null;
  supports_tools: boolean | null;
  supports_vision: boolean | null;
}

export async function modelsList(provider?: string): Promise<ProviderModel[]> {
  return invoke<ProviderModel[]>('models_list', { provider: provider ?? null });
}

export async function providerSetModel(provider: string, model: string): Promise<void> {
  return invoke<void>('provider_set_model', { provider, model });
}

export async function sessionCancel(): Promise<void> {
  return invoke<void>('session_cancel');
}

export async function sessionCompact(): Promise<void> {
  return invoke<void>('session_compact');
}

export async function sessionRespondPermission(
  requestId: string,
  decision: 'allow' | 'allow_session' | 'deny',
): Promise<void> {
  return invoke<void>('session_respond_permission', { requestId, decision });
}

export async function sessionAnswer(text: string): Promise<void> {
  return invoke<void>('session_answer', { text });
}

export async function sessionRewind(checkpointId: string): Promise<void> {
  return invoke<void>('session_rewind', { checkpointId });
}

export interface NousLoginInfo {
  user_code: string;
  verification_url: string;
  verification_url_complete: string | null;
}

export async function authNousLogin(): Promise<NousLoginInfo> {
  return invoke<NousLoginInfo>('auth_nous_login');
}

export async function authNousCancel(): Promise<void> {
  return invoke<void>('auth_nous_cancel');
}

export interface AuthStatus {
  kind: 'none' | 'api_key' | 'oauth';
  expires_at: number | null;
}

export async function authStatus(provider?: string): Promise<AuthStatus> {
  return invoke<AuthStatus>('auth_status', { provider: provider ?? null });
}

export async function authSetKey(provider: string, key: string): Promise<void> {
  return invoke<void>('auth_set_key', { provider, key });
}

export async function authLogout(provider: string): Promise<void> {
  return invoke<void>('auth_logout', { provider });
}

export interface ProviderInfo {
  name: string;
  kind: string;
  base_url: string;
  model: string;
  is_default: boolean;
  auth_kind: 'none' | 'api_key' | 'oauth';
  auth_expires_at: number | null;
}

export async function providersList(): Promise<ProviderInfo[]> {
  return invoke<ProviderInfo[]>('providers_list');
}

export interface ProviderPreset {
  name: string;
  label: string;
  kind: string;
  base_url: string;
  model: string;
  auth: 'api_key' | 'oauth' | 'none';
  env_hint: string | null;
  key_url: string | null;
}

export async function providerPresets(): Promise<ProviderPreset[]> {
  return invoke<ProviderPreset[]>('provider_presets');
}

export async function providerUpsert(
  name: string,
  kind: string,
  base_url: string,
  model: string,
): Promise<void> {
  return invoke<void>('provider_upsert', { name, kind, baseUrl: base_url, model });
}

export async function providerRemove(name: string): Promise<void> {
  return invoke<void>('provider_remove', { name });
}

export interface SkillInfo {
  name: string;
  title: string;
  triggers: string[];
  bytes: number;
  updated: number;
}

export async function skillsList(): Promise<SkillInfo[]> {
  return invoke<SkillInfo[]>('skills_list');
}

export async function skillsRead(name: string): Promise<string> {
  return invoke<string>('skills_read', { name });
}

export async function skillsDelete(name: string): Promise<boolean> {
  return invoke<boolean>('skills_delete', { name });
}

export async function skillsAutoGet(): Promise<boolean> {
  return invoke<boolean>('skills_auto_get');
}

export async function skillsSetAuto(enabled: boolean): Promise<void> {
  return invoke<void>('skills_set_auto', { enabled });
}

export interface ClaudeLoginInfo {
  authorize_url: string;
}

export async function authClaudeBegin(): Promise<ClaudeLoginInfo> {
  return invoke<ClaudeLoginInfo>('auth_claude_begin');
}

export async function authClaudeComplete(code: string): Promise<void> {
  return invoke<void>('auth_claude_complete', { code });
}

export async function workspaceGet(): Promise<string | null> {
  return invoke<string | null>('workspace_get');
}

export async function workspaceSet(path: string | null): Promise<void> {
  return invoke<void>('workspace_set', { path });
}

export async function workspacePick(): Promise<string | null> {
  return invoke<string | null>('workspace_pick');
}

export interface WorkspaceIndex {
  tree: import('./types').FileNode[];
  symbols: import('./types').SymbolHit[];
  truncated: boolean;
}

export async function workspaceIndex(path: string): Promise<WorkspaceIndex> {
  return invoke<WorkspaceIndex>('workspace_index', { path });
}

export interface SessionMeta {
  id: string;
  mode: 'chat' | 'code';
  title: string;
  provider: string;
  model: string;
  workspace: string | null;
  updated_at: number;
}

export interface SessionOpenResult {
  meta: SessionMeta;
  ui_state: string | null;
}

export async function sessionsList(mode?: 'chat' | 'code'): Promise<SessionMeta[]> {
  return invoke<SessionMeta[]>('sessions_list', { mode: mode ?? null });
}

export async function sessionNew(mode: 'chat' | 'code'): Promise<SessionMeta> {
  return invoke<SessionMeta>('session_new', { mode });
}

export async function sessionOpen(id: string): Promise<SessionOpenResult> {
  return invoke<SessionOpenResult>('session_open', { id });
}

export async function sessionDelete(id: string): Promise<void> {
  return invoke<void>('session_delete', { id });
}

export async function sessionSaveUi(id: string, uiState: string): Promise<void> {
  return invoke<void>('session_save_ui', { id, uiState });
}

export interface McpServerConfig {
  name: string;
  kind: 'stdio' | 'http';
  enabled: boolean;
  command?: string | null;
  args: string[];
  env: Record<string, string>;
  url?: string | null;
  headers: Record<string, string>;
}

export interface McpServerInfo extends McpServerConfig {
  state: string;
  tools: number;
  info: string;
}

export async function mcpList(): Promise<McpServerInfo[]> {
  return invoke<McpServerInfo[]>('mcp_list');
}

export async function mcpRefresh(): Promise<McpServerInfo[]> {
  return invoke<McpServerInfo[]>('mcp_refresh');
}

export async function mcpSave(servers: McpServerConfig[]): Promise<McpServerInfo[]> {
  return invoke<McpServerInfo[]>('mcp_save', { servers });
}

export interface UpdateInfo {
  version: string;
  notes: string | null;
  date: string | null;
}

export async function updateCheck(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>('update_check');
}

export async function updateInstall(): Promise<void> {
  return invoke('update_install');
}
