# Workboard

A local morning board for personal commitments, with an explicitly refreshed, read-only Jira snapshot. Jira owns ticket facts; Workboard owns your states, Today selections, notes, next actions, review dates, and links. It does not update Jira or hand work to Tracon automatically.

## Install and launch

Requires Rust/Cargo and Bun for building, plus an authenticated Atlassian `acli` only when refreshing Jira:

```sh
bun install --cwd spa
bun run --cwd spa build
cargo install --path . --locked
workboard serve --port 4510
```

Open `http://127.0.0.1:4510`. The server runs only while this command is running; it does not install a login item or background service. Build the frontend before compiling Rust: assets are embedded in the executable and a missing `spa/dist` fails compilation.

The database is `$HOME/.local/share/workboard/workboard.sqlite3` by default. `--data-dir PATH` overrides `WORKBOARD_DATA_DIR`; both replace the default directory. The directory and database are private (0700/0600). Back up the database and its WAL sidecars together when the server is stopped. Migrations are versioned; a database from a newer application version is rejected instead of recreated.

Options: `--port` changes the loopback port (default 4510), `--acli` chooses an executable (default `acli` on PATH), and `--jira-site` chooses the HTTPS Jira origin (default `https://example.atlassian.net`). Bind remains `127.0.0.1`; the browser must use the printed URL, not `localhost`, because Host and Origin are checked. Use a different port if 4510 is occupied.

## Daily use

- Add standalone work with **Add task**, including a next action and optional HTTP(S) links. New work enters Inbox.
- Refresh Jira with **Refresh Jira** only when wanted. All assigned issues are cached; only source issues whose status **category** is not `done` create Inbox tasks. Browse links are generated from the configured Jira site, not from issue `self` URLs.
- Triage Inbox with the editor or checkbox selection and **Move to Next**, **Move to Later**, or **Dismiss to Done**. Manual task states do not update Jira. Source completion never silently completes your task; disappearing from assigned results is labeled as unknown, not deleted.
- Mark a Next task **Today** to focus on it in Morning. Today persists overnight. Waiting/Later tasks with review dates on or before the server's local date appear in **Review due**; snoozing changes only the review date. Done tasks can be reopened explicitly.
- Edit the personal title to override an imported Jira summary; clear it to follow the source title again. Links are displayed as text and open in a separate tab without background fetching. Add Jira, MR, Tracon, or communications links manually as needed.

The CLI account/site is bound at the first successful Jira refresh. A changed identity, missing authentication, timeout, malformed response, or unavailable executable leaves prior snapshots and decisions intact and displays a recovery message. `acli jira auth login` is a user-run action only. Jira CLI issue `self` URLs may use the tenant origin or Atlassian's HTTPS `*.prod.atl-paas.net` backend; these URLs are validated but never followed or shown. No background Jira polling occurs.

## Development

```sh
bun run --cwd spa build
cargo build --locked
cargo test --locked
bun run --cwd spa check
bun test --cwd spa
```

The API is single-user and local-only: JSON under `/api`, optimistic task versions for edits, 64 KiB request cap, same-origin mutation protection, and no login or CORS surface. Frontend list selection and due ordering live in `spa/src/lib/views.ts`; transactional persistence and snapshot reconciliation live in `src/store.rs`.
