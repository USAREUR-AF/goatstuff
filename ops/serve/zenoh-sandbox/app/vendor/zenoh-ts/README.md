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
`cb6d288c1579d70225d3005a0a830aba975b9c3fc94ae6dafbf8c78498e128b9`.

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
whose SHA-512 differs from the lock file, and bundles the unpacked tree into
one ES module with esbuild's Go API (`vendor-tool/main.go`; esbuild version
pinned in `vendor-tool/go.mod` and `go.sum`). It needs python3 and Go, and
needs no npm and no Node. The same inputs always produce the same bytes.

`regenerate.sh --check` writes nothing and fails if `zenoh-ts.js`, `licenses/`
or the SHA-256 above differ from what the lock file produces. CI runs it on
every change under `ops/serve/zenoh-sandbox/`.

To move to another version: edit the versions, tarball URLs and integrity
values in `packages.lock.json`, run `regenerate.sh`, update the table and the
SHA-256 in this file, and commit all of it together.
