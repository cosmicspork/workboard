import type { Board, Task, State } from './api';

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

export function filterTasks(tasks: Task[], query: string, project: string): Task[] {
  const needle = query.trim().toLocaleLowerCase();
  const prefix = project.trim().toLocaleLowerCase();
  return tasks.filter(task => {
    if (needle && ![task.title, task.next_action, task.jira?.key ?? ''].some(value => value.toLocaleLowerCase().includes(needle))) return false;
    if (prefix === 'standalone') return task.jira === null;
    if (prefix && (!task.jira || task.jira.key.split('-')[0].toLocaleLowerCase() !== prefix)) return false;
    return true;
  });
}

export function projectPrefixes(tasks: Task[]): string[] {
  return [...new Set(tasks.flatMap(task => task.jira ? [task.jira.key.split('-')[0]] : []))].sort((a, b) => a.localeCompare(b));
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
