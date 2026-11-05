# filetree233 0.1.0

filetree233 provides command-line disk analysis, a bilingual treemap desktop app and an MCP stdio server. This is the first stable release; Windows is the primary desktop target for this milestone.

Highlights

- A redesigned desktop app: charcoal-and-teal dark theme, system fonts, card-based inspector, custom tree rows with painted icons, clickable breadcrumbs, a segmented Size / On disk / Files switch, rounded treemap tiles and clear empty and scanning states. The interface follows the operating-system language on first launch; `--lang en|zh` and the header button override it.
- Treemap colours classify entries by name, three levels are drawn by default, and hatching marks where a cleanup rule suggests space could be had back. Classification, hatching and suggestions remain hints and never authorize deletion.
- Marked entries feed the same plan, human confirmation code and revalidation pipeline as the command line. There is no second deletion path; nothing is selected or deleted automatically.
- Windows executables carry an application icon plus FileVersion/ProductVersion metadata. The window, Start Menu shortcut and Add/Remove Programs entry share the icon. macOS bundles carry a native `.icns` icon.
- Installers: Windows MSI and portable zip, macOS DMG, Linux tar.gz (command line only). All six x64/ARM64 assets are checksummed with SHA256SUMS; installer scripts now default to `v0.1.0`.

Upgrade note

The previous `0.1.0-alpha.1` MSI reports ProductVersion `0.1.0`, the same numeric version as this release, so Windows Installer cannot major-upgrade it in place. Uninstall the preview before installing 0.1.0.

Limitations

Experimental Windows NTFS MFT results are incomplete. Allocated bytes do not guarantee reclaimable space, and filesystem races remain possible. Installers are unsigned and not notarized, so SmartScreen and Gatekeeper warn on first launch. See README.md for installation, verification and rollback details.
