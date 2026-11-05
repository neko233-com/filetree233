# Unreleased

filetree233 provides command-line disk analysis, an optional treemap desktop app and an MCP stdio server. This is an early preview and is not yet a full replacement for WizTree.

- Scan directories, list volumes, export a tree, search, find duplicate contents, and review cleanup suggestions.
- Create a cleanup plan with logical and allocated sizes; applying it requires human confirmation and revalidation. Recycle is the default; permanent deletion is explicit.
- `filetree233 gui` opens the squarified treemap: colours classify entries by name, hatching marks where a cleanup rule suggests the space could be had back, and marking then planning reuses the command-line plan, confirmation code and revalidation. The interface is bilingual (English and Chinese) and picks the operating system language on first launch; `--lang en|zh` and the button in the toolbar override it. The GUI is an opt-in cargo feature, so command-line builds need no graphics dependencies.
- Windows, Linux and macOS builds target x64 and ARM64. Each release now also carries installers: Windows MSI and zip, macOS DMG, and a Linux tar.gz that keeps the command line only, avoiding display-server dependencies. Install scripts validate SHA-256 checksums.

Limitations: experimental Windows NTFS MFT results are incomplete; allocated bytes do not guarantee reclaimable space, and filesystem races remain possible. Classification, hatching and suggestions are hints and never authorize deletion. Installers are unsigned and not notarized, so SmartScreen and Gatekeeper warn on first launch. Real-drive performance and release installation must be verified separately on each platform. See README.md for details.
