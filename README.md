# Workboard

A local morning board for personal commitments, with an explicitly refreshed, read-only Jira snapshot. Jira owns ticket facts; Workboard owns your states, Today selections, priorities, tags, notes, next actions, review dates, and links. It does not update Jira or hand work to Tracon automatically.

## Install and launch

Requires Rust/Cargo and Bun for building. Jira refresh goes through a local [Tracon](https://github.com/cosmicspork/tracon) node, which holds the Jira credential; Workboard never sees it:

```sh
bun install --cwd spa
bun run --cwd spa build
cargo install --path . --locked
workboard serve --port 4510 --jira-site https://your-site.atlassian.net
```

Open `http://localhost:4510/` (or `http://127.0.0.1:4510/`). The server runs only while this command is running; it does not install a login item or background service. Build the frontend before compiling Rust: assets are embedded in the executable and a missing `spa/dist` fails compilation.

The database is `$HOME/.local/share/workboard/workboard.sqlite3` by default. `--data-dir PATH` overrides `WORKBOARD_DATA_DIR`; both replace the default directory. The directory and database are private (0700/0600). Back up the database and its WAL sidecars together when the server is stopped. Migrations are versioned; a database from a newer application version is rejected instead of recreated.

Options: `--port` changes the loopback port (default 4510), `--tracon-url` chooses the Tracon node (default `http://127.0.0.1:7420`; it must be a loopback `http://` origin), `--jira-channel` chooses the Tracon channel whose Jira credential is used (default `work`), and `--jira-site` (or `WORKBOARD_JIRA_SITE`) sets your HTTPS Jira origin, such as `https://example.atlassian.net`; it has no default. Bind remains `127.0.0.1`; both `localhost` and `127.0.0.1` work in the browser. Each mutation must send an Origin matching its own Host, and other hosts are rejected. Use a different port if 4510 is occupied.

The sidebar mark and favicon share `spa/public/workboard.svg`. `bun run --cwd spa icons` regenerates 16px/32px PNGs, a two-size `.ico`, and a 180px Apple touch icon from it. The SPA build runs this generator automatically; rebuild and reinstall the Rust executable after changing the SVG because assets are embedded.

## As a Tracon plugin

Workboard also ships as a [Tracon](https://github.com/cosmicspork/tracon) plugin with a UI. Once a release exists, install it on a node with:

```sh
tracon plugin add cosmicspork/workboard
```

The board then appears in Tracon's **Plugins** section, one instance per channel. The node starts `workboard serve --socket <path> --data <dir>`, serves the SPA from the release archive's `ui/` itself, and forwards the SPA's API requests to Workboard as the operator, each carrying a context the node signs. Workboard answers only those requests on `/api` (any other is refused with 401), and offers the board's operations as tools (see [Agent tools](#agent-tools)).

Configure an instance in `<data>/plugin.toml`; the node gives plugins no other way:

```toml
jira_site = "https://your-site.atlassian.net"   # needed for Refresh Jira and browse links
tracon_url = "http://127.0.0.1:7420"            # optional; the node's loopback origin
```

Without `jira_site` the board works, Jira keys show without links, and **Refresh Jira** says which file to edit. Restart the plugin after changing the file. A refresh uses the Jira credential of the channel the board was opened on (there is no `--jira-channel` in plugin mode).

To move an existing standalone board into an instance, stop both, then copy `~/.local/share/workboard/workboard.sqlite3` and any `-wal`/`-shm` sidecars into the instance's `<data>` directory under the same names. Keep the files private (0600).

Links that open in a new tab (Jira keys, task links) do not open from inside Tracon's frame for now; copy them instead, or use the standalone board.

## Agent tools

An agent reads and changes the board through the same operations as the page, as itself: every HTTP route is one tool, and every tool is one route. Both come from one table (`src/ops.rs`) and run the same store functions, and `tests/parity.rs` fails when the manifest, either tool list, the routes the server answers, or a call the SPA makes drifts from it.

| Tool | Route | Does |
|---|---|---|
| `board` | `GET /api/board` | The board: projects, tasks and the last refresh. Optional filters `project` (an id, or `none`), `tag` and `state`, as query parameters or arguments. |
| `task` | `GET /api/tasks/{id}` | One task. |
| `projects` | `GET /api/projects` | The projects in priority order. |
| `task_create` | `POST /api/tasks` | Add a task to Inbox. |
| `task_update` | `PUT /api/tasks/{id}` | Replace a task's own fields. Every field is sent, with the task's `version`. |
| `task_triage` | `POST /api/tasks/triage` | Move tasks to Next, Later or Done and set their review date. |
| `task_assign_project` | `POST /api/tasks/project` | Choose or clear a project for tasks. |
| `project_create` | `POST /api/projects` | Add a project. |
| `project_update` | `PUT /api/projects/{id}` | Rename a project or change its target or Jira keys. |
| `project_delete` | `DELETE /api/projects/{id}` | Delete a project. |
| `project_order` | `POST /api/projects/order` | Set the projects' priority order. |

A tool's arguments are the route's path parameters plus its JSON body (or its query string, for a `GET`): `PUT /api/tasks/{id}` with `{"version": 3, ...}` is `task_update` with `{"id": "...", "version": 3, ...}`. Two things are on one side only, on purpose (`ops::HTTP_ONLY`, `ops::TOOL_ONLY`):

- `POST /api/jira/refresh` is not a tool. A refresh reads Jira through the node's operator route, so an agent's call would reach Jira as the operator. An agent that wants fresh Jira facts calls jira-review's `search` under its own policy.
- `proposal_status` is a tool of the standalone `mcp` mode only, for hearing what the person decided in `workboard review`.

Under Tracon (`serve --socket`), the tools are `workboard__board` and so on:

- **Reads** answer the agent directly. Tracon asks the operator about any plugin tool its policy does not name, so add `allow = ["board", "task", "projects"]` to the `[[plugin]]` entry for `workboard` to let them run unattended.
- **Writes** are consequential: every call is a card for the operator, never allowed by name. The card shows Workboard's preview of the change, the current values next to the proposed ones (`tracon/preview`), and `next_action` and `notes` are marked as Markdown so the card renders them. A write runs only from an approved card (Workboard refuses one with no `approval_id`), and only against what that card showed: if the task's `version` has moved, or the preview no longer matches the board, it is refused as stale (`data.stale`) and nothing is written. The agent reads again and asks anew.
- **Who** is the caller the node attests in the signed context, never a name in the arguments. Workboard keeps no history of edits, so it logs each write with that caller, session, harness and approval in the plugin log (`tracon plugin logs workboard`); the node's own log records the call too. The page's API answers only the operator (`tracon/ui`), so an agent cannot borrow the operator's identity through it.

Without Tracon, `workboard mcp` is an MCP server on stdin and stdout with the same tools, for any agent client. Reads answer from the board; a write returns a `proposal_id` and its preview, and waits in `<data-dir>/proposals/` for you:

```sh
workboard review                 # what is waiting
workboard review show <id>       # the change, and whether the board has moved since
workboard review approve <id>    # apply it, unless the board has moved (then: stale)
workboard review reject <id> --reason "not now"
```

The agent hears the decision with `proposal_status`. `workboard op` lists the operations and `workboard op <name> '<json>'` runs one as you, as the page would. `mcp`, `review` and `op` take `--data-dir` (or `WORKBOARD_DATA_DIR`) and an optional `--jira-site` for browse links, and need no Tracon.

## Daily use

- Add standalone work with **Add task**, including a next action and optional HTTP(S) links. New work enters Inbox.
- Refresh Jira with **Refresh Jira** only when wanted. All assigned issues are cached; only source issues whose status **category** is not `done` create Inbox tasks. Browse links are generated from the configured Jira site, not from issue `self` URLs.
- Triage Inbox with the editor or checkbox selection and **Move to Next**, **Move to Later**, or **Dismiss to Done**. Manual task states do not update Jira. Source completion never silently completes your task; disappearing from assigned results is labeled as unknown, not deleted.
- Order your priorities under **Projects**. Next, Waiting, and Later group tasks under them in that order, with unassigned work last. A project's Jira keys can be issue or epic keys (`APP-264`, matching the issue and its children) or project keys (`APP`, catching any other issue in that Jira project); a key belongs to at most one project. Choosing a project on a task overrides the Jira match. Inbox can assign a project to its selection.
- Tag tasks with `name` or `key:value` tags such as `person:sam`, `repo:gateway`, or `kind:ops`. Imported issues also carry read-only Jira tags (`jira:app`, `epic:app-264`, `label:phase-3`) derived at read time, so a refresh never changes your own tags. Filter by project and by tags (all selected tags must match); search covers titles, next actions, notes, Jira keys, and tags. The last filter is remembered in this browser.
- Mark a Next task **Today** to focus on it in Morning. Today persists overnight. Waiting/Later tasks with review dates on or before the server's local date appear in **Review due**; snoozing changes only the review date. Done tasks can be reopened explicitly.
- Edit the personal title to override an imported Jira summary; clear it to follow the source title again. Links are displayed as text and open in a separate tab without background fetching. Add Jira, MR, Tracon, or communications links manually as needed.

Refresh pages through Tracon's `GET /api/jira/search`, which Tracon serves to loopback callers without a token. The Jira site and account Tracon reports are bound at the first successful refresh. A changed identity, a stopped or older Tracon node, a missing channel credential, a timeout, or a malformed response leaves prior snapshots and decisions intact and displays a recovery message. Configure the Jira credential in Tracon, not here. No background Jira polling occurs.

## License

[MIT](LICENSE)

## Development

```sh
bun run --cwd spa build
cargo build --locked
cargo test --locked
bun run --cwd spa check
bun test --cwd spa
```

The API is single-user and local-only: JSON under `/api`, one route per operation in `src/ops.rs` (add an operation there, not a route in `src/http.rs`), optimistic task versions for edits, `GET /api/tasks/{id}` to read one task by the Workboard ID shown in its editor, 64 KiB request cap, same-origin mutation protection (in plugin mode, the node's signed caller context instead), and no login or CORS surface. The SPA uses only relative URLs, since Tracon serves it under a path prefix. Frontend list selection, grouping, and filtering live in `spa/src/lib/views.ts`; transactional persistence, project resolution, and snapshot reconciliation live in `src/store.rs`.

Releases are cut with [release-please](https://github.com/googleapis/release-please). Each merge to `main` updates a release PR that bumps the version in `Cargo.toml`, `Cargo.lock`, `tracon-plugin.toml` and `spa/package.json` together and writes `CHANGELOG.md` from the conventional-commit PR titles (`feat` bumps the minor version, `fix` the patch, and before 1.0 a breaking change bumps the minor). Merging the release PR tags `v<version>`, creates the GitHub release, and the release workflow attaches `workboard-<version>-<target>.tar.gz` for each target (holding `tracon-plugin.toml`, `bin/workboard` and `ui/`) with `checksums.txt` and build provenance attestations.
