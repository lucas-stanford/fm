# Installing fm

`fm` is one binary with no runtime to install. Build it from source with Rust 1.95 or later
([rustup.rs](https://rustup.rs)), on Windows, macOS or Linux:

```sh
cargo install --locked --git https://github.com/lucas-stanford/fm flow-manager
```

or from a clone: `cargo install --locked --path crates/fm`. When a release is published,
`install.sh` (macOS, Linux) and `install.ps1` (Windows) download its prebuilt binary instead; see
the [README](../README.md#install).

Cargo puts `fm` in `~/.cargo/bin` (`%USERPROFILE%\.cargo\bin` on Windows), which rustup adds to
your PATH. Check it with `fm --version`.

Other software may also install an `fm` command. Check `command -v fm` on macOS or Linux, or
`Get-Command fm` in PowerShell. If another executable takes precedence, run this one by its full
path rather than overriding the other tool.

To upgrade, pull and run the same `cargo install` again. On Windows it cannot replace `fm.exe`
while a run is using it; if it fails for that reason, wait for the run to end and try again.

## Uninstall

1. Stop the watcher for this state folder, if one is registered: `fm watcher stop`.
2. Remove the binary: `cargo uninstall flow-manager`.
3. If you also want fm's state gone (consent, run records, config), delete its state folder:
   `FM_HOME` if you set it, else the per-OS folder in the
   [README](../README.md#where-state-lives).

Uninstalling leaves your automations alone, both in `~/.agents/automations/` and in each
repository's `.agents/automations/`.
