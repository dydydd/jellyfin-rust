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
- Keep database invariants in PostgreSQL where practical (constraints, indexes, atomic upserts, transactions), while keeping domain rules explicit in Rust.
- Avoid N+1 queries. Use set-based queries or bounded batches, and add migrations for indexes or constraints required by new query patterns.
- Build playback-aware queries from the target user's `user_data` rows and reverse hierarchy lookups rather than correlated scans over all `base_items`. Materialize shared candidate sets when count and page queries would otherwise repeat expensive work.
- Project inherited images for an item page with one batched DTO-image lookup. Do not call the image projector once per item.
- Coordinate remote-image downloads by URL so concurrent items share one bounded download. Validate that upstream content is an image, and remove or otherwise suppress permanently invalid remote references according to official behavior.
- Treat passwords, access tokens, API keys, and deployment credentials as secrets. Do not log or commit them.

## Compatibility expectations

- Match official Jellyfin DTO field names, nullability, defaults, HTTP status codes, authorization requirements, sorting, pagination, and case-insensitive matching.
- ASP.NET route, query-name, and JSON-property binding is case-insensitive. Compatibility tests must cover PascalCase, camelCase, and representative lowercase legacy requests; do not assume an Axum route or Serde field is equivalent merely because one casing works.
- Follow the official `JsonDefaults` value semantics. Where it permits them, accept numeric strings and case-insensitive or integer enum representations, and mirror the full official parameter set when implementing a legacy endpoint.
- Treat generated SDK models as executable compatibility specifications alongside the C# DTOs. Swift `Codable` rejects the entire enclosing item or page when one nested object, enum, dictionary value, or date has the wrong wire shape.
- Project the official single-item detail routes with their default all-fields `DtoOptions`: clients must receive media sources, nested and top-level media streams, and trickplay without supplying a non-official `Fields` query.
- Audit DTOs recursively: preserve object-array shapes, serialize API enums by their official names, keep string dictionaries string-valued, and emit full API `DateTime` values rather than storage-only dates.
- Treat alternate video versions as one playback group. Item details and `PlaybackInfo` must expose every version as a distinct `MediaSource`, honor `MediaSourceId` when opening static or transcoded content, and keep all stream and attachment loading batched by version identifiers.
- Order episode detail pages with the official aired-episode comparer before applying `StartItemId`, adjacency, or pagination. Specials with `AirsBeforeSeasonNumber`, `AirsAfterSeasonNumber`, or `AirsBeforeEpisodeNumber` must be positioned relative to regular episodes rather than compared with a single incompatible numeric key; season zero itself remains sorted by `SortName`.
- After scanning a movie directory, supplement the official filename-based version resolver with a conservative metadata match: same directory, non-empty case-insensitive title, and the same non-empty year, while rejecting the whole candidate group when TMDb, IMDb, or TVDb identifiers conflict. Never use collection identifiers as movie identity.
- Do not advertise a playback method unless the returned URL really implements it. In particular, never label unchanged container bytes as an MP4 direct stream; derive `SupportsDirectPlay`, `SupportsDirectStream`, and `SupportsTranscoding` from the final selected method and device policy.
- Proxy static HTTP media sources through the server like official Jellyfin instead of redirecting clients to private or signed upstream URLs. Forward byte ranges, preserve upstream status and content headers, stream without whole-file buffering, and keep signed URLs out of logs.
- A client may register its `DeviceProfile` once through session capabilities and omit it from later `PlaybackInfo` calls. Follow the official query-over-body precedence and fall back to the authenticated session profile before choosing a stream.
- Select HLS playlist mode after resolving `MediaSourceId`. A selected source with an unknown runtime must use a job-scoped EVENT playlist; do not coerce a null runtime into a zero-length VOD, and keep known positive runtimes on the finite VOD path.
- Preserve unknown or optional metadata where the official server does; a partial provider response must not erase valid existing metadata.
- Merge remote movie and series genres only when `LockedFields.Genres` permits it and the refresh replaces data or fills an empty target. An empty provider genre list must not erase established genres, and JSON `Genres` must stay atomic with normalized PostgreSQL genre relations.
- Merge episode metadata in official priority order: local metadata first, remote providers filling or replacing only eligible placeholders, and `LockedFields.Name` always protecting an established title. A repeated scan or alternate-version regroup must not turn a scraped episode title back into the series or filename-derived group name.
- Metadata providers must have deterministic priority and merge behavior. Network calls need timeouts, bounded concurrency, and useful error context.
- Lazy `.strm` probing must have a process-level deadline that terminates FFprobe before returning; an async timeout around an uncancelled blocking child is not sufficient because client retries can accumulate processes and memory.
- During episode refresh, merge a neighboring local NFO before remote metadata: preserve a non-empty local title, allow the first remote result to replace only a path-derived placeholder when no local title exists, and honor `LockedFields.Name` even for a full refresh.
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
