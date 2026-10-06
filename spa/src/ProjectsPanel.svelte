<script lang="ts">
  import type { Project, ProjectInput } from './lib/api';
  import { parseJiraKeys } from './lib/views';
  let { projects, counts, onCreate, onUpdate, onMove, onDelete }: {
    projects: Project[];
    counts: Record<string, number>;
    onCreate: (input: ProjectInput) => Promise<void>;
    onUpdate: (project: Project, input: ProjectInput) => Promise<void>;
    onMove: (ids: string[]) => Promise<void>;
    onDelete: (project: Project) => Promise<void>;
  } = $props();

  let editing = $state<string | null>(null);
  let confirming = $state<string | null>(null);
  let name = $state('');
  let targetOn = $state('');
  let keys = $state('');
  let newName = $state('');
  let newTarget = $state('');
  let newKeys = $state('');
  let error = $state('');
  let busy = $state(false);

  function startEdit(project: Project) {
    editing = project.id;
    confirming = null;
    name = project.name;
    targetOn = project.target_on ?? '';
    keys = project.jira_keys.join(', ');
    error = '';
  }
  async function run(action: () => Promise<void>) {
    error = '';
    busy = true;
    try { await action(); return true; }
    catch (reason) { error = reason instanceof Error ? reason.message : 'Could not save the project.'; return false; }
    finally { busy = false; }
  }
  async function save(event: SubmitEvent, project: Project) {
    event.preventDefault();
    if (await run(() => onUpdate(project, { name, target_on: targetOn || null, jira_keys: parseJiraKeys(keys) }))) editing = null;
  }
  async function create(event: SubmitEvent) {
    event.preventDefault();
    if (await run(() => onCreate({ name: newName, target_on: newTarget || null, jira_keys: parseJiraKeys(newKeys) }))) {
      newName = ''; newTarget = ''; newKeys = '';
    }
  }
  function move(index: number, offset: number) {
    const ids = projects.map(project => project.id);
    const [moved] = ids.splice(index, 1);
    ids.splice(index + offset, 0, moved);
    void run(() => onMove(ids));
  }
  function dateLabel(date: string) { return new Date(`${date}T12:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }); }
</script>

{#if error}<div class="alert" role="alert">{error}</div>{/if}
{#if projects.length}
  <ol class="project-list">
    {#each projects as project, index (project.id)}
      <li class="project-row">
        <span class="project-rank">{index + 1}</span>
        {#if editing === project.id}
          <form class="project-edit" onsubmit={(event) => save(event, project)}>
            <div class="form-grid"><label>Name<input bind:value={name} maxlength="200" required /></label><label>Target date<input type="date" bind:value={targetOn} /></label></div>
            <label>Jira keys<input bind:value={keys} placeholder="APP-264, WEB-22, or a project key like APP" /></label>
            <p class="field-hint">An issue or epic key matches that issue and its children; a project key catches any remaining issue in that Jira project.</p>
            <div class="editor-actions"><button type="button" class="quiet-button" onclick={() => editing = null}>Cancel</button><button type="submit" class="primary-button" disabled={busy}>Save project</button></div>
          </form>
        {:else}
          <div class="project-main">
            <button class="task-title" onclick={() => startEdit(project)}>{project.name}</button>
            <div class="task-meta">
              <span class="count-pill">{counts[project.id] ?? 0} open</span>
              {#if project.target_on}<span class="review-tag">Target {dateLabel(project.target_on)}</span>{/if}
              {#each project.jira_keys as key (key)}<span class="priority-tag">{key}</span>{/each}
            </div>
          </div>
          <div class="row-trailing">
            <button class="icon-button" aria-label={`Move ${project.name} up`} disabled={index === 0 || busy} onclick={() => move(index, -1)}>↑</button>
            <button class="icon-button" aria-label={`Move ${project.name} down`} disabled={index === projects.length - 1 || busy} onclick={() => move(index, 1)}>↓</button>
            {#if confirming === project.id}
              <button class="danger-button" onclick={() => run(() => onDelete(project))}>Delete</button><button class="quiet-button" onclick={() => confirming = null}>Keep</button>
            {:else}
              <button class="icon-button" aria-label={`Delete ${project.name}`} onclick={() => confirming = project.id}>×</button>
            {/if}
          </div>
        {/if}
      </li>
    {/each}
  </ol>
{:else}
  <div class="quiet-empty">No projects yet. Add your priorities in order; tasks group under them in Next, Waiting, and Later.</div>
{/if}
<form class="add-panel project-add" onsubmit={create}>
  <div class="panel-title"><div><p class="eyebrow">NEXT PRIORITY</p><h2>Add project</h2></div></div>
  <div class="form-grid"><label>Name<input bind:value={newName} maxlength="200" required placeholder="App4adm to Kubernetes" /></label><label>Target date<input type="date" bind:value={newTarget} /></label></div>
  <label>Jira keys<input bind:value={newKeys} placeholder="APP-264, WEB-22, or a project key like APP" /></label>
  <div class="editor-actions"><button type="submit" class="primary-button" disabled={busy}>Add project</button></div>
</form>
