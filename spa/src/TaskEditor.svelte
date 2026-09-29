<script lang="ts">
  import { ApiError, type Link, type State, type Task, type UpdateTask } from './lib/api';

  export let task: Task;
  export let onSave: (input: UpdateTask) => Promise<void>;
  export let onCancel: () => void;
  export let onReload: () => Promise<Task | undefined>;
  let titleOverride = task.title_override ?? (task.jira ? '' : task.title);
  let nextAction = task.next_action;
  let notes = task.notes;
  let state: State = task.state;
  let today = task.today;
  let waitingFor = task.waiting_for ?? '';
  let reviewOn = task.review_on ?? '';
  let links: Link[] = task.links.map(link => ({ ...link }));
  let busy = false;
  let error = '';
  let conflict = false;
  let editVersion = task.version;

  function addLink() { links = [...links, { label: '', url: '' }]; }
  function removeLink(index: number) { links = links.filter((_, itemIndex) => itemIndex !== index); }
  function changeState(value: string) {
    state = value as State;
    if (state !== 'next') today = false;
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
        links: links.filter(link => link.url.trim())
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
    error = '';
    conflict = false;
  }
</script>

<form class="editor" onsubmit={submit} aria-labelledby="editor-title">
  <div class="editor-heading"><div><p class="eyebrow">PERSONAL COMMITMENT</p><h2 id="editor-title">Edit task</h2></div><button class="icon-button" type="button" aria-label="Close editor" onclick={onCancel}>×</button></div>
  {#if task.jira}<p class="source-note">Imported from <strong>{task.jira.key}</strong>. Leave personal title blank to use the Jira summary: “{task.jira.summary}”.</p>{/if}
  <label>Personal title<input bind:value={titleOverride} maxlength="500" placeholder={task.jira ? task.jira.summary : 'Task title'} required={!task.jira} /></label>
  <label>Next action<textarea bind:value={nextAction} maxlength="16384" rows="2" placeholder="The next concrete step"></textarea></label>
  <div class="form-grid">
    <label>State<select value={state} onchange={(event) => changeState(event.currentTarget.value)}><option value="inbox">Inbox</option><option value="next">Next</option><option value="waiting">Waiting</option><option value="later">Later</option><option value="done">Done</option></select></label>
    {#if state === 'next'}<label class="check-label"><input type="checkbox" bind:checked={today} /> Select for Today</label>{/if}
    {#if state === 'waiting'}<label>Waiting for<input bind:value={waitingFor} maxlength="500" /></label>{/if}
    {#if state === 'waiting' || state === 'later'}<label>Review on<input type="date" bind:value={reviewOn} /></label>{/if}
  </div>
  <label>Notes<textarea bind:value={notes} maxlength="16384" rows="5"></textarea></label>
  <fieldset><legend>Links</legend>
    {#each links as link, index (index)}
      <div class="link-edit"><label>Label<input bind:value={link.label} maxlength="200" /></label><label>URL<input type="url" bind:value={link.url} maxlength="2048" placeholder="https://…" /></label><button class="icon-button" type="button" aria-label="Remove link" onclick={() => removeLink(index)}>×</button></div>
    {/each}
    {#if links.length < 20}<button type="button" class="quiet-button" onclick={addLink}>Add link</button>{/if}
  </fieldset>
  {#if error}<p class="form-error" role="alert">{error}</p>{#if conflict}<button type="button" class="quiet-button" onclick={reloadConflict}>Reload latest task</button>{/if}{/if}
  <div class="editor-actions"><button type="button" class="quiet-button" onclick={onCancel}>Cancel</button><button type="submit" class="primary-button" disabled={busy}>{busy ? 'Saving…' : 'Save changes'}</button></div>
</form>
