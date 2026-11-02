# NFO And Local Metadata

Apply this guide to NFO parsing, local metadata read/write, and scan-time
relation persistence.

## Persistence

- Parse and validate the complete NFO metadata before changing item state.
- Preserve existing credits and established metadata when an NFO only adds
  Studio or Person relations. Persist additive relations in one validated
  transaction per media item.
- Keep Person order and role conflict semantics stable across rescans.
- Persist per-item NFO relations outside the Series/Season hierarchy cache.
- Do not replace or delete established entities, images, or metadata merely to
  reduce scan round trips.
- Treat episode titles equal to the series name, `showtitle`, or filename as
  placeholders during later scans.
- Repeated scans and alternate-version regrouping must not replace a valid
  scraped title with a path- or series-derived placeholder.

## Merge And Safety

- Merge local metadata before remote providers. Do not erase valid established
  fields when an optional NFO field is absent or malformed.
- Accept punctuation-only Studio names in JSON/NFO even when a normalized
  search key cannot be persisted; do not turn that edit into a client error.
- Resolve paths under the expected metadata root and never follow a path outside
  it.
- Do not move, delete, or rewrite source media as part of metadata ingestion.
- Never log or persist secrets embedded in paths, URLs, or provider metadata.
