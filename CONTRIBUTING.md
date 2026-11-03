# Contributing

Thank you for working on goat. This page is the whole process; nothing else is required reading before your first pull request.

## The short version

1. Fork, or branch from `main` if you have write access. Branch names: `feat/<topic>`, `fix/<topic>`, `docs/<topic>`.
2. Make the change. Run the gates that apply (below). Add or update the design document in the same change when behaviour or topology changes.
3. Commit with a [Conventional Commits](https://www.conventionalcommits.org/) subject and a DCO sign-off: `git commit -s -m "fix(zenoh): …"`.
4. Open a pull request against `main`. Fill the template. One approving review from a code owner merges it, by squash.

## Commits

- **Conventional Commits.** `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `ci`, with a scope in parentheses naming the component (`feat(goatbuilder): …`). The subject is ≤ 100 characters and says what changed, in the imperative.
- **DCO sign-off on every commit** (`git commit -s`). By signing off you certify the [Developer Certificate of Origin 1.1](https://developercertificate.org/): you wrote the change or have the right to submit it under this project's licence. There is no CLA.
- **Signed commits** are required on `main`; set up [SSH or GPG commit signing](https://docs.github.com/en/authentication/managing-commit-signature-verification) once.
- One logical change per pull request. Refactors and behaviour changes are separate pull requests.

## Gates

Every check that has a script here can be run locally before you push. The ones that apply to most changes:

| Change touches | Run |
|---|---|
| anything | `ops/ci/check-no-identity-spillage.sh` — no keys, addresses or credential values in a committed file |
| Rust (`crates/`) | `cd crates && cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace` |
| Go (`ops/*/go.mod`) | `go vet ./... && go test ./...` in that module |
| Ansible | `ops/ci/check-role-template-refs-resolve.sh` · `ansible-playbook --syntax-check` on the playbook you changed |
| Shell | `shellcheck` · `ops/ci/check-bash-portability.sh` (scripts must run on bash 3.2) |

The pull-request workflow runs the same scripts. A red gate names the script, so the fix is reproducible locally. The fuller set of documentation gates, and the decision record, live in [`goatcore`](https://github.com/usareur-af/goatcore).

## Design documents

A change to a component's behaviour, data flow, topology or integration point updates that component's design document under `docs/design/` **in the same pull request**. Each design document states, per part, whether it is *built*, *designed* or *not built*, and the status words mean exactly that:

| Word | Means |
|---|---|
| built | the code exists and runs |
| measured | a command ran and produced the stated result |
| designed | decided, not implemented |
| not built | nothing exists |

A load-bearing decision — one a later contributor would need to know the reason for — is an ADR under `docs/adr/` in [`goatcore`](https://github.com/usareur-af/goatcore).

## Review

Reviewers look for: the change does what its subject says and nothing else; the gates pass; the design document moved with the code; status words are honest; no identifier of a real deployment (an address, a hostname, a key, a person) entered the tree. Review comments are about the change, not the author.

## Reporting a bug or proposing a feature

Open an issue with the template. For a vulnerability, **do not open an issue** — follow [SECURITY.md](SECURITY.md).

## Licence

By contributing you agree that your contribution is licensed under the [Apache License 2.0](LICENSE) that covers this project.
