# PostgreSQL And Repositories

Apply this guide to SeaORM entities and repositories, SQL, repository-facing
persistence, and PostgreSQL-backed tests. Create actual migrations in
`jellyfin-migration`.

## Schema And Transactions

- Keep constraints, indexes, atomic upserts, and transactional invariants in
  PostgreSQL where practical, while keeping domain rules explicit in Rust.
- Add a database index or constraint required by a new query pattern in a new
  migration. Never edit a migration that may already be applied.
- Keep entity changes synchronized with the corresponding migration.
- Validate and lock multiple rows in a deterministic order to avoid deadlocks.
- Replace chapters or other owned child sets under an owner-row lock in one
  transaction, including empty replacements. Roll back every batch on failure.
- Serialize PostgreSQL integration tests that mutate the shared virtual-folder
  catalog while running a full scan.
- Do not weaken count or failure checks to hide a scan race.

## Query Rules

- Use set-based queries or fixed-size batches. Do not issue per-item queries.
- Use fixed 128-row batches for large relation and chapter operations unless
  the existing repository establishes a smaller safe bound.
- Materialize shared candidate sets when count and page queries would otherwise
  repeat expensive work.
- For shared Resume candidates, materialize only item ids and latest playback
  dates after applying all authorization and metadata filters. Load complete
  page items afterward in the same repeatable-read transaction; do not put wide
  JSON metadata in the shared temporary result.
- Build playback-aware queries from target-user `user_data` and reverse
  hierarchy lookups, not correlated scans over all `base_items`.
- When a route has already resolved and authorized a user, reuse the authorized
  listing path instead of reloading that user. Public service entry points must
  still validate their target for callers without a resolved policy context.
- Build list counts, pages, and facets from the same filtered candidate set.
- Exclude alternate versions and owned non-extra rows from ordinary counts where
  official behavior does.
- Preserve the official nullable-user fallback for administrators requesting a
  nonexistent target.
- Apply `LatestItemsExcludes` only to implicit root scope. Apply grouping before
  the latest-items limit and keep candidate buffers bounded.
- Use nearest-ancestor closure-table queries for music and photo grouping.
- Count folder children, recursive items, and item values with
  target-user-policy-aware set queries.
- Keep filtering, counting, and media-source queries aware of canonical short
  item types and official legacy CLR-qualified persisted names.
- Preserve unknown plugin-defined item types. Hydrate persisted rows through the
  shared type registry and never leak old CLR names through public
  `BaseItemDto.Type`.

## Route-Specific Invariants

- For `/Years`, preserve the recursive-folder quirk: report visible filtered
  primary descendants before extracting distinct positive production years.
  Non-recursive and non-folder parents use distinct-year count.
- Keep item details, lists, and counts consistent about the primary that
  represents a version group.
- Keep alternate-version stream and attachment loading batched by every version
  id on the page.
- Persist deterministic parent-local `sort_order` for versions; preserve it
  across rescans and compact both sides when a child moves.
- Keep chapter reads ordered by `StartPositionTicks` and alternate versions
  isolated. Chapter image lookup uses `(item_id, ChapterIndex)`.
- Keep filters, years, ratings, and stream-language buckets derived from one
  target-user policy snapshot.
- Build Filters2 audio/subtitle languages with one policy-aware set query,
  including alternate versions and Episode descendants of Series/Season
  searches. Map missing languages to `und` and sort by localized display name.

## Character And Identifier Semantics

- Use official deterministic UTF-16LE identifier and `.NET Guid(byte[])`
  semantics for Genre, MusicGenre, Studio, Year, and Person item-by-name rows.
- Lowercase item-by-name keys when normalization is enabled or case-sensitive
  ids are disabled. Preserve path casing only when normalization is disabled
  and case-sensitive ids are enabled.
- Persist and round-trip `EnableNormalizedItemByNameIds` and
  `EnableCaseSensitiveItemIds`; case-sensitive ids default to true.
- Collapse equal persisted `PresentationUniqueKey` groups to the smallest UUID
  before counting, ordering, and paging.
- Keep `people.id` private as the credit key. Public Person ids are canonical
  Person `base_items.id` values.
- Translate `Items?PersonIds=` from public Person ids through exact persisted
  names to internal credit ids in one set-based query.
- Keep a person discoverable when any credited policy-visible item, including an
  alternate version, matches, while Person detail counts use policy-visible
  primary items.
- Derive remote image and media-segment provider ids with the documented
  UTF-16LE MD5 and `.NET Guid` rules. Compare provider names case-insensitively
  only where official behavior does.

## PostgreSQL Tests

- Some `jellyfin-data` integration tests create temporary databases named with a
  `jellyfin_` prefix. Use a disposable PostgreSQL server or database.
- Never point destructive tests at a database containing user data. The test
  account must be allowed to create and drop these temporary databases.
- Run PostgreSQL-backed route and scan tests against a disposable database.
- For Resume query changes, use the ignored
  `resume_materialization_plan` test and
  `JELLYFIN_RESUME_PLAN_DUMP`. Compare materialized row width and temporary
  blocks separately from process RSS.
- Execution-plan estimates and database cache growth alone do not establish a
  leak.
