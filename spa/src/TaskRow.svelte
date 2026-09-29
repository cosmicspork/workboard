<script lang="ts">
  import type { Task } from './lib/api';
  let { task, onEdit, selectable = false, selected = false, onSelect = () => {} }: {
    task: Task;
    onEdit: () => void;
    selectable?: boolean;
    selected?: boolean;
    onSelect?: () => void;
  } = $props();
  function relativeDay(date: string) {
    return new Date(`${date}T12:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }
</script>

<article class="task-row">
  {#if selectable}<input class="row-checkbox" type="checkbox" aria-label={`Select ${task.title}`} checked={selected} onchange={onSelect} />{/if}
  <div class="task-main"><button class="task-title" onclick={onEdit}>{task.title}</button>
    {#if task.next_action}<p class="next-action"><span>↳</span> {task.next_action}</p>{/if}
    <div class="task-meta">{#if task.jira}<a class="jira-key" href={task.jira.url} target="_blank" rel="noopener noreferrer">{task.jira.key}</a><span>{task.jira.status_name}</span>{#if task.jira.priority}<span class="priority-tag">{task.jira.priority}</span>{/if}{#if task.jira.status_category === 'done'}<span class="badge badge-done">Source done</span>{/if}{#if !task.jira.present_in_latest}<span class="badge">Not in latest assigned results</span>{/if}{/if}{#if task.review_on}<span class="review-tag">Review {relativeDay(task.review_on)}</span>{/if}{#if task.state === 'waiting' && task.waiting_for}<span>Waiting for {task.waiting_for}</span>{/if}{#if task.links.length}<span>{task.links.length} link{task.links.length === 1 ? '' : 's'}</span>{/if}</div>
    {#if task.links.length}<div class="task-links">{#each task.links as link}<a href={link.url} target="_blank" rel="noopener noreferrer">{link.label || link.url}</a>{/each}</div>{/if}
  </div><div class="row-trailing"><span class="state-tag">{task.state === 'next' && task.today ? 'Today' : task.state}</span><button class="icon-button edit-button" onclick={onEdit} aria-label={`Edit ${task.title}`}>···</button></div>
</article>
