# Security Policy

Montage is early-stage local-first software. Please do not report security
issues through public GitHub issues.

## Reporting a Vulnerability

Email security reports to the project maintainers or use GitHub private
vulnerability reporting when it is enabled for the repository.

Include:

- Affected version or commit.
- Operating system and install method.
- Clear reproduction steps.
- Impact and any known workarounds.

We will acknowledge valid reports as soon as practical, coordinate a fix, and
publish notes once users have a reasonable upgrade path.

## Scope

In scope:

- Local file access, project sandboxing, and path traversal issues.
- Secret handling and credential storage.
- Desktop app command execution boundaries.
- Vendored runtime behavior that Montage exposes directly.

Out of scope:

- Vulnerabilities requiring arbitrary local code execution before Montage starts.
- Issues in third-party services or model providers outside Montage's control.
- Social engineering or phishing reports.
