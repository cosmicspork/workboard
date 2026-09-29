<script lang="ts">
  import { onMount } from 'svelte';
  import TaskEditor from './TaskEditor.svelte';
  import TaskRow from './TaskRow.svelte';
  import { api, refreshErrorText, type Board, type CreateTask, type State, type Task, type UpdateTask } from './lib/api';
  import { filterTasks, inboxTasks, laneTasks, navigationCounts, projectPrefixes, reviewDueTasks, todayTasks } from './lib/views';

  const lanes: { id: State | 'morning'; label: string }[] = [
    { id: 'morning', label: 'Morning' }, { id: 'inbox', label: 'Inbox' }, { id: 'next', label: 'Next' },
    { id: 'waiting', label: 'Waiting' }, { id: 'later', label: 'Later' }, { id: 'done', label: 'Done' }
  ];
  let board = $state<Board | null>(null);
  let view = $state<State | 'morning'>('morning');
  let loading = $state(true);
  let loadError = $state('');
  let refreshBusy = $state(false);
  let query = $state('');
  let project = $state('');
  let selected = $state<string[]>([]);
  let reviewDate = $state('');
  let editorTask = $state<Task | null>(null);
  let showAdd = $state(false);
  let addTitle = $state('');
  let addAction = $state('');
  let addNotes = $state('');
  let addLinkLabel = $state('');
  let addLinkUrl = $state('');
  let addError = $state('');
  let mutationError = $state('');
  let adding = $state(false);

  function readHash() {
    const value = location.hash.replace(/^#\/?/, '') as State | 'morning';
    view = lanes.some(lane => lane.id === value) ? value : 'morning';
    selected = [];
  }
  async function loadBoard(showSpinner = false) {
    if (showSpinner) loading = true;
    try {
      board = await api.board();
      loadError = '';
    } catch (error) {
      loadError = error instanceof Error ? error.message : 'Could not load the board.';
    } finally { loading = false; }
  }
  function counts() { return board ? navigationCounts(board) : { inbox: 0, next: 0, waiting: 0, later: 0, done: 0 }; }
  async function reloadConflict(id: string) {
    await loadBoard();
    return board?.tasks.find(task => task.id === id);
  }
  function filtered(tasks: Task[]) { return filterTasks(tasks, query, project); }
  function projectOptions() { return board ? projectPrefixes(board.tasks) : []; }
  function toggleSelected(id: string) { selected = selected.includes(id) ? selected.filter(item => item !== id) : [...selected, id]; }
  function toggleAll(items: Task[]) { selected = selected.length === items.length ? [] : items.map(task => task.id); }
  function selectedVisible(items: Task[]) { return items.filter(task => selected.includes(task.id)); }
  async function mutate<T>(operation: () => Promise<T>) {
    mutationError = '';
    const result = await operation();
    await loadBoard();
    return result;
  }
  async function saveEdit(input: UpdateTask) {
    if (!editorTask) return;
    await mutate(() => api.update(editorTask!.id, input));
    editorTask = null;
  }
  async function saveNew(event: SubmitEvent) {
    event.preventDefault();
    addError = '';
    adding = true;
    const links = addLinkUrl.trim() ? [{ label: addLinkLabel.trim() || addLinkUrl.trim(), url: addLinkUrl.trim() }] : [];
    const input: CreateTask = { title: addTitle, next_action: addAction, notes: addNotes, links };
    try {
      await mutate(() => api.create(input));
      addTitle = ''; addAction = ''; addNotes = ''; addLinkLabel = ''; addLinkUrl = '';
      showAdd = false;
    } catch (error) {
      addError = error instanceof Error ? error.message : 'Could not create task.';
    } finally { adding = false; }
  }
  async function bulk(state: 'next' | 'later' | 'done') {
    if (!board) return;
    const visibleItems = selectedVisible(inboxVisible());
    if (!visibleItems.length) return;
    mutationError = '';
    try {
      await mutate(() => api.triage({
        items: visibleItems.map(task => ({ id: task.id, version: task.version })),
        state,
        review_on: state === 'later' ? (reviewDate || null) : null
      }));
      selected = []; reviewDate = '';
    } catch (error) { mutationError = error instanceof Error ? error.message : 'Bulk update failed.'; }
  }
  async function refreshJira() {
    refreshBusy = true;
    try { await api.refresh(); }
    catch { /* the board carries stable error metadata and recovery guidance */ }
    finally { refreshBusy = false; await loadBoard(); }
  }
  function shownTasks(): Task[] {
    if (!board) return [];
    return filtered(laneTasks(board.tasks, view as State));
  }
  function morningToday() { return board ? filtered(todayTasks(board.tasks)) : []; }
  function morningDue() { return board ? filtered(reviewDueTasks(board.tasks, board.local_date)) : []; }
  function inboxVisible() { return board ? filtered(inboxTasks(board.tasks)) : []; }
  function relativeDay(date: string) { return new Date(`${date}T12:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }); }

  onMount(() => {
    readHash();
    void loadBoard(true);
    window.addEventListener('hashchange', readHash);
    const refreshOnFocus = () => { if (document.visibilityState === 'visible') void loadBoard(); };
    window.addEventListener('focus', refreshOnFocus);
    document.addEventListener('visibilitychange', refreshOnFocus);
    const timer = window.setInterval(() => { if (document.visibilityState === 'visible') void loadBoard(); }, 60_000);
    return () => {
      window.removeEventListener('hashchange', readHash);
      window.removeEventListener('focus', refreshOnFocus);
      document.removeEventListener('visibilitychange', refreshOnFocus);
      window.clearInterval(timer);
    };
  });
</script>

<svelte:head><title>{view === 'morning' ? 'Morning' : lanes.find(item => item.id === view)?.label} · Workboard</title></svelte:head>
<div class="app-shell">
  <aside class="sidebar">
    <a class="brand" href="#/morning" aria-label="Workboard home"><span class="brand-mark">w</span><span>workboard</span></a>
    <p class="nav-label">YOUR BOARD</p>
    <nav aria-label="Workboard views">
      {#each lanes as lane (lane.id)}
        <a href={`#/${lane.id}`} class:active={view === lane.id} aria-current={view === lane.id ? 'page' : undefined}>
          <span>{lane.label}</span>{#if lane.id !== 'morning'}<span class="nav-count">{counts()[lane.id]}</span>{/if}
        </a>
      {/each}
    </nav>
    <div class="sidebar-bottom"><div class="sync-dot" class:syncing={board?.sync.refresh_in_progress || refreshBusy}></div><div><strong>Jira snapshot</strong><small>{board?.sync.last_success_at ? `Updated ${new Date(board.sync.last_success_at).toLocaleString()}` : 'Never refreshed'}</small></div></div>
  </aside>

  <main class="main-content">
    <header class="topbar">
      <div class="breadcrumbs">WORKBOARD <span>/</span> {lanes.find(item => item.id === view)?.label.toUpperCase()}</div>
      <div class="top-actions"><button class="quiet-button" onclick={() => showAdd = !showAdd}>＋ Add task</button><button class="primary-button refresh-button" onclick={refreshJira} disabled={refreshBusy || board?.sync.refresh_in_progress}>{refreshBusy || board?.sync.refresh_in_progress ? 'Refreshing…' : '↻ Refresh Jira'}</button></div>
    </header>
    <div class="content-wrap">
      {#if loadError}<div class="alert" role="alert"><strong>Could not load board</strong><span>{loadError}</span><button class="quiet-button" onclick={() => loadBoard(true)}>Try again</button></div>{/if}
      {#if board?.sync.error_code}<div class="alert jira-alert" role="status"><span>{refreshErrorText(board.sync.error_code)}</span></div>{/if}
      {#if mutationError}<div class="alert" role="alert">{mutationError}<button class="quiet-button" onclick={() => loadBoard()}>Reload board</button></div>{/if}

      {#if showAdd}
        <form class="add-panel" onsubmit={saveNew}>
          <div class="panel-title"><div><p class="eyebrow">CAPTURE A COMMITMENT</p><h2>New task</h2></div><button class="icon-button" type="button" aria-label="Close new task form" onclick={() => showAdd = false}>×</button></div>
          <label>Title<input bind:value={addTitle} maxlength="500" required placeholder="What needs doing?" /></label>
          <div class="form-grid"><label>Next action<input bind:value={addAction} maxlength="16384" placeholder="First concrete step (optional)" /></label><label>HTTPS or HTTP link<input bind:value={addLinkUrl} type="url" maxlength="2048" placeholder="https://…" /></label></div>
          {#if addLinkUrl}<label>Link label<input bind:value={addLinkLabel} maxlength="200" placeholder="Optional display label" /></label>{/if}
          <label>Notes<textarea bind:value={addNotes} maxlength="16384" rows="3"></textarea></label>
          {#if addError}<p class="form-error" role="alert">{addError}</p>{/if}
          <div class="editor-actions"><button class="quiet-button" type="button" onclick={() => showAdd = false}>Cancel</button><button class="primary-button" type="submit" disabled={adding}>{adding ? 'Adding…' : 'Add to Inbox'}</button></div>
        </form>
      {/if}

      {#if loading && !board}<div class="loading">Loading your board…</div>
      {:else if view === 'morning'}
        <section class="page-heading"><div><p class="eyebrow">{board?.local_date ? relativeDay(board.local_date) : ''}</p><h1>Good morning<span>.</span></h1><p class="subheading">A clear view of what matters today.</p></div><button class="date-chip" type="button" aria-label="Today's date">◷ {board?.local_date}</button></section>
        <div class="list-toolbar"><label class="search-box"><span aria-hidden="true">⌕</span><input bind:value={query} placeholder="Search title, next action, Jira key" aria-label="Search morning tasks" />{#if query}<button type="button" aria-label="Clear search" onclick={() => query = ''}>×</button>{/if}</label><label class="project-filter">Project<select bind:value={project}><option value="">All work</option><option value="standalone">Standalone</option>{#each projectOptions() as prefix}<option value={prefix}>{prefix}</option>{/each}</select></label></div>
        <section class="morning-section"><div class="section-heading"><div><span class="section-kicker">FOCUS</span><h2>Today <span class="count-pill">{morningToday().length}</span></h2></div><a href="#/next">Manage Next <span aria-hidden="true">↗</span></a></div>
          {#if morningToday().length}<div class="task-list">{#each morningToday() as task (task.id)}<TaskRow {task} onEdit={() => editorTask = task} />{/each}</div>
          {:else}<div class="empty-state"><span class="empty-icon">✳</span><div><strong>No focus items yet</strong><p>Choose tasks from Next to put them in Today.</p></div><a class="text-link" href="#/next">Explore Next →</a></div>{/if}
        </section>
        <section class="morning-section"><div class="section-heading"><div><span class="section-kicker">RESURFACING</span><h2>Review due <span class="count-pill">{morningDue().length}</span></h2></div><span class="muted">Waiting and Later commitments</span></div>
          {#if morningDue().length}<div class="task-list">{#each morningDue() as task (task.id)}<TaskRow {task} onEdit={() => editorTask = task} />{/each}</div>
          {:else}<div class="quiet-empty">Nothing needs review today.</div>{/if}
        </section>
        <section class="inbox-preview"><div><span class="section-kicker">TO SORT</span><h2>Inbox <span class="count-pill">{counts().inbox}</span></h2><p>Untriaged work, kept out of your focus list.</p></div><a class="primary-button" href="#/inbox">Review inbox →</a></section>
      {:else}
        <section class="page-heading lane-heading"><div><p class="eyebrow">YOUR COMMITMENTS</p><h1>{lanes.find(item => item.id === view)?.label}<span>.</span></h1><p class="subheading">{view === 'inbox' ? `${counts().inbox} items to triage` : view === 'next' ? 'Ready for your next actions.' : view === 'waiting' ? 'Work paused on someone or something else.' : view === 'later' ? 'Intentionally set aside for review.' : 'Completed or dismissed commitments.'}</p></div></section>
        <div class="list-toolbar"><label class="search-box"><span aria-hidden="true">⌕</span><input bind:value={query} placeholder="Search title, next action, Jira key" aria-label="Search tasks" />{#if query}<button type="button" aria-label="Clear search" onclick={() => query = ''}>×</button>{/if}</label><label class="project-filter">Project<select bind:value={project}><option value="">All work</option><option value="standalone">Standalone</option>{#each projectOptions() as prefix}<option value={prefix}>{prefix}</option>{/each}</select></label></div>
        {#if view === 'inbox'}
          {@const tasks = inboxVisible()}
          {#if tasks.length}<div class="bulk-toolbar"><label class="check-label"><input type="checkbox" checked={selectedVisible(tasks).length === tasks.length && tasks.length > 0} onchange={() => toggleAll(tasks)} /> Select all visible</label><span>{selectedVisible(tasks).length} selected</span>{#if selectedVisible(tasks).length}<label class="bulk-date">Review date <input type="date" bind:value={reviewDate} /></label><button class="quiet-button" onclick={() => bulk('next')}>Move to Next</button><button class="quiet-button" onclick={() => bulk('later')}>Move to Later</button><button class="danger-button" onclick={() => bulk('done')}>Dismiss to Done</button>{/if}</div>
            <div class="task-list">{#each tasks as task (task.id)}<TaskRow {task} selectable selected={selected.includes(task.id)} onSelect={() => toggleSelected(task.id)} onEdit={() => editorTask = task} />{/each}</div>
          {:else}<div class="large-empty"><span>✓</span><h2>{query || project ? 'No matching tasks' : 'Inbox is clear'}</h2><p>{query || project ? 'Try changing your search or project filter.' : 'New Jira issues and personal tasks will arrive here.'}</p>{#if !query && !project}<button class="primary-button" onclick={() => showAdd = true}>＋ Add a task</button>{/if}</div>{/if}
        {:else}
          {@const tasks = shownTasks()}
          {#if tasks.length}<div class="task-list">{#each tasks as task (task.id)}<TaskRow {task} onEdit={() => editorTask = task} />{/each}</div>
          {:else}<div class="large-empty"><span>{view === 'done' ? '✓' : '↗'}</span><h2>{query || project ? 'No matching tasks' : `Nothing in ${lanes.find(item => item.id === view)?.label}`}</h2><p>{query || project ? 'Try changing your search or project filter.' : 'Tasks you move here will stay until you choose another state.'}</p>{#if view === 'next'}<button class="primary-button" onclick={() => showAdd = true}>＋ Add a task</button>{/if}</div>{/if}
        {/if}
      {/if}
    </div>
  </main>
</div>

{#if editorTask}<div class="dialog-backdrop"><div role="dialog" aria-modal="true" aria-labelledby="editor-title"><TaskEditor task={editorTask} onSave={saveEdit} onCancel={() => editorTask = null} onReload={() => reloadConflict(editorTask!.id)} /></div></div>{/if}
