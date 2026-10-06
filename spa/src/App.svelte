<script lang="ts">
  import { onMount } from 'svelte';
  import FilterBar from './FilterBar.svelte';
  import ProjectsPanel from './ProjectsPanel.svelte';
  import TaskEditor from './TaskEditor.svelte';
  import TaskRow from './TaskRow.svelte';
  import { api, refreshErrorText, type Board, type CreateTask, type Project, type ProjectInput, type State, type Task, type UpdateTask } from './lib/api';
  import { filterTasks, groupByProject, inboxTasks, knownTags, laneTasks, navigationCounts, openCountsByProject, parseTags, reviewDueTasks, todayTasks, type Filter } from './lib/views';

  type View = State | 'morning' | 'projects';
  const FILTER_KEY = 'workboard.filter';
  const lanes: { id: View; label: string }[] = [
    { id: 'morning', label: 'Morning' }, { id: 'inbox', label: 'Inbox' }, { id: 'next', label: 'Next' },
    { id: 'waiting', label: 'Waiting' }, { id: 'later', label: 'Later' }, { id: 'done', label: 'Done' },
    { id: 'projects', label: 'Projects' }
  ];
  const grouped: View[] = ['next', 'waiting', 'later'];
  let board = $state<Board | null>(null);
  let view = $state<View>('morning');
  let loading = $state(true);
  let loadError = $state('');
  let refreshBusy = $state(false);
  let filter = $state<Filter>(loadFilter());
  let selected = $state<string[]>([]);
  let reviewDate = $state('');
  let assignTarget = $state('');
  let editorTask = $state<Task | null>(null);
  let showAdd = $state(false);
  let addTitle = $state('');
  let addAction = $state('');
  let addNotes = $state('');
  let addLinkLabel = $state('');
  let addLinkUrl = $state('');
  let addProject = $state('');
  let addTags = $state('');
  let addError = $state('');
  let mutationError = $state('');
  let adding = $state(false);

  function loadFilter(): Filter {
    const empty = { query: '', projectId: '', tags: [] };
    try {
      const saved = JSON.parse(localStorage.getItem(FILTER_KEY) ?? 'null') as Partial<Filter> | null;
      if (!saved) return empty;
      return { query: typeof saved.query === 'string' ? saved.query : '', projectId: typeof saved.projectId === 'string' ? saved.projectId : '', tags: Array.isArray(saved.tags) ? saved.tags.filter(tag => typeof tag === 'string') : [] };
    } catch { return empty; }
  }
  $effect(() => {
    const snapshot = JSON.stringify(filter);
    try { localStorage.setItem(FILTER_KEY, snapshot); } catch { /* storage may be unavailable; the filter still works for this visit */ }
  });

  function readHash() {
    const value = location.hash.replace(/^#\/?/, '') as View;
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
  function counts() {
    const base = board ? navigationCounts(board) : { inbox: 0, next: 0, waiting: 0, later: 0, done: 0 };
    return { ...base, morning: 0, projects: board?.projects.length ?? 0 };
  }
  function projects(): Project[] { return board?.projects ?? []; }
  function rank(project: Project) { return projects().findIndex(item => item.id === project.id) + 1; }
  function projectName(task: Task) { return projects().find(project => project.id === task.effective_project_id)?.name ?? null; }
  function filterActive() { return Boolean(filter.query || filter.projectId || filter.tags.length); }
  function addTagFilter(tag: string) { if (!filter.tags.includes(tag)) filter.tags = [...filter.tags, tag]; }
  async function reloadConflict(id: string) {
    await loadBoard();
    return board?.tasks.find(task => task.id === id);
  }
  function filtered(tasks: Task[]) { return filterTasks(tasks, filter); }
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
    const input: CreateTask = { title: addTitle, next_action: addAction, notes: addNotes, links, project_id: addProject || null, tags: parseTags(addTags) };
    try {
      await mutate(() => api.create(input));
      addTitle = ''; addAction = ''; addNotes = ''; addLinkLabel = ''; addLinkUrl = ''; addProject = ''; addTags = '';
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
  async function bulkAssign() {
    const visibleItems = selectedVisible(inboxVisible());
    if (!visibleItems.length) return;
    mutationError = '';
    try {
      await mutate(() => api.assignProject(visibleItems.map(task => ({ id: task.id, version: task.version })), assignTarget || null));
      selected = [];
    } catch (error) { mutationError = error instanceof Error ? error.message : 'Could not assign the project.'; }
  }
  async function projectAction(action: () => Promise<unknown>) {
    await mutate(action);
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
  function tagOptions() { return board ? knownTags(board.tasks) : []; }
  function relativeDay(date: string) { return new Date(`${date}T12:00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }); }
  function subheading() {
    switch (view) {
      case 'inbox': return `${counts().inbox} items to triage`;
      case 'next': return 'Ready for your next actions, in priority order.';
      case 'waiting': return 'Work paused on someone or something else.';
      case 'later': return 'Intentionally set aside for review.';
      case 'projects': return 'Your priorities, top to bottom. Tasks group under them everywhere.';
      default: return 'Completed or dismissed commitments.';
    }
  }

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

<svelte:head><title>{lanes.find(item => item.id === view)?.label} · Workboard</title></svelte:head>
<div class="app-shell">
  <aside class="sidebar">
    <a class="brand" href="#/morning" aria-label="Workboard home"><img class="brand-mark" src="/workboard.svg" alt="" width="29" height="29" /><span>workboard</span></a>
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
          <div class="form-grid"><label>Project<select bind:value={addProject}><option value="">No project</option>{#each projects() as project (project.id)}<option value={project.id}>{project.name}</option>{/each}</select></label><label>Tags<input bind:value={addTags} maxlength="1400" placeholder="person:sam, kind:ops" /></label></div>
          <label>Notes<textarea bind:value={addNotes} maxlength="16384" rows="3"></textarea></label>
          {#if addError}<p class="form-error" role="alert">{addError}</p>{/if}
          <div class="editor-actions"><button class="quiet-button" type="button" onclick={() => showAdd = false}>Cancel</button><button class="primary-button" type="submit" disabled={adding}>{adding ? 'Adding…' : 'Add to Inbox'}</button></div>
        </form>
      {/if}

      {#if loading && !board}<div class="loading">Loading your board…</div>
      {:else if view === 'morning'}
        <section class="page-heading"><div><p class="eyebrow">{board?.local_date ? relativeDay(board.local_date) : ''}</p><h1>Good morning<span>.</span></h1><p class="subheading">A clear view of what matters today.</p></div><button class="date-chip" type="button" aria-label="Today's date">◷ {board?.local_date}</button></section>
        <FilterBar bind:filter projects={projects()} tags={tagOptions()} label="Search morning tasks" />
        <section class="morning-section"><div class="section-heading"><div><span class="section-kicker">FOCUS</span><h2>Today <span class="count-pill">{morningToday().length}</span></h2></div><a href="#/next">Manage Next <span aria-hidden="true">↗</span></a></div>
          {#if morningToday().length}<div class="task-list">{#each morningToday() as task (task.id)}<TaskRow {task} projectName={projectName(task)} onTag={addTagFilter} onEdit={() => editorTask = task} />{/each}</div>
          {:else}<div class="empty-state"><span class="empty-icon">✳</span><div><strong>No focus items yet</strong><p>Choose tasks from Next to put them in Today.</p></div><a class="text-link" href="#/next">Explore Next →</a></div>{/if}
        </section>
        <section class="morning-section"><div class="section-heading"><div><span class="section-kicker">RESURFACING</span><h2>Review due <span class="count-pill">{morningDue().length}</span></h2></div><span class="muted">Waiting and Later commitments</span></div>
          {#if morningDue().length}<div class="task-list">{#each morningDue() as task (task.id)}<TaskRow {task} projectName={projectName(task)} onTag={addTagFilter} onEdit={() => editorTask = task} />{/each}</div>
          {:else}<div class="quiet-empty">Nothing needs review today.</div>{/if}
        </section>
        <section class="inbox-preview"><div><span class="section-kicker">TO SORT</span><h2>Inbox <span class="count-pill">{counts().inbox}</span></h2><p>Untriaged work, kept out of your focus list.</p></div><a class="primary-button" href="#/inbox">Review inbox →</a></section>
      {:else}
        <section class="page-heading lane-heading"><div><p class="eyebrow">{view === 'projects' ? 'YOUR PRIORITIES' : 'YOUR COMMITMENTS'}</p><h1>{lanes.find(item => item.id === view)?.label}<span>.</span></h1><p class="subheading">{subheading()}</p></div></section>
        {#if view === 'projects'}
          <ProjectsPanel
            projects={projects()}
            counts={openCountsByProject(board?.tasks ?? [])}
            onCreate={(input: ProjectInput) => projectAction(() => api.createProject(input))}
            onUpdate={(project: Project, input: ProjectInput) => projectAction(() => api.updateProject(project.id, project.version, input))}
            onMove={(ids: string[]) => projectAction(() => api.orderProjects(ids))}
            onDelete={(project: Project) => projectAction(() => api.deleteProject(project.id, project.version))} />
        {:else}
          <FilterBar bind:filter projects={projects()} tags={tagOptions()} label="Search tasks" />
          {#if view === 'inbox'}
            {@const tasks = inboxVisible()}
            {#if tasks.length}<div class="bulk-toolbar"><label class="check-label"><input type="checkbox" checked={selectedVisible(tasks).length === tasks.length && tasks.length > 0} onchange={() => toggleAll(tasks)} /> Select all visible</label><span>{selectedVisible(tasks).length} selected</span>{#if selectedVisible(tasks).length}<label class="bulk-date">Project <select bind:value={assignTarget}><option value="">No project</option>{#each projects() as project (project.id)}<option value={project.id}>{project.name}</option>{/each}</select></label><button class="quiet-button" onclick={bulkAssign}>Assign project</button><label class="bulk-date">Review date <input type="date" bind:value={reviewDate} /></label><button class="quiet-button" onclick={() => bulk('next')}>Move to Next</button><button class="quiet-button" onclick={() => bulk('later')}>Move to Later</button><button class="danger-button" onclick={() => bulk('done')}>Dismiss to Done</button>{/if}</div>
              <div class="task-list">{#each tasks as task (task.id)}<TaskRow {task} projectName={projectName(task)} onTag={addTagFilter} selectable selected={selected.includes(task.id)} onSelect={() => toggleSelected(task.id)} onEdit={() => editorTask = task} />{/each}</div>
            {:else}<div class="large-empty"><span>✓</span><h2>{filterActive() ? 'No matching tasks' : 'Inbox is clear'}</h2><p>{filterActive() ? 'Try changing your search, project, or tag filter.' : 'New Jira issues and personal tasks will arrive here.'}</p>{#if !filterActive()}<button class="primary-button" onclick={() => showAdd = true}>＋ Add a task</button>{/if}</div>{/if}
          {:else}
            {@const tasks = shownTasks()}
            {#if tasks.length && grouped.includes(view)}
              {#each groupByProject(tasks, projects()) as group (group.project?.id ?? 'none')}
                <section class="project-group"><div class="section-heading"><div><span class="section-kicker">{group.project ? `PRIORITY ${rank(group.project)}` : 'UNSORTED'}</span><h2>{group.project?.name ?? 'No project'} <span class="count-pill">{group.tasks.length}</span></h2></div>{#if group.project?.target_on}<span class="review-tag">Target {relativeDay(group.project.target_on)}</span>{/if}</div>
                  <div class="task-list">{#each group.tasks as task (task.id)}<TaskRow {task} onTag={addTagFilter} onEdit={() => editorTask = task} />{/each}</div>
                </section>
              {/each}
            {:else if tasks.length}<div class="task-list">{#each tasks as task (task.id)}<TaskRow {task} projectName={projectName(task)} onTag={addTagFilter} onEdit={() => editorTask = task} />{/each}</div>
            {:else}<div class="large-empty"><span>{view === 'done' ? '✓' : '↗'}</span><h2>{filterActive() ? 'No matching tasks' : `Nothing in ${lanes.find(item => item.id === view)?.label}`}</h2><p>{filterActive() ? 'Try changing your search, project, or tag filter.' : 'Tasks you move here will stay until you choose another state.'}</p>{#if view === 'next'}<button class="primary-button" onclick={() => showAdd = true}>＋ Add a task</button>{/if}</div>{/if}
          {/if}
        {/if}
      {/if}
    </div>
  </main>
</div>

{#if editorTask}<div class="dialog-backdrop"><div role="dialog" aria-modal="true" aria-labelledby="editor-title"><TaskEditor task={editorTask} projects={projects()} onSave={saveEdit} onCancel={() => editorTask = null} onReload={() => reloadConflict(editorTask!.id)} /></div></div>{/if}
