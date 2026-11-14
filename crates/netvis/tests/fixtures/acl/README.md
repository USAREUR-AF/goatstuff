# ACL fixtures

`netbird_acl_taxonomy.yml`, `netbird_acl_matrix.yml` and `netbird-acl.yaml` (rule
intents) are fixed copies of a full mesh ACL declaration. netvis's unit tests and
`ops/peerview/test_peerview.py` read them; nothing reads them at run time.

The declaration a goatnet runs on is goatcore's `ops/ansible/netbird_acl_taxonomy.yml`,
`ops/ansible/netbird_acl_matrix.yml` and `ops/warrant/rule-intents/netbird-acl.yaml`,
in the tree assembled from goatcore and this repository. This repository does not
carry those paths, so an assembled tree takes them from goatcore unchanged.

These copies are not kept in step with goatcore. Change them when a test needs a
different declaration.
