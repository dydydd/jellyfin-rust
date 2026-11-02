# Playback And Encoding

Apply this guide to ffprobe, FFmpeg, playback selection, progressive streaming,
HLS, and keyframe support.

## PlaybackInfo

- Match official query-over-body precedence for `DeviceProfile`. A client may
  register a profile through session capabilities and omit it later.
- Apply the profile independently to every returned `MediaSource`.
- Derive `SupportsDirectPlay`, `SupportsDirectStream`, and
  `SupportsTranscoding` from the final selected method and device policy. Never
  advertise a URL or method that is unimplemented.
- Without a profile, retain static direct play for supported local and
  HTTP-proxy sources, but leave direct stream and transcoding unavailable until
  a real route is selected.
- Generate `TranscodingUrl` for HTTP progressive and HLS transcoding selections.
- Project source bitrate from persisted metadata with an inferred fallback from
  non-external streams, independently per version.
- Project persisted/probed container first, then path extension as fallback.
  Strip URL query and fragment before extension checks.
- Preserve the displayed item's raw top-level `Container`; normalize each nested
  source against that version's own path.
- Project each source's own video type, size, file ETag, and bitrate. Do not
  reuse the displayed primary's values for an alternate.
- Keep `MediaSourceCount` count-only and batched. Always count the displayed
  source, preserve nullable single-source behavior, and include episode groups
  without loading every source.
- Apply user policy to static capability flags: audio transcoding controls audio
  sources; video transcoding and remuxing independently control video sources.

## Streams

- Bind the complete official stream query surface, including legacy, Android,
  mixed-case, and lowercase spellings.
- Resolve `MediaSourceId` inside the authorized alternate-version group before
  probing or opening a file.
- For `.strm`, stream the resolved target, not sidecar bytes. For direct remote
  HTTP, proxy bounded ranges unless official options allow a redirect.
- Forward only persisted required headers such as `User-Agent`. Never forward
  arbitrary client headers or expose remote authorization headers in DTOs.
- Local static playback serves unchanged bytes; `static=true` must not invoke
  FFmpeg.
- Progressive requests with `static` omitted or false must honor codecs, stream
  indexes, bitrate, dimensions, maximum frame rate, start position, subtitle
  mode, and audio channel settings.
- `AudioStreamIndex` and `SubtitleStreamIndex` are persisted global indexes.
  Convert subtitle indexes to zero-based embedded-subtitle positions only for
  FFmpeg filters.
- Burn subtitles only for official `Encode` behavior. `External` and `Drop`
  must not burn them.
- Use a unique output path or complete parameter key per request. Remove
  temporary outputs after the response consumes them.
- Start FFmpeg and stream growing output immediately; never wait for a complete
  transcode.
- Use Android-compatible fragmented MP4:
  `frag_keyframe+empty_moov+delay_moov` for video and
  `empty_moov+delay_moov` for MP4-family audio.
- Preserve `CopyTimestamps=true` for seek requests.
- Video direct stream requires unchanged local File-source video and audio copy
  into the selected progressive container. It must reject filters, scaling,
  bitrate changes, subtitle burn-in, and remote URLs.
- Include every output-affecting parameter in deterministic job identity. Never
  share output between different stream, codec, bitrate, seek, subtitle, or
  dimension options.
- Bind both official and legacy bitrate spellings: `VideoBitrate`,
  `AudioBitrate`, `VideoBitRate`, and `AudioBitRate`. Do not rely on
  case-insensitive matching to bridge intentionally different internal names.
- Include `AudioStreamIndex` and `MaxFramerate` in HLS identity. Use
  `-map 0:{index}` for an explicit audio index.
- Use a job-scoped EVENT playlist for unknown runtime and finite VOD only for a
  known positive runtime.

## Probing

- Persist scan-time size and media facts; do not stat media from browse or
  playback-info routes.
- Version successful local Audio/Video probes with normalized path, size, and
  modification time. A missing, stale, or mismatched marker triggers one
  bounded repair even when historical streams exist.
- Treat `.strm` and remote sources as lazy playback probes.
- Lazy `.strm` probing must synchronously terminate FFprobe before returning.
  An async timeout around an uncancelled blocking child is insufficient.
- Coordinate lazy probes by item and resolved target so concurrent requests
  share one bounded flight. Keep failure backoff short-lived and hard-bounded.
- External processes need owned handles, kill-on-cancel behavior, and bounded
  output. Do not spawn detached tasks to bypass cancellation.
- Test Range, HEAD, early streaming, and fallback behavior where the route
  advertises those semantics.
