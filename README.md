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

The API is single-user and local-only: JSON under `/api`, optimistic task versions for edits, `GET /api/tasks/{id}` to read one task by the Workboard ID shown in its editor, 64 KiB request cap, same-origin mutation protection, and no login or CORS surface. Frontend list selection, grouping, and filtering live in `spa/src/lib/views.ts`; transactional persistence, project resolution, and snapshot reconciliation live in `src/store.rs`.
