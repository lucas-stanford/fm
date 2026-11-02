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

`tests/install.rs` needs `bash` (Git for Windows provides one) and skips
itself without it.

## Pull requests

- Keep each pull request to one change, and describe what it changes and why.
- Add or update tests for any behaviour you change.
- Update `README.md`, `docs/SUPPORTED_MERMAID.md` and the `Unreleased` section
  of `CHANGELOG.md` when user-visible behaviour changes.
- merman is pinned to an exact version. When upgrading it, run
  `tests/merman_seam.rs` first; it pins the renderer behaviour fm relies on.

By contributing you agree that your contribution is licensed under the MIT
License.
