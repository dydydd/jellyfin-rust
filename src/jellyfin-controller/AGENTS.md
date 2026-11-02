# Controller And Library Work

Apply this guide to scans, watchers, metadata refresh, library queries, and
authorization orchestration. Keep HTTP parsing in `jellyfin-api` and persistence
in `jellyfin-data`.

## Scan Correctness

- Serial and concurrent scans must fail equivalently. Database, hierarchy, and
  filesystem/persistence failures fail the scan with an accurate total and a
  bounded sample of per-file diagnostics.
- An FFprobe failure is partial success: retain the item with fallback streams
  and do not fail the entire scan.
- Create Series and Season nodes idempotently while holding the PostgreSQL
  hierarchy lock. A duplicate-node race must not drop a media item.
- Reuse Series/Season resolution only within one bounded file batch. A cache miss
  must still use the locked create-or-read path.
- Persist per-item NFO relations outside the hierarchy cache. Store additive NFO
  Studio and Person relations in one validated transaction per item, preserving
  existing credits, Person order, and role semantics.
- Apply `.ignore` using the nearest-ancestor rule before adding a file to a
  snapshot, recursing, or resolving extras. Clear only the directory lookup at
  the start of each full or single-library scan. Keep the parsed-rule cache
  bounded and propagate file I/O failures as scan failures.
- Do not hide scan races by weakening failure checks.

## Watchers And Batches

- Keep watcher queues bounded and deduplicated.
- Coalesce changed paths by virtual library before scanning.
- Reuse one short-lived directory snapshot for sibling media discovery.
- Batch PostgreSQL reads and writes; do not issue per-file queries.
- Skip unavailable or non-directory roots and isolate each root's watch
  registration error. A successful parent watch may cover duplicate and child
  roots. A child may still register if its parent failed.
- Do not keep an idle watcher thread when no root was registered.

## Persisted File Facts

- Persist local size and media facts gathered from the scan snapshot. Do not stat
  media files from browse or playback-info request paths.
- Persist the local `.strm` file length during scans. Do not stat the remote
  target during item details.
- Version successful local Audio/Video probes with a fingerprint containing
  normalized path, size, and modification time.
- A missing, stale, or mismatched probe marker triggers one bounded repair even
  when streams exist. Write the marker only after all probe data is persisted.
- Treat `.strm` and remote sources as lazy playback probes.
- Lazy `.strm` probing must synchronously terminate FFprobe before returning.
  An async timeout around an uncancelled blocking child is insufficient.
- Coordinate lazy probes by item and resolved target so concurrent requests
  share one bounded flight. Keep failure backoff short-lived and hard-bounded.

## Metadata And Hierarchy

- Providers use deterministic priority and merge behavior. Merge local metadata
  before remote providers.
- Preserve unknown and optional metadata. A partial provider response must not
  erase valid established metadata.
- Honor `LockedFields.Name`, `Overview`, `OfficialRating`, and `Genres` during
  gap filling and full replacement where official behavior does. A locked Name
  also protects `SortName`.
- Remote blank or whitespace-only names never erase an established title. Merge
  genres only when permitted and keep JSON `Genres` atomic with normalized
  PostgreSQL genre relations.
- Treat episode NFO titles equal to the series name, `showtitle`, or filename as
  placeholders during later scans. Repeated scans and alternate-version
  regrouping must not replace a valid scraped title with a path- or
  series-derived placeholder.
- For bulk season refresh, apply metadata to the visible primary of each version
  group.
- Missing-metadata repair includes primary episodes whose title is empty or
  still equals the parent series, even when provider ids exist.
- An unlocked primary may use a non-placeholder `OriginalTitle` only when its
  name is empty, path-derived, or equal to the parent series.
- Do not turn an invalid episode `SortBy` into a 400 when official behavior
  treats malformed input as unset, and do not randomize when `Random` is not
  the final requested value.
- Bind Episodes `SortBy` through official enum-converter semantics: names,
  signed `Int32` values, and comma-delimited bitwise combinations. Only a final
  `Random` value randomizes; malformed input behaves as unset.
- Apply post-provider `OriginalTitle` repairs with one set-based PostgreSQL
  update scoped to the refreshed series or episode.
- If TMDb returns an episode name equal to its series in the preferred
  language, fetch English episode metadata once and use only a non-placeholder
  name as fallback. Preserve localized non-name fields.
- Use relational Series/Season links for DTO names and image inheritance instead
  of parsing paths at request time.
- Order episode pages with the official aired-episode comparer before
  `StartItemId`, adjacency, or paging. Place season-zero specials relative to
  regular episodes using the official air-before/after anchors.
- Apply shared image and user-data DTO options to Episodes, Seasons, Upcoming,
  and Next Up instead of accepting and discarding them.

## Item Values And People

- Use official deterministic UTF-16LE identifier and `.NET Guid(byte[])`
  semantics for Genre, MusicGenre, Studio, Year, and Person item-by-name rows.
- Lowercase item-by-name keys when normalization is enabled or case-sensitive
  ids are disabled. Preserve path casing only when normalization is disabled
  and case-sensitive ids are enabled.
- Persist and round-trip `EnableNormalizedItemByNameIds` and
  `EnableCaseSensitiveItemIds`; case-sensitive ids default to true.
- Create official metadata directories before inserting item-by-name rows.
- Backfill and reconcile in deterministic keyset pages with bounded batches.
  Never replace or delete established entities, images, or metadata during
  reconciliation.
- Genre and MusicGenre list routes and Filters2 expose persisted BaseItem ids,
  not normalized item-value ids.
- Collapse equal persisted `PresentationUniqueKey` groups to the smallest UUID
  before counting, ordering, and paging.
- Keep `people.id` private as the internal credit key. Public Person DTOs,
  `/Persons`, `BaseItemDto.People`, SearchHint, images, favorites, and user data
  use the canonical Person `base_items.id`.
- Translate `Items?PersonIds=` through exact persisted names to internal credit
  ids in one set-based query.
- A person is discoverable through `/Persons` when any credited policy-visible
  item, including an alternate version, matches. Person detail counts use only
  policy-visible primary items.

## Playback State

- Persist progress against the authorized selected `MediaSourceId`.
- When projecting the displayed primary, fall back to the latest visible
  alternate-version user-data row when the primary has none.
- Normalize start/progress reports against the active transcoding registry. An
  omitted method defaults to Transcode; downgrade to DirectPlay when
  `PlaySessionId` is blank or unknown.
- Build `NowPlayingItem` from the displayed item and authorized source, then
  reuse that snapshot when later progress reports omit `Item`.
- Validate `Item` as an object or null and every `NowPlayingQueue` entry as a
  valid official `QueueItem` before changing session state.
- Compute Next Up from visible primary episodes while aggregating played,
  resume, and activity state across versions.
- Apply `NextUpDateCutoff` to the latest activity date and use the official
  aired-episode comparer for placed season-zero specials.
- Preserve password-change session revocation and password-reset behavior.
  Device sessions remain bounded by their user's policy.

## Images, Lyrics, And External Data

- Single-flight remote image downloads by URL, with at most four leader
  downloads for distinct URLs. Acquire the global permit inside the
  single-flight initializer so followers consume no extra permit.
- Validate that upstream content is an image and suppress permanently invalid
  references according to official behavior.
- Do not decode, resize, reformat, or decorate images unless a route explicitly
  requires it. Image existence checks use PostgreSQL metadata, not DTO
  projection or BlurHash generation.
- Clone or reference legacy image files; never move or delete source files.
- Store uploaded and downloaded lyrics under the item's internal metadata
  directory with a same-directory temporary file and atomic rename, then
  register the file as a Lyric stream. Parsed JSON is only a cache.
- Authorize an Audio item before validating upload body or filename. Never
  remove unregistered files, symlinks, or paths outside the metadata root.
- Lyric search/download use at most four providers in flight, a 30-second
  deadline per provider, configured result order, and cancellation-by-drop.
  Search errors become isolated empty results; unknown, empty, or unparseable
  download responses are 404, while provider failures remain server errors.
- Derive provider ids from invariant-lowercase names with UTF-16LE MD5,
  `.NET Guid(byte[])` ordering, and lowercase hyphenless `N` formatting. Split a
  provider-owned lyric id only at its first underscore.
- Expose configured lyric providers as the music library's `LyricFetchers`
  options in execution order, case-insensitively deduplicated and enabled by
  default. Do not expose them for representative types without Audio.
- Filter media segments by registered provider ids and the owning library's
  disabled providers, comparing names case-insensitively. Return an empty
  result when no matching provider is enabled.

## Bounded Work

- Apply concurrency limits at the expensive resource boundary, not only at the
  request layer.
- Do not hold a transaction, lock, semaphore, or filesystem handle across
  unrelated network waits.
- Provider and remote HTTP calls require a deadline. External processes require
  owned handles, kill-on-cancel behavior, and output limits.
- Lock multiple rows in a deterministic order. Do not spawn detached tasks to
  work around cancellation; request-owned futures should cancel on drop.
- Release permits promptly on failure or cancellation. Avoid unbounded queues,
  caches, retry state, and per-request buffers.
- Do not assume allocator-retained RSS is a leak. Measure whether scan work is
  reclaimed and distinguish page cache from anonymous memory.
- For large scans, report baseline, peak, 60-second post-scan, and 300-second
  post-scan RSS and anonymous memory when possible.
