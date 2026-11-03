# microscope-oidc-client role tests

Parse-only smoke. End-to-end Hydra OAuth2 client provisioning is covered by
running `microscope.yml` against a goatnet, which imports this role before the
`microscope` role so the client exists before the fence that presents it.

The failure this role prevents is not caught by any syntax check: a fence
pointing at a client nothing created is green on every container health probe,
and only visible by loading the page and reading Ory's `invalid_client`. The end-to-end assertion is therefore
"open the URL and reach a login form", not "the play reported ok".

Run: `./syntax-check.sh`.
