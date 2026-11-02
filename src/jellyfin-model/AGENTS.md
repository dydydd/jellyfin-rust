# DTO And Wire Models

Apply this guide to public DTOs, enums, query models, and cross-layer value
types consumed by `jellyfin-api` or generated clients.

## Wire Semantics

- Preserve official property names and casing, omission versus `null`, empty
  arrays versus absent fields, enum wire values, GUID formatting, dates, numeric
  ranges, and dictionary value types.
- Serialize public enums by official names unless the contract explicitly uses
  integers. Never expose a local-only enum member.
- Treat generated Android/Kotlin and Swift models as executable wire contracts.
  A response either SDK cannot decode is incorrect even if it resembles the
  official C# DTO.
- Keep default all-fields item details populated with the official field groups,
  including media sources, nested and top-level streams, trickplay data, and
  other fields added by the official controller.
- Gate optional groups by official `ItemFields`; do not require a non-official
  field just to include default data.
- Preserve official `204 No Content` behavior and error shapes. Do not return a
  successful body where Jellyfin returns no content.
- Keep visible image collections present as an object or array when image
  projection is enabled, even when empty. `ImageTags` is an object;
  `BackdropImageTags` is an array.
- Apply official local image precedence before inherited Series/Season images.
  Episode primary prefers Season then Series, Logo uses the nearest parent,
  Thumb prefers Series over Season, and Backdrop uses the nearest available
  parent.
- Keep `HasLyrics` limited to Audio and derived from persisted Lyric streams;
  keep `HasSubtitles` limited to Video and derived from persisted Subtitle
  streams. Never derive either from stale JSON.
- Keep `CanDelete`, `CanDownload`, and `PlayAccess` absent unless requested,
  except where official default all-fields details include them.
- Keep `ExternalUrls` and `RemoteTrailers` field-gated. Emit requested empty
  arrays and do not inherit either from parents or alternate versions.
- Keep `SeriesName`, `SeasonName`, and `OriginalLanguage` populated from
  relational or persisted data.
- Keep music `Album`, `AlbumId`, `Artists`, `ArtistItems`, `AlbumArtist`, and
  `AlbumArtists` populated through the shared nearest-ancestor lookup.
- Populate folder `UserData.UnplayedItemCount` rather than leaving it null for
  Series or Season DTOs.
- Use official relational names for Series/Season fields instead of parsing
  paths in a DTO projector.
- Keep `MediaSourceCount` count-only and batched. Preserve nullable
  single-source behavior and count episode groups without loading every source.
- Do not expose internal `people.id` or normalized item-value ids through public
  DTOs. Use canonical Person `base_items.id` and persisted BaseItem ids.
- Use official item-name and provider-id normalization rules. Never invent a
  separator or formatting rule when the official server uses another.

## Compatibility Checks

- Add focused serialization/deserialization tests for changed shapes.
- Cover omission, `null`, empty collections, enum wire names, GUIDs, dates, and
  nested SDK-sensitive values.
- Run Kotlin and Swift validators when item pages, mobile bootstrap, auth, or
  shared DTO shapes change.
- Keep API docs synchronized when a query model or schema changes.
