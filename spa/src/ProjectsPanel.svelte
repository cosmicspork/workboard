<script lang="ts">
  import type { Project, ProjectInput } from './lib/api';
  import { parseJiraKeys } from './lib/views';
  import { Banner, Empty, Row, SideButton } from '@tracon/ui/svelte';
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

{#if error}<Banner tone="crit" role="alert">{error}</Banner>{/if}
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
            <div class="editor-actions"><button type="button" class="lnk" onclick={() => editing = null}>Cancel</button><button type="submit" class="btn p" disabled={busy}>Save project</button></div>
          </form>
        {:else}
          <Row>
            <div class="project-main">
              <button class="task-title" onclick={() => startEdit(project)}>{project.name}</button>
              <div class="task-meta">
                <span>{counts[project.id] ?? 0} open</span>
                {#if project.target_on}<span>target {dateLabel(project.target_on)}</span>{/if}
                {#each project.jira_keys as key (key)}<span class="tag">{key}</span>{/each}
              </div>
            </div>
            {#if confirming === project.id}
              <span class="confirm"><span class="note">Delete {project.name}? Its tasks keep their state and lose the project.</span><button class="btn d" onclick={() => run(() => onDelete(project))}>Delete</button><button class="lnk" onclick={() => confirming = null}>Keep</button></span>
            {/if}
            {#snippet actions()}
              <SideButton label={`Move ${project.name} up`} disabled={index === 0 || busy} onclick={() => move(index, -1)}>↑</SideButton>
              <SideButton label={`Move ${project.name} down`} disabled={index === projects.length - 1 || busy} onclick={() => move(index, 1)}>↓</SideButton>
              <SideButton label={`Delete ${project.name}`} disabled={confirming === project.id} onclick={() => confirming = project.id}>×</SideButton>
            {/snippet}
          </Row>
        {/if}
      </li>
    {/each}
  </ol>
{:else}
  <Empty>No projects yet. Add your priorities in order; tasks group under them in Next, Waiting, and Later.</Empty>
{/if}
<form class="add-panel project-add" onsubmit={create}>
  <div class="panel-title"><h2>Add project</h2></div>
  <div class="form-grid"><label>Name<input bind:value={newName} maxlength="200" required placeholder="App4adm to Kubernetes" /></label><label>Target date<input type="date" bind:value={newTarget} /></label></div>
  <label>Jira keys<input bind:value={newKeys} placeholder="APP-264, WEB-22, or a project key like APP" /></label>
  <div class="editor-actions"><button type="submit" class="btn" disabled={busy}>Add project</button></div>
</form>
