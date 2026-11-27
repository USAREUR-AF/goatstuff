# goat-feeds changelog

Each released version has a section `## <version>`; the release workflow
(`.github/workflows/goat-feeds-release.yml`) publishes that section as the release's notes
and refuses a tag whose version has none. What each version added is described in
[README.md](README.md).

## 0.5.0

The first signed release. A received feed carries its source's declaration of real,
simulated or unknown data (README, "Real or simulated"), readable by other systems at
`GET /api/sim-status`. Systems are named, placed and archived against the declared estate
(0.4.0).

The binaries are static, for linux/amd64 and linux/arm64. `SHA256SUMS` lists both and is
signed with cosign, keyless, by the release workflow at the release's tag. The deploy play
installs a release only through goatbuilder, which verifies the signature and the digest
before any host is touched ([RELEASING.md](RELEASING.md)).
