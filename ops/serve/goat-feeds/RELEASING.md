# Releasing goat-feeds

A release is a git tag `goat-feeds-v<version>` on `main`. The tag starts
`.github/workflows/goat-feeds-release.yml`, which builds, signs and publishes it. Nothing
is built or signed on a person's machine, and there is no long-lived signing key.

## What a release contains

| File | What it is |
|---|---|
| `goat-feeds-<version>-linux-amd64` | the static binary for x86-64 Linux, page embedded |
| `goat-feeds-<version>-linux-arm64` | the same for arm64 Linux |
| `SHA256SUMS` | the SHA-256 of both binaries |
| `SHA256SUMS.sigstore.json` | the keyless signature of `SHA256SUMS`: a Sigstore bundle v0.3, one `hashedrekord` entry in the public Rekor log, its certificate naming this workflow at the tag |

Before it publishes, the workflow checks that the tagged commit is on `main`, that the tag
is `goat-feeds-v` followed by `VERSION`, that the role's `goat_feeds_version` default and a `## <version>` section of
`CHANGELOG.md` match it, that the unit tests pass, that each binary is the one
`SHA256SUMS` lists, is statically linked and prints its version on Ubuntu 24.04 of its own
architecture, that the signature verifies for this workflow at the tag, and that the bundle
has the shape goatbuilder reads (one `hashedrekord` 0.0.1 entry with a signed entry
timestamp and an inclusion proof). It signs with the Sigstore signing config pinned in
`release/signing-config.json` (Fulcio and the Rekor v1 log) and cosign 3.1.4, so a change
to Sigstore's published config cannot move the signature to a log goatbuilder refuses; a
change to either is a pull request here.

## Cut a release

1. In one pull request: set `VERSION` and `goat_feeds_version` in
   `ops/ansible/roles/goat-feeds/defaults/main.yml` to the new version, and add its
   section to `CHANGELOG.md`.
2. After it merges, run the workflow by hand on `main` (Actions, goat-feeds-release, Run
   workflow). That is a dry run: the same build, runs, signature and checks, kept as the
   run's artifact `goat-feeds-signed`, with nothing published. Read its file list.
3. Tag the merged commit with a signed tag and push the tag:
   `git tag -s goat-feeds-v<version> <commit> -m "goat-feeds <version>"`, then
   `git push origin goat-feeds-v<version>`.
4. The workflow publishes the GitHub Release. A tag is public and is never moved or
   reused: a fix is a new version.

## Check a release by hand

```
cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
  --certificate-identity https://github.com/USAREUR-AF/goatstuff/.github/workflows/goat-feeds-release.yml@refs/tags/goat-feeds-v<version> \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
sha256sum -c SHA256SUMS
```

## Install a release

`goatbuilder deploy goat-feeds`, in a tree that layers goatstuff over goatcore, with a pin of
the form `USAREUR-AF/goatstuff@goat-feeds-v<version>@sha256:<SHA-256 of SHA256SUMS>`, its
`release:` naming `checksums: SHA256SUMS`, `bundle: sigstore`, the two files, and the
signer `https://github.com/USAREUR-AF/goatstuff/.github/workflows/goat-feeds-release.yml`.
goatbuilder verifies the release on the operator's machine, before any host is touched;
goatcore's `release-file` role puts the host's file in place and records the pin; then
`ops/ansible/goat-feeds-deploy.yml` runs that file, once it is held to its SHA-256 and
found writable by root alone. A trial can still install a binary from the controller with
`goat_feeds_binary_src`; the play takes exactly one of the two.
