# v0.1.0-alpha.1

Early preview. filetree233 is not yet a replacement for WizTree.

- Windows, Linux and macOS builds for x86-64 and ARM64; GUI, CLI and optional MCP server.
- Browse and filter one directory level, inspect logical file sizes, create a cleanup plan and require interactive human confirmation before deleting.
- Installers verify downloaded assets against SHA-256 checksums.

Known limitations: no NTFS MFT fast scan, allocated-space or hard-link accounting, complete file tree or real treemap, recycle bin, comprehensive search, or import/export. Logical bytes are not guaranteed reclaimable space. Cleanup is permanent and subject to filesystem races. macOS executables are unsigned and not notarized. GPU/window system and platform dependencies apply. No real-drive performance or full GUI acceptance has been completed. See README for details.
