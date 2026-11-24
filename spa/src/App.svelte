<script lang="ts">
  import { onMount } from 'svelte';
  import FilterBar from './FilterBar.svelte';
  import ProjectsPanel from './ProjectsPanel.svelte';
  import TaskEditor from './TaskEditor.svelte';
  import TaskRow from './TaskRow.svelte';
  import { api, ApiError, refreshErrorText, type Board, type CreateTask, type Project, type ProjectInput, type State, type Task, type UpdateTask } from './lib/api';
  import { Banner, Empty, SectionHeader } from '@tracon/ui/svelte';
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
    catch (error) {
      // A missing Jira site is configuration, not a failed sync, so the board carries no record of it.
      if (error instanceof ApiError && error.code === 'jira_site_missing') mutationError = error.message;
      /* otherwise the board carries stable error metadata and recovery guidance */
    }
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
{#snippet rows(tasks: Task[], showProject = true)}
  <div class="task-list">{#each tasks as task (task.id)}<TaskRow {task} today={board?.local_date ?? ''} projectName={showProject ? projectName(task) : null} onTag={addTagFilter} onEdit={() => editorTask = task} />{/each}</div>
{/snippet}
<div class="app-shell">
  <aside class="sidebar">
    <nav aria-label="Workboard views">
      {#each lanes as lane (lane.id)}
        <a href={`#/${lane.id}`} class:active={view === lane.id} aria-current={view === lane.id ? 'page' : undefined}>
          <span>{lane.label}</span>{#if lane.id !== 'morning'}<span class="nav-count" class:waiting={lane.id === 'inbox' && counts().inbox > 0}>{counts()[lane.id]}</span>{/if}
        </a>
      {/each}
    </nav>
    <div class="sidebar-bottom"><span class="sync-dot" class:syncing={board?.sync.refresh_in_progress || refreshBusy}></span><div><strong>Jira snapshot</strong><small>{board?.sync.last_success_at ? `Updated ${new Date(board.sync.last_success_at).toLocaleString()}` : 'Never refreshed'}</small></div></div>
  </aside>

  <main class="main-content">
    <header class="topbar">
      <button class="btn" onclick={() => showAdd = !showAdd}>＋ Add task</button>
      <button class="btn" onclick={refreshJira} disabled={refreshBusy || board?.sync.refresh_in_progress}>{refreshBusy || board?.sync.refresh_in_progress ? 'Refreshing…' : '↻ Refresh Jira'}</button>
    </header>
    <div class="content-wrap">
      {#if loadError}<Banner tone="crit" role="alert" detail={loadError}>Could not load the board{#snippet action()}<button class="lnk" onclick={() => loadBoard(true)}>Try again</button>{/snippet}</Banner>{/if}
      {#if board?.sync.error_code}<Banner tone="crit" role="status">{refreshErrorText(board.sync.error_code)}</Banner>{/if}
      {#if mutationError}<Banner tone="crit" role="alert">{mutationError}{#snippet action()}<button class="lnk" onclick={() => loadBoard()}>Reload board</button>{/snippet}</Banner>{/if}

      {#if showAdd}
        <form class="add-panel" onsubmit={saveNew}>
          <div class="panel-title"><h2>New task</h2><button class="lnk" type="button" aria-label="Close new task form" onclick={() => showAdd = false}>Close</button></div>
          <label>Title<input bind:value={addTitle} maxlength="500" required placeholder="What needs doing?" /></label>
          <div class="form-grid"><label>Next action<input bind:value={addAction} maxlength="16384" placeholder="First concrete step (optional)" /></label><label>HTTPS or HTTP link<input bind:value={addLinkUrl} type="url" maxlength="2048" placeholder="https://…" /></label></div>
          {#if addLinkUrl}<label>Link label<input bind:value={addLinkLabel} maxlength="200" placeholder="Optional display label" /></label>{/if}
          <div class="form-grid"><label>Project<select bind:value={addProject}><option value="">No project</option>{#each projects() as project (project.id)}<option value={project.id}>{project.name}</option>{/each}</select></label><label>Tags<input bind:value={addTags} maxlength="1400" placeholder="person:sam, kind:ops" /></label></div>
          <label>Notes<textarea bind:value={addNotes} maxlength="16384" rows="3"></textarea></label>
          {#if addError}<Banner tone="crit" role="alert">{addError}</Banner>{/if}
          <div class="editor-actions"><button class="lnk" type="button" onclick={() => showAdd = false}>Cancel</button><button class="btn p" type="submit" disabled={adding}>{adding ? 'Adding…' : 'Add to Inbox'}</button></div>
        </form>
      {/if}

      {#if loading && !board}<Empty loading>Loading your board…</Empty>
      {:else if view === 'morning'}
        <section class="page-heading"><h1>Good morning</h1><span class="date">{board?.local_date ?? ''}</span><p class="subheading">A clear view of what matters today.</p></section>
        <FilterBar bind:filter projects={projects()} tags={tagOptions()} label="Search morning tasks" />
        <section class="morning-section">
          <SectionHeader detail={morningToday().length}>Today{#snippet aside()}<a class="lnk" href="#/next">Manage Next</a>{/snippet}</SectionHeader>
          {#if morningToday().length}{@render rows(morningToday())}
          {:else}<Empty>No focus items yet. Choose tasks from Next to put them in Today.{#snippet action()}<a class="lnk" href="#/next">Explore Next</a>{/snippet}</Empty>{/if}
        </section>
        <section class="morning-section">
          <SectionHeader detail={morningDue().length}>Review due{#snippet aside()}<span class="note">Waiting and Later commitments</span>{/snippet}</SectionHeader>
          {#if morningDue().length}{@render rows(morningDue())}
          {:else}<Empty>Nothing needs review today.</Empty>{/if}
        </section>
        <section class="morning-section">
          <SectionHeader detail={counts().inbox}>Inbox{#snippet aside()}<a class="lnk" href="#/inbox">Review inbox</a>{/snippet}</SectionHeader>
          <p class="note">Untriaged work, kept out of your focus list.</p>
        </section>
      {:else}
        <section class="page-heading"><h1>{lanes.find(item => item.id === view)?.label}</h1><p class="subheading">{subheading()}</p></section>
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
            {#if tasks.length}<div class="bulk-toolbar"><label class="check-label"><input type="checkbox" checked={selectedVisible(tasks).length === tasks.length && tasks.length > 0} onchange={() => toggleAll(tasks)} /> Select all visible</label><span class="note">{selectedVisible(tasks).length} selected</span>{#if selectedVisible(tasks).length}<label class="bulk-date">Project <select bind:value={assignTarget}><option value="">No project</option>{#each projects() as project (project.id)}<option value={project.id}>{project.name}</option>{/each}</select></label><button class="btn" onclick={bulkAssign}>Assign project</button><label class="bulk-date">Review date <input type="date" bind:value={reviewDate} /></label><button class="btn" onclick={() => bulk('next')}>Move to Next</button><button class="btn" onclick={() => bulk('later')}>Move to Later</button><button class="lnk" onclick={() => bulk('done')}>Dismiss to Done</button>{/if}</div>
              <div class="task-list">{#each tasks as task (task.id)}<TaskRow {task} today={board?.local_date ?? ''} projectName={projectName(task)} onTag={addTagFilter} selectable selected={selected.includes(task.id)} onSelect={() => toggleSelected(task.id)} onEdit={() => editorTask = task} />{/each}</div>
            {:else}<Empty>{filterActive() ? 'No matching tasks. Try changing your search, project, or tag filter.' : 'Inbox is clear. New Jira issues and personal tasks will arrive here.'}{#snippet action()}{#if !filterActive()}<button class="lnk" onclick={() => showAdd = true}>Add a task</button>{/if}{/snippet}</Empty>{/if}
          {:else}
            {@const tasks = shownTasks()}
            {#if tasks.length && grouped.includes(view)}
              {#each groupByProject(tasks, projects()) as group (group.project?.id ?? 'none')}
                <section class="project-group">
                  <SectionHeader detail={group.tasks.length}>{group.project?.name ?? 'No project'}{#snippet aside()}<span class="mono">{group.project ? `priority ${rank(group.project)}` : 'unsorted'}{group.project?.target_on ? ` · target ${relativeDay(group.project.target_on)}` : ''}</span>{/snippet}</SectionHeader>
                  {@render rows(group.tasks, false)}
                </section>
              {/each}
            {:else if tasks.length}{@render rows(tasks)}
            {:else}<Empty>{filterActive() ? 'No matching tasks. Try changing your search, project, or tag filter.' : `Nothing in ${lanes.find(item => item.id === view)?.label}. Tasks you move here will stay until you choose another state.`}{#snippet action()}{#if view === 'next' && !filterActive()}<button class="lnk" onclick={() => showAdd = true}>Add a task</button>{/if}{/snippet}</Empty>{/if}
          {/if}
        {/if}
      {/if}
    </div>
  </main>
</div>

{#if editorTask}<div class="dialog-backdrop"><div role="dialog" aria-modal="true" aria-labelledby="editor-title"><TaskEditor task={editorTask} projects={projects()} onSave={saveEdit} onCancel={() => editorTask = null} onReload={() => reloadConflict(editorTask!.id)} /></div></div>{/if}
