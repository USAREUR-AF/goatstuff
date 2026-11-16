# ops/xf: the goat-transformers pin and pack bundles

goatstuff uses the [goat-transformers](https://github.com/USAREUR-AF/goat-transformers)
library in `ops/zenoh-shims/transform-shim/` (Rust engine) and in
`ops/serve/goat-map/` (Python engine). Design:
[docs/design/format-transformers.md](../../docs/design/format-transformers.md).

| Path | What |
|---|---|
| `goat-transformers.ref` | the library commit every consumer here uses |
| `bundle.py` | builds and checks a consumer's pack bundle: the model modules and packs it loads, with a manifest of SHA-256 values |
| `upstream-proposed/` | two model modules, a pack and five added lines of the `cot-xml` pack, written here and offered to the library |
| `tests/` | runs the proposed vectors and checks both bundles |

```bash
python3 ops/xf/bundle.py check ops/serve/goat-map/xf-bundle
python3 ops/xf/bundle.py check ops/zenoh-shims/transform-shim/bundle --library <goat-transformers checkout>
python3 -m pytest -q ops/xf
```
