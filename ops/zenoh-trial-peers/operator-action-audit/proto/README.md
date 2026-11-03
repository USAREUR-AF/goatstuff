# operator-action-audit proto

`operator_action_v1.proto` is the **canonical schema** for the operator-action
audit event.

## Lock-step rule

There is **no protoc / build.rs codegen** for this message — exactly the
convention used by the other `ops/zenoh-trial-peers` proto dirs (fabric-health's
`fabric_health_v1.proto`, time-health's `env_health_v1.proto`). The Rust struct
that produces + consumes the wire bytes is **hand-written** with `prost`-derive
in:

- `crates/goat-cli/src/cmd/audit.rs` (`OperatorActionV1`)

That struct MUST stay in lock-step with this file — same field names, same
proto field tags. A drift (a renumbered or added field on only one side)
silently breaks the wire. When you change this `.proto`, change the Rust struct
in the same change-set.

## On-wire format

The bytes published to `ops/audit/operator/<actor>` are **not** a bare
`OperatorActionV1`. They are:

```
age(roster, prost_encode(OperatorActionV1))
```

i.e. the whole serialized protobuf message, age-encrypted (binary, non-armored)
to the operator age roster (`40-dev-ca-operators.envrc`). The encryption wraps
the entire encoded body before it touches the wire (§7 encryption layer), so a
non-operator subscriber sees only opaque ciphertext even if the read ACL is
mis-set. Only an operator holding a roster age identity can decrypt + decode.
There is therefore no separate `ciphertext` field inside the message itself.

The reader is `goat audit ops` (decrypt-aware): it subscribes / queries
`ops/audit/operator/**`, decrypts each sample with the operator age identity,
prost-decodes, and renders the action.
