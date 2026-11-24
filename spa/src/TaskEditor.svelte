<script lang="ts">
  import { ApiError, type Link, type Project, type State, type Task, type UpdateTask } from './lib/api';
  import { parseTags } from './lib/views';
  import { Banner, Segmented, Toggle } from '@tracon/ui/svelte';

  const states: { value: State; label: string }[] = [
    { value: 'inbox', label: 'Inbox' }, { value: 'next', label: 'Next' }, { value: 'waiting', label: 'Waiting' },
    { value: 'later', label: 'Later' }, { value: 'done', label: 'Done' }
  ];

  export let task: Task;
  export let onSave: (input: UpdateTask) => Promise<void>;
  export let onCancel: () => void;
  export let onReload: () => Promise<Task | undefined>;
  export let projects: Project[] = [];
  let titleOverride = task.title_override ?? (task.jira ? '' : task.title);
  let nextAction = task.next_action;
  let notes = task.notes;
  let state: State = task.state;
  let today = task.today;
  let waitingFor = task.waiting_for ?? '';
  let reviewOn = task.review_on ?? '';
  let links: Link[] = task.links.map(link => ({ ...link }));
  let projectId = task.project_id ?? '';
  let tags = task.tags.join(', ');
  let busy = false;
  let error = '';
  let conflict = false;
  let editVersion = task.version;
  let copied = false;

  $: derivedProject = task.project_source === 'jira' ? projects.find(project => project.id === task.effective_project_id) : undefined;
  function addLink() { links = [...links, { label: '', url: '' }]; }
  function removeLink(index: number) { links = links.filter((_, itemIndex) => itemIndex !== index); }
  function changeState(value: string) {
    state = value as State;
    if (state !== 'next') today = false;
  }
  async function copyId() {
    try {
      await navigator.clipboard.writeText(task.id);
      copied = true;
      setTimeout(() => { copied = false; }, 1500);
    } catch { copied = false; }
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    error = '';
    conflict = false;
    busy = true;
    try {
      await onSave({
        version: editVersion,
        title_override: task.jira && !titleOverride.trim() ? null : titleOverride,
        next_action: nextAction,
        notes,
        state,
        today: state === 'next' && today,
        waiting_for: state === 'waiting' && waitingFor.trim() ? waitingFor : null,
        review_on: (state === 'waiting' || state === 'later') ? (reviewOn || null) : null,
        links: links.filter(link => link.url.trim()),
        project_id: projectId || null,
        tags: parseTags(tags)
      });
    } catch (reason) {
      conflict = reason instanceof ApiError && reason.code === 'task_conflict';
      error = reason instanceof Error ? reason.message : 'Could not save changes.';
    } finally { busy = false; }
  }
  async function reloadConflict() {
    const fresh = await onReload();
    if (!fresh) return;
    editVersion = fresh.version;
    titleOverride = fresh.title_override ?? (fresh.jira ? '' : fresh.title);
    nextAction = fresh.next_action;
    notes = fresh.notes;
    state = fresh.state;
    today = fresh.today;
    waitingFor = fresh.waiting_for ?? '';
    reviewOn = fresh.review_on ?? '';
    links = fresh.links.map(link => ({ ...link }));
    projectId = fresh.project_id ?? '';
    tags = fresh.tags.join(', ');
    error = '';
    conflict = false;
  }
</script>

<form class="editor" onsubmit={submit} aria-labelledby="editor-title">
  <div class="editor-heading"><h2 id="editor-title">Edit task</h2><button class="lnk" type="button" aria-label="Close editor" onclick={onCancel}>Close</button></div>
  <div class="task-id"><span>Workboard ID</span><code>{task.id}</code><button type="button" class="lnk" onclick={copyId}>{copied ? 'Copied' : 'Copy'}</button></div>
  {#if task.jira}<p class="source-note">Imported from <strong>{task.jira.key}</strong>. Leave personal title blank to use the Jira summary: “{task.jira.summary}”.</p>{/if}
  <label>Personal title<input bind:value={titleOverride} maxlength="500" placeholder={task.jira ? task.jira.summary : 'Task title'} required={!task.jira} /></label>
  <label>Next action<textarea bind:value={nextAction} maxlength="16384" rows="2" placeholder="The next concrete step"></textarea></label>
  <div class="form-grid">
    <div class="field"><span class="field-label">State</span><Segmented options={states} value={state} label="State" onchange={changeState} /></div>
    {#if state === 'next'}<Toggle checked={today} title="Select for Today" hint="Focus on it in Morning." onchange={(value) => (today = value)} />{/if}
    {#if state === 'waiting'}<label>Waiting for<input bind:value={waitingFor} maxlength="500" /></label>{/if}
    {#if state === 'waiting' || state === 'later'}<label>Review on<input type="date" bind:value={reviewOn} /></label>{/if}
  </div>
  <div class="form-grid">
    <label>Project<select bind:value={projectId}><option value="">{derivedProject ? `Automatic (from Jira: ${derivedProject.name})` : 'No project'}</option>{#each projects as project (project.id)}<option value={project.id}>{project.name}</option>{/each}</select></label>
    <label>Tags<input bind:value={tags} maxlength="1400" placeholder="person:sam, kind:ops" /></label>
  </div>
  {#if task.jira_tags.length}<p class="field-hint">From Jira: {task.jira_tags.join(', ')}</p>{/if}
  <label>Notes<textarea bind:value={notes} maxlength="16384" rows="5"></textarea></label>
  <fieldset><legend>Links</legend>
    {#each links as link, index (index)}
      <div class="link-edit"><label>Label<input bind:value={link.label} maxlength="200" /></label><label>URL<input type="url" bind:value={link.url} maxlength="2048" placeholder="https://…" /></label><button class="lnk d" type="button" aria-label="Remove link" onclick={() => removeLink(index)}>Remove</button></div>
    {/each}
    {#if links.length < 20}<button type="button" class="lnk" onclick={addLink}>Add link</button>{/if}
  </fieldset>
  {#if error}<Banner tone="crit" role="alert">{error}{#snippet action()}{#if conflict}<button type="button" class="lnk" onclick={reloadConflict}>Reload latest task</button>{/if}{/snippet}</Banner>{/if}
  <div class="editor-actions"><button type="button" class="lnk" onclick={onCancel}>Cancel</button><button type="submit" class="btn p" disabled={busy}>{busy ? 'Saving…' : 'Save changes'}</button></div>
</form>
