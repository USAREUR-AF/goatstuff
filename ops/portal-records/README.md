# Data standard records in the portal

Every topic a service reads or publishes has a record in the portal: its key,
its format, and a data dictionary people and the map link to.

One file per goatnet declares them (`ops/portal-records/<goatnet>.yaml` in the
goatnet's private repository). `portal_records.py` reads it:

| Verb | What it does |
|---|---|
| `check` | Offline, in CI: every fabric key in the named configuration files has a declared record |
| `plan` | Shows what the portal lacks or holds differently |
| `apply` | Creates topics, corrects format and description, uploads missing schema versions; never deletes |

To add a topic: add a row, open a pull request, merge, then `plan` and `apply`.
To change a dictionary: edit its schema and raise the standard's `version`.
A `*` segment covers every topic of that shape, so a feed approved later already
has its record. The portal registers a topic under its owner's anchor, so
`volatile/` is not part of a declared key.
