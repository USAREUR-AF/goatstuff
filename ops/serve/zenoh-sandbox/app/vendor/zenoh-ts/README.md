# Vendored: zenoh-ts browser client

`zenoh-ts.js` is the Zenoh client the sandbox loads. It is one ES module, built
from the npm packages below with the WebAssembly key-expression module
embedded. `js/zenoh.js` imports it by relative path; nothing is fetched from a
package registry or a CDN when the page loads or when the image is built.

Do not edit `zenoh-ts.js` or the files in `licenses/`. They are tool output.

## Contents

| Package | Version | License | Tarball integrity (SHA-512) |
|---|---|---|---|
| `@eclipse-zenoh/zenoh-ts` | 1.9.0 | Apache-2.0 OR EPL-2.0 | `sha512-qG3VR9Yt81+KjH0WOybyKcNVS0vtBmmhyUJ7dCc5n+8AeAEYTpEvMBTh43Lwu8P+imzMYvFqjaMAO2HFK+x9hA==` |
| `channel-ts` | 0.1.2 | MIT | `sha512-cI/XiDF+jB0v95Xup8xlM7k93lT3xwPl0WdjEZ9w9aUMf5N+3GQevspK2EDYfMyxcKcXdN1F6PDpuYRpUfaZmg==` |
| `typed-duration` | 2.0.0 | ISC | `sha512-XTOtfRR/l2hkCjFANwqu/IdLq17jBPts49tnxXc+Oc2ewngECupdu4FEH2H9hQHginRfOXCZ39tb9+pVcBlTcQ==` |
| `@thi.ng/leb128` | 3.1.94 | Apache-2.0 | `sha512-MNpWK3qnRJ7aUYB0qghtlm0uY4ULBWAPKcezqTMDAX+HVmGqONllhebTjgxmueCljKBqpJMugybnOO99cn5wCg==` |
| `@thi.ng/checks` | 3.11.1 | Apache-2.0 | `sha512-S64V2nAPA0Y66VwpJdi3fdwv4xP9G7M13nvQWzp6YTERsnusI6FtihbxhENt/DQ/k+iP+aVSsy+LgtqwkFvvag==` |
| `@thi.ng/errors` | 2.6.17 | Apache-2.0 | `sha512-momw7wo3EXes1wa6Smckr+l4BaJi/h+E67R3furYOBjQpciie1OgoTa43PXPb1POsuWxynkEF8OCnQgreE8twg==` |

The first three are what `@eclipse-zenoh/zenoh-ts` 1.9.0 imports at run time;
the two `@thi.ng` packages below `leb128` are its own dependencies. The same
versions and hashes are in `../../../vendor-tool/packages.lock.json`, which is
what the tool reads.

Bundle: `zenoh-ts.js`, SHA-256
`a574977706002bf6e1121a4af6218bd8d71d6862bb30bf0632d534b953d6477c`.

## Local input patches

The versions and tarball integrity values above remain the upstream inputs.
After verification, `vendor-tool/patch.py` applies the edits recorded in
`vendor-tool/patches.json`. Each of the five package files has an input and
output SHA-256 and exact edit counts; all are validated before any file is
changed. An upstream input change requires reviewing these patches again.

- `typed-duration/dist/lib/index.js`: remove the undefined check already
  excluded by the object type guard, retaining the null check.
- `@eclipse-zenoh/zenoh-ts/dist/link.js`: remove one duplicate constructor
  assignment.
- `@eclipse-zenoh/zenoh-ts/dist/key_expr/zenoh_keyexpr_wrapper_bg.js`:
  remove six unread local assignments immediately before throws. The
  deferred variables used by `finally` and the WebAssembly binary stay intact.
- `@thi.ng/errors/unsupported.js` and `@thi.ng/leb128/index.js`: annotate
  five pure factory calls so esbuild can remove unused results. These
  factories return classes or closures; calling them does not invoke the
  constructors or WebAssembly operations. Package exports stay intact.

These are downstream quality cleanups, not an upstream release or a change
to the Zenoh protocol, router, remote-API plugin or dependency pins. License
files are copied from the original tarballs unchanged.

## Licenses

`licenses/` holds the license file from each tarball, unchanged.
`@eclipse-zenoh/zenoh-ts` is offered under either license in its file; this
repository uses it under Apache-2.0. The `typed-duration` 2.0.0 tarball carries
no license file; its `package.json` declares `ISC`, and its source is
<https://github.com/jwulf/typed-duration>.

## Regenerate

```bash
ops/serve/zenoh-sandbox/vendor-tool/regenerate.sh
```

The script downloads each tarball named in `packages.lock.json`, refuses any
whose SHA-512 differs from the lock file, applies the reviewed input patches,
and bundles the unpacked tree into
one ES module with esbuild's Go API (`vendor-tool/main.go`; esbuild version
pinned in `vendor-tool/go.mod` and `go.sum`). It needs python3 and Go, and
needs no npm and no Node. The same inputs always produce the same bytes.

`regenerate.sh --check` writes nothing and fails if `zenoh-ts.js`, `licenses/`
or the SHA-256 above differ from what the lock file produces. CI runs it on
every change under `ops/serve/zenoh-sandbox/`.

To move to another version: edit the versions, tarball URLs and integrity
values in `packages.lock.json`, run `regenerate.sh`, update the table and the
SHA-256 in this file, and commit all of it together.
