# API And Wire Compatibility

Apply this guide to `jellyfin-api` and any change that affects public HTTP
behavior. The official C# controller and the generated Android/Kotlin and Swift
models define the contract.

## Binding And Routing

- ASP.NET route matching, query-name binding, and JSON-property binding are
  case-insensitive. Axum and Serde are not automatically equivalent.
- Register canonical routes and every fully lowercase alias required by Android
  and Swift. Every static segment must work in lowercase, including compound
  segments. Do not lowercase dynamic path values or query values.
- Bind mutations and query fields case-insensitively, including duplicate-key
  behavior. Preserve ASP.NET last-duplicate-wins semantics where the official
  binder uses them.
- Follow official value conversion. Accept numeric strings or integer and
  case-insensitive enum forms only where the official model binder accepts
  them.
- Match repeated-key and comma-delimited collection behavior at the same stage
  as ASP.NET. Do not split an element that came from a repeated key. Parse and
  validate the complete collection before changing persistent state.
- Reject an empty required identifier collection when official code requires at
  least one id. Deduplicate where required, but preserve mutation order.
- Register concrete routes before dynamic fallbacks. Do not keep duplicate route
  definitions. A fallback that serves several templates is acceptable only when
  each concrete dispatch path has a test.
- Test representative PascalCase, camelCase, and all-lowercase requests. Test
  route precedence in official and lowercase spellings.

## Wire Contract

- Preserve official property names and casing, omission versus `null`, empty
  arrays versus absent fields, enum wire values, GUID formatting, dates,
  numeric ranges, and dictionary value types.
- Serialize public enums by their official names unless the contract explicitly
  uses integers. Never expose a local-only enum member.
- Preserve official empty successes. A Jellyfin mutation that returns `204 No
  Content` must not become `200` without an explicit contract change.
- Default all-fields item details must include the official field groups,
  including media sources, nested and top-level media streams, trickplay data,
  and other fields added by the official controller.
- Gate optional `BaseItemDto` groups by their official `ItemFields`. Do not make
  clients request a non-official field merely to receive default data.
- Do not buffer a complete response just to adapt a few fields. Use the shared,
  batched projection path.
- Update OpenAPI and its tests whenever a route, query model, request body, or
  public schema changes.

## Authorization And Errors

- Every casing of a route must use the same handler policy. Lowercase aliases
  must not bypass authorization.
- A valid API key is user-less where the official route allows it. An explicit
  target user still applies that user's library policy.
- Treat a valid API key as an administrator for user creation, deletion,
  profile/configuration updates, and password changes through modern and
  legacy routes. An omitted or nil target remains 404; ordinary user mutations
  still require self access and `EnableUserPreferenceAccess`. Resolve the target
  before preference checks.
- Device sessions are bounded by their user's policy.
- Distinguish hidden or unknown resources from authorization failures according
  to the official controller. Do not collapse them into one status.
- When official resource errors take precedence, resolve and authorize the
  target user or item before parsing body fields or item identifiers.
- Validate bodies before mutating state. Reject malformed JSON, missing required
  fields, and invalid nullable values with the official status.
- Do not substitute the current user for an omitted target unless the official
  route does so.
- Do not expose a user-less policy bypass. When official behavior uses a global
  query, explicitly enable all folders instead of leaving a policy that hides
  nested media.
- Preserve password-change session revocation and password-reset behavior.

## Item Projection

- Apply the target user's enabled/blocked folders, allowed/blocked tags,
  parental-rating rules, blocked unrated kinds, and virtual-item rules to every
  list, count, facet, recommendation, similarity, and Next Up query.
- Resolve an explicit `UserId` before its nullable lookup. Authorize a regular
  user's foreign target and preserve the official administrator fallback for a
  nonexistent target where applicable.
- User-less API-key projections must not load user data, remembered stream
  selections, language preferences, or policy-aware child aggregates.
- Filter alternate versions through the same visibility policy before loading
  streams or attachments. Keep details, lists, and counts consistent about the
  primary for a version group.
- Project page images in one batched lookup, including inherited Series and
  Season images. Project `ImageBlurHashes` only from persisted image metadata;
  never decode image bytes to calculate it.
- When image projection is enabled, emit `ImageTags` as an object even when
  empty and `BackdropImageTags` as an array when that type is enabled even when
  empty.
- Batch parent Series/Season names, artist and album relations, external URLs,
  chapters, chapter image tags, requested counts, and alternate-version streams
  and attachments.
- Project `HasLyrics` only for Audio items from persisted Lyric stream
  existence. Project `HasSubtitles` only for Video items with persisted Subtitle
  streams. Do not derive either flag from stale JSON.
- Project `CanDelete`, `CanDownload`, and `PlayAccess` only when requested,
  except on official default all-fields item details. Apply user policy in the
  same bounded query path.
- Project `ExternalUrls` only for `ItemFields.ExternalUrls`, with an empty array
  when requested but unmatched. Project `RemoteTrailers` only for
  `ItemFields.RemoteTrailers` from the current item, without inheriting them
  from parents or alternate versions.
- Project intros, local trailers, special features, and additional parts through
  one batched projector using official default DTO options.
- Project `SeriesName`, `SeasonName`, and `OriginalLanguage` unconditionally
  from persisted item or relational parent data.
- Populate Series and Season `UserData.UnplayedItemCount` with one
  policy-aware recursive descendant query. Do not leave it null merely because
  the DTO represents a folder.
- Project music album and artist fields with one batched nearest-ancestor album
  lookup.
- Gate the settings group by `ItemFields.Settings`. Emit canonical
  `LockedFields`, empty defaults, and `LockData` only when requested.
- Build Filters and Filters2 buckets from one target-user policy snapshot.
  Bind nullable `IsAiring`, `IsMovie`, `IsSports`, `IsKids`, `IsNews`, and
  `IsSeries` case-insensitively, reject malformed booleans, and apply them
  consistently to genre and stream-language discovery.
- Do not use a DTO projector as an existence check. Use a targeted PostgreSQL
  query.

## Alternate Versions

- Treat alternate video versions as one playback group while exposing each
  version as a distinct `MediaSource`.
- Put the requested or displayed source first, then apply official ordering for
  grouping roots and local alternates.
- Resolve `MediaSourceId`, stream index, subtitle index, and attachment requests
  inside the authorized version group. Never serve an index from another
  displayed primary.
- Persist deterministic parent-local `sort_order`; preserve it across rescans
  and compact both sides when a child moves.
- Do not let alternate-only credits or playback rows change primary-only counts
  except where official user-data fallback requires it.
- Apply each playback `DeviceProfile` independently per source. Explicit audio
  or subtitle indexes apply only to the source selected by `MediaSourceId`.
- Project the selected subtitle index back to that source's
  `DefaultSubtitleStreamIndex`, including `-1` when subtitles are disabled.
- Populate subtitle `DeliveryMethod` and API/direct external subtitle URLs,
  including the API-key suffix, according to official
  `SetDeviceSpecificSubtitleInfo` behavior. Populate selected-source attachment
  URLs too.
- Set `SupportsExternalStream` for external streams and text, PGS, or VobSub
  subtitles as the official server does.

## Paging

All paging values are signed `Int32`; out-of-range values fail query binding.
Negative and zero behavior differs by route family:

| Route family | `StartIndex` | `Limit` |
| --- | --- | --- |
| `/Items`, Resume, Suggestions, Upcoming | Negative: no skip, echoed | `0`: empty; `<0`: all |
| `/Years` | Negative skips nothing and is echoed | Non-positive empty |
| `/Persons` | Non-positive skips nothing and is echoed | Non-positive unlimited |
| Genres, music genres, studios, artists | Negative: no skip, echoed | `0`: empty; `<0`: unlimited |
| Playlist items, item collections | Negative skips nothing and is echoed | Non-positive empty |
| Remote images | Negative skips nothing | Non-positive empty |
| Search hints | Negative skips nothing | Non-positive empty with zero total |
| Show episodes | Negative skips nothing and is echoed | Negative empty |
| Next Up | Negative skips nothing and is echoed | Non-positive unlimited |

- Compute totals according to the route: Suggestions can default to the
  returned-page count, Years can total before distinct-year extraction, and
  disabled totals can return zero.
- Apply the official order before paging. This includes aired-episode order for
  show episodes and Next Up.
- Add focused tests for `StartIndex` echoing, zero and negative values, totals,
  ordering, and out-of-range binding whenever a route has a special rule.

## Media Routes

- Resolve `MediaSourceId` inside the authorized alternate-version group before
  probing or opening a file.
- For `.strm`, stream the resolved target rather than sidecar bytes. For direct
  remote HTTP, proxy bounded ranges unless official options allow redirects.
- Forward only persisted required headers such as `User-Agent`. Never forward
  arbitrary client headers or expose remote authorization headers.
- Local static playback serves unchanged bytes and must not enter FFmpeg.
- Progressive requests with `static` omitted or false must honor codecs, stream
  indexes, bitrate, dimensions, frame rate, start position, subtitle mode, and
  audio channel settings.
- `AudioStreamIndex` and `SubtitleStreamIndex` are persisted global stream
  indexes. Convert subtitle indexes to zero-based embedded-subtitle positions
  only for FFmpeg filters.
- Burn subtitles only for official `Encode` behavior. `External` and `Drop`
  must not silently burn them.
- Use a unique output or complete parameter key per transcode request. Include
  every output-affecting parameter in its identity and remove temporary outputs
  after the response consumes them.
- Start FFmpeg and stream growing output immediately. Never wait for the complete
  transcode before responding.
- Android-compatible fragmented MP4 uses
  `frag_keyframe+empty_moov+delay_moov` for video and
  `empty_moov+delay_moov` for MP4-family audio.
- Preserve `CopyTimestamps=true` for seek requests. Video direct stream requires
  unchanged local File-source video and audio copy and must not accept filters,
  scaling, bitrate changes, subtitle burn-in, or a remote URL.
- Bind both official and legacy bitrate spellings. Include audio stream index
  and maximum frame rate in HLS job identity.
- Use a job-scoped EVENT playlist for unknown runtime and finite VOD only for a
  known positive runtime.

## Images, Chapters, And Lyrics

- Do not decode, resize, reformat, or decorate image bytes unless a route
  explicitly requires it. Stream original bytes with the official content
  type. Preserve supported APNG, AVIF, ICO (`image/x-icon`), TIFF, and Jellyfin
  TBN JPEG semantics without inspecting content.
- Image-info routes return persisted dimensions and BlurHash values without
  lazy decoding or metadata writes.
- Do not advertise byte ranges on ordinary images unless Range is implemented.
  Trickplay tiles are the range-aware exception.
- Stream trickplay tiles in bounded chunks and preserve HEAD and range behavior.
- Validate `ImageType` and `ImageFormat` names or integer values before resource
  lookup. Reject unknown values as bad requests.
- Clone or reference legacy image files; never move or delete source files
  during reconciliation.
- Apply local images before inherited Series/Season images. Local images
  suppress the corresponding inherited field; Series does not inherit parent
  images.
- Episode/Season DTOs always derive `SeriesPrimaryImageTag`. Episode primary
  prefers Season then Series, Logo uses the nearest parent, Thumb prefers Series
  over Season, and Backdrop uses the nearest available parent.
- Project chapters only for `ItemFields.Chapters`, except default all-fields
  details, which include an empty array. Load chapters in one batch and order by
  `StartPositionTicks`.
- Keep alternate-version chapters isolated. Derive chapter image tags from the
  owning media path and resolve chapter images by `(item_id, ChapterIndex)`.
- Replace chapters under an owner-row lock in one transaction, including empty
  replacements. Use fixed 128-row batches, preserve input order, and roll back
  all batches on failure.
- Store lyrics in the item's internal metadata directory with a same-directory
  temporary file and atomic rename, then register the file as a Lyric stream.
  The parsed JSON is only a cache.
- Reads prefer registered streams in stream-index order. Select parsers by the
  file path extension, not the persisted codec.
- Decode BOM-marked UTF-8, UTF-16LE/BE, and UTF-32LE/BE. Without a BOM, use
  UTF-8 replacement fallback. Preserve uploaded/downloaded bytes exactly.
- Authorize the Audio item before validating an upload body or filename. Never
  remove unregistered files, symlinks, or paths outside the metadata root.
- Lyric provider search and download use at most four providers in flight, a
  30-second deadline per provider, configured result order, and
  cancellation-by-drop. Search errors become isolated empty results; unknown,
  empty, or unparseable download responses are 404 while provider failures
  remain server errors.

## Mobile Aliases

- Keep mobile browse bootstrap routes reachable through lowercase static
  segments: public/system info, branding, users and views, devices, display
  preferences, sessions, latest/count/resume, and library available options.
- Keep `/users/public` before dynamic `/users/{id}`; otherwise `public` is
  parsed as a UUID.
- Keep `/users/authenticatebyname` on the same public policy and handler as
  `/Users/AuthenticateByName`.
- Keep lowercase aliases for item details, root/counts, Suggestions, themes,
  collections, intros/special features, show pages, InstantMix, search hints,
  trailers, additional parts, and user-data mutations.
- Keep `UserItems` user-data and rating, resume, `UserFavoriteItems`,
  `UserPlayedItems`, and legacy user item-data routes case-insensitive with the
  same authorization and mutation semantics.
- Keep both modern `/Items/Suggestions` and legacy
  `/Users/{userId}/Suggestions` routes reachable in lowercase with equivalent
  authorization and filtered results.
- Treat an omitted or empty Suggestions `UserId` as the official user-less
  global query: do not apply the authenticated user's roots, policy, user data,
  or stream preferences. Authorize an explicit non-empty id before lookup and
  group presentation keys only when that lookup resolves a user.
- Preserve the modern and legacy Suggestions result semantics: negative
  `StartIndex` skips nothing but is echoed, `Limit=0` is empty, a negative
  `Limit` is unlimited, and disabled totals report the returned page length.
- Project Suggestions with official all-field DTO options, including every
  alternate source and stream, source-specific metadata, trickplay data, and
  image metadata. A user-less response must not load user-policy aggregates.
- Keep `/Library/Series/Updated` and `/Library/Movies/Updated` external-source
  reports aligned with the official selection rules: select Series by TVDB
  including the omitted-id case, select Movies by nonblank IMDb before TMDB
  with case-insensitive provider ids, and trigger the bounded library-scan
  fallback once for a matching report.
- Keep playback-info GET and POST aliases on the same policy and handler.
- Keep non-Live-TV Channels routes case-insensitive. Bind `StartIndex`,
  `FolderId`, `ChannelIds`, sort, fields, and capability options through the
  shared DTO projector.
- Parse and validate the complete collection before changing persistent state,
  including repeated-key and comma-delimited bindings.
- Do not let a generic lowercase fallback capture a concrete route with a
  different policy. Add dispatch tests for every concrete alias.

## SDK Validation

- Run `android_sdk_compat` with a temporary PostgreSQL database and
  `JELLYFIN_ANDROID_DUMP` set when changing item pages, mobile bootstrap,
  authentication, or shared DTO shapes.
- Validate every dumped response with `tools/kotlin_validate.py` and
  `tools/swift_validate.py`. Include a real `/Users/AuthenticateByName` response
  so nested `UserDto` and `SessionInfoDto` shapes are checked.
- Run `tools/test_kotlin_validate.py`, `tools/test_swift_validate.py`, or
  `tools/test_runtime_sdk_validate.py` when changing the corresponding
  validator.
- Use a real Swift toolchain for `Codable` validation when available.
