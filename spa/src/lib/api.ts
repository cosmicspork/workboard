export type State = 'inbox' | 'next' | 'waiting' | 'later' | 'done';
export type Lane = Exclude<State, 'inbox'> | 'inbox';
export interface Link { label: string; url: string }
export interface JiraFact {
  key: string;
  summary: string;
  status_name: string;
  status_category: string;
  priority: string | null;
  issue_type: string | null;
  url: string;
  present_in_latest: boolean;
  last_seen_at: string;
}
export interface Task {
  id: string;
  jira_issue_id: string | null;
  title_override: string | null;
  title: string;
  next_action: string;
  notes: string;
  state: State;
  today: boolean;
  waiting_for: string | null;
  review_on: string | null;
  links: Link[];
  created_at: string;
  updated_at: string;
  version: number;
  jira: JiraFact | null;
}
export interface Sync {
  site: string | null;
  account_email: string | null;
  last_attempt_at: string | null;
  last_success_at: string | null;
  error_code: string | null;
  issue_count: number;
  open_count: number;
  done_count: number;
  refresh_in_progress: boolean;
}
export interface Board { local_date: string; tasks: Task[]; sync: Sync }
export interface CreateTask { title: string; next_action?: string; notes?: string; links?: Link[] }
export interface UpdateTask {
  version: number;
  title_override: string | null;
  next_action: string;
  notes: string;
  state: State;
  today: boolean;
  waiting_for: string | null;
  review_on: string | null;
  links: Link[];
}
export interface TriageInput { items: { id: string; version: number }[]; state: 'next' | 'later' | 'done'; review_on: string | null }

export class ApiError extends Error {
  constructor(public readonly code: string, message: string, public readonly status: number) { super(message); }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: { ...(init.body ? { 'Content-Type': 'application/json' } : {}), ...init.headers }
  });
  if (!response.ok) {
    let code = 'internal_error';
    let message = `Request failed (${response.status})`;
    try {
      const body = await response.json() as { error?: string; message?: string };
      code = body.error ?? code;
      message = body.message ?? message;
    } catch { /* retain safe status text */ }
    throw new ApiError(code, message, response.status);
  }
  return response.json() as Promise<T>;
}

export const api = {
  board: () => request<Board>('/api/board'),
  create: (input: CreateTask) => request<Task>('/api/tasks', { method: 'POST', body: JSON.stringify(input) }),
  update: (id: string, input: UpdateTask) => request<Task>(`/api/tasks/${encodeURIComponent(id)}`, { method: 'PUT', body: JSON.stringify(input) }),
  triage: (input: TriageInput) => request<Task[]>('/api/tasks/triage', { method: 'POST', body: JSON.stringify(input) }),
  refresh: () => request<Sync>('/api/jira/refresh', { method: 'POST', body: '{}' })
};

export function refreshErrorText(code: string | null): string {
  switch (code) {
    case 'jira_auth_required': return 'Jira authentication is required. In a terminal, run acli jira auth login, then refresh again.';
    case 'jira_identity_changed': return 'The authenticated Jira account or site changed. Select the intended account in acli before refreshing.';
    case 'jira_timeout': return 'Jira refresh timed out. Your previous snapshot is still available; try again later.';
    case 'jira_invalid_response': return 'Jira returned data Workboard could not safely use. Your previous snapshot is unchanged.';
    case 'jira_unavailable': return 'Jira is unavailable. Your previous snapshot and local tasks remain available.';
    default: return 'The latest Jira refresh failed. Your previous snapshot remains available.';
  }
}
