# Jellyfin Rust contributor guide

## Scope

- This repository reimplements the Jellyfin server in Rust while preserving compatibility with the official Jellyfin API and web client.
- The checked-out official server source in `jellyfin/` is the behavioral reference. Prefer matching its externally visible behavior, defaults, validation, authorization, ordering, and error semantics over inventing new behavior.
- Current optimization priorities are media-library management and scanning, users and policies, metadata scraping/providers, and PostgreSQL-backed data access.
- Do not work on Live TV unless a task explicitly asks for it. Avoid incidental changes under `src/jellyfin-live-tv`.
- Treat `Emby.ApiClients/Clients/Go/api/swagger.yaml` and its generated Java and Swift clients as
  the Emby wire contract. Keep Emby endpoints under `/emby`, register protocol-specific adapters
  before the shared Jellyfin fallback, and never change an unprefixed Jellyfin response merely to
  satisfy an Emby-only DTO.
- Keep the checked-in Emby operation inventory synchronized with the generated client source.
  Exclude only plugin operations, `/LiveTv` routes, and QuickConnect unless a task explicitly
  includes them;
  ordinary playback `LiveStreams` routes remain in scope. Track every other missing method/path in
  the explicit gap ledger, remove entries only with route and response-shape tests, and keep the
  combined server test proving `/emby` and Jellyfin root routes remain isolated.
- Normalize empty successful shared-handler responses to HTTP 200 only inside the `/emby` tree,
  because the generated 4.10.0.40 document declares 200 as the sole success status for all 548
  operations. Preserve Jellyfin's root and `/api` 204 mutation responses.
- Require `Size` on `/emby/Playback/BitrateTest`, bind it case-insensitively with the last duplicate
  winning, and accept API keys as generated-client authentication. Preserve Jellyfin root and
  `/api` default size 102400 and the shared inclusive 1..100000000 validation.
- Require one nonblank, case-insensitively bound, last-duplicate-wins `Id` for Emby's DELETE
  `/Devices` and POST `/Devices/Delete`. Keep Jellyfin's root and `/api` repeated/comma-separated
  device-id binder and its omitted-id no-op behavior unchanged.
- Bind the required `Id` query on `/Devices/Info` and `/Devices/Options` case-insensitively with the
  last duplicate winning on Jellyfin root, `/api`, and `/emby`. Apply the same semantics to every
  known `DeviceOptionsDto` JSON property, including `CustomName`, and ignore unknown properties
  without changing the existing authorization and success-status differences between protocols.
- Never serialize Jellyfin GUID strings into Emby's `NameLongIdPair.Id` fields. Until a stable Emby
  numeric-id mapping exists, `/emby` BaseItem responses must omit incompatible `Studios`,
  `GenreItems`, `TagItems`, and `Collections` relations; keep the names/ids unchanged on every
  unprefixed Jellyfin response and avoid whole-response buffering to adapt these fields.
- Keep Emby BaseItem enum adaptation protocol-local as well: omit unsupported Person `Type` values,
  filter Jellyfin-only `Lyric` media streams, omit the Jellyfin-only `Drop` subtitle delivery method,
  and omit `Remote`/`Offline` `LocationType` values from `/emby` responses. Preserve the person and
  every supported stream, and keep unprefixed Jellyfin DTOs byte-shape compatible with the Jellyfin
  contract.
- Keep Emby's item-image info list on its generated closed `ImageType` contract: omit Jellyfin-only
  `Profile` entries from `/emby/Items/{Id}/Images` instead of fabricating an Emby type, while
  preserving them on Jellyfin root and `/api` responses.
- Omit `ThemeMediaResult.OwnerId` only below `/emby`: Emby's generated clients model that optional
  property as `Int64`, so a Jellyfin UUID makes Swift reject the complete ThemeSongs, ThemeVideos,
  or ThemeMedia response. Preserve Jellyfin's UUID OwnerId on root and `/api` theme routes.
- Keep Emby virtual-folder `LibraryOptions.TypeOptions[].ImageOptions` on that same closed
  `ImageType` contract: omit Jellyfin-only `Profile` options from both Emby virtual-folder list
  shapes, while preserving them on Jellyfin root and `/api` responses.
- Register generated-client literal routes ahead of shared dynamic fallbacks and dispatch every
  static segment case-insensitively without normalizing dynamic values or query strings. Build the
  normalization set from the full generated Emby operation inventory, including compound segments
  such as `universal.{Container}`, and verify every supported shared or dedicated operation reaches
  the same handler under mixed casing. Keep Emby-only authorization matching gated by `/emby` so it
  cannot change root or `/api` error precedence. Mount
  Emby's legacy `/Audio/{Id}/live.m3u8` only under `/emby`; validate its required `Container` with
  the official encoding-container regex and reuse the authenticated HLS pipeline without changing
  Jellyfin's unprefixed Audio or existing Video live behavior.
- Keep Emby's top-level subtitle HLS routes protocol-local. Bind `SubtitleSegmentLength` and
  `ManifestSubtitles` case-insensitively, treat the latter as the segment format rather than a stream
  index, and expose only formats the shared subtitle pipeline can actually produce. Prefer a marked
  default Subtitle stream, then the smallest persisted stream index, and keep Jellyfin's modern
  item/media-source/index subtitle routes unchanged.
- Keep Emby's generated subtitle-delete aliases protocol-local. Both its legacy POST `/Delete` and
  canonical DELETE accept ordinary authenticated users, require a nonblank case-insensitive
  `MediaSourceId`, and return HTTP 200; Jellyfin's unprefixed DELETE remains elevated and returns
  204. Remove only explicitly external subtitle files before deleting their stream rows, and never
  unlink an embedded stream's video-container path.
- Do not serve Jellyfin trickplay sprite tiles as Emby BIF files. Until a real BIF timestamp index
  and individual-frame representation exists, validate the generated client's required signed
  `Width` and return an empty 404 without advertising byte ranges. Capability-discovery endpoints
  must return SDK-decodable empty collections when no corresponding provider exists rather than
  inventing unavailable features.
- Keep Emby HomeSections protocol-local and persist their ordered `ContentSection` objects in the
  target user's PostgreSQL-backed display preferences. Bind top-level section and mutation fields
  case-insensitively with last-duplicate-wins semantics, ignore unknown properties, preserve stable
  order for add/update/delete/move operations, and reject an invalid move before changing the
  stored sequence. Resolve and authorize the target user before binding mutation bodies; ordinary
  users may manage only themselves, while administrators and valid API keys may target any user.
- Keep Emby DisplayPreferences and UserSettings mutations on their generated empty-200 contract.
  Require the generated DisplayPreferences `UserId` and GET `Client` query values, bind query and
  DTO names case-insensitively with last-duplicate-wins semantics, and keep Jellyfin's root and
  `/api` DisplayPreferences binder and 204 mutation response unchanged.
- Resolve Emby section-item queries from the target user's persisted `Emby.HomeSections`
  preference. Merge section defaults without overriding explicitly supplied case-insensitive query
  keys, then delegate to the shared authorized user-items path so policy filtering, signed
  pagination, and batched DTO projection remain consistent. Keep this route confined to `/emby`.
- Keep Emby's `HideFromResume` state in the internal PostgreSQL `user_data.is_hidden_from_resume`
  flag without exposing it through Jellyfin's `UserItemDataDto`. Toggle only that flag atomically,
  preserve playback position and all ordinary user data, and exclude hidden rows from Resume and
  NextUp candidates while leaving played, favorite, rating, and recent activity semantics intact.
  Keep the route protocol-local, bind the required `Hide` query case-insensitively, and preserve
  ordinary self-only target authorization with administrator and API-key overrides.
- Keep legacy Emby GameGenre entities protocol-local. Reconcile their deterministic item-by-name
  rows from real Game genre values in bounded pages, preserve signed pagination and user-less global
  query behavior, apply an explicit user's library policy, and expose GameGenre image routes only
  below `/emby`; unprefixed Jellyfin must not acquire Game or GameGenre endpoints.
- Keep legacy Emby Game similarity and remote Game search protocol-local as well. Resolve Game and
  its CLR alias without registering either in Jellyfin's global item-type registry; omitted or
  unresolved `UserId` uses the official user-less query, while a resolved explicit user applies the
  ordinary folder, tag, rating, and parental policy. Accept Emby's nullable signed 64-bit Game
  search `ItemId`, including numeric strings, without fabricating a Jellyfin UUID mapping, and keep
  both routes absent from the root and `/api` trees.
- Keep Emby's extensionless `/swagger`, retired read-only Sync discovery, and DLNA ProfileInfos
  routes protocol-local. Empty providers return their generated SDK collection shapes; required
  Sync query names bind case-insensitively, DLNA profile discovery is administrator-only, and the
  unsupported Sync mutation methods stay in the explicit gap ledger. Emby System Ping, public
  system info, Branding configuration/CSS, and Features follow the generated Emby authenticated or
  elevated policies even though the corresponding Jellyfin bootstrap routes remain public.
- Keep `/emby/Sync/Options` protocol-local and return all four generated SDK collection members even
  when no legacy Sync provider exists. Bind its required `UserId`, optional item/parent/target ids,
  and one-based `SyncCategory` values case-insensitively with last-duplicate-wins semantics. Keep
  `/emby/Packages/Updates` administrator/API-key-only, require its case-insensitive `PackageType`,
  and return an SDK-decodable empty array when no package-update provider exists; mixed-case Emby
  authorization must not weaken this boundary or expose either route under Jellyfin root or `/api`.
- Keep all 16 generated Emby offline-Sync mutations mounted only below `/emby`, including their
  lowercase static aliases. Validate required query, path, and JSON body bindings before reporting
  the retired provider as not found; do not fabricate target, job, or job-item state. `Sync/Data`
  may return an empty `ItemIdsToRemove` collection because no legacy target has removal state.
- Implement `/emby/AudioBooks/NextUp` as the target user's policy-aware resumable AudioBook query,
  not a permanent empty placeholder. Bind the complete generated query surface case-insensitively,
  preserve signed `Int32` paging, map `AlbumId` to the shared album filter, and reuse the batched
  Items DTO projector so field, image, and user-data options remain consistent. Keep the route and
  its response adaptation confined to `/emby`.
- Derive Emby library-discovery prefixes, item types, audio/video/subtitle codecs, audio layouts,
  stream languages, tags, containers, extended video types, and official ratings from the full
  policy-filtered Items candidate set in one set-based PostgreSQL facet query per request. Preserve
  the generated `QueryResult<TagItem>` shapes, deterministic case-insensitive distinct ordering,
  signed paging, and the complete supported Items query binder; never load a full item page or
  issue per-item stream/value lookups. Keep `/emby/Features`
  administrator/API-key-only and return an empty SDK collection when no Emby feature provider is
  registered, without adding any of these protocol-owned routes to Jellyfin root or `/api`.
- Persist `POST /emby/Items/Access` assignments in the private PostgreSQL Emby relation without
  changing Jellyfin's item policy model or adding the route to root or `/api`. Bind the three body
  properties case-insensitively with last-duplicate-wins semantics, accept the generated .NET
  one-based numeric enum values and numeric strings, lock validated users/items in deterministic
  order, and apply each deduplicated Cartesian-product mutation atomically; `None`, null, or an
  omitted level removes explicit assignments.
- Implement `POST /emby/Items/Shared/Leave` as an atomic, idempotent removal from that same private
  Emby item-access relation. Bind nullable `ItemIds` and `UserId` case-insensitively with
  last-duplicate-wins semantics, target the current device user when `UserId` is omitted, and keep
  explicit targets self/administrator/API-key authorized. Resolve and authorize the target before
  parsing item ids, validate every referenced item before deleting any assignment, and keep an
  omitted or empty item list as a successful no-op without exposing the route under root or `/api`.
- Resolve `GET /emby/Persons/{Id}/Credits` only from the exact deterministic canonical Person item,
  while keeping internal `people.id` values private. Query every policy-visible credited item and
  its relationship set-wise, group results in Emby's eight generated `PersonType` enum order, and
  omit unsupported Jellyfin-only credit kinds so Swift can decode the entire response. The
  generated operation has no paging or user query: a device session supplies its own policy
  context, an API key uses the user-less global item view, and unknown query pairs are ignored.
- Keep Emby's BoxSet provider discovery routes provider-backed rather than returning placeholders.
  `ProviderItems` and `Missing` return a pre-page-counted `QueryResult<RemoteSearchResult>` from the
  configured TMDb collection parts, preserve provider order, compare existing linked children by
  case-insensitive provider ids in bounded set queries, and authorize the target BoxSet through the
  requested user's normal item policy. Preserve signed `Int32` Skip/Take paging; force missing-only
  results on `Missing`, exclude unaired entries unless requested, and keep both routes below
  `/emby` only. The inherited generated Items/DTO query surface is accepted but, matching Emby's
  official collection handler, only user, paging, missing, and unaired inputs affect this result.
- Implement `/emby/Shows/Missing` through the shared authorized Items query while forcibly replacing
  every casing or encoded spelling of `IncludeItemTypes` and `IsMissing` with `Episode` and `true`.
  Preserve all other query pairs, signed pagination, target-user policy, batched DTO projection, and
  the Emby response adapter, without exposing the route in Jellyfin's root or `/api` trees.
- Store Emby's opaque encoding editor objects under private `emby-encoding-*` named-configuration
  keys. GET requires an authenticated user and POST requires administrator or API-key authority;
  preserve submitted JSON objects verbatim, keep codec context keys distinct, and never expose these
  records through Jellyfin's public system-configuration namespace or treat them as runtime FFmpeg
  configuration.
- Persist Emby's nullable `WasSearched` report as per-user PostgreSQL state, with target lookup and
  self/administrator/API-key authorization ahead of body validation. Clear operations are
  idempotent, user deletion cascades the state, and neither the storage nor its routes may leak into
  the unprefixed Jellyfin API.
- Adapt Emby `UserPolicy` and `UserConfiguration` only on `/emby` user DTOs and mutations. Bind all
  top-level properties case-insensitively with last-duplicate-wins semantics, accept numeric strings
  where the official JSON defaults do, retain Emby-only enum values and fields in nested PostgreSQL
  JSON beside the shared typed view, and batch-load response contracts. Overlay current shared
  values onto every mapped response field while preserving protocol-only values, and preserve the
  nested documents when login counters or Jellyfin mutations rewrite shared policy/configuration.
  Do not buffer unrelated `/Users/**` item pages for this adaptation.
- Keep Emby's `/Users/Query` response on its generated `QueryResult<UserDto>` wire shape: return
  only `Items` and `TotalRecordCount`; use `StartIndex` solely to page and never serialize it into
  the response. Do not alter Jellyfin user-list response models.
- Keep generated Emby `/Auth/Keys` paging protocol-local. Bind optional signed Int32 `StartIndex`
  and `Limit` case-insensitively with last-duplicate-wins semantics, preserve deterministic key
  order and the pre-page `TotalRecordCount`, and authorize before binding. Jellyfin root and `/api`
  retain their unpaged behavior and ignore these query values.
- Bind generated mobile credential JSON and password query fields case-insensitively with
  last-duplicate-wins semantics. Keep Emby's `/emby/Users/{Id}/Password` on its protocol-private
  `Id`/`NewPw`/`ResetPassword` body: the path id is authoritative, an authorized ordinary user may
  change their own password without the absent `CurrentPw` field, and success is an empty 200.
  Preserve the unprefixed Jellyfin self-service requirement for `CurrentPw` and its 204 response;
  password changes revoke other user sessions while resets preserve them in both protocols.
- Bind Emby's `POST /emby/Sessions/{Id}/Message` from its generated query-only contract: require
  `Text` and `Header`, accept nullable signed `TimeoutMs`, bind names case-insensitively with the
  last duplicate winning, and return an empty 200 after normal session-control authorization.
  Keep Jellyfin's root and `/api` variants on their JSON `MessageCommand` body and 204 response.
- Treat Emby's `/LiveStreams/MediaInfo` as a lookup and access-time touch of a real entry in the
  shared live-stream registry. Match ids case-insensitively, reject missing ids, return not found for
  unknown, closed, or expired streams, and keep the authenticated empty-response operation under
  `/emby` without inventing a successful stream.
- Advertise Emby's three dashboard-backed CopyData categories in official UI order: `UserPolicy`,
  `UserConfiguration`, then `UserData`. Copy selected user JSON together with every corresponding
  derived column, and copy user-data rows to deduplicated targets with a set-based PostgreSQL
  operation, all in one transaction. Preserve administrator/enabled-user invariants, revoke sessions
  for newly disabled targets, retain unrelated target user-data rows, and make any missing source or
  target abort the whole copy. Apply the same categories to `/emby/Users/New` when
  `CopyFromUserId` is supplied; omitted or empty nullable copy-option collections copy no category
  because the dashboard always submits its checkbox array explicitly. Keep binding case-insensitive,
  mutations administrator/API-key-only, user-updated notifications equivalent, and all behavior
  protocol-local.
- Keep Emby's bulk metadata reset administrator/API-key-only and confined to `/emby`. Bind the
  required comma-separated item ids case-insensitively, validate and lock every target before any
  write, then remove historical casing variants of lock fields and persist the official unlocked
  values in fixed 128-item batches. Preserve provider ids and unrelated metadata for the subsequent
  full replacement refresh, reserve capacity in one bounded refresh queue before committing the
  reset, process queued items with at most four refreshes in flight, and return after enqueueing
  instead of waiting on remote providers. Never add this legacy mutation to Jellyfin's unprefixed
  routes.

## Working practices

- Read the relevant Rust implementation and its official C# counterpart before changing behavior. Record important parity assumptions in tests or focused comments.
- Keep changes small and independently reviewable. Accumulate a few coherent fixes across agents,
  validate them together in the remote deployment host's existing cached builder, then commit each
  fix separately. Do not rebuild or run a full test cycle after every small edit. Keep the coordinating
  agent focused on integration, remote builds, testing, and deployment while agents implement fixes.
- Preserve unrelated user changes and existing commits. Never rewrite history or use destructive Git commands.
- Prefer bounded concurrency, streaming or pagination, batched PostgreSQL operations, and short-lived buffers for library scans. Do not collect an entire library into memory when work can be processed incrementally.
- Keep filesystem watcher queues bounded and deduplicated. Coalesce changed paths by virtual library before scanning, reuse one short-lived directory snapshot for sibling media discovery, and batch PostgreSQL reads and writes instead of issuing per-item queries.
- Apply the official nearest-ancestor `.ignore` rules to files and directories before adding them to
  a scan snapshot, recursing, or resolving media/extras. Clear only the directory-to-rule lookup at
  the start of each full or single-library scan so newly created `.ignore` files take effect without
  discarding the bounded parsed-rule cache; propagate rule-file I/O failures as scan failures.
- Keep serial and concurrent media scans failure-equivalent: database writes, hierarchy creation, and filesystem/persistence errors must fail the scan with a bounded per-file failure report and accurate total, while FFprobe failures retain the item with fallback streams and are treated as partial success.
- Serialize PostgreSQL integration tests that mutate the shared virtual-folder catalog while invoking
  a full library scan. Otherwise one fixture can scan another fixture's folder after its temporary
  media directory has been removed; do not hide that fixture race by weakening real scan failures.
- Keep deterministic scan hierarchy creation idempotent under sibling-file concurrency. Series and
  season nodes must be checked and created while holding the PostgreSQL hierarchy lock so a
  duplicate-node race cannot silently drop one media item.
- Reuse Series and Season resolution only within one bounded scan file batch. Cache misses must
  still call the locked create-or-read path, and per-item NFO relation persistence must remain
  outside that cache.
- Treat filesystem and persistence failures from concurrent media-item scans as scan failures just
  as in serial scans. Return an accurate total plus only a bounded sample of per-file diagnostics;
  media-probe fallbacks remain partial successes and must not make the scan fail.
- Keep database invariants in PostgreSQL where practical (constraints, indexes, atomic upserts, transactions), while keeping domain rules explicit in Rust.
- Persist metadata-editor `Studios` from the official `NameGuidPair` object array, binding both
  outer and nested JSON properties case-insensitively. Use names rather than submitted ids, keep
  first-occurrence order with case-insensitive deduplication, preserve omitted/null collections,
  and clear on an empty array. Update JSON and normalized Studio relations in the existing item
  transaction using fixed 128-name batches, and replace or clear Movie NFO studios when local
  metadata saving is enabled. Accept punctuation-only names in JSON/NFO even when the existing
  searchable-key constraint prevents a normalized relation; do not turn such edits into a 400.
  Keep the lowercase `/items/{itemId}` POST alias on the same administrator authorization path.
- Avoid N+1 queries. Use set-based queries or bounded batches, and add migrations for indexes or constraints required by new query patterns.
- Persist additive NFO Studio and Person relations in one validated transaction per media item:
  preserve existing credits, keep Person `list_order`/role conflict semantics, and never replace
  all credits merely to reduce scan round trips.
- Build playback-aware queries from the target user's `user_data` rows and reverse hierarchy lookups rather than correlated scans over all `base_items`. Materialize shared candidate sets when count and page queries would otherwise repeat expensive work.
- Materialize only item identifiers and latest-playback dates for shared Resume count/page candidates.
  Apply every authorization and metadata filter against the full source item before materialization,
  then batch-load the bounded page's complete items within the same repeatable-read transaction;
  do not put every candidate's wide JSON metadata into PostgreSQL's shared temporary result.
- When an item-by-name list route has already authorized its target user and applied the resulting
  policy to its query, call the corresponding authorized list path rather than rereading that user.
  Keep each public service list entry point validating its target user so callers without a resolved
  policy context retain the authorization boundary.
- Project inherited images for an item page with one batched DTO-image lookup. Do not call the image projector once per item.
- Project `ImageBlurHashes` only from persisted image metadata in the same batched DTO-image lookup
  that produces the exposed image tags. Keep the top-level map present when empty, include hashes
  for inherited and Series primary tags, and never decode images or issue per-item lookups to fill it.
- Derive a requested `PrimaryImageAspectRatio` from persisted positive dimensions for a local
  Primary image even when DTO image projection is disabled. Use the item's default ratio only when
  those local dimensions are unavailable or the Primary image is remote, and omit the field when
  the item has no Primary image; never decode the image to calculate it.
- When DTO image projection is enabled (including the default `/UserViews` and legacy
  `/Users/{userId}/Views` bootstrap paths), emit `ImageTags` as an object even when empty, and
  emit `BackdropImageTags` as an array when the Backdrop image type is enabled even when it is
  empty. Omit `ImageTags` only when image projection was explicitly disabled, and omit
  `BackdropImageTags` when that type is disabled; Afuse iterates these collections directly.
- Project `Chapters` only when `ItemFields.Chapters` is requested, while default all-fields item
  details must include an empty array when none exist. Load page chapters in one PostgreSQL batch,
  order by `StartPositionTicks`, keep alternate versions isolated, and derive chapter image tags
  from the owning item's media path without applying DTO image enablement, selectors, or limits.
  Resolve Chapter image GET/HEAD by `(item_id, ChapterIndex)` from the chapter repository; never
  store or enumerate chapter thumbnails as ordinary `base_item_images`, and serve their source
  bytes without decoding or resizing.
- Replace chapters under an owner-row lock in one transaction, including empty replacements. Insert
  fixed batches of 128 without returning fields already supplied by the caller, preserve input order
  in the replacement result and start-position order in reads, and roll back every batch on failure
  so concurrent replacements can never merge their chapter sets.
- Match official TV hierarchy image inheritance from relational Series/Season links: Episode and
  Season DTOs always derive `SeriesPrimaryImageTag`; Episode parent Primary prefers Season then
  Series; parent Logo prefers the nearest parent, parent Thumb prefers Series over Season, and
  parent Backdrop uses the nearest available parent. Local images suppress the corresponding
  inherited field, and Series itself must not inherit parent images.
- Resolve Similar and InstantMix seeds through the target user's normal library policy, and apply the
  same folder, tag, rating, and parental filters to every candidate query. Similar defaults to 50
  returned items and reports the post-limit result count; legacy CLR item types must not bypass policy.
- Build `/Movies/Recommendations` from the official four category families: weighted similarities
  to recently played and liked movies, plus director and actor names from recent movies. Score
  Genre/Tag/Studio/Director/Actor matches in bounded PostgreSQL batches, filter all candidates once
  through unplayed, version-grouping, and target-user policy rules, and project the merged candidate
  set once. Preserve the official double-weighted round robin, category ordering, signed Int32
  limits, UTF-16LE MD5 person category ids, and fully lowercase route/query compatibility.
- Omit `CategoryId` only from `/emby/Movies/Recommendations`, because Emby's generated clients
  declare that nullable field as a signed `Int64` and Jellyfin UUID/MD5 GUID values have no lossless
  numeric mapping. Preserve the UUID string on Jellyfin root and `/api` recommendation responses.
- Keep every InstantMix route on the official DTO-options contract. Accept signed limits and
  case-insensitive repeated fields/image options, report the pre-limit total, validate the Playlist
  route's seed type, collect Folder descendant-audio genres in one policy-aware query, and treat an
  empty-genre Audio seed as an unfiltered visible-Audio mix while an unknown genre name stays empty.
- Keep the six Similar routes on one contract: bind `ExcludeArtistIds`, `UserId`, signed `Limit`, and
  `Fields` case-insensitively; return official empty results for Episodes and named items other than
  MusicArtist; project the bounded page with default images, user data, and ProviderIds; and keep
  every static path segment reachable in fully lowercase form.
- Project theme songs and theme videos with the official default all-fields `DtoOptions`. Resolve
  `inheritFromParent` nearest-first and independently for each media kind, preserve that owner's
  id, default to `SortName` ascending, and keep `SoundtrackSongsResult` as a distinct empty result.
  Batch candidate loading across the owner chain and apply the target user's normal library policy.
- Coordinate remote-image downloads by URL so concurrent items share one bounded download, and cap leader downloads across distinct URLs at four so a media wall cannot multiply the per-image buffer without bound. Acquire the global permit inside the single-flight initializer so same-URL followers consume no additional permits and cancellation promptly releases capacity. Validate that upstream content is an image, and remove or otherwise suppress permanently invalid remote references according to official behavior.
- Allow administrator RemoteImages downloads from an explicit `imageUrl` without requiring the
  corresponding search provider or a TMDb API key. Advertise only remote-image providers whose
  search implementation can actually return images for that item; provider selection must never
  lead an SDK into a permanently empty provider that the server only implements for metadata.
- Keep Emby remote-image downloads on their generated required JSON-body contract. Bind `Type`,
  `ProviderName`, `ImageUrl`, and nullable signed `ImageIndex` case-insensitively with
  last-duplicate-wins semantics; pass `ImageIndex` as the destination image ordinal while
  preserving explicit-URL downloads, administrator/API-key authorization, and the shared
  single-flight/four-download cap. Jellyfin root and `/api` remain query-only with optional bodies.
- Expose TMDb Person profile artwork as the item's `Primary` remote-image type, matching the
  official Person image provider; never advertise or map it as the user-only `Profile` type. When
  `IncludeAllLanguages` is false and a preferred metadata language is nonblank, retain that language,
  English, and language-neutral images before sorting; a blank preference must not filter languages.
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
- Return remote subtitle previews as the provider's original bytes with the MIME inferred from the
  provider format. Remote subtitle downloads must reuse the external-subtitle atomic persistence
  path and register the resulting Subtitle stream; isolate provider/download failures as the
  official 204 response does. Keep search, download, and preview routes reachable through fully
  lowercase static aliases, and populate Episode subtitle searches with the persisted or
  relational Series name without a per-item query.
- Keep external-subtitle deletion administrator-only like the official `RequiresElevation` action;
  `EnableSubtitleManagement` is sufficient for search, download, and upload but not deletion.
  Preserve elevated API-key access and the fully lowercase delete/upload route aliases.
- Enforce the official named `Download`, `SubtitleManagement`, and `LyricManagement` policies in
  route middleware before Axum extracts path, query, or body values. API keys have global
  permission; device sessions must first pass the normal remote-access and parental-schedule checks,
  then the corresponding user-policy flag. Keep canonical and fully lowercase routes equivalent,
  including provider preview routes; malformed inputs from a disallowed user remain 403 rather
  than leaking binding precedence as 400. Handlers must preserve that API-key permission instead of
  requiring a device session again; resolve item-scoped subtitle and lyric searches through a typed,
  unrestricted item lookup for API keys while retaining normal user-library policy for devices.
- Project intros, local trailers, special features, and video additional parts with the official
  default all-fields DTO options through one batched projector. Apply the target user's policy to
  both the requested owner and every resolved child before returning the original response shape.
- Project `/Items/{itemId}/Ancestors` nearest-first with the official default all-fields
  `DtoOptions`, including images, target-user data, media sources and streams, chapters, and
  trickplay. Reuse the shared bounded page projector for the resolved ancestor set rather than
  issuing one DTO lookup per parent or returning the minimal item shape.
- Resolve both `/Items/{itemId}/Ancestors` and `/Items/{itemId}/Collections` through the target
  user's normal library policy, then filter related rows in a set-based lookup. Collections must
  calculate its total after policy filtering and before signed pagination, bind repeated/case-
  insensitive `Fields`, and project the bounded page with the shared batched DTO projector.
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
- Build RemoteImages search and provider lists from the authorized item's persisted item-type
  `MetadataOptions`. Match the official `IncludeDisabledProviders=true` behavior: disabled image
  fetchers remain visible/searchable, `ImageFetcherOrder` changes ordering only, and configured
  order must not become an allow-list that drops unlisted providers. Reuse the already resolved item
  and keep canonical/lowercase routes and item-not-found precedence equivalent.
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
- Match `Library/Series/Updated` and `Library/Movies/Updated` external-source reports: select
  Series by TVDB (including the official omitted-id/no-TVDB case), select Movies by nonblank IMDb
  before TMDB with case-insensitive provider-id comparison, and feed a matching report into the
  bounded library-scan fallback once rather than silently accepting it as a no-op.
- Do not decode, resize, reformat, decorate, or otherwise transform images requested by API
  clients. Keep accepting the official image query surface for compatibility, but stream the
  original image bytes and content type so media-library browsing cannot create decoder-sized
  memory spikes or a family of derived cache files. Preserve the official MIME types for every
  accepted image extension, including APNG, AVIF, ICO (`image/x-icon`), TIFF, and Jellyfin TBN
  JPEG files, without inspecting or decoding their contents. Image-info endpoints must return
  persisted dimensions and BlurHash values without lazily decoding the source or writing metadata.
- During Photo scans, read embedded metadata without decoding pixels and keep EXIF/TIFF buffering
  bounded to 1 MiB. Support JPEG, TIFF/CR2, PNG `eXIf`, and WebP `EXIF`; malformed metadata must not
  drop an otherwise valid Photo, while filesystem read failures retain normal scan-failure
  semantics. Persist and project official Photo camera, exposure, location, orientation, and date
  fields only on Photo DTOs, accepting historical PascalCase, camelCase, and snake_case JSON keys.
  Recognize CR2 and AVIF as Photo extensions even when a format's EXIF container is not parsed.
- Keep image-route static segments and compound query names compatible with ASP.NET's
  case-insensitive binding, including representative all-lowercase legacy requests. Ordinary item,
  user, branding, by-name, and plugin image responses must not advertise byte ranges unless the
  handler actually implements Range semantics; trickplay tile routes remain the range-aware
  exception.
- Keep both public branding CSS routes reachable through fully lowercase static-path aliases;
  `/branding/css` and `/branding/css.css` must preserve the canonical content type and empty-body
  behavior used by mobile and web clients.
- Keep non-Live-TV Channels routes reachable through fully lowercase static-path aliases and
  bind their compound query names (`StartIndex`, `FolderId`, `ChannelIds`, sort and capability
  options) case-insensitively, matching the official ASP.NET binder and legacy SDK traffic.
  Channel Items and Latest Items must pass SDK `Fields` through the shared batched DTO projector
  rather than accepting them and returning the fixed minimal item shape.
- Keep the three Channels list APIs on their official signed `Int32` pagination contracts.
  `/Channels` uses the `ChannelManager` list-range behavior: non-positive limits are unlimited,
  while negative or past-end start indexes fail with a server error. Channel Items and Latest
  Items use repository paging: non-positive start indexes skip nothing but retain the signed value
  in `StartIndex`, `Limit=0` is empty, and a negative limit is unlimited. Normalize only the
  PostgreSQL offset/limit inputs; values outside `Int32` must fail binding on canonical and fully
  lowercase routes.
- Bind image `ImageType` and `ImageFormat` parameters from case-insensitive official names or their
  defined integer values. Reject unknown names and integer values as bad requests before resource
  lookup, including legacy user-image route parameters whose controller action otherwise ignores
  the value.
- Bind the legacy indexed user-profile-image upload, canonical DELETE, and POST `/Delete` mutation
  path `Index` as a signed `Int32` on Jellyfin root, `/api`, and `/emby`. The handler ignores every
  in-range value, including negatives and both boundaries; reject only values outside `Int32`
  without changing image-type, authorization, target, or body-validation precedence. Preserve root
  and `/api` empty 204 responses, `/emby` empty 200 responses, and idempotent deletion.
- Stream trickplay tile files with bounded chunks and preserve HEAD and byte-range semantics; never
  read an entire tile into a response buffer.
- Keep playback-info route static segments compatible with ASP.NET's case-insensitive routing:
  register both `/Items/{itemId}/PlaybackInfo` and `/items/{itemId}/playbackinfo` (including GET
  and POST) so generated Android and Swift SDK requests never depend on URL casing.
- Keep the generated Emby GET `/emby/Items/{itemId}/PlaybackInfo` contract protocol-local: require
  its case-insensitive `UserId` query before item lookup, while root and `/api` Jellyfin GET routes
  retain their authenticated-session user fallback. POST continues to use its query/body contract.
- Keep the authenticated bitrate-test route available as `/playback/bitratetest`, with the same
  bounded payload, query binding, and error semantics as `/Playback/BitrateTest`.
- Keep Open/Close LiveStreams available through fully lowercase static-path aliases; these are
  ordinary playback routes, not Live TV. Bind the complete Open query/body surface in
  PascalCase, camelCase, and representative lowercase form, with query values taking precedence,
  and pass device profile, bitrate, stream-selection, channel, seek, and direct-play options into
  the existing playback stream builder rather than silently ignoring them. API keys may Open only
  with an explicit valid target user and may Close without a device session. Scope every opened
  stream to its authorized item, keep the registry bounded and lazily expire abandoned entries, and
  validate that scope in PlaybackInfo, progressive Audio/Video, and HLS lookups. Include live-stream,
  device, and play-session identity in HLS job keys so one session cannot reuse or stop another's
  transcode. Close remains idempotent for unknown identifiers, including whitespace-only values.
- Keep video version merging and alternate-source deletion on the official `RequiresElevation`
  policy through canonical and fully lowercase routes. Elevated API keys are administrator
  equivalents for both mutations and must not be rejected by a device-session-only handler. For
  alternate-source deletion, return 404 for both missing and non-Video targets like the official
  typed item lookup.
- Cover the mobile browse bootstrap routes with fully lowercase aliases as well: public/system
  info, branding configuration, users and user views, devices, display preferences, sessions,
  modern and legacy item latest/counts/resume routes, and library available-options routes must
  preserve the official handler and authorization policy under lowercase static segments.
- Keep Environment, Localization, FallbackFont, UTC/Ping, ActivityLog, Logs, Endpoint, Restart,
  Shutdown, ClientLog, System Configuration, and ScheduledTasks reachable through fully lowercase
  aliases. Preserve first-time setup access for Environment and Localization, public UTC/Ping,
  LocalOrElevated restart, and Elevated log/shutdown/task/configuration mutations; adding an Axum
  alias without its canonical authorization policy is a security regression.
- Persist `ServerConfiguration.MetadataOptions` as a constrained PostgreSQL JSON array seeded with
  the official nine item-type defaults. Configuration GET/POST must round-trip the typed options,
  accept PascalCase, camelCase, and fully lowercase top-level and nested properties, omit a null or
  empty `ItemType` on output, and replace the array atomically. Typed remote search must read locale
  and the matching item-type provider disable/order rules from the same configuration snapshot.
- When typed remote search supplies `ItemId`, resolve the item and its own or nearest containing
  virtual library in one bounded PostgreSQL lookup. A matching `TypeOptions.MetadataFetchers` is an
  allow-list even when empty, and its `MetadataFetcherOrder` overrides the global order even when
  empty. `IncludeDisabledProviders` bypasses enablement filtering, while `SearchProviderName` only
  narrows the already enabled set; a missing reference item uses the official dummy/default options
  rather than turning the search into a 404.
- Bind Users `IsHidden`/`IsDisabled`, ScheduledTasks `IsHidden`/`IsEnabled`, and item ContentType
  `ContentType` with PascalCase, SDK camelCase, and fully lowercase query names. Fully lowercase
  values must retain the canonical filtering, mutation, and malformed-value semantics.
- Keep ActivityLog pagination on the official signed `Int32` contract: a negative `StartIndex`
  skips nothing but is echoed in `StartIndex`, a negative `Limit` follows the official SQLite
  unlimited-limit behavior, zero returns an empty page, and out-of-range values fail binding for
  canonical and fully lowercase routes.
- Bind ActivityLog `SortBy` and `SortOrder` as the official enum arrays. Accept comma-delimited and
  repeated query keys from Android and Swift, case-insensitive names, and defined integers while
  preserving field/direction order; invalid elements fail binding instead of silently selecting a
  different default.
- Keep ActivityLog response severity adaptation protocol-local. Under `/emby`, map Jellyfin's
  `Information`, `Warning`, and `Critical` names to Emby's generated `Info`, `Warn`, and `Fatal`,
  preserve `Debug` and `Error`, and omit `Trace` and `None` because Emby's nullable closed enum has
  no corresponding values. Keep Jellyfin root and `/api` severity names unchanged.
- Keep `/Users/Public` available as `/users/public`; otherwise Axum's dynamic `/users/{id}` route
  treats the SDK's lowercase public-user request as a UUID binding failure.
- Normalize canonical, lowercase, and mixed-case `/emby/Users/Public` requests inside the Emby
  route tree before authorization and shared-fallback dispatch. All three forms must remain
  anonymously accessible and return the raw SDK-decodable `UserDto[]`; do not broaden or otherwise
  change the unprefixed Jellyfin route while adding this Emby compatibility adapter.
- Persist Emby typed user settings as opaque bytes in a protocol-owned display-preference record.
  Preserve the body exactly through POST/GET, isolate keys and target users, and return an empty
  octet stream for an unset key. Authorize and resolve the target user before inspecting a rejected
  body, keep dynamic key casing intact while matching static path segments case-insensitively, and
  never register the typed-settings route on the unprefixed Jellyfin tree.
- Keep login case-insensitive through both static segments: `/users/authenticatebyname` must retain
  the canonical route's public authorization policy as well as its handler.
- Keep `/emby/LiveStreams/Close` on the generated Android and Swift two-query contract: bind
  `LiveStreamId` and `PlaySessionId` case-insensitively with last-duplicate-wins semantics and
  require both to be nonempty, while leaving Jellyfin's root and `/api` one-query behavior intact.
- Require a nonempty, non-nil `UserId` on `/emby/Shows/NextUp`, bind every generated scalar query
  case-insensitively with last-duplicate-wins semantics, and preserve repeated `Fields` and
  `EnableImageTypes`. Delegate to the shared policy-aware NextUp implementation without imposing
  this legacy requirement on Jellyfin's root or `/api` routes.
- Keep Startup, external library-update reports, elevated Person remote search, and elevated remote
  search Apply reachable through fully lowercase static aliases. Bind their JSON properties and
  compound query names in PascalCase, camelCase, and representative lowercase form; lowercase
  Person/Apply aliases must retain `RequiresElevation`, and authorization must precede malformed
  body or missing-item validation.
- Keep the complete ItemLookup SDK surface reachable through fully lowercase aliases: all typed
  RemoteSearch routes plus per-item MetadataEditor and ExternalIdInfos. The per-item routes remain
  elevated, while non-Person searches retain their ordinary authenticated policy.
- Keep the mobile authentication helpers fully lowercase too: auth providers, password-reset
  providers, API-key CRUD, forgot-password/PIN, and user-view grouping options must reuse the
  canonical Public, Elevated, or default authorization policy rather than falling through to a
  different middleware default.
- Keep the administrator-only Backup surface reachable through `/backup`, `/backup/create`,
  `/backup/manifest`, and `/backup/restore`, preserving the canonical handlers, query binding,
  and elevated authorization. Treat an omitted or JSON-null Create body as the default backup
  options, while rejecting malformed JSON and wrong JSON types as bad requests. Bind backup JSON
  properties and manifest query names case-insensitively, with last duplicate properties winning.
- Create PostgreSQL backups from one read-only repeatable-read snapshot and stream table rows into
  the archive rather than collecting the database in memory. Restore before migrations and HTTP
  startup in one serializable database transaction, validate the complete table/constraint shape,
  reset sequences, and reject traversal, duplicate, or symbolic-link ZIP entries. Stage files and
  replace them with same-directory atomic renames; never expose absolute server paths in the public
  manifest or accept a restore archive outside the configured backup directory.
- Treat valid API keys as administrators for user creation, deletion, profile/configuration updates,
  password changes, and profile-image upload/deletion through modern and legacy routes. An omitted
  or nil target for an API key's profile/configuration/password/image update remains a 404; ordinary
  user mutations still require self
  access and `EnableUserPreferenceAccess`. Keep target lookup before those preference checks,
  preserve password-change token revocation and
  reset-without-revocation behavior, and retain equivalent lowercase route authorization.
- Bind every top-level `UserPolicy` update property case-insensitively like ASP.NET JSON input,
  preserving the official last-duplicate-wins behavior and ignoring unknown properties. Do not let
  camelCase, lowercase, or mixed-case SDK payloads silently reset submitted policy values to defaults.
- Bind every top-level shared `UserConfiguration` update property case-insensitively as well. Keep
  the official constructor defaults for omitted fields, let the last case-insensitive duplicate win,
  ignore unknown properties, and preserve PascalCase output for both modern and legacy user routes.
- Keep `AccessSchedule` entries Kotlin-decodable: emit signed `Int32` `Id` and the owning compact
  `UserId` in addition to day/start/end fields. Backfill the owning user during DTO projection for
  historical policy JSON that predates those identity fields.
- Keep public mobile DTO counts on the official signed `Int32` wire contract, including generic and
  item query results, theme media, activity/search totals, aggregate item counts, media-source count,
  unplayed count, and `DeviceOptions.Id`. Use checked conversions and checked aggregation; never
  saturate, truncate, or silently omit an out-of-range value that Kotlin cannot decode.
- Keep lower-case aliases for item details, root/counts, suggestions, themes, collections,
  intros/special features, show pages, InstantMix, search hints, trailers, and video additional
  parts on the same handler and authorization contract as their canonical routes.
- Keep every SyncPlay route reachable through a fully lowercase static-path alias, including
  queue, playback-state, membership, ping, and dynamic group-detail routes; aliases must reuse the
  canonical handlers and authenticated policy.
- Keep collection creation and membership mutation reachable as `/collections` and
  `/collections/{collectionId}/items`; bind compound query names such as `ParentId` and `IsLocked`
  case-insensitively and retain the canonical collection-management authorization.
- Keep Android and Swift user-data routes case-insensitive too: `UserItems` user-data and rating,
  resume, `UserFavoriteItems`, `UserPlayedItems`, and legacy user item-data routes need fully
  lowercase aliases with the same authorization and mutation semantics.
- Keep Emby remembered-track clearing isolated below `/emby`: DELETE and legacy POST `/Delete`
  aliases must clear only the selected Audio or Subtitle index for every target-user row in one
  set-based PostgreSQL update. Ordinary users may target only themselves, while administrators and
  API keys may target another existing user; preserve target authorization/404 precedence over an
  invalid case-insensitive `TrackType`, return 200 on success, and do not expose either alias from
  the Jellyfin root router.
- Keep Emby's legacy `POST /emby/Videos/{Id}/AlternateSources/Delete` administrator-only and return
  200 after applying the real alternate-version unlink operation. Preserve typed-video 404 behavior,
  media rows and metadata while removing both linked-child and primary-version relationships; do
  not expose the POST alias from Jellyfin's root router or change its existing DELETE route's 204.
- Keep Emby CriticReviews and ThumbnailSet isolated below `/emby` and resolve the requested item
  through the caller's normal library policy before responding. With no critic-review persistence,
  return the SDK's real empty `QueryResult<BaseItemDto>` shape; do not reinterpret Jellyfin composite
  trickplay sprites as Emby's individually tagged thumbnails, and return 404 when no truthful
  ThumbnailSet mapping exists. Bind their signed Int32 paging/Width query values case-insensitively,
  and keep both paths absent from the Jellyfin root router.
- Bind target `UserId` as `userId`, `UserId`, and fully lowercase `userid` on UserData, Rating, and
  DisplayPreferences operations; bind DisplayPreferences `ItemId` equivalently. A lowercase target
  id must never be ignored and silently redirected to the authenticated user: foreign targets keep
  their 403 authorization result and malformed UUIDs keep their 400 binding result.
- Keep legacy `/Users/{userId}/Items/Root`, Intros, LocalTrailers, SpecialFeatures, and Lyrics,
  plus legacy FavoriteItems, PlayedItems, and Rating mutations, reachable through fully lowercase
  paths with the same target-user checks and response shapes.
- Project `CanDelete` only when requested, except on official default all-fields item and root
  details. For user-less pages expose only the item's intrinsic capability; for user pages combine
  it with the target user's global or CollectionFolder-scoped deletion policy in one batched
  hierarchy lookup. Preserve the official single-item Playlist owner/administrator override and
  the BoxSet collection-management authorization, while keeping batched Playlist DTOs on the normal
  intrinsic-plus-policy wrapper.
- Project Series `AirTime` and Series/BoxSet `DisplayOrder` directly from persisted metadata without
  an `ItemFields` gate. Project `CumulativeRunTimeTicks` only for folders when the field is requested
  by case-insensitive name, integer value `7`, or the default all-fields detail contract; never expose
  it for non-folder media even when `RunTimeTicks` is present.
- Project `IsHD` only when `ItemFields.IsHD` is requested (including default all-fields item
  details), and only emit it when the persisted item height is at least 720, matching the official
  legacy compatibility behavior. Preserve the uppercase acronym in the wire key, accept the field
  name case-insensitively and its defined integer value `47`, and never serialize it as `IsHd`.

## Compatibility expectations

- Match official Jellyfin DTO field names, nullability, defaults, HTTP status codes, authorization requirements, sorting, pagination, and case-insensitive matching.
- ASP.NET route, query-name, and JSON-property binding is case-insensitive. Compatibility tests must cover PascalCase, camelCase, and representative lowercase legacy requests; do not assume an Axum route or Serde field is equivalent merely because one casing works.
- Keep both the modern `/Items/Suggestions` route and legacy `/Users/{userId}/Suggestions`
  route reachable through fully lowercase aliases, with equivalent authorization and filtered
  results.
- Bind `/Items/Filters` and `/Items/Filters2` collection queries with the official comma-delimited
  collection model binder. A single value may contain commas, while repeated keys emitted by the
  Kotlin SDK must preserve every value; accept fully lowercase compound query names as well.
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
- After `/Years` resolves and applies a target user's library policy, use the already-authorized
  listing path rather than reloading that user. Keep the public service entry point validating its
  target user so this request-local optimization cannot weaken callers that have not applied policy.
- Preserve the `/Years` recursive-folder total-count quirk: report the number of policy-visible,
  filtered primary descendants before extracting distinct positive production years. For a
  non-recursive folder or a non-folder parent, report the distinct-year count instead.
- Project `/Years` and `/Studios` through their persisted item-by-name rows and the shared batched
  DTO projector. Honor `Fields`, `EnableImages`, `EnableUserData`, `ImageTypeLimit`, and
  `EnableImageTypes` with case-insensitive query binding while preserving endpoint-specific totals,
  ordering, and item-count overlays.
- Keep `/Persons` pagination signed as well, but preserve its different limit rule: a non-positive
  `Limit` is unlimited, while a non-positive `StartIndex` skips nothing and is still echoed.
- Project `/Persons` DTO image options with the shared official collection semantics: when images
  remain enabled, `ImageTypeLimit=0` or selectors that exclude every local single image still emit
  an empty `ImageTags` object. Emit an empty `BackdropImageTags` array when Backdrop is selected but
  absent, and omit these collections only when their corresponding projection is disabled.
- Keep item-by-name pagination such as `/Genres`, `/MusicGenres`, and `/Studios` signed: a negative
  `StartIndex` skips nothing but is echoed, `Limit=0` is empty, and a negative `Limit` follows the
  official SQLite unlimited-limit behavior. When `EnableTotalRecordCount` is false, return zero
  rather than the current page length.
- Keep `/Artists` and `/Artists/AlbumArtists` on the same signed `Int32` item-by-name pagination
  contract: a negative `StartIndex` skips nothing but is echoed, `Limit=0` is empty, a negative
  `Limit` is unlimited, and out-of-range query values fail binding for every supported casing.
- Project `/Genres` image options through persisted item-by-name rows while keeping user-data
  disabled as the official controller does. Project `/Artists` and `/Artists/AlbumArtists` with
  their image and user-data DTO options, resolving existing backing rows in one batch without
  creating rows during a filtered read; preserve the minimal fallback for legacy missing rows.
- Keep the modern and legacy `/Items` and Resume pages on their signed `Int32` pagination contract
  used by Android and Swift: negative `StartIndex` skips nothing but is echoed, `Limit=0` is empty,
  negative `Limit` is unlimited, and out-of-range values fail query binding.
- Bind every typed enum collection on the modern and legacy `/Items` pages with the official model
  binder: accept case-insensitive names and defined integer values, discard invalid elements, split
  commas only for a single query value, and do not re-split comma-containing repeated values. This
  includes item/media/location/image/video types, filters, fields, Series status, sort fields, and
  sort order. Authenticate before returning query binding errors.
- Keep modern and legacy `/Items/Latest` limits signed as well. A zero limit returns an empty array;
  a negative limit preserves the official repository/controller quirk and returns at most the first
  latest result rather than failing query binding. Values outside `Int32` must return 400 across
  canonical and lowercase route/query casing.
- Keep `/Playlists/{playlistId}/Items` on the official signed `Int32` contract used by Android and
  Swift: negative `StartIndex` skips nothing but is echoed, while a non-positive `Limit` returns an
  empty page through the controller's `Enumerable.Take` behavior; out-of-range values fail binding.
- Keep `/Items/{itemId}/Collections` on its official signed `Int32` collection-list contract:
  negative `StartIndex` skips nothing but is echoed, every non-positive `Limit` produces an empty
  page through `Enumerable.Take`, and out-of-range values fail binding across supported casing.
- Bind collection create/add/remove `Ids` with the official comma-delimited collection model
  binder. Preserve every repeated query key emitted by Android and Swift while still accepting a
  single comma-delimited value; keep canonical and lowercase routes equivalent.
- Honor `/Playlists/{playlistId}/Items` DTO options exactly like the official controller: bind
  case-insensitive `EnableImages`, `EnableUserData`, `ImageTypeLimit`, and `EnableImageTypes`, then
  pass them through the shared batched projector instead of silently ignoring SDK query values.
- Keep every PlaylistApi operation reachable through fully lowercase static aliases, including
  create, detail/update, users, item membership, move, and InstantMix. Bind compound creation and
  mutation query names such as `UserId`, `MediaType`, and `EntryIds` case-insensitively while
  reusing the canonical handlers and authorization checks.
- Bind `UpdatePlaylistUserDto.CanEdit` case-insensitively, ignore unknown JSON properties, and let
  the last case-insensitive duplicate win. A casing mismatch must not silently turn an editable
  playlist share into a read-only one.
- Keep `/Items/{itemId}/RemoteImages` on the official signed `Int32` paging contract used by Android
  and Swift: negative `StartIndex` skips nothing, non-positive `Limit` returns an empty image page,
  and values outside `Int32` fail binding before lookup work starts.
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
- Bind virtual-folder creation `Paths` with the official comma-delimited collection model binder:
  split a single comma-delimited value, but preserve every repeated query key emitted by the Kotlin
  SDK as one path. Never deserialize a collection-valued `paths` query into a scalar.
- Keep the complete LibraryStructure surface and `/Library/Refresh` reachable through fully
  lowercase aliases with the same first-time-setup-or-elevated and elevated policies. Bind virtual
  folder/media-path query names and top-level JSON DTO properties in PascalCase, camelCase, and
  representative lowercase forms while preserving the canonical handlers and methods.
- Keep the administrator-only `/Library/PhysicalPaths` SDK bootstrap request reachable as
  `/library/physicalpaths`, returning the identical string array for administrator sessions and API
  keys while retaining 401/403 behavior for anonymous and ordinary-user requests.
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
- Keep `/Search/Hints` on its official signed `Int32` pagination contract used by both mobile SDKs:
  negative `StartIndex` skips nothing, non-positive `Limit` returns the default empty result with a
  zero total, and values outside `Int32` fail binding. Apply the limit before candidate counting,
  rather than returning an empty page with an unbounded total.
- Project media SearchHint artwork for the bounded page with one ancestor-closure lookup and one
  batched DTO-image load. Primary never inherits and its aspect ratio exists only with the item's
  own Primary image; Thumb and Backdrop prefer the item's own image and expose the actual owner id.
  Episode Thumb prefers Series even when Season also has one, while Backdrop and ordinary-item
  inheritance use the nearest ancestor. Use persisted image dimensions without decoding files.
- Bind Search Hints `IncludeItemTypes`, `ExcludeItemTypes`, and `MediaTypes` with the official
  collection model binder: accept case-insensitive enum names and defined integers, discard invalid
  elements, split commas only for a single query value, and do not re-split comma-containing values
  when the SDK sends repeated keys. Authenticate before surfacing malformed query errors.
- Hydrate every persisted base item through the shared item-type registry before DTO projection,
  including playlist entries, so legacy CLR names never escape through `BaseItemDto.Type` and an
  unknown plugin row cannot make a client reject the enclosing page.
- Keep library-creation `CollectionTypeOptions` distinct from `BaseItemDto.CollectionType`: `mixed`
  is valid for a virtual-folder configuration but must be omitted from user-view item DTOs because
  the client DTO enum cannot decode it.
- Bind `/Libraries/AvailableOptions` nullable `LibraryContentType` with the official enum binder:
  accept case-insensitive names and every defined integer value, while invalid or undefined values
  fall back to the omitted/default behavior instead of failing the request. Bind `/UserViews`
  `PresetViews` with the official collection binder, discarding invalid elements and preserving its
  single-value comma versus repeated-key semantics; authenticate before malformed query errors.
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
- Project Audio `AlbumPrimaryImageTag`, `NormalizationGain`, and `AlbumNormalizationGain`
  unconditionally like the official DTO service. Prefer `LUFS` with the ReplayGain 2.0
  `-18 - LUFS` calculation over a persisted normalization gain, resolve every page's album rows and
  Primary image metadata in bounded batches, and merge an album image's persisted BlurHash into
  `ImageBlurHashes.Primary` without replacing hashes for the audio item's own or inherited images.
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
- Keep progressive Video stream query binding aligned with `VideosController`: accept the full
  case-insensitive request surface, including the query-only `container` fallback on extensionless
  stream URLs, and cover PascalCase, camelCase, and lowercase SDK requests in focused tests.
- Keep progressive Audio and Video numeric query fields on the official signed `Int32` contract,
  except `StartTimeTicks`, which is signed `Int64`. Validate container, codec, and level inputs with
  the official patterns; bind nullable `SubtitleDeliveryMethod` and `EncodingContext` from
  case-insensitive names or defined integers. Parse the legacy semicolon-delimited `params` after
  explicit query values so supported slots override them, including shared Device/MediaSource,
  bitrate, seek, session, live-stream, tag, subtitle-codec, and transcode-reason slots.
- Apply progressive Video `CpuCoreLimit` to FFmpeg `-threads`: omit the option when absent, map a
  non-positive value to automatic thread selection, and clamp a positive value to the server's
  available processor count like the official encoding helper.
- Apply progressive Audio `CpuCoreLimit` with that same rule. Honor Video
  `EnableMpegtsM2TsMode`, and emit fragmented MP4 options only for the default Streaming context;
  an explicit Static Video context must produce a normal non-fragmented MP4 output.
- Do not treat accepted progressive `PlaySessionId` and `DeviceId` parameters as inert. Register
  audio and video progressive transcodes in the shared job registry with cancellation-safe process
  cleanup so playstate ping/stop and play-method normalization can identify them; remove the job on
  every completion, failure, and client-disconnect path.
- Keep progressive and HLS stop matching aligned with the official manager: a nonblank
  `PlaySessionId` matches jobs case-insensitively without requiring the submitted `DeviceId`; only
  a missing session id falls back to case-insensitive device matching. Removing a job must also
  prune every empty playback-session reverse-index entry, and progressive `HEAD` must neither
  spawn `FFmpeg` nor register a job.
- Keep Video stream authorization aligned with the official default policy: device sessions resolve
  media through their user's library policy, while a valid API key is unrestricted but still must
  resolve an existing supported video item before either static local serving or remote proxying.
- Preserve the official relationship order when expanding alternate `MediaSources`: keep the
  explicitly requested source first, emit every primary or user-linked grouping root before local
  alternates, sort linked roots stably by non-empty `SortName` with link `sort_order` as the
  fallback, and sort local alternates by their root and persisted `sort_order`. Use UUID ordering
  only as a deterministic fallback for legacy rows without a relationship, and compute the order
  in the set-based version query rather than loading links per item.
- Resolve subtitle-stream, subtitle-playlist, and attachment route `MediaSourceId` values inside the requested item's authorized alternate-version group before reading runtime, streams, attachments, or files. Never serve a same-index stream from the displayed primary for an alternate source, and reject malformed or unrelated source ids with 404.
- Preserve scan-discovered local alternate versions in resolver input order. Batch assignment must ignore missing and self-referential pairs before applying first-valid-assignment wins, persist a contiguous per-primary `sort_order`, compact both sides when a child moves between primaries, and remain idempotent on repeated scans. Serialize competing reassignment transactions before taking row locks so opposing moves cannot deadlock, and report parents whose DTO-visible version order changed even when no child's `primary_version_id` changed.
- Apply the playback `DeviceProfile` independently to every returned `MediaSource`, preserving source order and producing version-specific flags and URLs. Only apply explicit audio or subtitle indexes to the source whose id matches an explicitly requested `MediaSourceId`.
- After applying a playback `DeviceProfile`, project the selected subtitle index back to that source's
  `DefaultSubtitleStreamIndex`, including `-1` when subtitles are disabled; never leak an explicit
  index to another version when no matching `MediaSourceId` was selected.
- After applying a playback `DeviceProfile`, match the official `SetDeviceSpecificSubtitleInfo`
  behavior: populate every subtitle stream's `DeliveryMethod`, expose API subtitle URLs (or a
  direct HTTP(S) URL when the persisted external stream is already in the selected format), and
  append `ApiKey` to API URLs. Populate every selected media source attachment's
  `/Videos/{itemId}/{mediaSourceId}/Attachments/{index}` `DeliveryUrl` as well.
- Match the official post-selection capability calculation: a successful `DirectPlay` source may
  also expose `SupportsDirectStream`, and `SupportsTranscoding` reflects an enabled compatible
  device transcoding profile (or an existing transcoding container), even when the current
  request selected DirectPlay and therefore has no `TranscodingUrl`.
- Bind audio stream `MediaSourceId` case-insensitively and resolve it inside the requested item's
  authorized alternate-version group before serving static audio bytes, matching the video
  route and the Android SDK's alternate-audio playback contract.
- Keep the Android audio stream route's progressive contract: omitted or false `static` requests
  must honor `audioCodec`, bitrate, sample-rate, channel-count, and `startTimeTicks` through the
  bounded FFmpeg path, while explicit `static=true` serves the selected source unchanged.
- Treat the Audio stream route suffix or query `container` as the requested progressive output
  container, with the route suffix taking precedence and a compatible codec inferred when none is
  explicit. Do not reject a non-static FLAC-to-MP3 request because the source extension differs;
  retain source-container validation only for explicit static streaming.
- Project each media source's persisted total bitrate, and when it is absent infer it from that
  source's non-external media streams as official Jellyfin does. Keep this per-version so item
  details and `PlaybackInfo` never reuse the displayed primary's bitrate for alternate versions.
- For profiled playback, generate `TranscodingUrl` for HTTP progressive as well as HLS
  transcoding selections. Match the official rewrite of non-DirectPlay selections to
  `PlayMethod.Transcode`, including Android audio profiles that consume `/audio/{id}/stream`.
- Preserve Android audio `AudioStreamIndex` and `TranscodingMaxAudioChannels` query values when
  serving progressive audio; the selected media stream must reach FFmpeg instead of silently
  falling back to the first audio stream.
- Match official audio encoding defaults across progressive and HLS requests: request
  `CpuCoreLimit` overrides configured threads, `EnableAudioVbrEncoding` defaults true, codec-aware
  channel/source/encoder caps apply before bitrate derivation, and segmented 3/4/5/7-channel layouts
  normalize to 2/2/6/8. Copy and lossless output must not receive lossy bitrate or VBR arguments.
  Parse codec-qualified `StreamOptions` from official lower-camel keys and the generated Kotlin and
  Swift dictionary encodings, with empty qualified values falling back to unqualified options.
- Keep video stream routes aligned with the official progressive contract: omitted or false
  `static` requests must transcode with the requested video/audio codecs, stream indexes,
  bitrate, dimensions, and start position; explicit `static=true` remains byte-for-byte static
  playback with remote Range forwarding.
- Accept the exact official URL spellings emitted by `StreamInfo.to_url`, including
  `VideoBitrate`, `AudioBitrate`, and Android's `videoBitrate`/`audioBitrate`, as well as legacy
  `VideoBitRate`/`AudioBitRate`; do not rely on case-insensitive matching to bridge the different
  internal capitalisation. Apply the same spellings to dynamic HLS bitrate parameters.
- Bind video `SubtitleStreamIndex` and `SubtitleMethod` on progressive stream routes. Match the
  official default `Encode` behavior by burning the selected local subtitle stream into video;
  external text subtitle files must be filtered by their own officially escaped path rather than an
  embedded subtitle ordinal, and seeked text burns must apply the official PTS correction. External
  graphical subtitles require a separate FFmpeg input (preferring a sibling `.idx` for VobSub),
  while DVB subtitles requested as `Embed` normalize to `Encode`. `Embed` must map the selected
  embedded or external input and apply the first requested subtitle codec (or copy an already
  matching codec); `External`, `Hls`, and `Drop` must not silently burn or mux it into the video.
- Enforce Audio and Video transcoding policies independently on progressive Video requests. Audio
  permission must never authorize video encoding, and video permission must never authorize audio
  encoding; stream copy remains allowed without the corresponding encoding permission.
- Bind the official video `MaxFramerate` query on progressive stream routes and apply it after
  any requested scaling, accepting PascalCase, camelCase, and lowercase spellings like the
  Android SDK and ASP.NET query binder.
- Bind progressive video `Width`/`Height` and normalize them to the same effective maximum
  dimensions as the official streaming helper; explicit `MaxWidth`/`MaxHeight` values take
  precedence when both forms are supplied.
- Preserve progressive `CopyTimestamps=true` semantics for Android seek requests by passing
  FFmpeg `-copyts -avoid_negative_ts disabled -start_at_zero` for both audio and video.
- Include every HLS stream-selection and video-rate input that changes FFmpeg output, including
  `AudioStreamIndex` and `MaxFramerate`, in the deterministic transcode job id so Android audio
  track switches and profile changes cannot reuse another rendition's segments.
- Treat `AudioStreamIndex` as the persisted global media-stream index in HLS FFmpeg mapping;
  explicit selections must use `-map 0:{index}`, matching the official encoder and progressive
  routes, while an omitted selection may still use the first-audio shorthand.
- Progressive audio/video stream responses must start FFmpeg and read the growing output file
  immediately, matching the official `ProgressiveFileStream` behavior; never await complete
  FFmpeg termination before returning the response, or Android playback of long media can stall.
- Match the official progressive MP4 muxing contract: video output uses
  `-f mp4 -movflags frag_keyframe+empty_moov+delay_moov`, while MP4-family audio output uses
  `-movflags empty_moov+delay_moov`, so Android can parse initialization metadata before the
  transcode finishes.
- Project `SupportsExternalStream` on every persisted media stream using the official rule: true
  for external streams and for text, PGS, or VobSub subtitles. Keep the value consistent between
  top-level streams and every single- or alternate-version media source.
- Explicit metadata refresh must repair missing or placeholder stream rows for the selected local
  file through the bounded media-probe pool, including alternate versions. Keep remote and `.strm`
  sources on the lazy playback probe path instead of opening upstream media during library browsing.
- Bind item-refresh query names case-insensitively and accept `MetadataRefreshMode` names without
  regard to case plus the defined integer values 0 through 3. Keep invalid or undefined modes as
  bad requests for authenticated callers, while authentication must precede query-binding errors.
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
- Project requested `LocalTrailerCount` and `SpecialFeatureCount` from relational extra children in
  bounded page batches, returning real zeroes when requested. Share video-version extras and merged
  Series extras like official owner resolution, count the complete official display-extra set, and
  keep both fields omitted when not requested. Project `PartCount` without an `ItemFields` gate only
  for Video items whose persisted `AdditionalParts` relationship is nonempty, using length plus one;
  never emit a placeholder one or leak it from similarly shaped metadata on non-Video items.
- Resolve each related-count batch's self, primary-version id, reverse alternate-version, and Series
  presentation-key owners through separate indexable set branches. Do not combine those owner paths
  into an `OR` join over all `base_items`; unbounded default-all-fields Suggestions pages amplify
  that scan once per bounded DTO batch even though their official nullable `Limit` must stay unbounded.
- Project `LocationType` for every non-Live-TV item from its persisted source and path: pathless
  Channel items and non-file URIs are Remote, pathless library items are Virtual, and ordinary paths
  plus file URIs are FileSystem. Project requested `EnableMediaSourceDisplay` as true for ordinary
  non-Channel items, but do not invent the ChannelManager-dependent value for Channel items.
- Project `IsPlaceHolder` only when a Video item's persisted metadata explicitly contains true; do
  not substitute `IsVirtualItem` or emit false. Project requested `DateLastMediaAdded` only for
  folders and only from persisted metadata. Resolve requested `SeriesStudio` for Episodes and
  Seasons with one batched parent-Series load, taking the first Studio from the parent's persisted
  JSON order rather than a normalized relation's sort order.
- Filter alternate `MediaSources` and their full-source `MediaSourceCount` by the target user's
  standalone item visibility before loading streams or attachments. Always retain the explicitly
  displayed source, while user-less global projections retain every source.
- Persist alternate-version relationships with a deterministic parent-local `sort_order`, repair
  legacy null orders under the PostgreSQL hierarchy lock, and preserve that order on repeated
  merges so playback groups remain stable across scans and Kotlin client deserialization.
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
  unlimited, and reject values outside the official `Int32` range. Preserve the filtered,
  pre-pagination `TotalRecordCount` even when `StartIndex` moves the returned page past its end.
  Honor the official
  `EnableImages`, `EnableUserData`, `ImageTypeLimit`, and `EnableImageTypes` DTO options through
  the shared batched projector rather than accepting and discarding SDK request parameters. Apply
  that same projector contract to the Episodes and Seasons TV routes, including lowercase SDK
  query names, so all three controller actions match `AddAdditionalDtoOptions`.
- Keep `Shows/Upcoming` on the official signed `Int32` pagination contract used by Android and
  Swift: a negative `StartIndex` skips nothing but is echoed, `Limit=0` is empty, a negative
  `Limit` is unlimited, and out-of-range values fail binding across supported query casing. Honor
  its `EnableImages`, `EnableUserData`, `ImageTypeLimit`, and `EnableImageTypes` with the shared
  batched DTO options instead of accepting and discarding mobile SDK parameters.
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
- Match the official `PlaybackInfo` fallback when no `DeviceProfile` is supplied: retain static direct-play capability for sources served by the implemented local-file (and video HTTP-proxy) routes, while leaving direct-stream and transcoding unavailable until a profile selects a real server route. Android clients may omit the profile after registration or on their first request.
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
- TMDb and OMDb Movie/Series scalar merges must honor `LockedFields.Name`, `Overview`, and
  `OfficialRating` both when filling gaps and when replacing metadata during a full refresh. A
  locked Name also preserves SortName, and remote blank or whitespace-only names must never erase
  an established title.
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
- Do not implement `ReplaceAllMetadata` as a direct switch on the current provider-by-provider
  writes. Match the official success-dependent replace decision: collect provider patches before
  persistence, preserve locked fields, replace missing metadata and normalized relations only
  after at least one remote provider succeeds, retain all existing metadata when every provider
  fails, and commit the final JSON, provider ids, studios, and people atomically.
- Lazy `.strm` probing must have a process-level deadline that terminates FFprobe before returning; an async timeout around an uncancelled blocking child is not sufficient because client retries can accumulate processes and memory.
- Coordinate lazy `.strm` probes by item and resolved target so concurrent playback requests share one bounded flight. Keep failure backoff state short-lived and hard-bounded so retries do not repeatedly pay the probe timeout or grow memory without limit.
- Recognize failed-probe placeholder streams semantically across nullable boolean persistence shapes, and inspect only embedded streams when deciding whether to retry. An external subtitle must not suppress a later successful media probe.
- Version successful local Audio/Video probes in `base_items.data` with a source fingerprint containing the normalized path, file size, and modification time. A missing, stale, malformed, or mismatched marker must trigger one bounded repair even when historical embedded streams already exist; write the marker only after all probed media information is persisted, and never replace existing streams, attachments, chapters, or item metadata when that schema-only probe fails. Keep failed local probes on a hard-bounded in-process backoff, while `.strm` and remote sources remain lazy playback probes.
- During episode refresh, merge a neighboring local NFO before remote metadata: preserve a non-empty established local title, but continue to treat a local title equal to the series or path-derived name as a replaceable placeholder; allow the first remote result to replace only such placeholders, and honor `LockedFields.Name` even for a full refresh.
- If TMDb returns an episode name equal to its series name in the preferred language, fetch the English episode metadata once and use only its non-placeholder name as a fallback. Preserve all localized non-name fields, and apply the same rule to direct episode refresh and bulk season refresh.
- Cancellation of scans and refreshes must promptly stop new work, release locks/permits, and leave the database in a consistent state.
- Filesystem watcher startup must skip unavailable or non-directory library roots and isolate each
  root's registration error. Successful recursive parent watches cover duplicate and child roots,
  while a child may still register if its parent's watch failed. Do not retain an idle watcher
  thread when no configured root was registered.

## Android playback compatibility

- For `Sessions/{sessionId}/Playing/{command}`, bind `ControllingUserId` case-insensitively as the
  official nullable string and forward it verbatim in `PlaystateRequest`. Keep the authenticated
  controller session id for authorization and command routing; do not synthesize the controlling
  user from the authenticated user's UUID when the query value was omitted.
- Apply the official session-control boundary before remote commands, viewing reports, additional-
  user mutations, and targeted capability updates. Allow a public target, its primary or additional
  users, callers with `EnableRemoteControlOfOtherUsers`, and privileged API-key contexts where that
  route accepts them; attaching a different user additionally requires an administrator. Never
  authorize a target merely because its session id exists.
- Bind full client-capability JSON case-insensitively with official enum names, integers, numeric
  strings, and array-or-comma-string capability collections. Keep `DeviceProfile` strongly typed so
  primitives fail binding and nested SDK casing/numeric values are normalized before persistence.
  Full capabilities bind only their official `Id` query and ignore unrelated query keys. Treat API
  keys as privileged user-less controllers for capabilities, general commands, and messages while
  retaining the request client's session id and a nil controlling user id.
- Keep Emby's generated session-capability contract protocol-local. Require its case-insensitive
  `Id`, bind query and top-level JSON duplicates with last-value-wins semantics, persist short-form
  `SupportsSync` and Full `PushToken`, `PushTokenType`, `SupportsSync`, and `AppId` in the device
  JSONB without teaching Jellyfin's shared DTO about them, and preserve omitted Emby-private fields
  across short, Full, and modern Jellyfin reports. Emby returns an empty 200; root and `/api` retain
  their optional session-id fallback, `SupportsPersistentIdentifier`, typed projection, and 204.
- Keep Emby Sessions `TranscodeReasons` decodable by its generated closed Swift enum. Translate
  Jellyfin's renamed external-audio and video-range reasons to their Emby names, omit newer
  Jellyfin-only reasons, and preserve the complete modern reason list on root and `/api` Sessions.
- Bind general-command and message JSON properties case-insensitively; accept official command enum
  names, integers, and numeric strings, and reject whitespace-only required message text. Apply the
  same enum rules to play and playstate query/path commands, preserve the collection binder's
  single-value versus repeated-key `ItemIds` behavior, and authenticate every command route before
  returning path, query, or body binding errors.
- Filter `/Sessions?ControllableByUserId=` through actual media-control capability and a connected
  controller, the caller's remote-control and device-access policy, and the controlled user's shared-
  device policy. A normal session list includes public and additional-user sessions; an explicit nil
  target follows `RequestHelpers.GetUserId`, and API keys retain the official privileged context.
- Persist playback progress against the authorized selected `MediaSourceId`, but when projecting a
  displayed primary item's `UserData`, fall back to the latest visible alternate-version row when
  the primary has no row. This keeps ISO and alternate playback resume positions visible on item
  details, lists, and Continue Watching without duplicating playstate rows.
- For Series and Season DTOs, populate `UserData.UnplayedItemCount` from one target-user-policy-
  aware recursive descendant count and played-descendant count; never leave this official folder
  field permanently null.
- Universal Audio must bind the complete generated SDK query surface, including `MediaSourceId`,
  `AudioBitRate`, `TranscodingAudioChannels`, protocol, sample-rate, bit-depth, and feature flags.
  Resolve an explicit audio `MediaSourceId` inside the authorized version group before selecting
  the source path. Use `AudioBitRate` as the encoding target and `MaxStreamingBitrate` only as its
  fallback; use `TranscodingAudioChannels` for the encoder channel count.
- Resolve Universal Audio `.strm` sources through the persisted remote target rather than the
  local sidecar path. For a direct-compatible remote HTTP source, proxy it with the same bounded
  range and persisted-User-Agent behavior as Video unless *both* `EnableRemoteMedia` and
  `EnableRedirection` explicitly opt into the official temporary redirect.
- When Universal Audio requests HLS for a non-direct source, serve the existing authenticated
  Audio master/main/segment pipeline rather than creating a progressive file. Carry the selected
  media source, target user, stream limits, and seek into the generated playlist request, and keep
  the generated VOD playlist on MPEG-TS until fMP4 init-segment support exists end to end.
- Treat Universal Audio's comma-delimited `Container` values as official `container|codec...`
  direct-play profiles. A matching container with a declared codec list may direct-play only when
  the selected persisted Audio stream has a listed codec; preserve a container-only fallback when
  historical media has no persisted probe data.
- Universal Audio must resolve the requested target user's policy before it enters its HLS or
  progressive encode branch. A user with `EnableAudioPlaybackTranscoding=false` may still use a
  compatible direct source, but must receive the normal playback-policy denial before FFmpeg work
  or an HLS job starts.
- Universal Audio and its HLS follow-ups must accept API keys under the ordinary default route
  policy. With no `UserId`, an API key is user-less and unrestricted; with an explicit non-nil
  `UserId`, resolve that user and apply the same library and transcoding policy to the master and
  every generated segment request.
- `SubtitleStreamIndex` is the persisted media-stream index exposed by the API. When constructing an FFmpeg `subtitles` filter, convert it to the zero-based index among embedded subtitle streams only; external subtitle streams are file inputs and must never be passed as the filter's `si` value. Keep this conversion consistent for progressive video and HLS playback.
- Progressive audio and video transcodes must use an output path unique to the request (or an equivalent complete parameter key); never let concurrent requests with different stream, codec, bitrate, seek, subtitle, or dimension options share one FFmpeg output file. Remove the temporary output after the response has consumed it.
- Advertise video direct stream only when the selected local File source can be copied unchanged into
  the selected progressive container. The remux command must map the selected streams with
  `-c:v copy -c:a copy` and must never accept filters, bitrate/size/frame-rate changes, subtitle
  burn-in, or a remote URL; fall back to an authorized encode or report the source unplayable.
- For an authorized static HTTP media source, proxy byte ranges without buffering and forward only
  the persisted `RequiredHttpHeaders.User-Agent` value. Do not copy arbitrary client headers or
  expose a media item's remote authorization headers in unrelated DTO fields.
- Keep the ordinary Audio stream route on the same default authorization contract as Video: an API
  key is unrestricted and user-less, while a device session resolves the item through its own
  library policy. Resolve Audio alternate `MediaSourceId` values within the audio/video version
  group (not the video-only group), and use a `.strm` item's resolved source rather than serving
  its sidecar bytes.
- Treat an explicit nil `UserId` like an omitted value on authenticated endpoints that use the
  official `RequestHelpers.GetUserId` helper. Resolve the current device user before authorization
  and policy lookup across root, related-item, theme, show, Resume/Latest, HLS, user-image, Channel,
  and equivalent lowercase routes; never try to load the all-zero UUID as a persisted user.
- Keep `/users/me` on the same ordinary default authorization policy as `/Users/Me`; lowercase
  compatibility must not bypass parental-control checks. Bind `/Items` `Genres` and `Tags` with the
  official pipe-delimited binder, while preserving repeated-query-value behavior.
- Serialize every virtual folder's `LibraryOptions` as the complete official constructor-default
  object, including when Kotlin sends a null AddVirtualFolder body. Normalize historical partial or
  mixed-case JSON on reads, preserve unknown extension properties, and replace relational
  `PathInfos` from the authoritative path rows.
- `UserViews.IncludeExternalContent` defaults to true. Append policy-visible non-Live-TV Channel
  views to both modern and legacy home views, honor Enabled/Blocked Channels and ordinary metadata
  policy, suppress them only when explicitly false, and keep official ordered-view/sort-name order.
- Keep `GET /emby/Users/{UserId}/Views` on its generated legacy contract: require
  `IncludeExternalContent`, bind its query name case-insensitively with last-duplicate-wins
  semantics, and resolve the authenticated target user before surfacing a missing or malformed
  query. Jellyfin root and `/api` user-view routes retain their optional, default-true query.
- Apply non-Live-TV Channel capability, favorite, and `ItemFilter` queries before count and paging.
  Persisted channels without a provider capability match explicit false but not true. Accept enum
  names and defined integers case-insensitively, reject conflicting filters, batch requested Channel
  validation and descendant lookup, and pass requested DTO fields through the batched projector.
- For subtitle stream routes, apply `StartPositionTicks`, `EndPositionTicks`, and `CopyTimestamps`
  only when converting formats, matching `SubtitleEncoder.FilterEvents`. Equal formats and SSA to
  ASS return the original stream bytes even with a time window; VTT timestamp-map decoration remains
  a post-processing step and must not cause otherwise equivalent subtitles to be parsed or rewritten.
- Emit `UserDataChanged` WebSocket data as the official `UserDataChangeInfo` object: include the
  compact `UserId` and a required `UserDataList` array, even when broadcasting one item. Do not
  substitute the historical singular `ItemId`/`UserData` fields, which mobile SDKs cannot decode.
- Emit `RefreshProgress` WebSocket data as a string-valued map. Both `ItemId` and invariant-culture
  `Progress` must be JSON strings, matching the official generated Kotlin and Swift contracts.
- Keep legacy Emby Connect isolated under `/emby`; it is not Jellyfin QuickConnect and must never
  register root or `/api` aliases. Without an external Connect provider, return an empty pending
  list, 404 for token exchange, 503 for link, and an idempotent success for unlink while retaining
  the official administrator authorization on pending and user-link mutations.
- Keep Emby Parties process-local until a durable party provider exists. Bind its route and DTO
  casing like the generated clients, isolate it from Jellyfin SyncPlay, enforce membership and
  ownership transitions, and never advertise a process-local party as durable across restarts.
- Expose generic Emby UI state and web-string resources only under `/emby`. Preserve the official
  elevated policy for UI view/command operations and the runtime's anonymous policy for web strings,
  including mixed-case static paths and case-insensitive request binding.
- Match the official empty notification-provider registry instead of manufacturing notifier
  options or successful delivery records. Keep configuration/default/test operations on their
  generated authorization, binding, and not-found behavior until a real provider is registered.
- Restore only Rust-native PostgreSQL backup ZIP archives through the existing validated restore
  engine. Reject legacy SQLite, lightweight, and selective-user restore modes with 422 instead of
  partially importing them, and retain path, archive-entry, and transaction safety checks.
- Persist Emby DLNA user profiles in protocol-private named configuration. Keep profile management
  administrator-only, bind nested JSON properties and enums case-insensitively, and do not register
  these profiles as Jellyfin playback device profiles.
- Keep the Emby UPnP/DLNA description, SCPD, SOAP control, and icon resources unauthenticated and
  isolated under `/emby`, matching static segments case-insensitively and accepting stale UDN path
  values like the official controller. Bound SOAP bodies, return official 401 SOAP faults for
  invalid actions, and use a legal empty DIDL response until browsing can apply a real DLNA profile
  and media policy; never expose an unfiltered library from this public transport surface.
- Keep retired Emby offline-Sync routes separate from Jellyfin SyncPlay. When no legacy Sync
  provider is registered, discovery collections are empty and individual job or job-file lookups
  are 404 after normal authentication and query binding; do not create placeholder files or claim
  mutation success until a durable provider and persistence model exist.
- Keep Emby CameraUploads history protocol-local and keyed only by the authenticated request's
  reported DeviceId, case-insensitively, so the same device retains one append-only history when
  users change. Persist history across user/session/device-auth deletion like the official device
  JSON, enforce the protocol-private `AllowCameraUpload` role before query/body extraction, and
  stream raw or first-file multipart bodies into a bounded same-directory temporary file followed
  by atomic rename. Sanitize filesystem components without altering the original SDK-visible
  `LocalFileInfo`, keep all paths inside the internal camera-uploads root, and remove a published
  unique file if its PostgreSQL history append fails.
- Bind `/emby/Sessions/{Id}/Playing/{Command}` from the generated `PlaystateRequest` JSON body,
  with case-insensitive property names and last-duplicate-wins semantics while keeping the path
  command authoritative. Return Emby's empty 200 response, but do not leak that body binding or
  status into Jellyfin's root and `/api` query-based routes, which retain their empty 204 response.
- Require the generated `PlayRequest` JSON body on `/emby/Sessions/{Id}/Playing`. Keep `ItemIds`,
  `PlayCommand`, and `StartPositionTicks` query-owned, while the body exclusively supplies
  `ControllingUserId`, `SubtitleStreamIndex`, `AudioStreamIndex`, `MediaSourceId`, and `StartIndex`.
  Bind body names case-insensitively with last-duplicate-wins semantics, ignore unknown properties,
  queue every bound field, and preserve Jellyfin root and `/api` query-only behavior and 204 status.
- Keep the generated legacy `/emby/Users/{UserId}/PlayingItems/**` request contract protocol-local.
  Require its `MediaSourceId`, require `NextMediaType` for stop requests, and require and validate
  the progress JSON body. Bind query and body names case-insensitively with last-duplicate-wins
  semantics, while Jellyfin's root and `/api` legacy routes retain optional query values, ignore
  progress bodies, and return their existing empty 204 responses.

## Validation

Prepare focused regression tests while implementing several related fixes, then run their narrow
targets together using the remote host's existing builder cache. Avoid local or per-edit rebuilds.
Broaden validation for that completed batch before committing its independently reviewable fixes:

Treat the union of generated Kotlin and Swift operation method/path templates as a route-coverage
gate, excluding only explicitly out-of-scope APIs. Keep the Rust OpenAPI inventory aligned with
the real router, including Swift's explicit image `HEAD` operations and the concrete trickplay
`{index}.jpg` template rather than exposing only an internal catch-all route.

```bash
cargo fmt --all -- --check
cargo test -p <affected-crate>
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

For Android wire compatibility, run `android_sdk_compat` with a temporary PostgreSQL database and
`JELLYFIN_ANDROID_DUMP` set, then validate every dumped response with `tools/kotlin_validate.py`.
The Kotlin schema extractor must anchor primary-constructor parsing at the generated `data class`
declaration rather than a preceding file-level serializer annotation, include `override val`
constructor fields, and never silently accept a selected model with zero parsed fields. Validate
enum-keyed maps, their nested values, UUID syntax, and non-null collection elements as strictly as
the SDK serializers do.
Keep the fixture's canonical Person reconciliation and storage paths aligned with `AppState`, and
use current generated Kotlin response types (including root arrays) rather than obsolete wrapper
names. Include an actual `/Users/AuthenticateByName` response in that dump rather than only seeded
tokens, so its nested `UserDto` and `SessionInfoDto` are checked before mobile bootstrap. Validate
that same dump with `tools/swift_validate.py` against the generated Swift Codable
models: Swift rejects a present nested scalar, enum, dictionary, array, or ISO date whose wire
shape differs, even when the property itself is optional. Run `tools/test_swift_validate.py` when
changing that static validator. The validator must traverse root `List<T>` responses, reject
unknown root model names, and enforce fields decoded with Swift's non-optional `decode` rather than
silently treating them like `decodeIfPresent`. Include representative response models beyond the
core item page—such as task, activity, metadata-editor, image-provider, playlist-user, and movie
recommendation DTOs—and use real Swift Codable decoding as an additional check whenever a Swift
toolchain is available. Generate the dump only after every requested route and assertion succeeds;
record each route plus its Kotlin and Swift root model in the manifest. Validate it with
`tools/validate_mobile_dump.sh`, which must fail on a missing, malformed, invalid, or empty manifest
instead of accepting zero responses. Keep `none` fixtures genuinely anonymous and include an active
transcode in the Sessions response so nested transcoding reasons are exercised. Do not add the
checked-out SDK source tree or Python bytecode to commits.

For the protocol-private Emby surface, run `emby_swift_compat` with a temporary PostgreSQL database
and `JELLYFIN_EMBY_SWIFT_DUMP` set, then validate the completed manifest with
`tools/validate_emby_swift_dump.sh`. Derive the schema from the checked-out generated Emby Swift
`Codable` models, keep the dump free of access-token values, and require every named runtime case to
pass before writing the manifest. Keep the validator aligned with Emby's generated date decoder,
including its distinction between timezone-less millisecond timestamps and rejected
timezone-less whole-second ISO timestamps.

Some `jellyfin-data` integration tests require PostgreSQL and create temporary databases whose names begin with `jellyfin_`. Do not point those tests at a database containing user data.

For scan-memory work, include a repeatable large-directory or synthetic-library measurement when possible. Report baseline, peak, 60-second, and 300-second post-scan values. Separate process RSS and anonymous memory (`RssAnon` or `smaps_rollup` Anonymous) from cgroup `file` and `inactive_file`; metadata image page cache is reclaimable and must not be reported as a Rust heap leak. Also report whether memory returns after the scan, and do not infer a leak from allocator-retained RSS alone.

For Resume query memory changes, the optional ignored `resume_materialization_plan` test with
`JELLYFIN_RESUME_PLAN_DUMP` records isolated PostgreSQL execution plans. Compare materialized row
width and temporary blocks separately from process RSS or heap measurements; neither execution-plan
estimates nor reclaimable database caches alone establish a memory leak.

## Deployment verification

- The deployment checkout is `/home/lqs/jellyfin-rust` on the configured test host. Inspect its current state before changing it; do not assume the local workspace path is valid remotely.
- Deploy only committed revisions. Record the revision tested and verify health, relevant API behavior, scan completion/cancellation, and service logs.
- Do not delete databases, media, configuration, containers, or volumes during deployment validation unless the user explicitly requests it.

## Commit style

- One coherent, tested change per commit.
- Use concise imperative subjects with a conventional prefix when appropriate, for example `fix: bound library scan buffering` or `perf: batch item upserts`.
- Do not include generated build output, credentials, local logs, or deployment-only files.
