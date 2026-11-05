# Security policy

## Supported versions

Security fixes land on the latest `0.1.x` release. Older previews are not
supported and should be upgraded.

## Reporting a vulnerability

Report vulnerabilities through GitHub's private advisory flow:
https://github.com/neko233-com/filetree233/security/advisories/new

Please do not open a public issue for a suspected vulnerability. Include the
version, platform, reproduction steps and the impact you expect. You should get
a first response within a week.

## Release artifacts

- Binaries and installers are not code signed or notarized. Windows SmartScreen
  and macOS Gatekeeper warn on first launch; verify downloads against the
  `SHA256SUMS` asset published with every release.
- The MSI installs per machine and adds the install directory to the system
  `PATH`. It runs no network services and no background tasks.
- The desktop app, CLI and MCP server never delete anything on their own.
  Cleanup requires an explicitly created plan, a confirmation code typed by a
  human, and revalidation through the same plan/apply pipeline. Classification
  colours, hatching and cleanup suggestions are hints only.
