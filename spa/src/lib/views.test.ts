import { describe, expect, test } from 'bun:test';
import type { Project, Task } from './api';
import { filterTasks, groupByProject, isReviewDue, knownTags, openCountsByProject, parseTags, reviewDueTasks, todayTasks, UNASSIGNED } from './views';

function task(id: string, state: Task['state'], review_on: string | null, today = false): Task {
  return {
    id, jira_issue_id: null, title_override: id, title: id, next_action: '', notes: '', state, today,
    waiting_for: null, review_on, links: [], created_at: `2026-01-0${id === 'first' ? 1 : 2}T00:00:00Z`,
    updated_at: '', version: 1, jira: null, project_id: null, effective_project_id: null, project_source: null,
    tags: [], jira_tags: []
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


function project(id: string, position: number): Project {
  return { id, name: id, position, target_on: null, jira_keys: [], created_at: '', updated_at: '', version: 1 };
}
function inProject(id: string, projectId: string | null, tags: string[] = [], jiraTags: string[] = []): Task {
  return { ...task(id, 'next', null), effective_project_id: projectId, tags, jira_tags: jiraTags };
}

describe('projects and tags', () => {
  test('groups follow project position with unassigned work last and empty groups dropped', () => {
    const groups = groupByProject(
      [inProject('a', 'second'), inProject('b', null), inProject('c', 'first'), inProject('d', 'deleted')],
      [project('second', 2), project('first', 1), project('empty', 3)]
    );
    expect(groups.map(group => group.project?.id ?? 'none')).toEqual(['first', 'second', 'none']);
    expect(groups[2].tasks.map(item => item.id)).toEqual(['b', 'd']);
  });

  test('tag filters combine with AND across user and Jira tags', () => {
    const tasks = [
      inProject('both', 'p', ['person:sam'], ['jira:app']),
      inProject('sam', 'p', ['person:sam']),
      inProject('app', null, [], ['jira:app'])
    ];
    expect(filterTasks(tasks, { query: '', projectId: '', tags: ['person:sam', 'jira:app'] }).map(item => item.id)).toEqual(['both']);
    expect(filterTasks(tasks, { query: '', projectId: UNASSIGNED, tags: [] }).map(item => item.id)).toEqual(['app']);
    expect(filterTasks(tasks, { query: '', projectId: 'p', tags: [] }).map(item => item.id)).toEqual(['both', 'sam']);
  });

  test('search covers notes and tags', () => {
    const noted = { ...inProject('noted', null), notes: 'Ask about the Cribl pipeline' };
    const tagged = inProject('tagged', null, ['repo:gateway']);
    expect(filterTasks([noted, tagged], { query: 'cribl', projectId: '', tags: [] }).map(item => item.id)).toEqual(['noted']);
    expect(filterTasks([noted, tagged], { query: 'gateway', projectId: '', tags: [] }).map(item => item.id)).toEqual(['tagged']);
  });

  test('tag parsing, known tags, and open counts', () => {
    expect(parseTags(' Person:Sam, kind:ops kind:ops ')).toEqual(['person:sam', 'kind:ops']);
    expect(knownTags([inProject('a', null, ['b'], ['a']), inProject('c', null, ['b'])])).toEqual(['a', 'b']);
    const done = { ...inProject('done', 'p'), state: 'done' as const };
    expect(openCountsByProject([inProject('a', 'p'), done, inProject('b', null)])).toEqual({ p: 1 });
  });
});
