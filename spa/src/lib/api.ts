export type State = 'inbox' | 'next' | 'waiting' | 'later' | 'done';
export type Lane = Exclude<State, 'inbox'> | 'inbox';
export interface Link { label: string; url: string }
export interface ParentIssue { key: string; summary: string | null; issue_type: string | null }
export interface JiraFact {
  key: string;
  summary: string;
  status_name: string;
  status_category: string;
  priority: string | null;
  issue_type: string | null;
  url: string | null;
  present_in_latest: boolean;
  last_seen_at: string;
  labels: string[];
  parent: ParentIssue | null;
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
  project_id: string | null;
  effective_project_id: string | null;
  project_source: 'manual' | 'jira' | null;
  tags: string[];
  jira_tags: string[];
}
export interface Project {
  id: string;
  name: string;
  position: number;
  target_on: string | null;
  jira_keys: string[];
  created_at: string;
  updated_at: string;
  version: number;
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
export interface Board { local_date: string; projects: Project[]; tasks: Task[]; sync: Sync }
export interface CreateTask { title: string; next_action?: string; notes?: string; links?: Link[]; project_id?: string | null; tags?: string[] }
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
  project_id: string | null;
  tags: string[];
}
export interface ProjectInput { name: string; target_on: string | null; jira_keys: string[] }
export interface VersionedId { id: string; version: number }
export interface TriageInput { items: { id: string; version: number }[]; state: 'next' | 'later' | 'done'; review_on: string | null }

export class ApiError extends Error {
  constructor(public readonly code: string, message: string, public readonly status: number) { super(message); }
}

// Paths are relative to the page, so they reach /api standalone and /t/<token>/api under Tracon.
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
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

export const api = {
  board: () => request<Board>('api/board'),
  create: (input: CreateTask) => request<Task>('api/tasks', { method: 'POST', body: JSON.stringify(input) }),
  update: (id: string, input: UpdateTask) => request<Task>(`api/tasks/${encodeURIComponent(id)}`, { method: 'PUT', body: JSON.stringify(input) }),
  triage: (input: TriageInput) => request<Task[]>('api/tasks/triage', { method: 'POST', body: JSON.stringify(input) }),
  refresh: () => request<Sync>('api/jira/refresh', { method: 'POST', body: '{}' }),
  assignProject: (items: VersionedId[], project_id: string | null) => request<Task[]>('api/tasks/project', { method: 'POST', body: JSON.stringify({ items, project_id }) }),
  createProject: (input: ProjectInput) => request<Project>('api/projects', { method: 'POST', body: JSON.stringify(input) }),
  updateProject: (id: string, version: number, input: ProjectInput) => request<Project>(`api/projects/${encodeURIComponent(id)}`, { method: 'PUT', body: JSON.stringify({ version, ...input }) }),
  orderProjects: (ids: string[]) => request<Project[]>('api/projects/order', { method: 'POST', body: JSON.stringify({ ids }) }),
  deleteProject: (id: string, version: number) => request<void>(`api/projects/${encodeURIComponent(id)}`, { method: 'DELETE', body: JSON.stringify({ version }) })
};

export function refreshErrorText(code: string | null): string {
  switch (code) {
    case 'tracon_unavailable': return 'Tracon did not answer the Jira refresh. Restart Workboard in Tracon, then refresh again.';
    case 'tracon_unsupported': return 'This Tracon node does not let Workboard call jira-review. Update Tracon, then restart Workboard in Tracon.';
    case 'tracon_forbidden': return 'Tracon refused Workboard\'s call. Restart Workboard in Tracon, then refresh again.';
    case 'jira_refused': return 'Tracon did not let Workboard run jira-review\'s search: its policy or the [[plugin]] entry for jira-review refused it, or this channel binds no Jira credential. The plugin log says which. Your previous snapshot remains available.';
    case 'jira_review_missing': return 'jira-review is not installed. Under Tracon, add the jira-review plugin; standalone, put jira-review on PATH or name it with --jira-review. Then refresh again.';
    case 'jira_identity_changed': return "jira-review's Jira account or site differs from the one this board is bound to. Your previous snapshot is unchanged.";
    case 'jira_timeout': return 'Jira refresh timed out. Your previous snapshot is still available; try again later.';
    case 'jira_invalid_response': return 'Jira returned data Workboard could not safely use. Your previous snapshot is unchanged.';
    case 'jira_unavailable': return 'jira-review could not search Jira. Check its Jira credential (under Tracon, the channel\'s jira binding; standalone, jira-review\'s own configuration). Your previous snapshot remains available.';
    default: return 'The latest Jira refresh failed. Your previous snapshot remains available.';
  }
}
