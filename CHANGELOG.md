# Changelog

## Unreleased

- Starting the desktop app from the Start Menu or a shortcut no longer flashes a console window: `filetree233-gui.exe`, a windowless launcher with the same icon, starts `filetree233 gui` without a console. The shortcut no longer passes arguments, and running `filetree233 gui` from a terminal keeps its console output.
- Rebuilt the desktop visual language: a painted filetree233 mark, compact branded header, capacity cards for every volume in the idle state, a slimmer status footer, and a quieter control surface with consistent rounded pills, custom dropdowns and chevrons, and a card-based inspector. The directory tree now shows each entry's size and truncates long names with an ellipsis.
- Add production gates on every push and pull request: formatting, clippy with warnings denied, CLI and GUI tests, an MSRV 1.95 check and a RustSec dependency audit. Dependabot keeps Cargo and GitHub Actions dependencies current.
- Add the MIT license, `SECURITY.md` and `CONTRIBUTING.md`, fill in crates.io metadata and enable thin LTO with symbol stripping for release builds.
- Opening the GUI from Explorer or the Start Menu no longer leaves a console window behind; a console shared with a terminal is kept for CLI output.
- The Start Menu shortcut is now named `filetree233` instead of `filetree233 treemap`.

## 0.1.0 — 2026-11-05

First stable release. Windows is the primary desktop target for this milestone.

- Redesigned the desktop app: a cohesive charcoal-and-teal dark theme with Segoe UI and Microsoft YaHei, card-based inspector, custom directory rows with painted disclosure and folder/file glyphs, clickable breadcrumbs, a segmented metric switch, rounded treemap tiles with hover and focus outlines, and clear empty and scanning states.
- Treemap polish: richer classification colours, `debug`/`release`/`deps`/`incremental` classified, three levels drawn by default, and a readable default window size.
- Windows paths are displayed without the `\\?\` verbatim prefix while scans keep using the exact path.
- The executable now carries the application icon, FileVersion/ProductVersion, company and copyright metadata; the window, Start Menu shortcut and Add/Remove Programs entry use the same icon.
- macOS bundles ship a native `.icns` icon; Linux packages remain command-line only.
- Installer scripts default to `v0.1.0`, and installer tests derive the expected version from Cargo.toml instead of hard-coding it.
- All release packaging was rebuilt and re-verified: portable zip, MSI install/uninstall, application icon and version metadata, Start Menu shortcut and PATH entry.

Upgrade note: the previous `0.1.0-alpha.1` MSI also reports ProductVersion `0.1.0`, so Windows Installer cannot major-upgrade it in place. Uninstall it before installing this release.

## 0.1.0-alpha.1

First preview: native GUI, CLI and MCP, explicit cleanup plan and confirmation, six platform builds and checksummed installers. See RELEASE_NOTES.md and README.md for important limitations.
