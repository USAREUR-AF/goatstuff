# panoscope role tests

Per ADR 50H — every load-bearing Ansible role ships a `tests/`
directory with a parse-only syntax check.

## Structural coverage

- [`playbook.yml`](playbook.yml) — minimal fixture that loads the
  role.
- [`syntax-check.sh`](syntax-check.sh) — runs `ansible-playbook
  --syntax-check` to catch YAML / Jinja parse errors.

## Behavioral coverage

Behavioral validation (real container deploy, real cert mounts, real
Traefik routing, real Hydra OAuth2 client provisioning) happens at
82Q.8 smoke deploy on nova-a — see `docs/project/implementation-plan.md`
Block 82Q.8 + the operator runbook at
`docs/operations/panoscope-runbook.md`
(landed in 82Q.9).
