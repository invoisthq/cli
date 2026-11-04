# Changelog

All notable changes to the Invoist CLI are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the CLI uses
[Semantic Versioning](https://semver.org/): the commands, flags, exit codes
and `--json` output are the public interface.

## [0.1.0] - Unreleased

The first release, built on the Rust SDK 1.0.1.

### Added

- `invoist login`: sign in with the browser and approve the CLI in Invoist,
  or paste an API key with `--with-key`. Sign-ins are kept in the system
  keychain, renew themselves, and can be kept apart in profiles.
- `logout`, `status` and `profile list` / `use`.
- For a business: `extract` (files, folders or standard input, several at
  once), `documents list` / `get` / `download`, `export` to CSV or JSON,
  `review list` / `resolve`, `categories`, `usage`, `stats`, and
  `webhooks list` / `create` / `delete` / `verify`.
- For a household: `bills` (what's left to pay), `bills list` / `get` /
  `download` / `paid` / `unpaid` / `stats`, and `held-emails`.
- Output as a table, JSON, NDJSON or CSV, with stable exit codes.
- A full-screen terminal app (`invoist` on its own, or `invoist ui`) with
  screens for each account, a command palette, and mouse support.
- Shell completions and man pages.

[0.1.0]: https://github.com/invoisthq/cli/releases/tag/v0.1.0
