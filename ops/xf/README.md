# ops/xf: the goat-transformers pin and pack bundles

goatstuff uses the [goat-transformers](https://github.com/USAREUR-AF/goat-transformers)
library in `ops/zenoh-shims/transform-shim/` (Rust engine) and in
`ops/serve/goat-map/` (Python engine). Design:
[docs/design/format-transformers.md](../../docs/design/format-transformers.md).

| Path | What |
|---|---|
| `goat-transformers.ref` | the library commit every consumer here uses |
| `bundle.py` | builds and checks a consumer's pack bundle: the model modules and packs it loads, with a manifest of SHA-256 values |
| `tests/` | checks both bundles against their manifests and the pin |

The pin is goat-transformers `9e951e9`. Every bundle file is the library's
file at that commit: the `geo-track-v1` pack, the `geotrack` module and the
`cot-xml` keys once proposed from `upstream-proposed/` are in the library, and
that directory is deleted. goat-map's comparison of its two CoT paths, and what
the library still lacks, are in the design document, section 4.1.

```bash
python3 ops/xf/bundle.py check ops/serve/goat-map/xf-bundle
python3 ops/xf/bundle.py check ops/zenoh-shims/transform-shim/bundle --library <goat-transformers checkout>
python3 -m pytest -q ops/xf
```
