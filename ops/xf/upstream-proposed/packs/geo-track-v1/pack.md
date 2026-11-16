# Pack: geo-track-v1

GeoTrackV1, the display track of the goat map surface in the public repository
`USAREUR-AF/goatstuff` (`ops/serve/goat-map/schemas/geo/track.v1.schema.json`).
Public. One flat JSON object per track, on the `json` wire.

| Schema member | Model key |
|---|---|
| `schema` | none: constant `goat.geo.track.v1`, checked on decode and written on encode |
| `id` | `geotrack.id`, derived in the module from `geotrack.sensor`, a colon and `uid` |
| `label` | `uid` |
| `kind` | `geotrack.kind` |
| `source.sensor`, `.site`, `.node`, `.goatnet` | `geotrack.sensor`, `geotrack.site`, `geotrack.node`, `geotrack.goatnet` |
| `lat`, `lon`, `alt_m` | `lat`, `lon`, `hae` |
| `course_deg`, `speed_mps` | `course`, `speed_ms` |
| `ce_m`, `le_m` | `ce`, `le` |
| `cot_type`, `how` | `type`, `how`; an absent type is written `""` |
| `descriptor` | `cotxml.uid_droid` |
| `reported_at`, `stale_at` | `cotxml.time_text`, `cotxml.stale_text` (the sender's text), else `time_ms`, `stale_ms` |
| `observed_at` | `geotrack.observed_at` (text, as the host wrote it) |
| `producer_anchor` | `geotrack.producer_anchor` |
| `synthetic` | `geotrack.synthetic` (1 for `true`, 0 for `false`) |
| `org` | `geotrack.org` |

The kind requires the model keys of the schema's required members:
`geotrack.id`, `uid`, `geotrack.kind`, `geotrack.sensor`, `geotrack.site`,
`geotrack.node`, `lat`, `lon`, `geotrack.observed_at`,
`geotrack.producer_anchor`, `geotrack.synthetic`. A sensor format holds none
of the `geotrack` keys, so a conversion into this format supplies them as
context; without them, or without a position or an identifier, it is refused
with `no-kind`.

Not mapped:

- `affiliation` and `dimension` are the second and third token of the type,
  each looked up in a table, `unknown` when missing or unlisted. The pack
  language has no function that takes a token of a string, so they round-trip
  through residue and a conversion from another format does not write them.
- `updates` and `first_seen_at` round-trip through residue.
- `trail` is an array of number pairs and is not carried.
- A member whose value is `null` is absent, and an absent key writes no
  member. The schema allows both forms for every member this affects.
- `ce_m` and `le_m` carry `9999999` as a number. The schema's rule that this
  value becomes `null` needs absent-value sentinels.

`reported_at` and `stale_at` are the sender's text when the source format
kept it (`cot-xml` does, in the module `cotxml`): the schema calls the member
verbatim and unrepaired. The text line is before the instant line, so it is
the one encode writes (ruling 16 of spec 0.5). From a format that holds only
the instant, the member is written as `YYYY-MM-DDTHH:MM:SS.fffZ`. A track
whose `reported_at` is not an instant does not decode.
