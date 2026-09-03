//! Startup migration routines ported from `Jellyfin.Server/Migrations`.
//!
//! Jellyfin runs these once per database and records their id in
//! `jellyfin.startup_migrations` so they never run twice. The upstream set is
//! split across `PreStartupRoutines/` (five routines that reshape XML config
//! files before the server boots) and `Routines/` (the rest), and every entry
//! is tagged with `[JellyfinMigration(id, name, guid?, Stage?, RunMigrationOnSetup?)]`.
//!
//! Each routine below keeps the upstream id, name, guid, stage and
//! `run_on_setup` flag. What differs is how the work is expressed:
//!
//! * Routines whose upstream body rewrites the database are carried over as
//!   SQL written against this port's schema. Column names are the `snake_case`
//!   ones this port uses (`item_type`, not `Type`) and data that upstream keeps
//!   in dedicated columns but this port keeps in `base_items.data` (for
//!   example `ExtraType`) is read out of the JSONB payload.
//! * Routines whose upstream body reads a legacy `SQLite` file (`users.db`,
//!   `library.db`, `displaypreferences.db`, `activitylog.db`,
//!   `authentication.db`) or walks the filesystem have no counterpart here and
//!   are marked as not applicable.
//! * Routines whose upstream body only changes in-memory or XML configuration
//!   (`DisableTranscodingThrottling`, `DisableLegacyAuthorization`,
//!   `CreateUserLoggingConfigFile`, ...) are likewise not applicable, because
//!   this port keeps server configuration in the database and seeds it
//!   directly.
//! * `RefreshInternalDateModified` is a no-op here: `base_items.date_modified`
//!   is `NOT NULL`, so upstream's "fill in the nulls" case cannot arise.
//! * `RefreshCleanNamesAndValues` only refreshes `item_values.clean_value`.
//!   `base_items.clean_name` is a stored generated column in this port, so the
//!   database already keeps it in step with `name`.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

/// The point of the startup sequence at which a routine runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MigrationStage {
    PreInitialisation = 0,
    CoreInitialisation = 1,
    AppInitialisation = 2,
    PostInitialisation = 3,
}

/// A single upstream startup migration routine.
#[derive(Clone, Copy, Debug)]
pub struct StartupMigrationRoutine {
    /// Upstream id, taken verbatim from `[JellyfinMigration]`.
    pub id: &'static str,
    /// Upstream routine name.
    pub name: &'static str,
    /// Legacy migration key, only present on routines that predate EF Core.
    pub guid: Option<&'static str>,
    pub stage: MigrationStage,
    /// Whether the routine also runs on a brand new setup.
    pub run_on_setup: bool,
    /// SQL to apply, or `None` when the routine has no database effect here.
    pub sql: Option<&'static str>,
}

/// Removes duplicate extras that share a parent and a path.
const REMOVE_DUPLICATE_EXTRAS_SQL: &str = r"
    DELETE FROM jellyfin.base_items
    WHERE id IN (
        SELECT id
        FROM (
            SELECT id,
                   ROW_NUMBER() OVER (
                       PARTITION BY parent_id, path
                       ORDER BY date_created, id
                   ) AS row_num
            FROM jellyfin.base_items
            WHERE path IS NOT NULL
              AND parent_id IS NOT NULL
              AND data ->> 'ExtraType' IS NOT NULL
        ) duplicates
        WHERE row_num > 1
    );
";

/// Seeds the default plugin repository on installs that have none.
const ADD_DEFAULT_PLUGIN_REPOSITORY_SQL: &str = r"
    UPDATE jellyfin.server_configuration
    SET plugin_repositories = jsonb_build_array(jsonb_build_object(
            'Name', 'Jellyfin Stable',
            'Url', 'https://repo.jellyfin.org/files/plugin/manifest.json',
            'Enabled', true
        ))
    WHERE plugin_repositories = '[]'::jsonb
       OR plugin_repositories IS NULL;
";

/// Re-adds the default plugin repository, leaving custom ones untouched.
const READD_DEFAULT_PLUGIN_REPOSITORY_SQL: &str = r"
    UPDATE jellyfin.server_configuration
    SET plugin_repositories = CASE
            WHEN plugin_repositories = '[]'::jsonb OR plugin_repositories IS NULL
                THEN jsonb_build_array(jsonb_build_object(
                    'Name', 'Jellyfin Stable',
                    'Url', 'https://repo.jellyfin.org/files/plugin/manifest.json',
                    'Enabled', true
                ))
            ELSE plugin_repositories
        END
    WHERE plugin_repositories = '[]'::jsonb
       OR plugin_repositories IS NULL;
";

/// Gives ownerless playlists an owner taken from their first editable share.
const FIX_PLAYLIST_OWNER_SQL: &str = r"
    WITH ownerless AS (
        SELECT playlist_id,
               (
                   SELECT (entry ->> 'UserId')::uuid
                   FROM jsonb_array_elements(shares) AS entry
                   WHERE (entry ->> 'CanEdit')::boolean IS TRUE
                   LIMIT 1
               ) AS new_owner
        FROM jellyfin.playlists
        WHERE owner_user_id IS NULL
           OR owner_user_id = '00000000-0000-0000-0000-000000000000'
    )
    UPDATE jellyfin.playlists p
    SET owner_user_id = ownerless.new_owner,
        shares = COALESCE((
            SELECT jsonb_agg(entry)
            FROM jsonb_array_elements(p.shares) AS entry
            WHERE entry ->> 'UserId' <> ownerless.new_owner::text
        ), '[]'::jsonb)
    FROM ownerless
    WHERE ownerless.playlist_id = p.playlist_id
      AND ownerless.new_owner IS NOT NULL;

    UPDATE jellyfin.playlists
    SET open_access = true
    WHERE (owner_user_id IS NULL
           OR owner_user_id = '00000000-0000-0000-0000-000000000000')
      AND NOT EXISTS (
          SELECT 1
          FROM jsonb_array_elements(shares) AS entry
          WHERE (entry ->> 'CanEdit')::boolean IS TRUE
      );
";

/// Seeds the two upstream Chromecast receiver applications.
const ADD_DEFAULT_CAST_RECEIVERS_SQL: &str = r"
    UPDATE jellyfin.server_configuration
    SET cast_receiver_applications = jsonb_build_array(
            jsonb_build_object('Id', 'F007D354', 'Name', 'Stable'),
            jsonb_build_object('Id', '6F511C87', 'Name', 'Unstable')
        )
    WHERE cast_receiver_applications = '[]'::jsonb
       OR cast_receiver_applications IS NULL;
";

/// Points the retired stable repository url at the current manifest.
const UPDATE_DEFAULT_PLUGIN_REPOSITORY_SQL: &str = r"
    UPDATE jellyfin.server_configuration
    SET plugin_repositories = (
        SELECT jsonb_agg(
            CASE
                WHEN lower(value ->> 'Url') =
                     'https://repo.jellyfin.org/releases/plugin/manifest-stable.json'
                    THEN jsonb_set(value, '{Url}',
                        to_jsonb('https://repo.jellyfin.org/files/plugin/manifest.json'::text))
                ELSE value
            END
        )
        FROM jsonb_array_elements(plugin_repositories) AS value
    )
    WHERE EXISTS (
        SELECT 1
        FROM jsonb_array_elements(plugin_repositories) AS value
        WHERE lower(value ->> 'Url') =
              'https://repo.jellyfin.org/releases/plugin/manifest-stable.json'
    );
";

/// Clears `DateTime.MinValue` sentinels that upstream stored for "no date".
const FIX_DATES_SQL: &str = r"
    UPDATE jellyfin.base_items
    SET premiere_date = NULL
    WHERE premiere_date IS NOT NULL
      AND premiere_date < '0001-01-02 00:00:00+00'::timestamptz;
";

/// Re-derives `is_folder` from the item type.
const RESEED_FOLDER_FLAG_SQL: &str = r"
    UPDATE jellyfin.base_items
    SET is_folder = true
    WHERE is_folder IS DISTINCT FROM true
      AND item_type IN (
          'AggregateFolder', 'BasePluginFolder', 'BoxSet', 'Channel',
          'CollectionFolder', 'Folder', 'ManualPlaylistsFolder', 'MusicAlbum',
          'MusicArtist', 'PhotoAlbum', 'Photo', 'Playlist', 'PlaylistsFolder',
          'Season', 'Series', 'UserRootFolder', 'UserView'
      );
";

/// Drops the artist person links the 10.11 release candidates created.
const CLEAN_MUSIC_ARTIST_SQL: &str = r"
    DELETE FROM jellyfin.people_base_item_map
    WHERE person_type IN ('Artist', 'AlbumArtist');

    DELETE FROM jellyfin.people p
    WHERE NOT EXISTS (
        SELECT 1
        FROM jellyfin.people_base_item_map m
        WHERE m.person_id = p.id
    );
";

/// Repairs `linked_children` and the alternate-version back reference.
const MIGRATE_LINKED_CHILDREN_SQL: &str = r"
    DELETE FROM jellyfin.linked_children lc
    WHERE NOT EXISTS (SELECT 1 FROM jellyfin.base_items b WHERE b.id = lc.child_id)
       OR NOT EXISTS (SELECT 1 FROM jellyfin.base_items b WHERE b.id = lc.parent_id);

    UPDATE jellyfin.base_items child
    SET primary_version_id = lc.parent_id
    FROM jellyfin.linked_children lc
    WHERE lc.child_id = child.id
      AND lc.child_type IN (2, 3)
      AND (child.primary_version_id IS NULL
           OR child.primary_version_id <> lc.parent_id);
";

/// Removes repeated rows for one path and fills in alternate-version links.
const FIX_INCORRECT_OWNER_ID_RELATIONSHIPS_SQL: &str = r"
    DELETE FROM jellyfin.base_items
    WHERE id IN (
        SELECT id
        FROM (
            SELECT id,
                   ROW_NUMBER() OVER (
                       PARTITION BY path
                       ORDER BY date_created, id
                   ) AS row_num
            FROM jellyfin.base_items
            WHERE path IS NOT NULL
        ) duplicates
        WHERE row_num > 1
    );

    UPDATE jellyfin.base_items child
    SET primary_version_id = lc.parent_id
    FROM jellyfin.linked_children lc
    WHERE lc.child_id = child.id
      AND lc.child_type IN (2, 3)
      AND (child.primary_version_id IS NULL
           OR child.primary_version_id <> lc.parent_id);
";

/// Merges `MusicArtist` rows that differ only by name casing.
const MERGE_DUPLICATE_MUSIC_ARTISTS_SQL: &str = r"
    WITH ranked AS (
        SELECT b.id,
               lower(b.name) AS name_key,
               COALESCE(child.child_count, 0) AS child_count,
               COALESCE(anc.ancestor_count, 0) AS ancestor_count,
               COALESCE(lnk.linked_count, 0) AS linked_count,
               b.date_created
        FROM jellyfin.base_items b
        LEFT JOIN (
            SELECT parent_id, COUNT(*) AS child_count
            FROM jellyfin.base_items
            WHERE parent_id IS NOT NULL
            GROUP BY parent_id
        ) child ON child.parent_id = b.id
        LEFT JOIN (
            SELECT parent_item_id, COUNT(*) AS ancestor_count
            FROM jellyfin.ancestor_ids
            GROUP BY parent_item_id
        ) anc ON anc.parent_item_id = b.id
        LEFT JOIN (
            SELECT parent_id, COUNT(*) AS linked_count
            FROM jellyfin.linked_children
            GROUP BY parent_id
        ) lnk ON lnk.parent_id = b.id
        WHERE b.item_type = 'MusicArtist'
          AND b.name IS NOT NULL
    ),
    keepers AS (
        SELECT DISTINCT ON (name_key) id AS keeper_id, name_key
        FROM ranked
        ORDER BY name_key, child_count DESC, ancestor_count DESC,
                 linked_count DESC, date_created, id
    ),
    dupes AS (
        SELECT r.id AS dupe_id, k.keeper_id
        FROM ranked r
        JOIN keepers k ON k.name_key = r.name_key
        WHERE r.id <> k.keeper_id
    ),
    reparented AS (
        UPDATE jellyfin.base_items
        SET parent_id = dupes.keeper_id
        FROM dupes
        WHERE jellyfin.base_items.parent_id = dupes.dupe_id
        RETURNING 1
    ),
    ancestor_collisions AS (
        DELETE FROM jellyfin.ancestor_ids a
        USING dupes d
        WHERE a.parent_item_id = d.dupe_id
          AND EXISTS (
              SELECT 1
              FROM jellyfin.ancestor_ids k
              WHERE k.item_id = a.item_id
                AND k.depth = a.depth
                AND k.parent_item_id <> a.parent_item_id
          )
        RETURNING 1
    ),
    ancestor_moved AS (
        UPDATE jellyfin.ancestor_ids a
        SET parent_item_id = d.keeper_id
        FROM dupes d
        WHERE a.parent_item_id = d.dupe_id
        RETURNING 1
    ),
    linked_parent_collisions AS (
        DELETE FROM jellyfin.linked_children l
        USING dupes d
        WHERE l.parent_id = d.dupe_id
          AND EXISTS (
              SELECT 1
              FROM jellyfin.linked_children k
              WHERE k.parent_id = d.keeper_id
                AND k.child_id = l.child_id
          )
        RETURNING 1
    ),
    linked_parent_moved AS (
        UPDATE jellyfin.linked_children l
        SET parent_id = d.keeper_id
        FROM dupes d
        WHERE l.parent_id = d.dupe_id
        RETURNING 1
    ),
    linked_child_collisions AS (
        DELETE FROM jellyfin.linked_children l
        USING dupes d
        WHERE l.child_id = d.dupe_id
          AND EXISTS (
              SELECT 1
              FROM jellyfin.linked_children k
              WHERE k.child_id = d.keeper_id
                AND k.parent_id = l.parent_id
          )
        RETURNING 1
    ),
    linked_child_moved AS (
        UPDATE jellyfin.linked_children l
        SET child_id = d.keeper_id
        FROM dupes d
        WHERE l.child_id = d.dupe_id
        RETURNING 1
    ),
    user_data_collisions AS (
        DELETE FROM jellyfin.user_data u
        USING dupes d
        WHERE u.item_id = d.dupe_id
          AND EXISTS (
              SELECT 1
              FROM jellyfin.user_data k
              WHERE k.item_id = d.keeper_id
                AND k.user_id = u.user_id
                AND k.custom_data_key = u.custom_data_key
          )
        RETURNING 1
    ),
    user_data_moved AS (
        UPDATE jellyfin.user_data u
        SET item_id = d.keeper_id
        FROM dupes d
        WHERE u.item_id = d.dupe_id
        RETURNING 1
    ),
    removed AS (
        DELETE FROM jellyfin.base_items b
        USING dupes d
        WHERE b.id = d.dupe_id
        RETURNING 1
    )
    SELECT 1;
";

/// Merges `Person` rows that differ only by name casing.
const MERGE_DUPLICATE_PEOPLE_SQL: &str = r"
    WITH ranked AS (
        SELECT b.id,
               lower(b.name) AS name_key,
               COALESCE(ud.user_data_count, 0) AS user_data_count,
               COALESCE(lnk.linked_count, 0) AS linked_count,
               b.date_created
        FROM jellyfin.base_items b
        LEFT JOIN (
            SELECT item_id, COUNT(*) AS user_data_count
            FROM jellyfin.user_data
            GROUP BY item_id
        ) ud ON ud.item_id = b.id
        LEFT JOIN (
            SELECT item_id, COUNT(*) AS linked_count
            FROM (
                SELECT parent_id AS item_id FROM jellyfin.linked_children
                UNION ALL
                SELECT child_id FROM jellyfin.linked_children
            ) entries
            GROUP BY item_id
        ) lnk ON lnk.item_id = b.id
        WHERE b.item_type = 'Person'
          AND b.name IS NOT NULL
    ),
    keepers AS (
        SELECT DISTINCT ON (name_key) id AS keeper_id, name_key
        FROM ranked
        ORDER BY name_key, user_data_count DESC, linked_count DESC,
                 date_created, id
    ),
    dupes AS (
        SELECT r.id AS dupe_id, k.keeper_id
        FROM ranked r
        JOIN keepers k ON k.name_key = r.name_key
        WHERE r.id <> k.keeper_id
    ),
    reparented AS (
        UPDATE jellyfin.base_items
        SET parent_id = dupes.keeper_id
        FROM dupes
        WHERE jellyfin.base_items.parent_id = dupes.dupe_id
        RETURNING 1
    ),
    ancestor_collisions AS (
        DELETE FROM jellyfin.ancestor_ids a
        USING dupes d
        WHERE a.parent_item_id = d.dupe_id
          AND EXISTS (
              SELECT 1
              FROM jellyfin.ancestor_ids k
              WHERE k.item_id = a.item_id
                AND k.depth = a.depth
                AND k.parent_item_id <> a.parent_item_id
          )
        RETURNING 1
    ),
    ancestor_moved AS (
        UPDATE jellyfin.ancestor_ids a
        SET parent_item_id = d.keeper_id
        FROM dupes d
        WHERE a.parent_item_id = d.dupe_id
        RETURNING 1
    ),
    linked_parent_collisions AS (
        DELETE FROM jellyfin.linked_children l
        USING dupes d
        WHERE l.parent_id = d.dupe_id
          AND EXISTS (
              SELECT 1
              FROM jellyfin.linked_children k
              WHERE k.parent_id = d.keeper_id
                AND k.child_id = l.child_id
          )
        RETURNING 1
    ),
    linked_parent_moved AS (
        UPDATE jellyfin.linked_children l
        SET parent_id = d.keeper_id
        FROM dupes d
        WHERE l.parent_id = d.dupe_id
        RETURNING 1
    ),
    linked_child_collisions AS (
        DELETE FROM jellyfin.linked_children l
        USING dupes d
        WHERE l.child_id = d.dupe_id
          AND EXISTS (
              SELECT 1
              FROM jellyfin.linked_children k
              WHERE k.child_id = d.keeper_id
                AND k.parent_id = l.parent_id
          )
        RETURNING 1
    ),
    linked_child_moved AS (
        UPDATE jellyfin.linked_children l
        SET child_id = d.keeper_id
        FROM dupes d
        WHERE l.child_id = d.dupe_id
        RETURNING 1
    ),
    user_data_collisions AS (
        DELETE FROM jellyfin.user_data u
        USING dupes d
        WHERE u.item_id = d.dupe_id
          AND EXISTS (
              SELECT 1
              FROM jellyfin.user_data k
              WHERE k.item_id = d.keeper_id
                AND k.user_id = u.user_id
                AND k.custom_data_key = u.custom_data_key
          )
        RETURNING 1
    ),
    user_data_moved AS (
        UPDATE jellyfin.user_data u
        SET item_id = d.keeper_id
        FROM dupes d
        WHERE u.item_id = d.dupe_id
        RETURNING 1
    ),
    removed AS (
        DELETE FROM jellyfin.base_items b
        USING dupes d
        WHERE b.id = d.dupe_id
        RETURNING 1
    )
    SELECT 1;
";

/// Keeps `normalized_username` in step with the invariant-uppercased name.
const UPDATE_NORMALIZED_USERNAME_SQL: &str = r"
    UPDATE jellyfin.users
    SET normalized_username = UPPER(username)
    WHERE normalized_username IS NULL
       OR normalized_username <> UPPER(username);
";

/// Refreshes `item_values.clean_value` from `value`.
///
/// `base_items.clean_name` needs no refresh: it is a stored generated column.
const REFRESH_CLEAN_NAMES_AND_VALUES_SQL: &str = r"
    UPDATE jellyfin.item_values
    SET clean_value = jellyfin.normalize_search_text(value)
    WHERE btrim(value) <> ''
      AND jellyfin.normalize_search_text(value) <> ''
      AND clean_value IS DISTINCT FROM jellyfin.normalize_search_text(value);
";

/// Re-derives the season and episode presentation keys from the series id.
const RECOMPUTE_SERIES_PRESENTATION_KEY_SQL: &str = r"
    UPDATE jellyfin.base_items
    SET series_presentation_unique_key = replace(series_id::text, '-', '')
    WHERE series_id IS NOT NULL
      AND series_presentation_unique_key IS DISTINCT FROM
          replace(series_id::text, '-', '');

    UPDATE jellyfin.base_items
    SET presentation_unique_key =
            replace(series_id::text, '-', '') || '_' || index_number
    WHERE item_type = 'Season'
      AND series_id IS NOT NULL
      AND index_number IS NOT NULL
      AND presentation_unique_key IS DISTINCT FROM
          replace(series_id::text, '-', '') || '_' || index_number;
";

/// Every upstream startup routine, ordered by id.
pub const ALL_STARTUP_ROUTINES: &[StartupMigrationRoutine] = &[
    StartupMigrationRoutine {
        id: "2025-04-20T00:00:00",
        name: "CreateNetworkConfiguration",
        guid: Some("9B354818-94D5-4B68-AC49-E35CB85F9D84"),
        stage: MigrationStage::PreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T01:00:00",
        name: "MigrateNetworkConfiguration",
        guid: Some("4FB5C950-1991-11EE-9B4B-0800200C9A66"),
        stage: MigrationStage::PreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T02:00:00",
        name: "MigrateMusicBrainzTimeout",
        guid: Some("A6DCACF4-C057-4Ef9-80D3-61CEF9DDB4F0"),
        stage: MigrationStage::PreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T03:00:00",
        name: "MigrateEncodingOptions",
        guid: Some("A8E61960-7726-4450-8F3D-82C12DAABBCB"),
        stage: MigrationStage::PreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T04:00:00",
        name: "RenameEnableGroupingIntoCollections",
        guid: Some("E73B777D-CD5C-4E71-957A-B86B3660B7CF"),
        stage: MigrationStage::PreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T05:00:00",
        name: "DisableTranscodingThrottling",
        guid: Some("4124C2CD-E939-4FFB-9BE9-9B311C413638"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T06:00:00",
        name: "CreateUserLoggingConfigFile",
        guid: Some("EF103419-8451-40D8-9F34-D1A8E93A1679"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T07:00:00",
        name: "MigrateActivityLogDb",
        guid: Some("3793eb59-bc8c-456c-8b9f-bd5a62a42978"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T08:00:00",
        name: "RemoveDuplicateExtras",
        guid: Some("ACBE17B7-8435-4A83-8B64-6FCF162CB9BD"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(REMOVE_DUPLICATE_EXTRAS_SQL),
    },
    StartupMigrationRoutine {
        id: "2025-04-20T09:00:00",
        name: "AddDefaultPluginRepository",
        guid: Some("EB58EBEE-9514-4B9B-8225-12E1A40020DF"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: true,
        sql: Some(ADD_DEFAULT_PLUGIN_REPOSITORY_SQL),
    },
    StartupMigrationRoutine {
        id: "2025-04-20T10:00:00",
        name: "MigrateUserDb",
        guid: Some("5C4B82A2-F053-4009-BD05-B6FCAD82F14C"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T11:00:00",
        name: "ReaddDefaultPluginRepository",
        guid: Some("5F86E7F6-D966-4C77-849D-7A7B40B68C4E"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: true,
        sql: Some(READD_DEFAULT_PLUGIN_REPOSITORY_SQL),
    },
    StartupMigrationRoutine {
        id: "2025-04-20T12:00:00",
        name: "MigrateDisplayPreferencesDb",
        guid: Some("06387815-C3CC-421F-A888-FB5F9992BEA8"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T13:00:00",
        name: "RemoveDownloadImagesInAdvance",
        guid: Some("A81F75E0-8F43-416F-A5E8-516CCAB4D8CC"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T14:00:00",
        name: "MigrateAuthenticationDb",
        guid: Some("5BD72F41-E6F3-4F60-90AA-09869ABE0E22"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T15:00:00",
        name: "FixPlaylistOwner",
        guid: Some("615DFA9E-2497-4DBB-A472-61938B752C5B"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(FIX_PLAYLIST_OWNER_SQL),
    },
    StartupMigrationRoutine {
        id: "2025-04-20T16:00:00",
        name: "AddDefaultCastReceivers",
        guid: Some("34A1A1C4-5572-418E-A2F8-32CDFE2668E8"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: true,
        sql: Some(ADD_DEFAULT_CAST_RECEIVERS_SQL),
    },
    StartupMigrationRoutine {
        id: "2025-04-20T17:00:00",
        name: "UpdateDefaultPluginRepository",
        guid: Some("852816E0-2712-49A9-9240-C6FC5FCAD1A8"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: true,
        sql: Some(UPDATE_DEFAULT_PLUGIN_REPOSITORY_SQL),
    },
    StartupMigrationRoutine {
        id: "2025-04-20T18:00:00",
        name: "FixAudioData",
        guid: Some("CF6FABC2-9FBE-4933-84A5-FFE52EF22A58"),
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T19:30:00",
        name: "MigrateLibraryDbCompatibilityCheck",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T20:00:00",
        name: "MigrateLibraryDb",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T21:00:00",
        name: "MoveExtractedFiles",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T23:00:00",
        name: "MoveTrickplayFiles",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: true,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-20T23:00:00",
        name: "RefreshInternalDateModified",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-04-21T00:00:00",
        name: "MigrateKeyframeData",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-06-18T01:00:00",
        name: "MigrateLibraryUserData",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2025-06-20T18:00:00",
        name: "FixDates",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(FIX_DATES_SQL),
    },
    StartupMigrationRoutine {
        id: "2025-07-30T21:50:00",
        name: "ReseedFolderFlag",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(RESEED_FOLDER_FLAG_SQL),
    },
    StartupMigrationRoutine {
        id: "2025-10-09T20:00:00",
        name: "CleanMusicArtist",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(CLEAN_MUSIC_ARTIST_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-01-13T12:00:00",
        name: "MigrateLinkedChildren",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(MIGRATE_LINKED_CHILDREN_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-01-13T23:00:00",
        name: "CleanupOrphanedExtras",
        guid: None,
        stage: MigrationStage::AppInitialisation,
        run_on_setup: false,
        sql: Some(REMOVE_DUPLICATE_EXTRAS_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-01-15T12:00:00",
        name: "FixIncorrectOwnerIdRelationships",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(FIX_INCORRECT_OWNER_ID_RELATIONSHIPS_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-02-06T20:00:00",
        name: "FixLibrarySubtitleDownloadLanguages",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2026-03-02T09:00:00",
        name: "MigrateRatingLevels",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2026-05-08T12:00:00",
        name: "MergeDuplicateMusicArtists",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(MERGE_DUPLICATE_MUSIC_ARTISTS_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-05-08T13:00:00",
        name: "MergeDuplicatePeople",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(MERGE_DUPLICATE_PEOPLE_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-05-22T09:23:04",
        name: "UpdateNormalizedUsername",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(UPDATE_NORMALIZED_USERNAME_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-05-25T01:00:00",
        name: "CleanupOrphanedExternalData",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2026-05-31T16:00:00",
        name: "DisableLegacyAuthorization",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2026-06-10T12:00:00",
        name: "RefreshCleanNamesAndValues",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(REFRESH_CLEAN_NAMES_AND_VALUES_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-07-22T12:00:00",
        name: "RefreshForcedSortNames",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2026-07-29T12:00:00",
        name: "RestorePlaylistChildrenFromMetadata",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
    StartupMigrationRoutine {
        id: "2026-08-21T12:00:00",
        name: "RecomputeSeriesPresentationKey",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: Some(RECOMPUTE_SERIES_PRESENTATION_KEY_SQL),
    },
    StartupMigrationRoutine {
        id: "2026-08-25T20:00:00",
        name: "ConsolidateLocalizedUserViews",
        guid: None,
        stage: MigrationStage::CoreInitialisation,
        run_on_setup: false,
        sql: None,
    },
];

/// Runs the startup migration routines.
pub struct StartupMigrationRunner;

impl StartupMigrationRunner {
    /// Applies every routine that has not run yet and returns their ids.
    ///
    /// On a brand new setup the routines that only make sense against a
    /// pre-existing library are recorded without running, which mirrors
    /// upstream seeding its migration history for a fresh install.
    ///
    /// # Errors
    ///
    /// Returns `DbErr` when the migration bookkeeping table cannot be created
    /// or a routine's SQL fails.
    pub async fn run(
        manager: &SchemaManager<'_>,
        is_new_setup: bool,
    ) -> Result<Vec<String>, DbErr> {
        let connection = manager.get_connection();

        connection
            .execute_unprepared(
                r"
                CREATE TABLE IF NOT EXISTS jellyfin.startup_migrations (
                    routine_id text PRIMARY KEY,
                    applied_at timestamptz NOT NULL DEFAULT clock_timestamp()
                );
                ",
            )
            .await?;

        let rows = connection
            .query_all(Statement::from_string(
                DbBackend::Postgres,
                "SELECT routine_id FROM jellyfin.startup_migrations".to_owned(),
            ))
            .await?;

        let applied_ids = rows
            .into_iter()
            .filter_map(|row| row.try_get::<String>("", "routine_id").ok())
            .collect::<std::collections::HashSet<_>>();

        let mut applied_now = Vec::new();

        for routine in ALL_STARTUP_ROUTINES {
            let routine_id = routine.id.to_owned();
            if applied_ids.contains(&routine_id) {
                continue;
            }

            if is_new_setup && !routine.run_on_setup {
                // Nothing to migrate on a fresh install, but the routine still
                // has to be recorded so it is not attempted on a later start.
                Self::mark_applied(connection, &routine_id).await?;
                applied_now.push(routine_id);
                continue;
            }

            if let Some(sql) = routine.sql {
                connection.execute_unprepared(sql).await?;
            }

            Self::mark_applied(connection, &routine_id).await?;
            applied_now.push(routine_id);
        }

        Ok(applied_now)
    }

    async fn mark_applied(
        connection: &impl ConnectionTrait,
        routine_id: &str,
    ) -> Result<(), DbErr> {
        connection
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO jellyfin.startup_migrations (routine_id) VALUES ($1) \
                 ON CONFLICT (routine_id) DO NOTHING",
                [routine_id.to_owned().into()],
            ))
            .await?;
        Ok(())
    }
}
