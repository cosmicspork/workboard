<script lang="ts">
  import type { Project } from './lib/api';
  import { UNASSIGNED, type Filter } from './lib/views';
  let { filter = $bindable(), projects, tags, label }: { filter: Filter; projects: Project[]; tags: string[]; label: string } = $props();
  let draft = $state('');

  function addTag(raw: string) {
    const tag = raw.trim().toLocaleLowerCase();
    draft = '';
    if (tag && !filter.tags.includes(tag)) filter.tags = [...filter.tags, tag];
  }
  function removeTag(tag: string) { filter.tags = filter.tags.filter(item => item !== tag); }
  function keydown(event: KeyboardEvent) {
    if (event.key === 'Enter' || event.key === ',') { event.preventDefault(); addTag(draft); }
    if (event.key === 'Backspace' && !draft && filter.tags.length) removeTag(filter.tags[filter.tags.length - 1]);
  }
  function picked(event: Event) {
    const value = (event.currentTarget as HTMLInputElement).value;
    if (tags.includes(value.trim().toLocaleLowerCase())) addTag(value);
  }
</script>

<div class="list-toolbar">
  <label class="search-box"><span aria-hidden="true">⌕</span><input bind:value={filter.query} placeholder="Search title, notes, Jira key, tags" aria-label={label} />{#if filter.query}<button type="button" aria-label="Clear search" onclick={() => filter.query = ''}>×</button>{/if}</label>
  <label class="project-filter">Project<select bind:value={filter.projectId}><option value="">All work</option>{#each projects as project (project.id)}<option value={project.id}>{project.name}</option>{/each}<option value={UNASSIGNED}>No project</option></select></label>
</div>
<div class="tag-filter">
  {#each filter.tags as tag (tag)}<span class="tag-chip active-chip">{tag}<button type="button" aria-label={`Remove tag filter ${tag}`} onclick={() => removeTag(tag)}>×</button></span>{/each}
  <input class="tag-input" bind:value={draft} onkeydown={keydown} oninput={picked} list="known-tags" placeholder={filter.tags.length ? 'Add another tag' : 'Filter by tag, e.g. person:sam'} aria-label="Filter by tag" />
  <datalist id="known-tags">{#each tags as tag (tag)}<option value={tag}></option>{/each}</datalist>
  {#if filter.tags.length}<button type="button" class="text-button" onclick={() => filter.tags = []}>Clear tags</button>{/if}
</div>
