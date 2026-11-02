# Jellyfin Rust Engineering Guide

This document defines the rules for contributing to this repository. Apply it
to every change unless the user explicitly overrides a rule.

## 1. Mission and Sources of Truth

- This repository is a Rust reimplementation of the Jellyfin server. The primary
  goal is behavioral compatibility with the official server, web client, and
  generated Android and Swift SDKs.
- Compatibility takes precedence over local design preferences. Match
  externally visible behavior, including defaults, validation, authorization,
  response shapes, status codes, error precedence, ordering, and pagination.
- The checked-out official server in `jellyfin/` is the behavioral reference.
  Read the relevant C# controller, service, DTO, and query implementation before
  changing compatibility-sensitive behavior.
- Generated Android/Kotlin and Swift models are executable wire contracts. A
  response that either SDK cannot decode is incorrect even when it appears
  close to the C# DTO definition.
- Do not invent behavior absent from the official server. If Rust behavior
  cannot be derived from the official source, document the assumption in a
  focused test or comment rather than silently choosing new semantics.
- The legacy Emby compatibility API is not part of this codebase. Do not add
  `/emby` routes, Emby-specific DTO adaptation, or a separate Emby API crate
  unless a future task explicitly requests it.
- Current priorities are media-library scanning and management, users and
  policies, metadata providers, playback-aware queries, and PostgreSQL-backed
  data access.
- Live TV is out of scope unless a task explicitly requests it. Avoid incidental
  changes under `src/jellyfin-live-tv`.

## 2. Repository Structure

This is a Rust 2024 Cargo workspace. Preserve these responsibilities:

| Crate or area | Responsibility |
| --- | --- |
| `jellyfin-api` | Axum routes, extractors, HTTP status mapping, OpenAPI |
| `jellyfin-controller` | Domain services, authorization orchestration, scans, refreshes |
| `jellyfin-data` | SeaORM repositories, entities, PostgreSQL queries |
| `jellyfin-migration` | Forward-only schema migrations |
| `jellyfin-model` | Public DTOs, enums, query models, cross-layer value types |
| `jellyfin-providers` | Metadata provider logic and merge primitives |
| `jellyfin-naming` | Filename and media hierarchy parsing |
| `jellyfin-media-encoding*` | ffprobe, encoding, HLS, and keyframe support |
| `jellyfin-networking` | HTTP clients, proxies, and network policy |
| `jellyfin-xbmc-metadata` | NFO and local metadata parsing/writing |
| `jellyfin-server` | Startup and dependency wiring |
| `jellyfin-server-implementations` | Authentication, sessions, and shared implementations |

Rules:

- Keep HTTP concerns in `jellyfin-api`. Do not put SQL, scans, or provider
  workflows directly in route handlers.
- Keep domain rules explicit in Rust even when PostgreSQL also enforces the same
  invariant.
- Do not make lower-level crates depend on the server binary or Axum handlers.
- Update `src/jellyfin-api/src/openapi.rs` and its tests when a public operation
  or schema changes.
- Add a new migration instead of editing an applied migration.
- Use workspace dependencies unless a local dependency is necessary.
- Do not change workspace-wide formatting, lint settings, or dependency versions
  as part of an unrelated fix.

## 3. Engineering Ground Rules

- Read both the Rust implementation and the corresponding official C# behavior
  before editing.
- Preserve unrelated user changes and existing commits. Never discard work you
  did not create.
- Never use destructive Git operations or rewrite history unless the user
  explicitly requests that exact operation.
- Treat passwords, access tokens, API keys, cookies, signed URLs, and deployment
  credentials as secrets. Never log or commit them.
- Prefer streaming, pagination, keyset scans, set-based queries, bounded
  batches, and short-lived buffers.
- Avoid N+1 database queries. Do not issue per-item filesystem or network work
  from list and detail routes.
- Every background task, network call, and external process must have an owner,
  a timeout or cancellation path, and a bounded concurrency limit.
- Do not accept a request option and silently ignore it. Either implement the
  official behavior or reject malformed values at binding time.
- Do not advertise a feature, media source, URL, byte range, or provider unless
  the code path actually supports it.
- Cancellation must stop new work, terminate child processes, release permits
  and locks, and leave persistent state consistent.
- `unsafe` code is forbidden by the workspace lint configuration.

## 4. Change Workflow

1. Locate the exact official behavior and the current Rust implementation.
2. Define the observable contract: route, query binding, body, authorization,
   response, status, ordering, and error behavior.
3. Add or update focused regression tests.
4. Implement the smallest coherent change.
5. Accumulate related fixes and validate the batch with the remote host's
   existing cached builder. Avoid a full rebuild after every small edit.
6. Review the complete diff for unrelated changes, secrets, generated files, and
   compatibility drift.
7. Commit each independently reviewable fix separately.

When coordinating multiple agents:

- The coordinating agent owns integration, validation, deployment, and final
  review.
- Assign disjoint files or clearly separated concerns to implementation agents.
- Do not let multiple agents change the same compatibility contract unless one
  agent is explicitly the owner.

## 5. API Compatibility

### 5.1 General wire contract

- Preserve official DTO property names, casing, omission versus `null`, empty
  arrays versus absent fields, enum wire values, GUID formatting, dates, numeric
  ranges, and dictionary value types.
- Default all-fields item details must include the official field groups,
  including media sources, nested and top-level media streams, trickplay data,
  and other fields the official controller adds by default.
- Gate optional `BaseItemDto` groups by their official `ItemFields`. Do not make
  clients send a non-official `Fields` query to receive default data.
- Serialize API enums by official names unless the contract explicitly uses
  integers. Never emit a local-only enum member on a public response.
- Keep object-array shapes, string dictionaries, and ISO `DateTime` values
  consistent with the generated SDKs.
- Preserve official empty-success behavior. Jellyfin mutations that return
  `204 No Content` must not become `200` without an explicit contract change.
- Do not buffer a complete response merely to adapt a few fields. Use shared
  batched projection.

### 5.2 Routes and request binding

- ASP.NET route matching, query-name binding, and JSON-property binding are
  case-insensitive. Axum and Serde are not automatically equivalent.
- Register canonical routes and the fully lowercase aliases required by Android
  and Swift. Every static path segment must work in lowercase, including
  compound segments.
- Do not normalize dynamic path values or query strings while adding lowercase
  aliases.
- Bind mutations and query fields case-insensitively with
  last-duplicate-wins semantics where ASP.NET does so.
- Follow official JSON value conversion. Accept numeric strings and integer or
  case-insensitive enum representations only where the official model binder
  accepts them.
- Tests must cover representative PascalCase, camelCase, and lowercase requests.
  One passing casing does not prove compatibility.
- Register concrete routes before dynamic fallbacks. Test route precedence in
  both official and lowercase spellings.
- Keep duplicate route definitions out of the router. A fallback may implement
  several templates only when each concrete dispatch path has a test.

### 5.3 Collection query values

- Match official repeated-key and comma-delimited behavior.
- Split commas only at the stage where the official binder does so. Do not
  re-split an element that came from a repeated key.
- Parse and validate the complete collection before mutating persistent state.
- Discard invalid enum elements only where the official enum collection
  converter does so; otherwise return the official binding error.
- Reject an empty required identifier collection when the official route
  requires at least one item.
- Deduplicate identifiers where required, but preserve the official operation
  order when applying mutations.

### 5.4 Pagination contracts

All paging values are signed `Int32`; out-of-range values fail query binding.
The meaning of negative and zero values differs by route family:

| Route family | `StartIndex` | `Limit` | Additional rule |
| --- | --- | --- | --- |
| `/Items`, Resume, Suggestions, Upcoming | Negative skips nothing and is echoed | `0` empty; negative unlimited | Suggestions defaults to returned-page count when totals are disabled |
| `/Years` | Negative skips nothing and is echoed | Non-positive empty | Total is computed before paging |
| `/Persons` | Non-positive skips nothing and is echoed | Non-positive unlimited | |
| `/Genres`, `/MusicGenres`, `/Studios`, `/Artists`, `/Artists/AlbumArtists` | Negative skips nothing and is echoed | `0` empty; negative unlimited | Disabled total count returns zero |
| `/Playlists/{id}/Items`, `/Items/{id}/Collections` | Negative skips nothing and is echoed | Non-positive empty | Official controller `Take` semantics |
| `/Items/{id}/RemoteImages` | Negative skips nothing | Non-positive empty | Reject range errors before lookup |
| `/Search/Hints` | Negative skips nothing | Non-positive empty with zero total | Apply limit before candidate counting |
| `/Shows/{id}/Episodes` | Negative skips nothing and is echoed | Negative empty | Apply aired-episode order before paging |
| `/Shows/NextUp` | Negative skips nothing and is echoed | Non-positive unlimited | Authorize user before Series filter |

Add a focused test whenever a route has a special paging rule. Cover the echoed
`StartIndex`, zero and negative behavior, total count, and out-of-range binding.

### 5.5 Authorization and error precedence

- Enforce the same policy on every casing of a route. Lowercase aliases must not
  bypass the canonical handler's authorization.
- A valid API key is user-less and unrestricted where the official route allows
  it. An explicit target user still applies that user's library policy.
- Device sessions are bounded by their user's policy.
- Distinguish a hidden or unknown resource from an authorization failure
  according to the official controller. Do not collapse both into one status.
- Resolve and authorize the target user or item before parsing body fields or
  item identifiers when official resource errors take precedence.
- Validate bodies before mutating state. Reject malformed JSON, missing required
  fields, and invalid nullable values.
- Do not turn an omitted target into the current user unless the official route
  does so.
- Preserve password-change session revocation and password-reset behavior.
- Do not expose a user-less policy bypass. When official behavior uses a global
  query, explicitly enable all folders rather than leaving a default policy that
  hides nested media.

### 5.6 Mobile and legacy route aliases

- Keep the mobile browse bootstrap surface reachable through lowercase static
  segments: public/system info, branding configuration, users and views,
  devices, display preferences, sessions, latest/count/resume, and library
  available-options routes.
- Keep `/users/public` ahead of the dynamic `/users/{id}` route. Otherwise the
  dynamic route attempts a UUID lookup for the literal `public` segment.
- Keep `/users/authenticatebyname` on the same public authorization policy and
  handler as `/Users/AuthenticateByName`.
- Keep lowercase aliases for item details, root/counts, Suggestions, themes,
  collections, intros/special features, show pages, InstantMix, Search Hints,
  trailers, additional parts, and user-data mutations.
- Keep playback-info GET and POST aliases such as
  `/items/{itemId}/playbackinfo` on the same policy and handler.
- Keep non-Live-TV Channels routes case-insensitive. Bind `StartIndex`,
  `FolderId`, `ChannelIds`, sort, fields, and capability options through the
  shared DTO projector.
- Do not let a generic lowercase fallback capture a concrete route with different
  authorization. Dispatch tests must prove every concrete alias reaches the
  intended handler.

## 6. Item Queries and DTO Projection

### 6.1 Policy and visibility

- Apply the target user's enabled/blocked folders, allowed/blocked tags,
  parental-rating rules, blocked unrated kinds, and virtual-item rules to every
  list, count, facet, recommendation, similarity, and Next Up query.
- Resolve an explicit `UserId` before its nullable lookup. Authorize a regular
  user's foreign target; preserve the official administrator fallback for a
  nonexistent user where applicable.
- User-less API-key projections must not load user data, remembered stream
  selections, language preferences, or policy-aware child aggregates.
- Filter alternate versions through the same item visibility policy before
  loading their streams or attachments.
- Keep item details, lists, and counts consistent about which primary represents
  a version group.
- Apply the related-media filter to `/Persons` list results. A person is visible
  when at least one credited item passes the target user's policy. Keep this
  set-based.

### 6.2 Batched projection

- Project page images in one batched lookup, including inherited Series and
  Season images required by official hierarchy rules.
- When image projection is enabled, emit `ImageTags` as an object even when
  empty. Emit `BackdropImageTags` as an array when that type is enabled, even
  when empty.
- Project `ImageBlurHashes` only from persisted metadata in the same batched
  image lookup. Never decode image bytes to calculate it.
- Batch parent Series/Season names, artist and album relations, external URLs,
  chapters, chapters' image tags, and requested counts.
- Project `HasLyrics` only for Audio items from persisted Lyric stream existence.
  Project `HasSubtitles` only for Video items with persisted Subtitle streams.
  Do not update these flags from stale JSON.
- Project `CanDelete`, `CanDownload`, and `PlayAccess` only when requested, except
  on official default all-fields item details. Compute intrinsic capability and
  apply user policy in the same bounded query path.
- Project `ExternalUrls` only for `ItemFields.ExternalUrls`; emit an empty array
  when requested but unmatched.
- Project `RemoteTrailers` only for `ItemFields.RemoteTrailers`, from the current
  item's persisted value, without inheriting from parents or alternate versions.
- Project intros, local trailers, special features, and additional parts through
  one batched projector using official default DTO options.
- Project `SeriesName`, `SeasonName`, and `OriginalLanguage` unconditionally from
  persisted item or relational parent data for item details and lists.
- Populate Series and Season `UserData.UnplayedItemCount` with one target-user-
  policy-aware recursive descendant query. Do not leave the field null merely
  because the DTO is a folder.
- Project music `Album`, `AlbumId`, `Artists`, `ArtistItems`, `AlbumArtist`, and
  `AlbumArtists` unconditionally on item details and lists, using one batched
  nearest-ancestor album lookup.
- Gate the BaseItem settings group by `ItemFields.Settings`. Emit canonical
  `LockedFields`, default empty locks, and false `LockData` only when requested.
- Build all Filters and Filters2 buckets from one target-user-policy snapshot.
  Apply folder, tag, parental-rating, and unrated filters consistently to item
  values, years, ratings, and stream languages.
- Build Filters2 audio/subtitle language choices with one policy-aware set query.
  Include alternate versions and Episode descendants of Series/Season searches,
  map missing languages to `und`, and sort by localized display name.
- Bind all six nullable Filters2 classifiers (`IsAiring`, `IsMovie`, `IsSports`,
  `IsKids`, `IsNews`, and `IsSeries`) case-insensitively and apply each to both
  genre and stream-language discovery. Reject malformed booleans.
- Do not use a DTO projector as an existence check. Use a targeted PostgreSQL
  query instead.

### 6.3 Alternate versions

- Treat alternate video versions as one playback group while exposing each
  version as a distinct `MediaSource`.
- Keep the requested or displayed source first, then apply the official ordering
  for grouping roots and local alternates.
- Resolve explicit `MediaSourceId`, stream index, subtitle index, and attachment
  requests inside the authorized version group. Never serve the same index from
  a different displayed primary.
- Keep stream and attachment loading batched by all version identifiers on a
  page.
- Persist a deterministic parent-local `sort_order`; preserve it across rescans
  and compact both sides when a child moves.
- Do not let an alternate-only credit or playback row change primary-only counts,
  except where official user-data fallback explicitly requires it.
- Apply each playback `DeviceProfile` independently per source. Explicit audio
  or subtitle indexes apply only to the source selected by `MediaSourceId`.
- Project the selected subtitle index back to that source's
  `DefaultSubtitleStreamIndex`, including `-1` when subtitles are disabled.
- Populate subtitle stream `DeliveryMethod` and API or direct external subtitle
  URLs, including the API key suffix, according to official `SetDeviceSpecificSubtitleInfo`
  behavior. Populate selected-source attachment delivery URLs as well.
- Project every persisted stream's `SupportsExternalStream`: true for external
  streams and text, PGS, or VobSub subtitles.

## 7. Scanning and Filesystem Behavior

### 7.1 Scan correctness

- Keep serial and concurrent scans failure-equivalent. Database, hierarchy, and
  filesystem/persistence failures fail the scan with an accurate total and a
  bounded sample of per-file diagnostics.
- An FFprobe failure is partial success: retain the item with fallback streams
  and do not fail the entire scan.
- Create Series and Season nodes idempotently while holding the PostgreSQL
  hierarchy lock. A duplicate-node race must not drop a media item.
- Reuse Series/Season resolution only within one bounded file batch. A cache miss
  must still call the locked create-or-read path.
- Keep per-item NFO relation persistence outside the hierarchy cache.
- Persist additive NFO Studio and Person relations in one validated transaction
  per media item. Preserve existing credits and Person order/role semantics.
- Persist and project `.ignore` behavior using the nearest-ancestor rule before
  adding a file to a snapshot, recursing, or resolving extras.
- Clear only the directory-to-rule lookup at the start of each full or
  single-library scan. Keep the parsed-rule cache bounded.
- Propagate `.ignore` file I/O failures as scan failures.

### 7.2 Watcher and batching

- Keep watcher queues bounded and deduplicated.
- Coalesce changed paths by virtual library before scanning.
- Reuse one short-lived directory snapshot for sibling media discovery.
- Batch PostgreSQL reads and writes instead of issuing per-file operations.
- Skip unavailable or non-directory library roots and isolate each root's watch
  registration error.
- A successful parent watch may cover duplicate and child roots. A child may
  still register if its parent watch failed.
- Do not retain an idle watcher thread when no root was registered.

### 7.3 Persisted file facts

- Persist local file size and media facts gathered from the scan snapshot. Do not
  stat media files from browse or playback-info request paths.
- Persist the local `.strm` shortcut file length during scans and rescans. Do not
  stat a remote target during item details.
- Version successful local Audio/Video probes with a source fingerprint containing
  normalized path, size, and modification time.
- A missing, stale, or mismatched probe marker triggers one bounded repair even
  when historical streams exist. Write the marker only after all probe data is
  persisted.
- Treat `.strm` and remote sources as lazy playback probes.
- Lazy `.strm` probing must synchronously terminate FFprobe before returning.
  An async timeout around an uncancelled blocking child is not sufficient.
- Coordinate lazy probes by item and resolved target so concurrent requests share
  one bounded flight. Keep failure backoff short-lived and hard-bounded.

## 8. Metadata and Providers

### 8.1 Merge rules

- Providers have deterministic priority and merge behavior.
- Local metadata is merged before remote providers.
- Preserve unknown or optional metadata. A partial provider response must not
  erase valid established metadata.
- Honor `LockedFields.Name`, `Overview`, `OfficialRating`, and `Genres` on both
  gap filling and full replacement where the official server does.
- A locked Name also protects `SortName`.
- Remote blank or whitespace-only names must never erase an established title.
- Merge genres only when permitted. Keep JSON `Genres` atomic with normalized
  PostgreSQL genre relations.
- Provider network calls require timeouts, bounded concurrency, and useful error
  context without leaking credentials or signed URLs.
- Provider failures and timeouts must remain distinguishable from valid empty
  results.

### 8.2 Episodes and TV hierarchy

- Treat episode NFO titles equal to the series name, `showtitle`, or filename as
  placeholders during later scans.
- Do not let a repeated scan or alternate-version regroup replace a valid scraped
  title with a path-derived or series-derived placeholder.
- For bulk season refresh, apply metadata to the visible primary of each version
  group.
- Missing-metadata repair includes primary episodes whose title is empty or
  still equals the parent series title, even when provider ids already exist.
- An unlocked primary may use a non-placeholder `OriginalTitle` only when its
  name is empty, path-derived, or equal to the parent series.
- Apply post-provider `OriginalTitle` repairs with one set-based PostgreSQL
  update scoped to the refreshed series or episode.
- If TMDb returns an episode name equal to its series name in the preferred
  language, fetch the English episode metadata once and use only a non-placeholder
  name as a fallback. Preserve all localized non-name fields.
- Use relational Series/Season links for DTO names and image inheritance rather
  than parsing paths at request time.
- Order episode detail pages with the official aired-episode comparer before
  `StartItemId`, adjacency, or paging. Specials with air-before/after anchors
  must be positioned relative to regular episodes.
- Bind Episodes `SortBy` through official enum-converter semantics: names,
  signed `Int32` values, and comma-delimited bitwise combinations. Only a final
  `Random` value randomizes; malformed input behaves as unset.
- Apply the shared DTO image and user-data options to Episodes, Seasons, Upcoming,
  and NextUp instead of accepting and discarding them.

### 8.3 Item-by-name and Person entities

- Use official deterministic UTF-16LE identifier and `.NET Guid(byte[])`
  semantics for Genre, MusicGenre, Studio, Year, and Person item-by-name rows.
- Lowercase item-by-name keys when normalization is enabled or case-sensitive ids
  are disabled. Preserve path casing only when normalization is disabled and
  case-sensitive ids are enabled.
- Persist and round-trip `EnableNormalizedItemByNameIds` and
  `EnableCaseSensitiveItemIds`; the case-sensitive setting defaults to true.
- Create official metadata directories before inserting item-by-name rows.
- Backfill and reconcile in deterministic keyset pages with bounded PostgreSQL
  batches. Never replace or delete established entities, images, or metadata
  during reconciliation.
- Genre and MusicGenre list routes and Filters2 expose persisted BaseItem ids,
  not normalized item-value ids.
- Collapse equal persisted `PresentationUniqueKey` groups to the smallest UUID
  before counting, ordering, and paging.
- Keep `people.id` private as the internal credit key. Public Person DTOs,
  `/Persons`, `BaseItemDto.People`, SearchHint, images, favorites, and user data
  use the exact canonical Person `base_items.id`.
- Translate `Items?PersonIds=` from public Person ids through exact persisted
  names to internal credit ids in one set-based query.
- A person remains discoverable through `/Persons` when any credited policy-visible
  item, including an alternate version, matches. Person detail counts use only
  policy-visible primary items.

## 9. Playback and Transcoding

### 9.1 PlaybackInfo and capabilities

- Match official query-over-body precedence for `DeviceProfile`. A client may
  register its profile through session capabilities and omit it from later
  `PlaybackInfo` calls.
- Apply the profile independently to every returned `MediaSource`.
- Derive `SupportsDirectPlay`, `SupportsDirectStream`, and
  `SupportsTranscoding` from the final selected method and device policy. Do not
  advertise a URL or method that is not implemented.
- When no profile is supplied, retain static direct play for supported local and
  HTTP-proxy sources while leaving direct stream and transcoding unavailable
  until a real route is selected.
- Generate `TranscodingUrl` for HTTP progressive and HLS transcoding selections.
- Project source bitrate from persisted metadata, with an inferred fallback from
  non-external streams, independently per version.
- Project the persisted/probed container first, then use the path extension only
  as a fallback. Strip URL query and fragment components before extension checks.
- Preserve the displayed item's raw top-level `Container`; normalize each nested
  source against that version's own path.
- Project the actual `VideoType` for each source and the top-level item.
- Project each source's own size, file ETag, and bitrate. Do not reuse the
  displayed primary's values for an alternate.
- Keep `MediaSourceCount` count-only and batched. Always count the displayed
  source, retain the official nullable single-source behavior, and include
  episode groups without loading every source.
- Apply static capability flags from the target user's policy: audio transcoding
  controls audio sources, while video transcoding and remuxing independently
  control video sources.

### 9.2 Progressive and HLS streams

- Bind the complete official stream query surface, including legacy, Android,
  mixed-case, and lowercase spellings.
- Resolve `MediaSourceId` inside the authorized alternate-version group before
  probing or opening a file.
- For `.strm`, stream the resolved target rather than sidecar bytes. For direct
  remote HTTP, proxy bounded byte ranges through the server unless official
  options explicitly allow a temporary redirect.
- Forward only persisted required headers such as `User-Agent`. Never forward
  arbitrary client headers or expose remote authorization headers in DTOs.
- For local static playback, serve unchanged bytes. `static=true` must not enter
  FFmpeg.
- Progressive requests with `static` omitted or false must honor the requested
  codecs, stream indexes, bitrate, dimensions, maximum frame rate, start
  position, subtitle behavior, and audio channel settings.
- `AudioStreamIndex` and `SubtitleStreamIndex` are persisted global media-stream
  indexes. For FFmpeg subtitle filters, convert the subtitle index to the
  zero-based index among embedded subtitle streams only.
- Burn subtitles only for official `Encode` behavior. `External` and `Drop` must
  not silently burn them into video.
- Use a unique output path or complete parameter key per transcode request. Remove
  temporary outputs after the response consumes them.
- Start FFmpeg and stream the growing output immediately. Never wait for the
  complete transcode before responding.
- Use fragmented MP4 output compatible with Android: video uses
  `-movflags frag_keyframe+empty_moov+delay_moov`; MP4-family audio uses
  `empty_moov+delay_moov`.
- Preserve `CopyTimestamps=true` semantics for seek requests.
- Video direct stream requires unchanged local File-source video and audio copy
  into the selected progressive container. It must not accept filters,
  bitrate/size/frame-rate changes, subtitle burn-in, or a remote URL.
- Include every output-affecting parameter in the deterministic transcode job id.
- Never share a transcode output between requests with different stream, codec,
  bitrate, seek, subtitle, or dimension options.
- Bind both official and legacy bitrate capitalizations, including
  `VideoBitrate`, `AudioBitrate`, `VideoBitRate`, and `AudioBitRate`. Do not rely
  on case-insensitive matching to bridge intentionally different internal names.
- Include `AudioStreamIndex` and `MaxFramerate` in HLS job identity. Use
  `-map 0:{index}` for an explicit persisted audio index.
- Generate a job-scoped EVENT playlist when the selected source has unknown
  runtime. Use finite VOD only for a known positive runtime.

### 9.3 Playback state

- Persist progress against the authorized selected `MediaSourceId`.
- When projecting the displayed primary, fall back to the latest visible
  alternate-version user-data row when the primary has no row.
- Normalize start/progress reports against the active transcoding registry. An
  omitted method defaults to Transcode; downgrade to DirectPlay when
  `PlaySessionId` is blank or unknown.
- Build `NowPlayingItem` from the displayed item and authorized selected source.
  Reuse that snapshot when later progress reports omit `Item`.
- Validate `Item` as an object or null and every `NowPlayingQueue` entry as a
  valid official `QueueItem` before changing session state.
- Compute Next Up from visible primary episodes while aggregating played,
  resume, and activity state across all versions.
- Apply `NextUpDateCutoff` to the latest activity date. Handle placed
  season-zero specials with the official aired-episode comparer.

## 10. Images, Chapters, Lyrics, and Segments

### 10.1 Images

- Do not decode, resize, reformat, decorate, or otherwise transform image bytes
  requested by API clients unless a route explicitly requires it.
- Stream original bytes with the official content type. Support APNG, AVIF, ICO
  (`image/x-icon`), TIFF, and Jellyfin TBN JPEG semantics without inspecting
  content.
- Image-info routes return persisted dimensions and BlurHash values without
  lazy decoding or metadata writes.
- Do not advertise byte ranges on ordinary image responses unless Range is
  implemented. Trickplay tile routes are the range-aware exception.
- Stream trickplay tiles in bounded chunks and preserve HEAD and range behavior.
- Validate `ImageType` and `ImageFormat` names or integer values before resource
  lookup. Reject unknown values as bad requests.
- Clone or reference legacy image files; never move or delete source files during
  reconciliation.
- Apply official local images before inherited Series/Season images. Local
  images suppress the corresponding inherited field, and Series does not inherit
  parent images.
- Episode/Season DTOs always derive `SeriesPrimaryImageTag`; Episode primary
  prefers Season then Series, Logo uses the nearest parent, Thumb prefers Series
  over Season, and Backdrop uses the nearest available parent.

### 10.2 Chapters

- Project chapters only for `ItemFields.Chapters`, except default all-fields item
  details, which include an empty array when none exist.
- Load page chapters in one PostgreSQL batch and order by
  `StartPositionTicks`.
- Keep alternate versions isolated.
- Derive chapter image tags from the owning item's media path without applying
  DTO image selectors or limits.
- Resolve chapter images by `(item_id, ChapterIndex)` from the chapter
  repository. Do not store chapter thumbnails as ordinary item images.
- Replace chapters under an owner-row lock in one transaction, including empty
  replacements.
- Insert in fixed 128-row batches, preserve input order in the replacement result,
  and roll back all batches on failure.

### 10.3 Lyrics

- Store uploaded and downloaded lyrics in the item's internal metadata directory
  using a same-directory temporary file and atomic rename.
- Register the file as a Lyric media stream. The parsed JSON is only a cache.
- Reads prefer registered streams, in stream-index order.
- Select parsers by the file path's actual extension, not the persisted codec.
- Decode BOM-marked UTF-8, UTF-16LE/BE, and UTF-32LE/BE. Without a BOM, use UTF-8
  replacement fallback. Malformed or incomplete input produces replacement
  characters, not a hard failure.
- Preserve uploaded/downloaded bytes exactly; decoding is only for parsing DTOs.
- Do not remove unregistered files, symlinks, or paths outside the metadata root.
- Resolve and authorize the Audio item before validating upload body or filename.
- Lyric provider search and download use at most four providers in flight, a
  30-second deadline per provider, configured result order, and cancellation by
  dropping futures rather than spawning detached work.
- Provider errors and timeouts in search become isolated empty results. Download
  unknown/empty/unparseable responses are 404; provider failures remain server
  errors.
- Keep remote lyric metadata within the strongly typed `LyricMetadata` contract.
- Derive provider ids from invariant-lowercase provider names using UTF-16LE
  MD5, `.NET Guid(byte[])` ordering, and lowercase hyphenless `N` formatting.
- Split a provider-owned lyric id only at its first underscore.
- Build provider search requests from the policy-authorized Audio item and pass
  its original path as `LyricSearchRequest.MediaPath`, with official name, album,
  artist, album artist, and duration fields.
- Expose configured lyric providers as the music library's `LyricFetchers`
  options in execution order, case-insensitively deduplicated and enabled by
  default. Do not expose them for representative types that omit Audio.

### 10.4 Remote images and media segments

- Single-flight remote image downloads by URL, with at most four leader
  downloads for distinct URLs.
- Acquire the global permit inside the single-flight initializer so same-URL
  followers consume no extra permit.
- Validate that upstream content is an image and suppress permanently invalid
  references according to official behavior.
- Filter media segments by registered provider ids and the owning library's
  disabled providers, comparing provider names case-insensitively.
- Derive media-segment provider ids with the same UTF-16LE MD5 and `.NET Guid`
  rules used for providers.
- Return an empty result when no corresponding provider is enabled instead of
  exposing stale persisted rows.

## 11. PostgreSQL and Data Access

- Keep constraints, indexes, atomic upserts, and transactional invariants in
  PostgreSQL where practical.
- Add indexes or constraints required by new query patterns in a migration.
- Use set-based queries or fixed-size batches. Do not issue per-item queries.
- Use fixed 128-row batches for large relation and chapter operations unless the
  existing repository establishes a smaller safe bound.
- Serialize PostgreSQL integration tests that mutate the shared virtual-folder
  catalog while running a full scan.
- Do not hide scan races by weakening failure checks.
- Materialize shared candidate sets when count and page queries would otherwise
  repeat expensive work.
- For shared Resume candidates, materialize only item identifiers and latest
  playback dates after applying all authorization and metadata filters. Load
  full page items afterward in the same repeatable-read transaction.
- Build playback-aware queries from target-user `user_data` and reverse hierarchy
  lookups, not correlated scans over all `base_items`.
- When a route has already resolved and authorized a user, reuse the authorized
  listing path instead of reloading the user. Keep public service entry points
  validating their target user for callers without a resolved policy context.
- Build list counts, pages, and facets from the same filtered candidate set.
- Exclude alternate versions and owned non-extra rows from ordinary item counts
  where the official server does.
- Preserve the official nullable-user fallback for administrators requesting a
  nonexistent target.
- For `/Years`, preserve the recursive-folder total-count quirk: report visible
  filtered primary descendants before extracting distinct positive production
  years. Non-recursive and non-folder parents use distinct-year count.
- For latest items, apply grouping before the result limit and keep candidate
  buffers bounded. Load final containers and fallbacks in batches.
- Apply `LatestItemsExcludes` only to implicit root scope.
- Use nearest-ancestor closure-table queries for music and photo grouping.
- Count folder children, recursive item counts, and item-value counts with
  target-user-policy-aware set queries.
- Do not let old or plugin-defined CLR item types escape through public
  `BaseItemDto.Type`. Hydrate persisted rows through the shared type registry.
- Keep filtering, counting, and media-source queries aware of canonical short item
  types and official legacy CLR-qualified persisted names.
- Preserve unknown plugin-defined types instead of discarding them.

## 12. Concurrency, Cancellation, and Resource Limits

- Concurrency limits must be explicit and applied at the expensive resource,
  not only at the request layer.
- Do not hold a database transaction, lock, semaphore, or filesystem handle
  across unrelated network waits.
- Provider and remote HTTP calls require a deadline.
- External processes require owned handles, kill-on-cancel behavior, and output
  limits.
- Lock ordering must be deterministic. Validate and lock multiple rows in a
  stable order to avoid deadlocks.
- Do not spawn detached tasks to work around cancellation. Futures that own the
  request lifetime should be cancelled by drop.
- Release permits promptly when the owner fails or is cancelled.
- Avoid unbounded queues, caches, retry state, and per-request buffers.
- Do not assume allocator-retained RSS is a leak. Measure whether work is
  reclaimed and distinguish page cache from anonymous memory.

## 13. Testing and Validation

Tests, comments, and this guide must use the API, enum, and setting names that
actually exist in the checked-out code. If a documented name differs from the
Rust or official C# identifier, use the real identifier and note the mapping
rather than allowing a stale contract to remain.

### 13.1 Focused tests

Every behavior change needs a focused regression test covering:

- official and lowercase route casing where applicable;
- PascalCase, camelCase, and repeated/comma-delimited query behavior;
- signed pagination boundaries and echoed `StartIndex`;
- authorization and status/error precedence;
- response field omission, null, empty array, enum, and date shapes;
- set-based behavior, stable ordering, and idempotency;
- cancellation, timeout, and bounded concurrency where relevant.

Do not write a test that only asserts the implementation's current behavior when
the official server specifies something different. Record parity assumptions in
test names or focused comments.

### 13.2 Batch validation

After a coherent batch is ready, use the remote host's existing cached builder
and run the applicable checks:

```bash
cargo fmt --all -- --check
cargo test -p <affected-crate>
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Do not run a full build or full test suite after every small edit.

### 13.3 PostgreSQL tests

- Some `jellyfin-data` integration tests create temporary databases whose names
  begin with `jellyfin_`.
- Do not point destructive integration tests at a database containing user data.
- The account must have permission to create and drop those temporary databases.
- Run PostgreSQL-backed route and scan tests against a disposable database.

### 13.4 SDK wire validation

- Run the `android_sdk_compat` test with a temporary PostgreSQL database and
  `JELLYFIN_ANDROID_DUMP` set when changing item pages, mobile bootstrap routes,
  authentication responses, or shared DTO shapes.
- Validate every dumped response with `tools/kotlin_validate.py`.
- Validate the same dump with `tools/swift_validate.py`; Swift `Codable` rejects
  a complete enclosing response when a nested enum, date, scalar, dictionary, or
  array has the wrong wire shape.
- Include an actual `/Users/AuthenticateByName` response in the dump so nested
  `UserDto` and `SessionInfoDto` are checked.
- Run `tools/test_kotlin_validate.py`, `tools/test_swift_validate.py`, or
  `tools/test_runtime_sdk_validate.py` when changing the corresponding validator.
- Use a real Swift toolchain for `Codable` decoding when available.
- Never commit checked-out SDK trees, generated dumps, or Python bytecode.

### 13.5 Memory and query plans

For scan-memory work, use a repeatable large-directory or synthetic-library
measurement when possible. Report:

- baseline, peak, 60-second post-scan, and 300-second post-scan values;
- process RSS and anonymous memory separately from cgroup file cache;
- whether memory returns after the scan.

Do not report reclaimable metadata image page cache as a Rust heap leak.

For Resume query changes, the ignored `resume_materialization_plan` test with
`JELLYFIN_RESUME_PLAN_DUMP` records isolated PostgreSQL plans. Compare
materialized row width and temporary blocks separately from process RSS.
Execution-plan estimates and database caches alone do not establish a leak.

### 13.6 Network and media tests

- Mock provider HTTP calls; tests must not depend on live third-party services.
- Use bounded fixtures for provider failures, timeouts, malformed bodies, and
  cancellation.
- Exercise ffprobe fallback behavior without requiring valid media when the code
  path does not need real decoding.
- Test Range, HEAD, and early-streaming behavior for media routes that advertise
  those semantics.

## 14. Deployment Verification

- The deployment checkout is `/home/lqs/jellyfin-rust` on the configured test
  host. Inspect its current state before changing it; do not assume local paths
  are valid remotely.
- Deploy only committed revisions.
- Record the revision tested.
- Verify service health, relevant API behavior, scan completion or cancellation,
  and service logs.
- Do not delete databases, media, configuration, containers, or volumes during
  deployment validation unless the user explicitly requests it.
- Treat `/home/lqs/Videos` and `/CloudNAS` as user data when present.
- Do not expose secrets from Compose, environment files, or logs in reports.

## 15. Git and Commit Rules

- Keep one coherent, tested change per commit.
- Use a concise imperative subject and a conventional prefix, for example:
  `fix: preserve alternate media source ids`
  `perf: batch item page image lookup`
  `test: cover lowercase playback info routes`
- Do not mix formatting-only changes with behavioral changes.
- Do not commit `target/`, local logs, caches, runtime data, media, generated SDK
  trees, dumps, secrets, or deployment-only files.
- Review `git diff --check` and the final staged file list before committing.
- Do not amend, rebase, force-push, or rewrite existing history unless the user
  explicitly requests it.

## 16. Definition of Done

A change is complete only when all relevant conditions are met:

- The official C# behavior has been checked.
- The Rust implementation preserves the observable contract.
- Route and query casing is covered where applicable.
- Authorization and error precedence are preserved.
- Pagination, ordering, and totals match the endpoint contract.
- Database access is set-based or bounded; no N+1 path was added.
- Long-running work is cancellable and resource use is bounded.
- Focused tests cover the regression.
- The affected crate tests pass.
- Workspace check, formatting, and clippy pass for the completed batch.
- SDK wire validation is run when public DTO shapes change.
- No unrelated changes or secrets are included.
- The final diff is independently reviewable.
