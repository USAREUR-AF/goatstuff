# Proposed for goat-transformers

**Everything in this directory is written here and offered to the
[goat-transformers](https://github.com/USAREUR-AF/goat-transformers) library.
None of it is in the library at the commit in `../goat-transformers.ref`.**

It has the library's layout, so it can be copied into a library checkout as it
is:

```
model/geotrack.def                 model module `geotrack`
model/cotxml.def                   model module `cotxml`
packs/geo-track-v1/                pack.def, pack.md, PROVENANCE.md, vectors/
packs/cot-xml/pack.def             the library's cot-xml pack with five lines added
packs/cot-xml/vectors/geo-*.json   vectors to add to the library's cot-xml pack
```

`../bundle.py` takes a file from here before the library's file of the same
path and marks it `upstream-proposed` in the bundle manifest. With
`--library`, its check refuses a file here that drops or changes a line of the
library file it replaces.
`../tests/test_upstream_proposed.py` runs the vectors. When the library
carries these files, this directory is deleted and the bundles are rebuilt.
