<script lang="ts">
  import { Row, SideButton } from '@tracon/ui/svelte';
  import type { Task } from './lib/api';
  let { task, onEdit, selectable = false, selected = false, onSelect = () => {}, projectName = null, onTag = () => {}, today = '' }: {
    task: Task;
    onEdit: () => void;
    selectable?: boolean;
    selected?: boolean;
    onSelect?: () => void;
    projectName?: string | null;
    onTag?: (tag: string) => void;
    /** The board's local date, YYYY-MM-DD: a review on or before it is due. */
    today?: string;
  } = $props();
  function relativeDay(date: string) {
    return new Date(`${date}T12:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }
  // State is the 3px bar and its wash. Amber only for what waits on you: a
  // review that is due. Today's focus is the accent; waiting on someone else
  // is quiet; done is green.
  const reviewDue = $derived(Boolean(task.review_on && today && task.review_on <= today && (task.state === 'waiting' || task.state === 'later')));
  const tone = $derived(reviewDue ? 'due' : task.state === 'next' && task.today ? 'today' : task.state === 'done' ? 'done' : task.state === 'waiting' ? 'waiting' : '');
</script>

<div class="task-row {tone}">
  <Row gap={0}>
    {#snippet lead()}<span class="bar" aria-hidden="true"></span>{#if selectable}<input class="row-checkbox" type="checkbox" aria-label={`Select ${task.title}`} checked={selected} onchange={onSelect} />{/if}{/snippet}
    <div class="task-main"><button class="task-title" onclick={onEdit}>{task.title}</button>
      {#if task.next_action}<p class="next-action"><span aria-hidden="true">↳</span> {task.next_action}</p>{/if}
      <div class="task-meta"><span class="state-tag">{task.state === 'next' && task.today ? 'today' : task.state}</span>{#if task.jira}{#if task.jira.url}<a class="jira-key" href={task.jira.url} target="_blank" rel="noopener noreferrer">{task.jira.key}</a>{:else}<span class="jira-key">{task.jira.key}</span>{/if}<span>{task.jira.status_name}</span>{#if task.jira.priority}<span class="tag">{task.jira.priority}</span>{/if}{#if task.jira.status_category === 'done'}<span class="tag done">Source done</span>{/if}{#if !task.jira.present_in_latest}<span class="tag">Not in latest assigned results</span>{/if}{/if}{#if task.review_on}<span class="review-tag" class:due={reviewDue}>Review {relativeDay(task.review_on)}</span>{/if}{#if task.state === 'waiting' && task.waiting_for}<span>Waiting for {task.waiting_for}</span>{/if}{#if task.links.length}<span>{task.links.length} link{task.links.length === 1 ? '' : 's'}</span>{/if}{#if projectName}<span class="project-tag">{projectName}</span>{/if}</div>
      {#if task.tags.length || task.jira_tags.length}<div class="task-tags">{#each task.tags as tag (tag)}<button type="button" class="tag-chip" onclick={() => onTag(tag)}>{tag}</button>{/each}{#each task.jira_tags as tag (tag)}<button type="button" class="tag-chip jira-chip" onclick={() => onTag(tag)}>{tag}</button>{/each}</div>{/if}
      {#if task.links.length}<div class="task-links">{#each task.links as link}<a class="lnk" href={link.url} target="_blank" rel="noopener noreferrer">{link.label || link.url}</a>{/each}</div>{/if}
    </div>
    {#snippet actions()}<SideButton onclick={onEdit} label={`Edit ${task.title}`}>edit</SideButton>{/snippet}
  </Row>
</div>
