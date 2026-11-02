# Unreleased

filetree233 provides command-line disk analysis and an optional MCP stdio server. This is an early preview and is not yet a full replacement for WizTree.

- Scan directories, list volumes, export a tree, search, find duplicate contents, and review cleanup suggestions.
- Create a cleanup plan with logical and allocated sizes; applying it requires human confirmation and revalidation. Recycle is the default; permanent deletion is explicit.
- Windows, Linux and macOS builds target x64 and ARM64. Installers validate SHA-256 checksums.
- The graphical interface and its build dependencies have been removed.

Limitations: experimental Windows NTFS MFT results are incomplete; allocated bytes do not guarantee reclaimable space, and filesystem races remain possible. macOS binaries are unsigned and not notarized. Real-drive performance and release installation must be verified separately on each platform. See README.md for details.
