# Security policy

goat handles identity, authentication and encrypted transit at the boundary between edge and cloud. Security reports are prioritised over feature work.

## Reporting a vulnerability

**Do not open a public issue or pull request to report a vulnerability.**

**[Report a vulnerability privately](https://github.com/USAREUR-AF/goatstuff/security/advisories/new)**

You can also open the reporting form from this repository's *Security* tab by selecting *Report a vulnerability*.

Reports are private and accessible to repository administrators, organization owners, security managers, the reporter, and collaborators explicitly added to the advisory. Being listed in `CODEOWNERS` does not itself grant access. Reports remain private until an authorized maintainer publishes the advisory. See [GitHub's advisory permissions](https://docs.github.com/en/code-security/reference/permissions/repository-security-advisory) for details.

Include: a description of the issue and its impact; reproduction steps or a proof of concept; the affected versions, commits or deployment surfaces; whether you have shared it elsewhere.

## What to expect

| Step | Target |
|---|---|
| Acknowledgement | 3 working days |
| Triage and severity | 7 working days |
| Fix or mitigation for a confirmed high or critical issue | 30 days, with a coordinated disclosure date agreed with you |
| Public advisory | when the fix is released, crediting you unless you prefer otherwise |

## Scope

In scope: the code in this repository and the images and release artifacts built from it. Out of scope: the upstream projects goat assembles (NetBird, Zenoh, Ory, SPIRE, Postgres, WireGuard) — report those to their maintainers, and tell us too if goat's integration is affected; any deployment you do not own.

## Supported versions

Pre-1.0, the latest tagged release of each component is supported. Fixes land on `main` and are released by tag.
