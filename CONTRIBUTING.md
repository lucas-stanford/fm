# Contributing

Bug reports and pull requests are welcome.

There is no CI yet. Run the checks below locally before opening a pull request.

## Build and test

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first
use. The minimum supported Rust version is 1.95.

```sh
cargo build
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
```

`crates/fm/tests/install.rs` needs `bash` (Git for Windows provides one) and
skips itself without it.

`crates/fm/tests/docs.rs` checks that `docs/CLI.md`, `docs/CONFIG.md` and the
README match the code: update them with any command, flag, environment variable
or `config.toml` key you change. `tests/spec/` is a copy of the specification's
pages; its README says how to refresh it.

`real_cli_smoke` and the other tests that call a real agent CLI cost a model
call and are ignored by default; [docs/AGENTS.md](docs/AGENTS.md) says how to
run them.

## Pull requests

- Keep each pull request to one change, and describe what it changes and why.
- Add or update tests for any behaviour you change.
- Update `README.md`, the pages in `docs/` and the `Unreleased` section of
  `CHANGELOG.md` when user-visible behaviour changes.
- merman is pinned to an exact version. When upgrading it, run
  `crates/fm-render/tests/merman_seam.rs` first; it pins the renderer behaviour fm relies on.

By contributing you agree that your contribution is licensed under the MIT
License.
