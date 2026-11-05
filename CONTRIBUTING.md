# Contributing

## Build and test

Rust 1.95 or newer is required (edition 2024 plus the locked dependency set).

```sh
cargo test --locked                      # CLI and shared analysis code
cargo test --locked --features gui       # desktop app logic
cargo clippy --locked --all-targets --features gui -- -D warnings
cargo fmt --all -- --check
```

On Windows, run the commands from a Visual Studio developer shell, or source
the environment first if you have one. Release builds enable the `gui` feature
on Windows and macOS; Linux releases stay CLI-only so they need no display
libraries.

## Safety invariants

These are not optional; changes that weaken them will not be merged:

- Scanning, classification, hatching and cleanup suggestions are read-only
  hints. Nothing is ever selected or deleted automatically.
- Deletion requires a plan created explicitly, a confirmation code typed by a
  human, and revalidation through the same plan/apply pipeline used by the CLI.
- The GUI must not grow a second deletion path.

## Packaging

- Windows: `./scripts/package.ps1` builds the portable zip and the MSI. It needs
  the .NET SDK and installs a pinned WiX v5 toolset under
  `%LOCALAPPDATA%\filetree233\wix`.
- macOS and Linux: `./scripts/package.sh`.
- CI builds all six targets, installs every artifact and publishes tagged
  releases with checksums.

Keep installers unsigned unless code signing is added deliberately; document
the SmartScreen/Gatekeeper warning instead of hiding it.

## Pull requests

Keep a pull request focused, describe the user-visible change and the tests you
ran, and update `CHANGELOG.md` for user-visible changes. CI must stay green:
formatting, clippy with warnings denied, tests, the MSRV check and the
dependency audit all run on every pull request.
