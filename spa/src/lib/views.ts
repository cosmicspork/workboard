import type { Board, Project, Task, State } from './api';

const byCreated = (a: Task, b: Task) => a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id);
const byReview = (a: Task, b: Task) => (a.review_on ?? '9999-99-99').localeCompare(b.review_on ?? '9999-99-99') || byCreated(a, b);

export function isReviewDue(task: Task, localDate: string): boolean {
  return (task.state === 'waiting' || task.state === 'later') && task.review_on !== null && task.review_on <= localDate;
}

export function todayTasks(tasks: Task[]): Task[] {
  return tasks.filter(task => task.state === 'next' && task.today).sort(byCreated);
}

export function reviewDueTasks(tasks: Task[], localDate: string): Task[] {
  return tasks.filter(task => isReviewDue(task, localDate)).sort(byReview);
}

export function inboxTasks(tasks: Task[]): Task[] {
  return tasks.filter(task => task.state === 'inbox').sort((a, b) => {
    const priority = Number(b.jira?.status_category === 'indeterminate') - Number(a.jira?.status_category === 'indeterminate');
    return priority || byCreated(a, b);
  });
}

export function laneTasks(tasks: Task[], state: State): Task[] {
  const selected = tasks.filter(task => task.state === state);
  if (state === 'waiting' || state === 'later') return selected.sort(byReview);
  if (state === 'inbox') return inboxTasks(tasks);
  return selected.sort(byCreated);
}

export interface Filter { query: string; projectId: string; tags: string[] }
export interface ProjectGroup { project: Project | null; tasks: Task[] }

export const UNASSIGNED = 'unassigned';

export function allTags(task: Task): string[] {
  return [...task.tags, ...task.jira_tags];
}

export function filterTasks(tasks: Task[], filter: Filter): Task[] {
  const needle = filter.query.trim().toLocaleLowerCase();
  return tasks.filter(task => {
    const tags = allTags(task);
    if (needle && ![task.title, task.next_action, task.notes, task.jira?.key ?? '', ...tags].some(value => value.toLocaleLowerCase().includes(needle))) return false;
    if (filter.projectId === UNASSIGNED && task.effective_project_id !== null) return false;
    if (filter.projectId && filter.projectId !== UNASSIGNED && task.effective_project_id !== filter.projectId) return false;
    return filter.tags.every(tag => tags.includes(tag));
  });
}

export function groupByProject(tasks: Task[], projects: Project[]): ProjectGroup[] {
  const ordered = [...projects].sort((a, b) => a.position - b.position);
  const groups: ProjectGroup[] = ordered.map(project => ({ project, tasks: tasks.filter(task => task.effective_project_id === project.id) }));
  const known = new Set(ordered.map(project => project.id));
  groups.push({ project: null, tasks: tasks.filter(task => !task.effective_project_id || !known.has(task.effective_project_id)) });
  return groups.filter(group => group.tasks.length);
}

export function knownTags(tasks: Task[]): string[] {
  return [...new Set(tasks.flatMap(allTags))].sort((a, b) => a.localeCompare(b));
}

export function openCountsByProject(tasks: Task[]): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const task of tasks) {
    if (task.state === 'done' || !task.effective_project_id) continue;
    counts[task.effective_project_id] = (counts[task.effective_project_id] ?? 0) + 1;
  }
  return counts;
}

export function parseTags(raw: string): string[] {
  return [...new Set(raw.split(/[\s,]+/).map(tag => tag.trim().toLocaleLowerCase()).filter(Boolean))];
}

export function parseJiraKeys(raw: string): string[] {
  return [...new Set(raw.split(/[\s,]+/).map(key => key.trim().toUpperCase()).filter(Boolean))];
}

export function navigationCounts(board: Board): Record<State, number> {
  return {
    inbox: board.tasks.filter(task => task.state === 'inbox').length,
    next: board.tasks.filter(task => task.state === 'next').length,
    waiting: board.tasks.filter(task => task.state === 'waiting').length,
    later: board.tasks.filter(task => task.state === 'later').length,
    done: board.tasks.filter(task => task.state === 'done').length
  };
}
