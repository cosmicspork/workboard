# Changelog

## [0.5.0](https://github.com/cosmicspork/workboard/compare/v0.4.0...v0.5.0) (2026-11-24)


### Features

* **spa:** build the page on Tracon's shared UI kit ([#14](https://github.com/cosmicspork/workboard/issues/14)) ([8782cf7](https://github.com/cosmicspork/workboard/commit/8782cf70b44895dbf4116feb1cd1a28d56c555f6))

## [0.4.0](https://github.com/cosmicspork/workboard/compare/v0.3.0...v0.4.0) (2026-11-22)


### ⚠ BREAKING CHANGES

* refresh Jira through jira-review directly ([#9](https://github.com/cosmicspork/workboard/issues/9))

### Features

* refresh Jira through jira-review directly ([#9](https://github.com/cosmicspork/workboard/issues/9)) ([3eb3c12](https://github.com/cosmicspork/workboard/commit/3eb3c12927c7c56ddb6008db22eda4d5d48b8ce0))

## [0.3.0](https://github.com/cosmicspork/workboard/compare/v0.2.0...v0.3.0) (2026-11-21)


### Features

* expose the board's operations as plugin tools, one per HTTP route ([#7](https://github.com/cosmicspork/workboard/issues/7)) ([fa8cace](https://github.com/cosmicspork/workboard/commit/fa8cace7bc583f0788e3d11e98b5ce82645d97d9))

## [0.2.0](https://github.com/cosmicspork/workboard/compare/v0.1.0...v0.2.0) (2026-11-20)


### Features

* add dark mode with a light, dark, and auto theme toggle ([a9aaf85](https://github.com/cosmicspork/workboard/commit/a9aaf85b22c9c5197b6093ad353b08ec5d54ef01))
* add local morning workboard ([1dce100](https://github.com/cosmicspork/workboard/commit/1dce10036e4ad002025fdeb9d40f70785e9d6da3))
* add shared Workboard favicon assets ([d3b69e5](https://github.com/cosmicspork/workboard/commit/d3b69e5cb78adba772740335f0621a0a99f8e727))
* dark mode with a light, dark, and auto theme toggle ([7cdb364](https://github.com/cosmicspork/workboard/commit/7cdb36427f153b7a8f60967dd19dd3b20ef4a678))
* group the board by priority and filter by tags ([53a5c73](https://github.com/cosmicspork/workboard/commit/53a5c732c1fd05216d437ea1972102c76c678a21))
* read Jira through Tracon and add projects and tags ([e8d9e8e](https://github.com/cosmicspork/workboard/commit/e8d9e8e56ec33db6c8fff3963a4db910a3d77e78))
* run as a Tracon plugin with a UI, released with release-please ([#3](https://github.com/cosmicspork/workboard/issues/3)) ([20071fa](https://github.com/cosmicspork/workboard/commit/20071fac79b3ca96a3345255e56b0780c6c6e86d))
* show and copy the Workboard ID in the task editor ([a380de5](https://github.com/cosmicspork/workboard/commit/a380de5a04acb59245aaced9846703ce89ec2e84))
* show and copy the Workboard ID in the task editor ([93a5994](https://github.com/cosmicspork/workboard/commit/93a599495f6daec15344e24621d9963ad9a91338))


### Bug Fixes

* allow localhost workboard access ([62eea68](https://github.com/cosmicspork/workboard/commit/62eea6854817ecb192fdf308614dd4282182d064))

## 0.1.0

- Standalone local Rust/Svelte morning board with durable private SQLite tasks and reversible personal states.
- Manual read-only Jira refresh with account binding, source-status badges, and retained snapshots on failure.
- Today focus, review dates, Inbox bulk triage, standalone tasks, and local same-origin API protections.
