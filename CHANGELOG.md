# Changelog

## Unreleased

- `--jira-site` (or `WORKBOARD_JIRA_SITE`) is now required instead of defaulting to a specific Jira site.
- Released under the MIT license.
- Read Jira through a local Tracon node instead of `acli`, adding parent epics and labels to the snapshot. `--acli` is replaced by `--tracon-url` and `--jira-channel`.
- Ordered projects that hold your priorities, matched to Jira issues, epics, or project keys, with Next, Waiting, and Later grouped by priority.
- `key:value` tags on tasks plus derived Jira tags, with tag filters, project filters, and search over notes and tags.
- Versioned schema migrations; existing databases upgrade in place.
- Accept `http://localhost:<port>/` alongside the IPv4 loopback URL, while requiring matching Host and Origin on mutations.
- Share one vector Workboard mark across the sidebar and favicon, with generated PNG, ICO, and Apple touch sizes.

## 0.1.0

- Standalone local Rust/Svelte morning board with durable private SQLite tasks and reversible personal states.
- Manual read-only Jira refresh with account binding, source-status badges, and retained snapshots on failure.
- Today focus, review dates, Inbox bulk triage, standalone tasks, and local same-origin API protections.
