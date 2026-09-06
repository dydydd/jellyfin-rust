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
- Project `ImageBlurHashes` only from persisted image metadata in the same batched DTO-image lookup
  that produces the exposed image tags. Keep the top-level map present when empty, include hashes
  for inherited and Series primary tags, and never decode images or issue per-item lookups to fill it.
- Project `Chapters` only when `ItemFields.Chapters` is requested, while default all-fields item
  details must include an empty array when none exist. Load page chapters in one PostgreSQL batch,
  order by `StartPositionTicks`, keep alternate versions isolated, and derive chapter image tags
  from the owning item's media path without applying DTO image enablement, selectors, or limits.
  Resolve Chapter image GET/HEAD by `(item_id, ChapterIndex)` from the chapter repository; never
  store or enumerate chapter thumbnails as ordinary `base_item_images`, and serve their source
  bytes without decoding or resizing.
- Match official TV hierarchy image inheritance from relational Series/Season links: Episode and
  Season DTOs always derive `SeriesPrimaryImageTag`; Episode parent Primary prefers Season then
  Series; parent Logo prefers the nearest parent, parent Thumb prefers Series over Season, and
  parent Backdrop uses the nearest available parent. Local images suppress the corresponding
  inherited field, and Series itself must not inherit parent images.
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
- Coordinate remote-image downloads by URL so concurrent items share one bounded download, and cap leader downloads across distinct URLs at four so a media wall cannot multiply the per-image buffer without bound. Acquire the global permit inside the single-flight initializer so same-URL followers consume no additional permits and cancellation promptly releases capacity. Validate that upstream content is an image, and remove or otherwise suppress permanently invalid remote references according to official behavior.
- Persist uploaded and remotely downloaded lyrics under the item's internal metadata directory with a same-directory temporary file and atomic rename, then register the file as a Lyric media stream. Keep the parsed JSON only as a compatibility cache; reads prefer the registered stream, and deletion must never remove unregistered files, symlinks, or files outside the internal metadata root.
- Decode uploaded and local lyrics with the official BOM-aware UTF-8, UTF-16LE, UTF-16BE, UTF-32LE,
  and UTF-32BE behavior. Without a BOM, use UTF-8 replacement fallback; malformed or incomplete
  UTF-16/UTF-32 must produce replacement characters rather than fail. Parse the decoded text while
  preserving uploaded file bytes exactly as received.
- Read registered lyric streams in stream-index order and select parsers from each file path's actual
  extension, not its persisted codec. Continue to later streams when no parser accepts one, while
  preserving filesystem read failures instead of hiding them behind a fallback lyric.
- Project `HasLyrics` only for Audio items and derive it from persisted Lyric media-stream
  existence. Use one set-based PostgreSQL query for item and playlist pages; stale JSON lyric caches
  must not produce a true value, and non-Audio DTOs must omit the property even if they own a Lyric
  stream.
- Project `HasSubtitles` only for Video items with at least one persisted Subtitle media stream.
  Omit false and non-Video values, ignore stale item JSON, and use one set-based existence query for
  item and playlist pages instead of maintaining a second scan-time boolean.
- Project intros, local trailers, special features, and video additional parts with the official
  default all-fields DTO options through one batched projector. Apply the target user's policy to
  both the requested owner and every resolved child before returning the original response shape.
- When page DTOs request media sources, batch Audio and AudioBook stream and attachment loading
  alongside expanded video versions. `MediaSources` alone nests the streams, `MediaStreams` alone
  projects them at the top level, and requesting both exposes the item's streams in both locations.
- For lyric uploads, resolve the authorized Audio item before validating the body or filename so
  missing, hidden, and non-Audio targets retain the official 404 precedence over malformed uploads.
  Parse and persist through the same service operation without loading the item twice.
- Build remote lyric searches from the policy-authorized Audio item and pass its original path as
  `LyricSearchRequest.MediaPath` together with the official name, album, artist, album-artist, and
  duration fields so path-aware providers receive the same request as the official server.
- Keep lyric-provider search asynchronous and error-aware. Search all enabled providers with at
  most four in flight, refill completed slots without waiting for earlier providers, and still
  flatten results in configured provider order. Bound each provider by a 30-second deadline,
  isolate provider errors and timeouts as empty results, and propagate caller cancellation by
  dropping in-flight and queued futures rather than spawning detached tasks.
- Model remote lyric payloads as an explicit provider format plus the original bytes. Decode those
  bytes only for DTO parsing; provider preview must not persist them, while item-scoped download
  must save them without BOM or character-encoding conversion. Keep downloads asynchronous with a
  30-second provider deadline and cancellation-by-drop: unknown, empty, or unparseable responses
  are 404, while provider failures and timeouts remain server errors instead of false not-found.
- Keep remote lyric metadata on the official strongly typed `LyricMetadata` wire contract. Omit
  absent nullable fields and never let arbitrary provider JSON make the enclosing Swift SDK search
  result undecodable.
- Derive remote lyric provider ids from the invariant-lowercase provider name using UTF-16LE MD5,
  then format the digest with `.NET Guid(byte[])` byte ordering and the lowercase, hyphenless `N`
  format. Keep provider-id lookup case-sensitive like the official ordinal comparison, split a
  provider-owned lyric id only at its first underscore, and pass the whole id through when that
  separator is absent.
- Project configured lyric providers in execution order as the music library's `LyricFetchers`
  available options, case-insensitively deduplicate provider names, and mark every returned option
  enabled by default. Do not expose lyric fetchers for representative types that omit Audio.
- Check whether provider artwork exists with a PostgreSQL image-type query. Do not route existence checks through DTO image projection, local dimension inspection, or BlurHash generation.
- Keep the external-URL provider registry limited to the providers present in the checked-out
  official server and preserve its provider-name ordering. Interpolate persisted provider ids
  verbatim, ignore only empty values, and do not substitute collection or legacy TV provider ids
  where the official provider does not.
- Project `ExternalUrls` only for `ItemFields.ExternalUrls`, with an empty array when requested but
  no provider matches. Resolve Season and Episode Series/Season context from real relational rows
  in one batched page lookup, and keep provider names, URL strings, and provider order SDK-safe.
- Project only the current item's persisted `RemoteTrailers` when `ItemFields.RemoteTrailers` is
  requested. Preserve stored `NamedURL`/legacy-string order, emit an empty array when requested but
  absent, and include it through the default all-fields single-item contract without inheriting or
  merging trailers from parents or alternate versions.
- Map TMDb remote trailers with official semantics: accept only YouTube Trailer and Teaser videos;
  for Movies and Series place Trailer entries before Teaser entries with stable source order and
  preserve names, while Episodes keep source order, deduplicate URLs case-insensitively like
  `AddTrailerUrl`, leave trailer names absent, and construct the URL even when TMDb omits the key.
- Treat passwords, access tokens, API keys, and deployment credentials as secrets. Do not log or commit them.
- Do not decode, resize, reformat, decorate, or otherwise transform images requested by API
  clients. Keep accepting the official image query surface for compatibility, but stream the
  original image bytes and content type so media-library browsing cannot create decoder-sized
  memory spikes or a family of derived cache files. Preserve the official MIME types for every
  accepted image extension, including APNG, AVIF, ICO (`image/x-icon`), TIFF, and Jellyfin TBN
  JPEG files, without inspecting or decoding their contents. Image-info endpoints must return
  persisted dimensions and BlurHash values without lazily decoding the source or writing metadata.
- Keep image-route static segments and compound query names compatible with ASP.NET's
  case-insensitive binding, including representative all-lowercase legacy requests. Ordinary item,
  user, branding, by-name, and plugin image responses must not advertise byte ranges unless the
  handler actually implements Range semantics; trickplay tile routes remain the range-aware
  exception.
- Bind image `ImageType` and `ImageFormat` parameters from case-insensitive official names or their
  defined integer values. Reject unknown names and integer values as bad requests before resource
  lookup, including legacy user-image route parameters whose controller action otherwise ignores
  the value.
- Stream trickplay tile files with bounded chunks and preserve HEAD and byte-range semantics; never
  read an entire tile into a response buffer.
- Project `CanDelete` only when requested, except on official default all-fields item and root
  details. For user-less pages expose only the item's intrinsic capability; for user pages combine
  it with the target user's global or CollectionFolder-scoped deletion policy in one batched
  hierarchy lookup. Preserve the official single-item Playlist owner/administrator override and
  the BoxSet collection-management authorization, while keeping batched Playlist DTOs on the normal
  intrinsic-plus-policy wrapper.

## Compatibility expectations

- Match official Jellyfin DTO field names, nullability, defaults, HTTP status codes, authorization requirements, sorting, pagination, and case-insensitive matching.
- ASP.NET route, query-name, and JSON-property binding is case-insensitive. Compatibility tests must cover PascalCase, camelCase, and representative lowercase legacy requests; do not assume an Axum route or Serde field is equivalent merely because one casing works.
- Keep both the modern `/Items/Suggestions` route and legacy `/Users/{userId}/Suggestions`
  route reachable through fully lowercase aliases, with equivalent authorization and filtered
  results.
- Keep both Suggestions routes on the official signed `Int32` pagination contract: a negative
  `StartIndex` skips nothing but is echoed, `Limit=0` is empty, a negative `Limit` follows the
  official SQLite unlimited-limit behavior, and out-of-range values fail binding. Preserve the
  endpoint's default disabled-total behavior, which reports the returned page length.
- Bind Suggestions `MediaType` and `Type` as the official enum collections: accept case-insensitive
  names and defined integer values, discard invalid elements, split commas only for one query value,
  and do not re-split comma-containing values when the SDK sends repeated keys.
- Treat an omitted or empty Suggestions `UserId` as an official user-less global query: do not
  apply the authenticated user's library root, policy, user data, or stream preferences. Authorize
  an explicit non-empty id before its nullable lookup, so a normal user's unknown foreign id is
  forbidden while an administrator's unknown id falls back to the same global query. Only group
  presentation keys when that lookup resolves a user, matching `InternalItemsQuery(User?)`.
  Explicitly enable all folders for the user-less query so the default policy struct cannot hide
  media nested below a `CollectionFolder`.
- Project Suggestions with the official default all-fields `DtoOptions`, including every alternate
  media source and its streams, per-source bitrate/container/size, top-level streams, source count,
  trickplay, and image metadata. Keep the page projection user context optional: a user-less global
  response must not load UserData, remembered stream selections, language preferences, or
  user-policy-aware child aggregates.
- Keep `/Years` pagination on the official signed 32-bit contract. A negative `StartIndex` skips
  nothing but is preserved in the response, a non-positive `Limit` returns an empty page, values
  outside `Int32` fail binding, and `TotalRecordCount` is computed before endpoint pagination.
- Keep `/Persons` pagination signed as well, but preserve its different limit rule: a non-positive
  `Limit` is unlimited, while a non-positive `StartIndex` skips nothing and is still echoed.
- Keep item-by-name pagination such as `/Genres`, `/MusicGenres`, and `/Studios` signed: a negative
  `StartIndex` skips nothing but is echoed, `Limit=0` is empty, and a negative `Limit` follows the
  official SQLite unlimited-limit behavior. When `EnableTotalRecordCount` is false, return zero
  rather than the current page length.
- Filter `/Persons` through the media items visible to the target user: a person remains visible
  when at least one credited item passes enabled/blocked folder, allowed/blocked tag, parental-
  rating, and unrated-item policy. Keep this set-based, and do not apply the related-media filter
  to the single person-by-name route.
- When a catch-all implements several official HLS or trickplay route templates, keep concrete
  official-path dispatch tests and representative lowercase aliases so Axum does not regress the
  case-insensitive ASP.NET route contract. Lowercase compatibility must include every static path
  segment, including compound segments such as `ActiveEncodings`.
- Follow the official `JsonDefaults` value semantics. Where it permits them, accept numeric strings and case-insensitive or integer enum representations, and mirror the full official parameter set when implementing a legacy endpoint.
- Bind the eight official virtual-folder `CollectionTypeOptions` values case-insensitively and persist/project their canonical lowercase wire names. Keep `mixed` valid for virtual-folder management but omit it from `BaseItemDto.CollectionType`, and tolerate legacy mixed-case persisted view metadata.
- Resolve direct Genre and MusicGenre detail names through their official deterministic item-by-name
  path and UTF-16LE identifier, creating the persisted entity idempotently. Hyphenated slug names
  only search persisted entities in `&`, `/`, then `?` substitution order; a miss returns an empty
  Genre DTO but a MusicGenre 404. These detail routes bind only `UserId`, and an administrator's
  nonexistent target user still receives the item without user data.
- Upgrade databases that have genre item values but no item-by-name rows on the first authorized
  Genre, MusicGenre, or Filters2 list. Share one process-local single-flight across both kinds,
  keyset-page required values, create official metadata directories before persistence, and batch
  inserts; do not require a full library scan or repeat the reconciliation after success.
- Resolve every Studio detail name directly, including names containing hyphens, and every positive
  Year through its official persisted item-by-name path and UTF-16LE identifier. Their detail routes
  bind only `UserId`, use default all-fields DTO projection, and let an administrator target a
  nonexistent user without attaching user data. Backfill legacy Studio values and production years
  in bounded batches, and ensure each paged Year result has a persisted entity before returning it.
- Persist newly discovered Person items at the official `metadata/People/<first alphanumeric>/<name>`
  path with the deterministic `MediaBrowser.Controller.Entities.Person` UTF-16LE identifier. Keep
  `people.id` as the internal credit key, make canonical Person items non-folder and non-virtual,
  fill missing provider ids without replacing established metadata, and reuse legacy image files
  only by copying database references; never move or delete the source image or legacy row.
- Reconcile referenced Person names in fixed 128-item `(clean_name, id)` keyset pages. Prepare
  directories before each PostgreSQL batch, then atomically create canonical rows, fill only missing
  metadata, merge ProviderIds without replacing case-insensitive existing keys, and copy missing
  image database references from the best locked/metadata/image/newest legacy candidate. After a
  non-cancelled RefreshPeople reconciliation, verify every page against its exact configured
  deterministic id in one batched read and fail the task if any canonical row is missing; this
  verifier must not create directories or fall back to name or clean-name matching. Cancellation
  must stop both operations before their next page; never load the full people catalog, issue
  per-credit queries, refresh remote metadata, or delete people, legacy Person rows, image rows, or
  image files during this reconciliation phase.
- Expose only the exact deterministic canonical `base_items.id` for people through `/Persons`,
  `BaseItemDto.People`, and person `SearchHint` results; `people.id` remains an internal credit
  foreign key and must never escape on those API surfaces. Resolve page credits and their Primary
  image tags in batches, and keep Person detail, image, favorite, and user-data operations centered
  on the canonical item without name or clean-name fallback. Translate `Items?PersonIds=` from the
  supplied public BaseItem ids through exact persisted names to internal people ids in one set-based
  query, preserving `PersonTypes`; resolve favorite Person rows to exact canonical ids before count,
  ordering, and pagination so same-name legacy rows cannot change results.
- Treat generated SDK models as executable compatibility specifications alongside the C# DTOs. Swift `Codable` rejects the entire enclosing item or page when one nested object, enum, dictionary value, or date has the wrong wire shape.
- Hydrate every persisted base item through the shared item-type registry before DTO projection,
  including playlist entries, so legacy CLR names never escape through `BaseItemDto.Type` and an
  unknown plugin row cannot make a client reject the enclosing page.
- Keep library-creation `CollectionTypeOptions` distinct from `BaseItemDto.CollectionType`: `mixed`
  is valid for a virtual-folder configuration but must be omitted from user-view item DTOs because
  the client DTO enum cannot decode it.
- Project the official single-item detail routes with their default all-fields `DtoOptions`: clients must receive media sources, nested and top-level media streams, and trickplay without supplying a non-official `Fields` query.
- Keep the modern and legacy single-item detail routes reachable through fully lowercase aliases,
  and bind `UserId` case-insensitively. Treat a nil user id as omitted, authorize a regular user's
  foreign target before looking it up, and allow an API key with an explicit valid target user while
  still applying that target user's normal library policy.
- Project Episode `SeriesName` and `SeasonName` from the linked Series and Season rows when legacy
  items lack the denormalized JSON fields. Resolve parent names in one bounded batch for item pages
  and show episode pages; do not add a parent lookup per episode.
- Project persisted `SeriesName` and `SeasonName` on `BaseItemDto`; these are unconditional
  episode/season identity fields in official item details and lists, not optional `Fields` values.
- Project persisted `OriginalLanguage` unconditionally on item details and lists. When expanding
  alternate versions, use each source item's own original language for its stream defaults and
  keep an exact alternate-id detail tied to that alternate rather than the displayed primary.
- Keep the BaseItemDto Settings field group gated by `ItemFields.Settings`: emit only canonical,
  SDK-safe `LockedFields`, default empty locks and false `LockData`, and omit all five settings
  properties from ordinary item pages unless requested. Single-item details use the official
  default all-fields DTO options and therefore include the Settings group.
- Project music `Album`, `AlbumId`, `Artists`, `ArtistItems`, `AlbumArtist`, and `AlbumArtists`
  unconditionally on item details and lists. Resolve audio albums through one batched nearest-
  ancestor lookup and preserve metadata artist order while attaching normalized relation ids.
- Bind legacy Artists `Filters` by case-insensitive name or integer and reject the three official
  conflicting pairs. Apply favorite, liked, and played state to the target user's matching
  item-by-name `MusicArtist` rows; preserve the official no-op behavior for folder and resumable
  filters and its favorite-only `IsFavoriteOrLikes` behavior.
- Apply artist and album-artist metadata filters to the matching outer `MusicArtist` entity, not
  to media items that merely contribute the artist name. Resolve `GenreIds` and `StudioIds`
  through referenced base-item clean names, preserve the official pipe/comma delimiters and
  `Studios`-over-`StudioIds` precedence, and let official ratings match descendants and linked
  children in one set-based query.
- Keep accepting Artists and AlbumArtists `MinCommunityRating`, `Person`, `PersonIds`, and
  `PersonTypes` with official casing, binding, and validation semantics, but do not apply them to
  results: the official ByName repository currently drops all four when it constructs its inner
  and outer item-value queries.
- Resolve `GET /Artists/{name}` from an exact raw-name persisted `MusicArtist` first, preferring a
  physical artist over an accessed-by-name row; otherwise persist the official deterministic
  lowercase `artists` path fallback with `Artist-` presentation key and `IsFolder = false`.
  Project details with default all-fields DTO options, merge Artist and AlbumArtist links into
  distinct Audio, MusicAlbum, and MusicVideo counts, and omit `UserData` when an administrator
  targets a nonexistent user.
- Audit DTOs recursively: preserve object-array shapes, serialize API enums by their official names, keep string dictionaries string-valued, and emit full API `DateTime` values rather than storage-only dates.
- Treat alternate video versions as one playback group. Item details and `PlaybackInfo` must expose every version as a distinct `MediaSource`, honor `MediaSourceId` when opening static or transcoded content, and keep all stream and attachment loading batched by version identifiers.
- Resolve subtitle-stream, subtitle-playlist, and attachment route `MediaSourceId` values inside the requested item's authorized alternate-version group before reading runtime, streams, attachments, or files. Never serve a same-index stream from the displayed primary for an alternate source, and reject malformed or unrelated source ids with 404.
- Preserve scan-discovered local alternate versions in resolver input order. Batch assignment must ignore missing and self-referential pairs before applying first-valid-assignment wins, persist a contiguous per-primary `sort_order`, compact both sides when a child moves between primaries, and remain idempotent on repeated scans. Serialize competing reassignment transactions before taking row locks so opposing moves cannot deadlock, and report parents whose DTO-visible version order changed even when no child's `primary_version_id` changed.
- Apply the playback `DeviceProfile` independently to every returned `MediaSource`, preserving source order and producing version-specific flags and URLs. Only apply explicit audio or subtitle indexes to the source whose id matches an explicitly requested `MediaSourceId`.
- After applying a playback `DeviceProfile`, project the selected subtitle index back to that source's
  `DefaultSubtitleStreamIndex`, including `-1` when subtitles are disabled; never leak an explicit
  index to another version when no matching `MediaSourceId` was selected.
- Project each media source's persisted total bitrate, and when it is absent infer it from that
  source's non-external media streams as official Jellyfin does. Keep this per-version so item
  details and `PlaybackInfo` never reuse the displayed primary's bitrate for alternate versions.
- Project `SupportsExternalStream` on every persisted media stream using the official rule: true
  for external streams and for text, PGS, or VobSub subtitles. Keep the value consistent between
  top-level streams and every single- or alternate-version media source.
- Explicit metadata refresh must repair missing or placeholder stream rows for the selected local
  file through the bounded media-probe pool, including alternate versions. Keep remote and `.strm`
  sources on the lazy playback probe path instead of opening upstream media during library browsing.
- Project each source's persisted, probed container before considering its path extension. When a
  persisted container lists alternatives, select the path-matching value or the first value, and
  strip URL query/fragment components before any extension fallback.
- Preserve the displayed item's persisted raw `Container` on the top-level DTO, while normalizing
  each nested media source independently against that version's path. Do not derive the top-level
  value from a path or reuse another version's container.
- Project each video's actual `VideoType` on both the top-level item DTO and its media source.
  Resolve the official string or integer enum representation independently for every alternate
  version; never report disc or ISO versions as the displayed primary's `VideoFile` type.
- Project every audio and subtitle stream through the same language/localization path for item
  details and `PlaybackInfo`, including every alternate media source. Canonicalize recognized ISO
  639-2 bibliographic codes, preserve unrecognized codes, and let `DisplayTitle` fall back to that
  raw code when `LocalizedLanguage` is unavailable.
- Build `Items/Filters2` audio and subtitle language options with one target-user-policy-aware
  set query. Include alternate versions, add Episode to Series or Season stream searches, map
  missing language values to `und`, and sort the localized `NameValuePair` values by display name.
- Build every `Items/Filters` and `Items/Filters2` bucket from one shared target-user policy
  snapshot. Apply enabled and blocked folders, allowed and blocked tags, parental ratings, and
  blocked unrated kinds consistently to item values, years, ratings, and stream languages.
- Bind all six nullable `Items/Filters2` classifiers (`IsAiring`, `IsMovie`, `IsSports`, `IsKids`,
  `IsNews`, and `IsSeries`) case-insensitively and apply each one to both genre discovery and
  audio/subtitle language discovery. Reject malformed boolean values instead of ignoring them.
- Project count-only `MediaSourceCount` for item pages with one batched, target-user-policy-aware
  alternate-version query. Always count the displayed item, preserve the official nullable-single-
  source behavior, and include episode groups without loading every `MediaSource`.
- Filter alternate `MediaSources` and their full-source `MediaSourceCount` by the target user's
  standalone item visibility before loading streams or attachments. Always retain the explicitly
  displayed source, while user-less global projections retain every source.
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
  virtual-item, and favorite filters before grouping by item type. Evaluate favorite state on the
  visible primary only; user data on an alternate version must not change `/Items/Counts`. When an
  administrator requests a nonexistent target user, preserve the official nullable-user fallback
  and return global counts rather than a missing-user response. Explicitly enable all folders for
  that user-less query so media below real `CollectionFolder` roots remains in the aggregate.
- Project requested folder ChildCount values in one batch. Count episodes by SeasonId, prefer linked
  children, deduplicate merged folder children by PresentationUniqueKey, and honor the user's
  DisplayMissingEpisodes preference without issuing per-folder queries.
- Project requested `RecursiveItemCount` values with one batched, user-policy-aware leaf query.
  Traverse hierarchy and linked descendants, expand merged folder groups, exclude virtual leaves,
  alternate versions, and owned non-extra rows, and return zero entries without per-folder fallbacks.
- Order episode detail pages with the official aired-episode comparer before applying `StartItemId`, adjacency, or pagination. Specials with `AirsBeforeSeasonNumber`, `AirsAfterSeasonNumber`, or `AirsBeforeEpisodeNumber` must be positioned relative to regular episodes rather than compared with a single incompatible numeric key; season zero itself remains sorted by `SortName`.
- Bind `Shows/{SeriesId}/Episodes` pagination as signed 32-bit values. Preserve a negative
  `StartIndex` in the response while treating it as no skip, let a negative `Limit` return an empty
  page, and reject values outside the official `Int32` range.
- Bind the nullable Episodes `SortBy` through the official enum-converter semantics: accept
  case-insensitive names, signed `Int32` values, and comma-delimited values combined bitwise. Only
  a final value equal to `ItemSortBy.Random` randomizes; malformed input behaves as unset.
- Keep `Shows/NextUp` pagination on its distinct signed 32-bit contract. Preserve a negative
  `StartIndex` in the response while treating it as no skip, treat every non-positive `Limit` as
  unlimited, and reject values outside the official `Int32` range.
- Resolve `Shows/NextUp` target-user authorization before its Series filter. Treat an empty,
  unknown, or non-Series `SeriesId` as absent and fall back to `ParentId` or the user's root; a
  valid Series wins over `ParentId` and scopes episodes by its `PresentationUniqueKey`. Keep that
  candidate set subject to the target user's tag, rating, parental, and folder policy.
- `Items/Latest` defaults `GroupItems` to true. For TV, select the top Series groups from the
  complete policy-filtered Episode set before applying the result limit, then analyze each Series'
  inclusive 24-hour window in PostgreSQL. Return Series for cross-season additions; for one Season
  containing multiple recent Episodes or the complete Season, return Season when the Series has
  multiple Seasons and Series otherwise. Count only visible primary, non-virtual Episodes, expose
  the recent-child count, and fall back to the newest Episode when the selected container is hidden.
  Keep candidate buffers bounded and load final containers and fallback Episodes in batches.
- When `Items/Latest` omits `IncludeItemTypes`, derive MediaTypes from the target user's visible
  collection folders: books use Book and Audio, music uses Audio, photos and home videos use Photo
  and Video, and other collection types use Video. Apply `LatestItemsExcludes` only for the implicit
  root scope; explicit parents ignore those exclusions. A movies or tvshows UserView instead
  derives Movie or Episode respectively, and explicit `IncludeItemTypes` always wins.
- Resolve `Items/Latest` Audio and Photo grouping containers from the nearest matching
  `MusicAlbum` or `PhotoAlbum` ancestor by closure-table depth, not only the direct parent. Load
  all resolved containers through one target-user-policy-aware batch; fall back to the media item
  when its container is not visible. A `MusicAlbum` replaces even one recent Audio item, while a
  `PhotoAlbum` replaces its Photos only when at least two recent items share it.
- Item-value `ItemCounts` inherit Genre and Studio links from a Series to its visible descendant
  Episodes, but do not inherit Artist or other value kinds. Count direct and inherited matches with
  set-based PostgreSQL queries and deduplicate Episodes that carry the same value directly. Keep
  internal item-by-name discovery type filters out of the count scope; only the client's explicit
  `ExcludeItemTypes` may remove a type from the returned count buckets. Derive `ChildCount` from the
  item-count fields supported by `BaseItemDto`; Book, BoxSet, folder, and plugin links may discover
  an item-by-name value but must not inflate its unrepresentable child total.
- Project exact Person DTO `ItemCounts` from the target user's policy-visible credited primary
  items. Keep alternate-only credits discoverable on `/Persons`, but do not let alternate versions
  inflate Person detail counts, and recognize canonical and legacy CLR item types in every bucket.
- Project exact Year DTO `ItemCounts` from the target user's policy-visible primary items for that
  production year. Emit every supported numeric bucket and `ChildCount`, recognize canonical and
  legacy CLR item types, and do not let alternate versions inflate the totals.
- Filter MediaSegments by registered provider ids and the owning virtual library's
  `DisabledMediaSegmentProviders`, matching provider names case-insensitively. Derive provider ids
  from the invariant-lowercase name with UTF-16LE MD5 and `.NET Guid(byte[])` ordering; when no
  provider is registered or enabled, return an empty result instead of exposing stale persisted rows.
- Reconcile persisted `Genre`, `MusicGenre`, and `Studio` item-by-name entities after full and
  single-library scans. Create their official metadata paths before insertion, derive IDs with the configured
  official UTF-16LE/.NET `Guid(byte[])` semantics, process deterministic keyset pages in bounded
  PostgreSQL batches, and never replace or delete existing entities, images, or metadata.
- Return persisted `Genre` and `MusicGenre` `BaseItem` identifiers from their list routes and from
  Filters2 rather than exposing normalized item-value identifiers. Collapse equal persisted
  `PresentationUniqueKey` groups to their smallest UUID before total counting, ordering, and
  pagination, and keep the media-value discovery and count aggregation set-based.
- Persist and round-trip both `EnableNormalizedItemByNameIds` and `EnableCaseSensitiveItemIds`
  (default true). Lowercase item-by-name ID keys when normalization is forced or case-sensitive IDs
  are disabled; only preserve path casing when normalization is disabled and case sensitivity is enabled.
- `IncludeItemTypes` and `ExcludeItemTypes` filters, count queries, and media-source queries must
  recognize canonical short item types and official legacy CLR-qualified persisted names, including
  case-insensitive API enum input. Preserve unknown plugin-defined types instead of discarding them.
  Continue folding alternate rows before `/Items/Counts` buckets are calculated so legacy versions
  do not inflate either a typed bucket or `ItemCount`.
- After scanning a movie directory, supplement the official filename-based version resolver with a conservative metadata match: same directory, non-empty case-insensitive title, and the same non-empty year, while rejecting the whole candidate group when TMDb, IMDb, or TVDb identifiers conflict. Never use collection identifiers as movie identity.
- Do not advertise a playback method unless the returned URL really implements it. In particular, never label unchanged container bytes as an MP4 direct stream; derive `SupportsDirectPlay`, `SupportsDirectStream`, and `SupportsTranscoding` from the final selected method and device policy.
- Normalize playback start and progress reports against the active transcoding-job registry. Treat
  an omitted method as `Transcode`, downgrade it to `DirectPlay` when `PlaySessionId` is blank or
  unknown, and preserve `Transcode` only while that playback session has a registered job.
- Build a playback session's `NowPlayingItem` from the displayed item and the authorized selected
  `MediaSourceId`: project the selected version's runtime and localized media streams, then reuse
  that same-item snapshot on later progress reports that omit `Item` instead of replacing it with
  a shallow DTO.
- Validate modern playback callback bodies before changing session state: `Item` must be an object
  or null, and every `NowPlayingQueue` entry must be an official `QueueItem` with a valid Guid `Id`.
  Re-serialize queue entries with canonical PascalCase fields and compact Guid values.
- Apply static media-source capability flags from the target user's policy, including when an
  administrator requests another user's item or `PlaybackInfo`: audio transcoding controls audio
  sources, while video transcoding and playback remuxing independently control video sources.
- Derive intrinsic `CanDelete` from the official item-type overrides. Most items require a non-empty
  local File-protocol path; root and metadata-projection folders remain false, a physical
  MusicArtist is true independently of its path, and an accessed-by-name MusicArtist is false.
  Do not reject an otherwise local item merely because it is virtual, and keep Playlist owner/admin
  authorization separate from the user-less intrinsic capability.
- Keep single and batch item deletion on the official authorization and binding contract. API keys
  are unrestricted and user-less, ordinary users resolve each item through their visibility policy,
  and an item-level deletion denial is 401 while a hidden item is 404. Bind optional `Ids`
  case-insensitively with the official comma-or-repeated-value rules, discard malformed values, and
  execute the surviving identifiers sequentially in request order without rolling back earlier
  deletions when a later identifier fails. A surviving empty GUID is a bad request, not the user
  root. Preserve fully lowercase route aliases.
- Project `CanDownload` and `PlayAccess` only when their `ItemFields` are requested (single-item
  details request all fields by default). Compute intrinsic download capability from the official
  item-type overrides, then apply the target user's download and playback policy once per page;
  user-less API-key projections keep intrinsic `CanDownload` and omit `PlayAccess`. Downloading a
  `.strm` item returns its local sidecar path, never the resolved remote target.
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
- Version successful local Audio/Video probes in `base_items.data` with a source fingerprint containing the normalized path, file size, and modification time. A missing, stale, malformed, or mismatched marker must trigger one bounded repair even when historical embedded streams already exist; write the marker only after all probed media information is persisted, and never replace existing streams, attachments, chapters, or item metadata when that schema-only probe fails. Keep failed local probes on a hard-bounded in-process backoff, while `.strm` and remote sources remain lazy playback probes.
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
