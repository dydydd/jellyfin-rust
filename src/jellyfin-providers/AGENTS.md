# Metadata Providers

Apply this guide to provider search, matching, enrichment, and merge logic.

## Behavior

- Providers have deterministic priority and merge behavior.
- Merge local metadata before remote providers.
- Preserve unknown or optional metadata. A partial provider response must not
  erase valid established metadata.
- Honor `LockedFields.Name`, `Overview`, `OfficialRating`, and `Genres` on both
  gap filling and full replacement where official behavior does. A locked Name
  also protects `SortName`.
- Remote blank or whitespace-only names must never erase an established title.
- Merge genres only when permitted. Keep JSON `Genres` atomic with normalized
  PostgreSQL genre relations.
- Treat provider failure and timeout as different from a valid empty result.
- Keep provider errors useful without logging credentials, API keys, signed
  URLs, or secret-bearing query strings.
- Use official provider ids and ordering. Do not substitute collection or
  legacy TV ids where the official provider does not.
- Map TMDb trailers with official semantics: accept only Trailer and Teaser
  YouTube videos; for Movies and Series place Trailer before Teaser with stable
  source order; for Episodes retain source order, deduplicate URLs
  case-insensitively, leave names absent, and construct the URL even when TMDb
  omits the key.
- Project external URLs only from the provider registry present in the official
  server, preserving provider order and interpolating persisted ids verbatim.
- Treat remote blank or whitespace-only values as missing and never replace a
  valid established field with an empty provider response.
- For bulk season refresh, apply metadata to the visible primary of each version
  group and preserve established scraped titles during rescans.

## Resource Limits

- Provider HTTP calls require timeouts and bounded concurrency.
- Do not hold a database transaction or lock across provider network work.
- Mock provider HTTP in tests; do not depend on live third-party services.
- Cover failures, timeouts, malformed bodies, empty responses, and cancellation
  with bounded fixtures.
- Release permits promptly on failure or cancellation and do not spawn detached
  work to bypass cancellation.
