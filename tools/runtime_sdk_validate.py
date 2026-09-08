#!/usr/bin/env python3
"""Read-only Jellyfin runtime SDK compatibility audit.

Usage: runtime_sdk_validate.py --auth-file /tmp/auth.json
The authentication file must contain an AuthenticationResult response. The
runner never logs its token, response bodies, item ids, or item names.
"""
import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

from kotlin_validate import validate as validate_kotlin
from swift_validate import validate as validate_swift


@dataclass(frozen=True)
class Case:
    name: str
    path: str
    kotlin: str
    swift: str


def get(base_url, token, path):
    request = Request(
        f"{base_url.rstrip('/')}/{path.lstrip('/')}",
        headers={"X-Emby-Token": token},
        method="GET",
    )
    try:
        with urlopen(request, timeout=30) as response:
            return response.status, response.read()
    except HTTPError as error:
        return error.code, error.read()
    except URLError as error:
        raise RuntimeError(f"request failed for {path}: {error.reason}") from error


def response_json(base_url, token, path):
    status, body = get(base_url, token, path)
    if status != 200:
        return status, None
    try:
        return status, json.loads(body)
    except json.JSONDecodeError:
        return status, None


def recursive_item_pages(base_url, token, start=0, max_pages=None):
    seen, page_count = start, 0
    while True:
        status, page = response_json(base_url, token, f"/Items?Limit=200&Recursive=true&StartIndex={start}")
        if status != 200 or not isinstance(page, dict):
            raise RuntimeError(f"recursive item page {start} returned HTTP {status}")
        page_items = page.get("Items")
        if not isinstance(page_items, list):
            raise RuntimeError(f"recursive item page {start} has no Items array")
        print(f"PAGE start={start} items={len(page_items)}", flush=True)
        yield page
        page_count += 1
        seen += len(page_items)
        total = page.get("TotalRecordCount")
        if (max_pages is not None and page_count >= max_pages) or not isinstance(total, int) or seen >= total or not page_items:
            return
        start += len(page_items)
def item_id(result, item_type):
    for item in result.get("Items", []):
        if item.get("Type") == item_type and isinstance(item.get("Id"), str):
            return item["Id"]
    return None


def item_ids(result):
    ids = {}
    for item in result.get("Items", []):
        item_type, identifier = item.get("Type"), item.get("Id")
        if isinstance(item_type, str) and isinstance(identifier, str):
            ids.setdefault(item_type, identifier)
    return ids


def named_item(result):
    for item in result.get("Items", []):
        if isinstance(item.get("Name"), str):
            return item["Name"]
    return None


def static_cases(user_id):
    return [
        Case("system_info", "/System/Info", "SystemInfo", "SystemInfo"),
        Case("public_system_info", "/System/Info/Public", "PublicSystemInfo", "PublicSystemInfo"),
        Case("configuration", "/System/Configuration", "ServerConfiguration", "ServerConfiguration"),
        Case("metadata_options", "/System/Configuration/MetadataOptions/Default", "MetadataOptions", "MetadataOptions"),
        Case("endpoint", "/System/Endpoint", "EndPointInfo", "EndPointInfo"),
        Case("storage", "/System/Info/Storage", "SystemStorageDto", "SystemStorageDto"),
        Case("activity_log", "/System/ActivityLog/Entries", "ActivityLogEntryQueryResult", "ActivityLogEntryQueryResult"),
        Case("logs", "/System/Logs", "List<LogFile>", "List<LogFile>"),
        Case("utc_time", "/GetUtcTime", "UtcTimeResponse", "UtcTimeResponse"),
        Case("users", "/Users", "List<UserDto>", "List<UserDto>"),
        Case("current_user", "/Users/Me", "UserDto", "UserDto"),
        Case("user", f"/Users/{user_id}", "UserDto", "UserDto"),
        Case("user_views", f"/Users/{user_id}/Views", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("views", "/UserViews", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("root", "/Items/Root", "BaseItemDto", "BaseItemDto"),
        Case("counts", "/Items/Counts", "ItemCounts", "ItemCounts"),
        Case("filters", "/Items/Filters", "QueryFiltersLegacy", "QueryFiltersLegacy"),
        Case("filters2", "/Items/Filters2", "QueryFilters", "QueryFilters"),
        Case("items", "/Items?Limit=5", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("suggestions", "/Items/Suggestions", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("latest", "/Items/Latest", "List<BaseItemDto>", "List<BaseItemDto>"),
        Case("resume", f"/UserItems/Resume?userId={user_id}", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("user_items", f"/Users/{user_id}/Items?Limit=5", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("user_suggestions", f"/Users/{user_id}/Suggestions", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("user_latest", f"/Users/{user_id}/Items/Latest", "List<BaseItemDto>", "List<BaseItemDto>"),
        Case("user_resume", f"/Users/{user_id}/Items/Resume", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("years", "/Years", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("genres", "/Genres", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("music_genres", "/MusicGenres", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("studios", "/Studios", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("persons", "/Persons", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("artists", "/Artists", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("album_artists", "/Artists/AlbumArtists", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("search_hints", "/Search/Hints?searchTerm=movie", "SearchHintResult", "SearchHintResult"),
        Case("sessions", "/Sessions", "List<SessionInfoDto>", "List<SessionInfoDto>"),
        Case("public_users", "/Users/Public", "List<UserDto>", "List<UserDto>"),
        Case("view_grouping", f"/UserViews/GroupingOptions?userId={user_id}", "List<NameIdPair>", "List<NameIDPair>"),
        Case("display_preferences", f"/DisplayPreferences/ui-user-{user_id}?client=androidtv-native", "DisplayPreferencesDto", "DisplayPreferencesDto"),
        Case("tasks", "/ScheduledTasks", "List<TaskInfo>", "List<TaskInfo>"),
        Case("auth_keys", "/Auth/Keys", "AuthenticationInfoQueryResult", "AuthenticationInfoQueryResult"),
        Case("auth_providers", "/Auth/Providers", "List<NameIdPair>", "List<NameIDPair>"),
        Case("password_reset_providers", "/Auth/PasswordResetProviders", "List<NameIdPair>", "List<NameIDPair>"),
        Case("sync_play", "/SyncPlay/List", "List<GroupInfoDto>", "List<GroupInfoDto>"),
        Case("virtual_folders", "/Library/VirtualFolders", "List<VirtualFolderInfo>", "List<VirtualFolderInfo>"),
        Case("media_folders", "/Library/MediaFolders", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("physical_paths", "/Library/PhysicalPaths", "List<String>", "List<String>"),
        Case("library_options", "/Libraries/AvailableOptions", "LibraryOptionsResultDto", "LibraryOptionsResultDto"),
        Case("fallback_fonts", "/FallbackFont/Fonts", "List<FontFile>", "List<FontFile>"),
        Case("branding", "/Branding/Configuration", "BrandingOptionsDto", "BrandingOptionsDto"),
        Case("backups", "/Backup", "List<BackupManifestDto>", "List<BackupManifestDto>"),
        Case("packages", "/Packages", "List<PackageInfo>", "List<PackageInfo>"),
        Case("plugins", "/Plugins", "List<PluginInfo>", "List<PluginInfo>"),
        Case("repositories", "/Repositories", "List<RepositoryInfo>", "List<RepositoryInfo>"),
        Case("cultures", "/Localization/Cultures", "List<CultureDto>", "List<CultureDto>"),
        Case("countries", "/Localization/Countries", "List<CountryInfo>", "List<CountryInfo>"),
        Case("ratings", "/Localization/ParentalRatings", "List<ParentalRating>", "List<ParentalRating>"),
        Case("localization_options", "/Localization/Options", "List<LocalizationOption>", "List<LocalizationOption>"),
        Case("devices", "/Devices?Limit=5", "DeviceInfoDtoQueryResult", "DeviceInfoDtoQueryResult"),
        Case("channels", "/Channels", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("channel_features", "/Channels/Features", "List<ChannelFeatures>", "List<ChannelFeatures>"),
        Case("latest_channel_items", "/Channels/Items/Latest", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("movie_recommendations", f"/Movies/Recommendations?userId={user_id}", "List<RecommendationDto>", "List<RecommendationDto>"),
        Case("next_up", "/Shows/NextUp", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("upcoming", "/Shows/Upcoming", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("trailers", "/Trailers", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        Case("default_directory", "/Environment/DefaultDirectoryBrowser", "DefaultDirectoryBrowserInfoDto", "DefaultDirectoryBrowserInfoDto"),
        Case("drives", "/Environment/Drives", "List<FileSystemEntryInfo>", "List<FileSystemEntryInfo>"),
        Case("web_configuration_pages", "/web/ConfigurationPages", "List<ConfigurationPageInfo>", "List<ConfigurationPageInfo>"),
        Case("startup_configuration", "/Startup/Configuration", "StartupConfigurationDto", "StartupConfigurationDto"),
        Case("startup_user", "/Startup/User", "StartupUserDto", "StartupUserDto"),
        Case("first_user", "/Startup/FirstUser", "StartupUserDto", "StartupUserDto"),
        Case("lowercase_cultures", "/localization/cultures", "List<CultureDto>", "List<CultureDto>"),
        Case("lowercase_countries", "/localization/countries", "List<CountryInfo>", "List<CountryInfo>"),
        Case("lowercase_ratings", "/localization/parentalratings", "List<ParentalRating>", "List<ParentalRating>"),
        Case("lowercase_options", "/localization/options", "List<LocalizationOption>", "List<LocalizationOption>"),
    ]


def dynamic_cases(user_id, items):
    ids = item_ids(items)
    movie = ids.get("Movie")
    episode = ids.get("Episode")
    series = ids.get("Series")
    cases, skipped = [], []
    if movie:
        cases.extend([
            Case("movie", f"/Items/{movie}", "BaseItemDto", "BaseItemDto"),
            Case("movie_for_user", f"/Users/{user_id}/Items/{movie}", "BaseItemDto", "BaseItemDto"),
            Case("movie_similar", f"/Items/{movie}/Similar", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("movie_similar_legacy", f"/Movies/{movie}/Similar", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("trailer_similar", f"/Trailers/{movie}/Similar", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("movie_collections", f"/Items/{movie}/Collections", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("movie_intros", f"/Items/{movie}/Intros", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("movie_special_features", f"/Items/{movie}/SpecialFeatures", "List<BaseItemDto>", "List<BaseItemDto>"),
            Case("movie_theme_media", f"/Items/{movie}/ThemeMedia", "AllThemeMediaResult", "AllThemeMediaResult"),
            Case("movie_theme_songs", f"/Items/{movie}/ThemeSongs", "ThemeMediaResult", "ThemeMediaResult"),
            Case("movie_theme_videos", f"/Items/{movie}/ThemeVideos", "ThemeMediaResult", "ThemeMediaResult"),
            Case("movie_playback", f"/Items/{movie}/PlaybackInfo", "PlaybackInfoResponse", "PlaybackInfoResponse"),
            Case("movie_parts", f"/Videos/{movie}/AdditionalParts", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("movie_ancestors", f"/Items/{movie}/Ancestors?userId={user_id}", "List<BaseItemDto>", "List<BaseItemDto>"),
            Case("movie_external_ids", f"/Items/{movie}/ExternalIdInfos", "List<ExternalIdInfo>", "List<ExternalIDInfo>"),
            Case("movie_remote_images", f"/Items/{movie}/RemoteImages", "RemoteImageResult", "RemoteImageResult"),
            Case("movie_image_providers", f"/Items/{movie}/RemoteImages/Providers", "List<ImageProviderInfo>", "List<ImageProviderInfo>"),
            Case("movie_local_trailers", f"/Items/{movie}/LocalTrailers", "List<BaseItemDto>", "List<BaseItemDto>"),
            Case("movie_metadata_editor", f"/Items/{movie}/MetadataEditor", "MetadataEditorInfo", "MetadataEditorInfo"),
            Case("movie_segments", f"/MediaSegments/{movie}", "MediaSegmentDtoQueryResult", "MediaSegmentDtoQueryResult"),
            Case("movie_user_data", f"/UserItems/{movie}/UserData?userId={user_id}", "UserItemDataDto", "UserItemDataDto"),
            Case("movie_user_data_legacy", f"/Users/{user_id}/Items/{movie}/UserData", "UserItemDataDto", "UserItemDataDto"),
            Case("movie_user_intros", f"/Users/{user_id}/Items/{movie}/Intros", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("movie_user_local_trailers", f"/Users/{user_id}/Items/{movie}/LocalTrailers", "List<BaseItemDto>", "List<BaseItemDto>"),
            Case("movie_user_special_features", f"/Users/{user_id}/Items/{movie}/SpecialFeatures", "List<BaseItemDto>", "List<BaseItemDto>"),
            Case("movie_instant_mix", f"/Items/{movie}/InstantMix", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        ])
    else:
        skipped.append("movie-dependent routes (no visible Movie)")
    if episode:
        cases.extend([
            Case("episode", f"/Items/{episode}", "BaseItemDto", "BaseItemDto"),
            Case("episode_playback", f"/Items/{episode}/PlaybackInfo", "PlaybackInfoResponse", "PlaybackInfoResponse"),
            Case("episode_similar", f"/Items/{episode}/Similar", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        ])
    else:
        skipped.append("episode-dependent routes (no visible Episode)")
    if series:
        cases.extend([
            Case("series_seasons", f"/Shows/{series}/Seasons", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("series_episodes", f"/Shows/{series}/Episodes", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("series_similar", f"/Shows/{series}/Similar", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        ])
    else:
        skipped.append("series-dependent routes (no visible Series)")
    audio = ids.get("Audio")
    if audio:
        cases.extend([
            Case("audio_playback", f"/Items/{audio}/PlaybackInfo", "PlaybackInfoResponse", "PlaybackInfoResponse"),
            Case("audio_similar", f"/Items/{audio}/Similar", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("song_instant_mix", f"/Songs/{audio}/InstantMix", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("audio_album_instant_mix", f"/Albums/{audio}/InstantMix", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        ])
    else:
        skipped.append("audio-dependent routes (no visible Audio)")
    album = ids.get("MusicAlbum")
    if album:
        cases.extend([
            Case("album_similar", f"/Albums/{album}/Similar", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("album_instant_mix", f"/Albums/{album}/InstantMix", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        ])
    else:
        skipped.append("album-dependent routes (no visible MusicAlbum)")
    artist = ids.get("MusicArtist")
    if artist:
        cases.extend([
            Case("artist_similar", f"/Artists/{artist}/Similar", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("artist_instant_mix", f"/Artists/{artist}/InstantMix", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("artists_instant_mix", f"/Artists/InstantMix?id={artist}", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        ])
    else:
        skipped.append("artist-dependent routes (no visible MusicArtist)")
    playlist = ids.get("Playlist")
    if playlist:
        cases.extend([
            Case("playlist", f"/Playlists/{playlist}", "PlaylistDto", "PlaylistDto"),
            Case("playlist_permissions", f"/Playlists/{playlist}/Users/{user_id}", "PlaylistUserPermissions", "PlaylistUserPermissions"),
            Case("playlist_items", f"/Playlists/{playlist}/Items", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
            Case("playlist_instant_mix", f"/Playlists/{playlist}/InstantMix", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        ])
    else:
        skipped.append("playlist-dependent routes (no visible Playlist)")
    channel = ids.get("Channel")
    if channel:
        cases.extend([
            Case("channel_features", f"/Channels/{channel}/Features", "ChannelFeatures", "ChannelFeatures"),
            Case("channel_items", f"/Channels/{channel}/Items?fields=MediaStreams,Chapters", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"),
        ])
    else:
        skipped.append("channel-dependent routes (no visible Channel)")
    return cases, skipped


def detail_cases(base_url, token):
    cases, skipped = [], []
    for label, page, prefix, instant_mix in [
        ("year", "/Years", "/Years", False),
        ("genre", "/Genres", "/Genres", False),
        ("music_genre", "/MusicGenres", "/MusicGenres", True),
        ("studio", "/Studios", "/Studios", False),
        ("person", "/Persons", "/Persons", False),
        ("artist", "/Artists", "/Artists", False),
    ]:
        status, result = response_json(base_url, token, page)
        name = named_item(result) if status == 200 and isinstance(result, dict) else None
        if name is None:
            skipped.append(f"{label} detail (no visible {label})")
        else:
            path = f"{prefix}/{quote(name, safe='')}"
            cases.append(Case(f"{label}_detail", path, "BaseItemDto", "BaseItemDto"))
            if instant_mix:
                cases.append(Case(f"{label}_instant_mix", f"{path}/InstantMix", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"))
                identifier = item_id(result, "MusicGenre")
                if identifier:
                    cases.append(Case(f"{label}_instant_mix_by_id", f"/MusicGenres/InstantMix?id={identifier}", "BaseItemDtoQueryResult", "BaseItemDtoQueryResult"))
    return cases, skipped


def discovered_cases(base_url, token):
    status, tasks = response_json(base_url, token, "/ScheduledTasks")
    if status != 200 or not isinstance(tasks, list):
        return [], ["scheduled task detail (task list unavailable)"]
    for task in tasks:
        identifier = task.get("Id") if isinstance(task, dict) else None
        if isinstance(identifier, str):
            return [Case("task_detail", f"/ScheduledTasks/{identifier}", "TaskInfo", "TaskInfo")], []
    return [], ["scheduled task detail (no task id)"]


def audit(base_url, auth_file):
    authentication = json.loads(Path(auth_file).read_text(encoding="utf-8"))
    token = authentication.get("AccessToken")
    user_id = authentication.get("User", {}).get("Id")
    if not isinstance(token, str) or not isinstance(user_id, str):
        raise ValueError("auth file lacks AccessToken or User.Id")
    items, item_types, item_page_count = {"Items": []}, set(), 0
    failures = []
    for index, page in enumerate(recursive_item_pages(base_url, token, max_pages=1)):
        item_page_count += 1
        kotlin_errors = validate_kotlin("BaseItemDtoQueryResult", page)
        swift_errors = validate_swift("BaseItemDtoQueryResult", page)
        if kotlin_errors:
            failures.append(f"recursive_items_page_{index}: Kotlin {kotlin_errors[0]}")
        if swift_errors:
            failures.append(f"recursive_items_page_{index}: Swift {swift_errors[0]}")
        for item in page["Items"]:
            item_type = item.get("Type")
            if isinstance(item_type, str) and item_type not in item_types:
                item_types.add(item_type)
                items["Items"].append(item)
    cases = static_cases(user_id)
    dynamic, skipped = dynamic_cases(user_id, items)
    cases.extend(dynamic)
    details, detail_skipped = detail_cases(base_url, token)
    cases.extend(details)
    skipped.extend(detail_skipped)
    discovered, discovered_skipped = discovered_cases(base_url, token)
    cases.extend(discovered)
    skipped.extend(discovered_skipped)
    for case in cases:
        status, document = response_json(base_url, token, case.path)
        if status != 200 or document is None:
            failures.append(f"{case.name}: HTTP {status} or non-JSON response")
            continue
        kotlin_errors = validate_kotlin(case.kotlin, document)
        swift_errors = validate_swift(case.swift, document)
        if kotlin_errors:
            failures.append(f"{case.name}: Kotlin {kotlin_errors[0]}")
        if swift_errors:
            failures.append(f"{case.name}: Swift {swift_errors[0]}")
    for reason in skipped:
        print(f"SKIP {reason}")
    for failure in failures:
        print(f"FAIL {failure}")
    print(f"checked={len(cases) + item_page_count} failures={len(failures)} skipped={len(skipped)}")
    return not failures


def audit_items(base_url, auth_file, start, max_pages):
    authentication = json.loads(Path(auth_file).read_text(encoding="utf-8"))
    token = authentication.get("AccessToken")
    if not isinstance(token, str):
        raise ValueError("auth file lacks AccessToken")
    checked, failures, types = 0, [], set()
    for index, page in enumerate(recursive_item_pages(base_url, token, start, max_pages)):
        checked += 1
        kotlin_errors = validate_kotlin("BaseItemDtoQueryResult", page)
        swift_errors = validate_swift("BaseItemDtoQueryResult", page)
        if kotlin_errors:
            failures.append(f"items_page_{start + index * 200}: Kotlin {kotlin_errors[0]}")
        if swift_errors:
            failures.append(f"items_page_{start + index * 200}: Swift {swift_errors[0]}")
        types.update(item.get("Type") for item in page["Items"] if isinstance(item.get("Type"), str))
    for failure in failures:
        print(f"FAIL {failure}")
    print(f"checked_item_pages={checked} failures={len(failures)} types={','.join(sorted(types))}")
    return not failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--auth-file", required=True)
    parser.add_argument("--base-url", default="http://127.0.0.1:18096")
    parser.add_argument("--items-only", action="store_true")
    parser.add_argument("--start-index", type=int, default=0)
    parser.add_argument("--max-item-pages", type=int)
    args = parser.parse_args()
    run = audit_items if args.items_only else audit
    if args.items_only:
        success = run(args.base_url, args.auth_file, args.start_index, args.max_item_pages)
    else:
        success = run(args.base_url, args.auth_file)
    raise SystemExit(0 if success else 1)


if __name__ == "__main__":
    main()
