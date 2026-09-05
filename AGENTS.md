# Jellyfin Rust contributor guide

## Scope

- This repository reimplements the Jellyfin server in Rust while preserving compatibility with the official Jellyfin API and web client.
- The checked-out official server source in `jellyfin/` is the behavioral reference. Prefer matching its externally visible behavior, defaults, validation, authorization, ordering, and error semantics over inventing new behavior.
- Current optimization priorities are media-library management and scanning, users and policies, metadata scraping/providers, and PostgreSQL-backed data access.
- Do not work on Live TV unless a task explicitly asks for it. Avoid incidental changes under `src/jellyfin-live-tv`.

## Working practices

- Read the relevant Rust implementation and its official C# counterpart before changing behavior. Record important parity assumptions in tests or focused comments.
- Keep changes small and independently reviewable. Complete one coherent fix, run its focused tests, and commit it before starting another fix.
- Preserve unrelated user changes and existing commits. Never rewrite history or use destructive Git commands.
- Prefer bounded concurrency, streaming or pagination, batched PostgreSQL operations, and short-lived buffers for library scans. Do not collect an entire library into memory when work can be processed incrementally.
- Keep filesystem watcher queues bounded and deduplicated. Coalesce changed paths by virtual library before scanning, reuse one short-lived directory snapshot for sibling media discovery, and batch PostgreSQL reads and writes instead of issuing per-item queries.
- Keep deterministic scan hierarchy creation idempotent under sibling-file concurrency. Series and
  season nodes must be checked and created while holding the PostgreSQL hierarchy lock so a
  duplicate-node race cannot silently drop one media item.
- Keep database invariants in PostgreSQL where practical (constraints, indexes, atomic upserts, transactions), while keeping domain rules explicit in Rust.
- Avoid N+1 queries. Use set-based queries or bounded batches, and add migrations for indexes or constraints required by new query patterns.
- Build playback-aware queries from the target user's `user_data` rows and reverse hierarchy lookups rather than correlated scans over all `base_items`. Materialize shared candidate sets when count and page queries would otherwise repeat expensive work.
- Project inherited images for an item page with one batched DTO-image lookup. Do not call the image projector once per item.
- Resolve Similar and InstantMix seeds through the target user's normal library policy, and apply the
  same folder, tag, rating, and parental filters to every candidate query. Similar defaults to 50
  returned items and reports the post-limit result count; legacy CLR item types must not bypass policy.
- Keep every InstantMix route on the official DTO-options contract. Accept signed limits and
  case-insensitive repeated fields/image options, report the pre-limit total, validate the Playlist
  route's seed type, collect Folder descendant-audio genres in one policy-aware query, and treat an
  empty-genre Audio seed as an unfiltered visible-Audio mix while an unknown genre name stays empty.
- Keep the six Similar routes on one contract: bind `ExcludeArtistIds`, `UserId`, signed `Limit`, and
  `Fields` case-insensitively; return official empty results for Episodes and named items other than
  MusicArtist; and project the bounded page with default images, user data, and ProviderIds.
- Project theme songs and theme videos with the official default all-fields `DtoOptions`. Resolve
  `inheritFromParent` nearest-first and independently for each media kind, preserve that owner's
  id, default to `SortName` ascending, and keep `SoundtrackSongsResult` as a distinct empty result.
  Batch candidate loading across the owner chain and apply the target user's normal library policy.
- Coordinate remote-image downloads by URL so concurrent items share one bounded download. Validate that upstream content is an image, and remove or otherwise suppress permanently invalid remote references according to official behavior.
- Check whether provider artwork exists with a PostgreSQL image-type query. Do not route existence checks through DTO image projection, local dimension inspection, or BlurHash generation.
- Treat passwords, access tokens, API keys, and deployment credentials as secrets. Do not log or commit them.
- Do not decode, resize, reformat, decorate, or otherwise transform images requested by API
  clients. Keep accepting the official image query surface for compatibility, but stream the
  original image bytes and content type so media-library browsing cannot create decoder-sized
  memory spikes or a family of derived cache files. Image-info endpoints must return persisted
  dimensions and BlurHash values without lazily decoding the source or writing metadata.
- Stream trickplay tile files with bounded chunks and preserve HEAD and byte-range semantics; never
  read an entire tile into a response buffer.

## Compatibility expectations

- Match official Jellyfin DTO field names, nullability, defaults, HTTP status codes, authorization requirements, sorting, pagination, and case-insensitive matching.
- ASP.NET route, query-name, and JSON-property binding is case-insensitive. Compatibility tests must cover PascalCase, camelCase, and representative lowercase legacy requests; do not assume an Axum route or Serde field is equivalent merely because one casing works.
- When a catch-all implements several official HLS or trickplay route templates, keep concrete
  official-path dispatch tests and representative lowercase aliases so Axum does not regress the
  case-insensitive ASP.NET route contract. Lowercase compatibility must include every static path
  segment, including compound segments such as `ActiveEncodings`.
- Follow the official `JsonDefaults` value semantics. Where it permits them, accept numeric strings and case-insensitive or integer enum representations, and mirror the full official parameter set when implementing a legacy endpoint.
- Bind the eight official virtual-folder `CollectionTypeOptions` values case-insensitively and persist/project their canonical lowercase wire names. Keep `mixed` valid for virtual-folder management but omit it from `BaseItemDto.CollectionType`, and tolerate legacy mixed-case persisted view metadata.
- Treat generated SDK models as executable compatibility specifications alongside the C# DTOs. Swift `Codable` rejects the entire enclosing item or page when one nested object, enum, dictionary value, or date has the wrong wire shape.
- Hydrate every persisted base item through the shared item-type registry before DTO projection,
  including playlist entries, so legacy CLR names never escape through `BaseItemDto.Type` and an
  unknown plugin row cannot make a client reject the enclosing page.
- Keep library-creation `CollectionTypeOptions` distinct from `BaseItemDto.CollectionType`: `mixed`
  is valid for a virtual-folder configuration but must be omitted from user-view item DTOs because
  the client DTO enum cannot decode it.
- Project the official single-item detail routes with their default all-fields `DtoOptions`: clients must receive media sources, nested and top-level media streams, and trickplay without supplying a non-official `Fields` query.
- Project persisted `SeriesName` and `SeasonName` on `BaseItemDto`; these are unconditional
  episode/season identity fields in official item details and lists, not optional `Fields` values.
- Project persisted `OriginalLanguage` unconditionally on item details and lists. When expanding
  alternate versions, use each source item's own original language for its stream defaults and
  keep an exact alternate-id detail tied to that alternate rather than the displayed primary.
- Audit DTOs recursively: preserve object-array shapes, serialize API enums by their official names, keep string dictionaries string-valued, and emit full API `DateTime` values rather than storage-only dates.
- Treat alternate video versions as one playback group. Item details and `PlaybackInfo` must expose every version as a distinct `MediaSource`, honor `MediaSourceId` when opening static or transcoded content, and keep all stream and attachment loading batched by version identifiers.
- Apply the playback `DeviceProfile` independently to every returned `MediaSource`, preserving source order and producing version-specific flags and URLs. Only apply explicit audio or subtitle indexes to the source whose id matches an explicitly requested `MediaSourceId`.
- Project each media source's persisted total bitrate, and when it is absent infer it from that
  source's non-external media streams as official Jellyfin does. Keep this per-version so item
  details and `PlaybackInfo` never reuse the displayed primary's bitrate for alternate versions.
- Project each source's persisted, probed container before considering its path extension. When a
  persisted container lists alternatives, select the path-matching value or the first value, and
  strip URL query/fragment components before any extension fallback.
- Project every audio and subtitle stream through the same language/localization path for item
  details and `PlaybackInfo`, including every alternate media source. Canonicalize recognized ISO
  639-2 bibliographic codes, preserve unrecognized codes, and let `DisplayTitle` fall back to that
  raw code when `LocalizedLanguage` is unavailable.
- Project `MediaSourceCount` for item pages with one batched alternate-version query. Preserve the
  official nullable-single-source behavior, and include episode groups so the web client can show
  merged episode versions without loading every `MediaSource`.
- When item pages request `MediaSources`, expand every alternate-version group and load all streams
  and attachments for the page in bounded batches. Do not issue one version, stream, or attachment
  query per displayed item.
- Persist scan-discovered versions as `LocalAlternateVersion` relationships and user `MergeVersions`
  groups as `LinkedAlternateVersion`. Clearing alternate sources removes only the user-created layer
  and restores every local subgroup. On details, the requested source is `Default`, other manually
  linked roots are `Grouping`, and local alternates remain `Default`; exact alternate-id details must
  retain normal target-user policy checks even though list queries fold alternates into the primary.
- Name versioned media sources from the common prefix of their library file stems so clients see
  concise version labels. For `.strm` items, derive the label from the sidecar filename while using
  the resolved remote target only for the source path and protocol.
- Persist media file sizes from the scan's directory snapshot and project each source's own Size,
  file ETag, and VideoType. Do not stat media files while serving browse or playback-info APIs.
- Build item-count aggregates from the same filtered candidate set as item pages: exclude alternate
  versions and owned non-extra rows by default, and apply the target user's folder, tag, parental,
  virtual-item, and favorite filters before grouping by item type.
- Project requested folder ChildCount values in one batch. Count episodes by SeasonId, prefer linked
  children, deduplicate merged folder children by PresentationUniqueKey, and honor the user's
  DisplayMissingEpisodes preference without issuing per-folder queries.
- Project requested `RecursiveItemCount` values with one batched, user-policy-aware leaf query.
  Traverse hierarchy and linked descendants, expand merged folder groups, exclude virtual leaves,
  alternate versions, and owned non-extra rows, and return zero entries without per-folder fallbacks.
- Order episode detail pages with the official aired-episode comparer before applying `StartItemId`, adjacency, or pagination. Specials with `AirsBeforeSeasonNumber`, `AirsAfterSeasonNumber`, or `AirsBeforeEpisodeNumber` must be positioned relative to regular episodes rather than compared with a single incompatible numeric key; season zero itself remains sorted by `SortName`.
- `Items/Latest` defaults `GroupItems` to true. For TV, select the top Series groups from the
  complete policy-filtered Episode set before applying the result limit, then analyze each Series'
  inclusive 24-hour window in PostgreSQL. Return Series for cross-season additions; for one Season
  containing multiple recent Episodes or the complete Season, return Season when the Series has
  multiple Seasons and Series otherwise. Count only visible primary, non-virtual Episodes, expose
  the recent-child count, and fall back to the newest Episode when the selected container is hidden.
  Keep candidate buffers bounded and load final containers and fallback Episodes in batches.
- Resolve `Items/Latest` Audio and Photo grouping containers from the nearest matching
  `MusicAlbum` or `PhotoAlbum` ancestor by closure-table depth, not only the direct parent. Load
  all resolved containers through one target-user-policy-aware batch; fall back to the media item
  when its container is not visible. A `MusicAlbum` replaces even one recent Audio item, while a
  `PhotoAlbum` replaces its Photos only when at least two recent items share it.
- Item-value `ItemCounts` inherit Genre and Studio links from a Series to its visible descendant
  Episodes, but do not inherit Artist or other value kinds. Count direct and inherited matches with
  set-based PostgreSQL queries and deduplicate Episodes that carry the same value directly.
- `IncludeItemTypes` and `ExcludeItemTypes` filters, count queries, and media-source queries must
  recognize canonical short item types and official legacy CLR-qualified persisted names, including
  case-insensitive API enum input. Preserve unknown plugin-defined types instead of discarding them.
  Continue folding alternate rows before `/Items/Counts` buckets are calculated so legacy versions
  do not inflate either a typed bucket or `ItemCount`.
- After scanning a movie directory, supplement the official filename-based version resolver with a conservative metadata match: same directory, non-empty case-insensitive title, and the same non-empty year, while rejecting the whole candidate group when TMDb, IMDb, or TVDb identifiers conflict. Never use collection identifiers as movie identity.
- Do not advertise a playback method unless the returned URL really implements it. In particular, never label unchanged container bytes as an MP4 direct stream; derive `SupportsDirectPlay`, `SupportsDirectStream`, and `SupportsTranscoding` from the final selected method and device policy.
- Proxy static HTTP media sources through the server like official Jellyfin instead of redirecting clients to private or signed upstream URLs. Forward byte ranges, preserve upstream status and content headers, stream without whole-file buffering, and keep signed URLs out of logs.
- A client may register its `DeviceProfile` once through session capabilities and omit it from later `PlaybackInfo` calls. Follow the official query-over-body precedence and fall back to the authenticated session profile before choosing a stream.
- Resolve an explicit `MediaSourceId` inside the authorized alternate-version group before lazy `.strm` probing, and hydrate that selected source rather than the displayed primary. Probe diagnostics must identify the item without logging a target path or signed URL.
- Select HLS playlist mode after resolving `MediaSourceId`. A selected source with an unknown runtime must use a job-scoped EVENT playlist; do not coerce a null runtime into a zero-length VOD, and keep known positive runtimes on the finite VOD path.
- Preserve unknown or optional metadata where the official server does; a partial provider response must not erase valid existing metadata.
- Merge remote movie and series genres only when `LockedFields.Genres` permits it and the refresh replaces data or fills an empty target, including a lower-priority provider filling a gap left by the preferred provider. An empty provider genre list must not erase established genres, and JSON `Genres` must stay atomic with normalized PostgreSQL genre relations.
- Merge episode metadata in official priority order: local metadata first, remote providers filling or replacing only eligible placeholders, and `LockedFields.Name` always protecting an established title. A repeated scan or alternate-version regroup must not turn a scraped episode title back into the series or filename-derived group name. During later scans, treat an Episode NFO title equal to its `showtitle`, known series name, or media filename as a placeholder: keep the established `Name` and `SortName` while still merging the NFO's other fields.
- During bulk season refresh, select the visible primary of each alternate-version group before applying episode metadata and use the same title merge rules as direct episode refresh. Missing-metadata repair must include primary episodes whose title is empty or still equals the parent series title, even when an overview and provider identifiers already exist.
- After local metadata and the configured remote-provider sequence have had a chance to establish an episode title, an unlocked visible primary may use a non-placeholder `OriginalTitle` only when its name is still empty, path-derived, or equal to the parent series. Never let this fallback replace a localized remote/NFO title or update an alternate version.
- Apply post-provider `OriginalTitle` episode repairs as one PostgreSQL set-based update scoped to the refreshed series or episode. Do not load every series descendant and issue per-episode updates for this repair.
- Compute Next Up from visible primary episodes, but aggregate played, resume, and activity state
  across every alternate version. Advance from the highest aired watched position, order series by
  their latest played date, and apply `NextUpDateCutoff` to that activity date rather than to the
  candidate episode's premiere date.
- When `DisplaySpecialsWithinSeasons` is enabled, include only placed season-zero specials in Next
  Up and compare them with the last watched and next regular episode using the official aired-episode
  comparer. Apply played/rewatch semantics before final count and pagination; keep ordinary season-zero
  ordering by `SortName` on episode-list routes.
- Metadata providers must have deterministic priority and merge behavior. Network calls need timeouts, bounded concurrency, and useful error context.
- Lazy `.strm` probing must have a process-level deadline that terminates FFprobe before returning; an async timeout around an uncancelled blocking child is not sufficient because client retries can accumulate processes and memory.
- Coordinate lazy `.strm` probes by item and resolved target so concurrent playback requests share one bounded flight. Keep failure backoff state short-lived and hard-bounded so retries do not repeatedly pay the probe timeout or grow memory without limit.
- Recognize failed-probe placeholder streams semantically across nullable boolean persistence shapes, and inspect only embedded streams when deciding whether to retry. An external subtitle must not suppress a later successful media probe.
- During episode refresh, merge a neighboring local NFO before remote metadata: preserve a non-empty established local title, but continue to treat a local title equal to the series or path-derived name as a replaceable placeholder; allow the first remote result to replace only such placeholders, and honor `LockedFields.Name` even for a full refresh.
- If TMDb returns an episode name equal to its series name in the preferred language, fetch the English episode metadata once and use only its non-placeholder name as a fallback. Preserve all localized non-name fields, and apply the same rule to direct episode refresh and bulk season refresh.
- Cancellation of scans and refreshes must promptly stop new work, release locks/permits, and leave the database in a consistent state.

## Validation

Run the narrowest relevant checks while iterating, then broaden validation before committing:

```bash
cargo fmt --all -- --check
cargo test -p <affected-crate>
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Some `jellyfin-data` integration tests require PostgreSQL and create temporary databases whose names begin with `jellyfin_`. Do not point those tests at a database containing user data.

For scan-memory work, include a repeatable large-directory or synthetic-library measurement when possible. Report baseline, peak, 60-second, and 300-second post-scan values. Separate process RSS and anonymous memory (`RssAnon` or `smaps_rollup` Anonymous) from cgroup `file` and `inactive_file`; metadata image page cache is reclaimable and must not be reported as a Rust heap leak. Also report whether memory returns after the scan, and do not infer a leak from allocator-retained RSS alone.

## Deployment verification

- The deployment checkout is `/home/lqs/jellyfin-rust` on the configured test host. Inspect its current state before changing it; do not assume the local workspace path is valid remotely.
- Deploy only committed revisions. Record the revision tested and verify health, relevant API behavior, scan completion/cancellation, and service logs.
- Do not delete databases, media, configuration, containers, or volumes during deployment validation unless the user explicitly requests it.

## Commit style

- One coherent, tested change per commit.
- Use concise imperative subjects with a conventional prefix when appropriate, for example `fix: bound library scan buffering` or `perf: batch item upserts`.
- Do not include generated build output, credentials, local logs, or deployment-only files.
