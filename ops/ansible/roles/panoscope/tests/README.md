# panoscope role tests

Per ADR 50H — every load-bearing Ansible role ships a `tests/`
directory with a parse-only syntax check.

## Structural coverage

- [`playbook.yml`](playbook.yml) — minimal fixture that loads the
  role.
- [`syntax-check.sh`](syntax-check.sh) — runs `ansible-playbook
  --syntax-check` to catch YAML / Jinja parse errors.

## Behavioral coverage

Follow the [operator runbook](../../../../../docs/operations/panoscope-runbook.md)
for container deployment, certificate mounts, routing and OAuth2 client
provisioning. These checks require a running environment.
