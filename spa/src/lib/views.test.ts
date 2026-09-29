import { describe, expect, test } from 'bun:test';
import type { Task } from './api';
import { isReviewDue, reviewDueTasks, todayTasks } from './views';

function task(id: string, state: Task['state'], review_on: string | null, today = false): Task {
  return {
    id, jira_issue_id: null, title_override: id, title: id, next_action: '', notes: '', state, today,
    waiting_for: null, review_on, links: [], created_at: `2026-01-0${id === 'first' ? 1 : 2}T00:00:00Z`,
    updated_at: '', version: 1, jira: null
  };
}

describe('date-driven morning views', () => {
  test('includes due today and overdue, but not tomorrow', () => {
    const due = task('due', 'waiting', '2026-03-14');
    const overdue = task('old', 'later', '2026-03-13');
    const tomorrow = task('future', 'waiting', '2026-03-15');
    expect(isReviewDue(due, '2026-03-14')).toBe(true);
    expect(reviewDueTasks([tomorrow, due, overdue], '2026-03-14').map(item => item.id)).toEqual(['old', 'due']);
  });

  test('Done is never due and Today contains only selected Next tasks', () => {
    const done = task('done', 'done', '2026-03-10', true);
    const waiting = task('waiting', 'waiting', '2026-03-10', true);
    const selected = task('first', 'next', null, true);
    const unselected = task('next', 'next', null);
    expect(isReviewDue(done, '2026-03-14')).toBe(false);
    expect(todayTasks([done, unselected, waiting, selected]).map(item => item.id)).toEqual(['first']);
  });
});
