# d3 (vendored)

`d3.min.js` is the d3 v7.9.0 UMD bundle, copied unchanged from
`dist/d3.min.js` in the npm package `d3@7.9.0`.

| | |
|---|---|
| Version | 7.9.0 |
| Source | https://registry.npmjs.org/d3/-/d3-7.9.0.tgz, file `package/dist/d3.min.js` |
| Tarball integrity (npm) | `sha512-e1U46jVP+w7Iut8Jt8ri1YsPOvFpg46k+K8TpCb0P+zjCkjkPnV7WzfDJzMHy1LnA+wj5pLT1wjO901gLXeEhA==` |
| `d3.min.js` sha256 | `f2094bbf6141b359722c4fe454eb6c4b0f0e42cc10cc7af921fc158fceb86539` |
| License | ISC (text below) |
| Upstream | https://github.com/d3/d3 |

`../../src/render.js` imports it by relative path. Loaded as an ES module the UMD
wrapper sets `globalThis.d3`, which `render.js` reads.

To update: download the new release tarball from the npm registry, check its
integrity against the registry metadata, copy `package/dist/d3.min.js` here,
and update the version, integrity and sha256 in this file. Check the sha256
with `shasum -a 256 d3.min.js`.

## License

```
Copyright 2010-2023 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
```
