use sea_orm_migration::prelude::*;

mod m20260722_000001_create_users;
mod m20260722_000002_create_activity_logs;
mod m20260722_000003_create_authentication;
mod m20260722_000004_create_user_data;
mod m20260722_000005_create_base_items;
mod m20260722_000006_create_item_values;
mod m20260722_000007_create_virtual_folders;
mod m20260722_000008_create_people;
mod m20260722_000009_optimize_item_queries;
mod m20260722_000010_create_quick_connect;
mod m20260722_000011_normalize_base_item_search;
mod m20260722_000012_add_alternate_item_versions;
mod m20260722_000013_optimize_version_playback;
mod m20260722_000014_create_tuner_hosts;
mod m20260722_000015_add_user_policy_providers;
mod m20260722_000016_create_server_configuration;
mod m20260722_000017_add_content_type_overrides;
mod m20260722_000018_create_user_profile_images;
mod m20260722_000019_create_base_item_images;
mod m20260722_000020_create_keyframe_data;
mod m20260722_000021_create_media_streams;
mod m20260722_000022_create_media_attachments;
mod m20260722_000023_add_playstate_resume_configuration;
mod m20260722_000024_add_client_log_upload_configuration;
mod m20260722_000025_optimize_device_session_queries;
mod m20260723_000026_add_device_capabilities;
mod m20260723_000027_create_device_options;
mod m20260723_000028_create_session_command_outbox;
mod m20260723_000029_add_session_now_viewing;
mod m20260723_000030_add_session_additional_users;
mod m20260723_000031_add_session_playback_state;
mod m20260723_000032_create_password_resets;
mod m20260724_000033_create_display_preferences;
mod m20260724_000034_optimize_year_queries;
mod m20260724_000035_add_base_item_official_rating;
mod m20260724_000036_add_plugin_repositories;
mod m20260724_000037_create_named_configurations;
mod m20260724_000038_add_base_item_premiere_date;
mod m20260725_000039_create_trickplay_infos;
mod m20260725_000040_create_linked_children;
mod m20260725_000041_optimize_trickplay_manifests;
mod m20260725_000042_add_trickplay_configuration;
mod m20260725_000043_create_playlists;
mod m20260725_000044_add_remote_access_configuration;

#[doc(hidden)]
pub use m20260722_000001_create_users::Migration as CreateUsersMigration;
#[doc(hidden)]
pub use m20260722_000002_create_activity_logs::Migration as CreateActivityLogsMigration;
#[doc(hidden)]
pub use m20260722_000003_create_authentication::Migration as CreateAuthenticationMigration;
#[doc(hidden)]
pub use m20260722_000004_create_user_data::Migration as CreateUserDataMigration;
#[doc(hidden)]
pub use m20260722_000005_create_base_items::Migration as CreateBaseItemsMigration;
#[doc(hidden)]
pub use m20260722_000006_create_item_values::Migration as CreateItemValuesMigration;
#[doc(hidden)]
pub use m20260722_000007_create_virtual_folders::Migration as CreateVirtualFoldersMigration;
#[doc(hidden)]
pub use m20260722_000008_create_people::Migration as CreatePeopleMigration;
#[doc(hidden)]
pub use m20260722_000009_optimize_item_queries::Migration as OptimizeItemQueriesMigration;
#[doc(hidden)]
pub use m20260722_000010_create_quick_connect::Migration as CreateQuickConnectMigration;
#[doc(hidden)]
pub use m20260722_000011_normalize_base_item_search::Migration as NormalizeBaseItemSearchMigration;
#[doc(hidden)]
pub use m20260722_000012_add_alternate_item_versions::Migration as AddAlternateItemVersionsMigration;
#[doc(hidden)]
pub use m20260722_000013_optimize_version_playback::Migration as OptimizeVersionPlaybackMigration;
#[doc(hidden)]
pub use m20260722_000014_create_tuner_hosts::Migration as CreateTunerHostsMigration;
#[doc(hidden)]
pub use m20260722_000015_add_user_policy_providers::Migration as AddUserPolicyProvidersMigration;
#[doc(hidden)]
pub use m20260722_000016_create_server_configuration::Migration as CreateServerConfigurationMigration;
#[doc(hidden)]
pub use m20260722_000017_add_content_type_overrides::Migration as AddContentTypeOverridesMigration;
#[doc(hidden)]
pub use m20260722_000018_create_user_profile_images::Migration as CreateUserProfileImagesMigration;
#[doc(hidden)]
pub use m20260722_000019_create_base_item_images::Migration as CreateBaseItemImagesMigration;
#[doc(hidden)]
pub use m20260722_000020_create_keyframe_data::Migration as CreateKeyframeDataMigration;
#[doc(hidden)]
pub use m20260722_000021_create_media_streams::Migration as CreateMediaStreamsMigration;
#[doc(hidden)]
pub use m20260722_000022_create_media_attachments::Migration as CreateMediaAttachmentsMigration;
#[doc(hidden)]
pub use m20260722_000023_add_playstate_resume_configuration::Migration as AddPlaystateResumeConfigurationMigration;
#[doc(hidden)]
pub use m20260722_000024_add_client_log_upload_configuration::Migration as AddClientLogUploadConfigurationMigration;
#[doc(hidden)]
pub use m20260722_000025_optimize_device_session_queries::Migration as OptimizeDeviceSessionQueriesMigration;
#[doc(hidden)]
pub use m20260723_000026_add_device_capabilities::Migration as AddDeviceCapabilitiesMigration;
#[doc(hidden)]
pub use m20260723_000027_create_device_options::Migration as CreateDeviceOptionsMigration;
#[doc(hidden)]
pub use m20260723_000028_create_session_command_outbox::Migration as CreateSessionCommandOutboxMigration;
#[doc(hidden)]
pub use m20260723_000029_add_session_now_viewing::Migration as AddSessionNowViewingMigration;
#[doc(hidden)]
pub use m20260723_000030_add_session_additional_users::Migration as AddSessionAdditionalUsersMigration;
#[doc(hidden)]
pub use m20260723_000031_add_session_playback_state::Migration as AddSessionPlaybackStateMigration;
#[doc(hidden)]
pub use m20260723_000032_create_password_resets::Migration as CreatePasswordResetsMigration;
#[doc(hidden)]
pub use m20260724_000033_create_display_preferences::Migration as CreateDisplayPreferencesMigration;
#[doc(hidden)]
pub use m20260724_000034_optimize_year_queries::Migration as OptimizeYearQueriesMigration;
#[doc(hidden)]
pub use m20260724_000035_add_base_item_official_rating::Migration as AddBaseItemOfficialRatingMigration;
#[doc(hidden)]
pub use m20260724_000036_add_plugin_repositories::Migration as AddPluginRepositoriesMigration;
#[doc(hidden)]
pub use m20260724_000037_create_named_configurations::Migration as CreateNamedConfigurationsMigration;
#[doc(hidden)]
pub use m20260724_000038_add_base_item_premiere_date::Migration as AddBaseItemPremiereDateMigration;
#[doc(hidden)]
pub use m20260725_000039_create_trickplay_infos::Migration as CreateTrickplayInfosMigration;
#[doc(hidden)]
pub use m20260725_000040_create_linked_children::Migration as CreateLinkedChildrenMigration;
#[doc(hidden)]
pub use m20260725_000041_optimize_trickplay_manifests::Migration as OptimizeTrickplayManifestsMigration;
#[doc(hidden)]
pub use m20260725_000042_add_trickplay_configuration::Migration as AddTrickplayConfigurationMigration;
#[doc(hidden)]
pub use m20260725_000043_create_playlists::Migration as CreatePlaylistsMigration;
#[doc(hidden)]
pub use m20260725_000044_add_remote_access_configuration::Migration as AddRemoteAccessConfigurationMigration;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260722_000001_create_users::Migration),
            Box::new(m20260722_000002_create_activity_logs::Migration),
            Box::new(m20260722_000003_create_authentication::Migration),
            Box::new(m20260722_000004_create_user_data::Migration),
            Box::new(m20260722_000005_create_base_items::Migration),
            Box::new(m20260722_000006_create_item_values::Migration),
            Box::new(m20260722_000007_create_virtual_folders::Migration),
            Box::new(m20260722_000008_create_people::Migration),
            Box::new(m20260722_000009_optimize_item_queries::Migration),
            Box::new(m20260722_000010_create_quick_connect::Migration),
            Box::new(m20260722_000011_normalize_base_item_search::Migration),
            Box::new(m20260722_000012_add_alternate_item_versions::Migration),
            Box::new(m20260722_000013_optimize_version_playback::Migration),
            Box::new(m20260722_000014_create_tuner_hosts::Migration),
            Box::new(m20260722_000015_add_user_policy_providers::Migration),
            Box::new(m20260722_000016_create_server_configuration::Migration),
            Box::new(m20260722_000017_add_content_type_overrides::Migration),
            Box::new(m20260722_000018_create_user_profile_images::Migration),
            Box::new(m20260722_000019_create_base_item_images::Migration),
            Box::new(m20260722_000020_create_keyframe_data::Migration),
            Box::new(m20260722_000021_create_media_streams::Migration),
            Box::new(m20260722_000022_create_media_attachments::Migration),
            Box::new(m20260722_000023_add_playstate_resume_configuration::Migration),
            Box::new(m20260722_000024_add_client_log_upload_configuration::Migration),
            Box::new(m20260722_000025_optimize_device_session_queries::Migration),
            Box::new(m20260723_000026_add_device_capabilities::Migration),
            Box::new(m20260723_000027_create_device_options::Migration),
            Box::new(m20260723_000028_create_session_command_outbox::Migration),
            Box::new(m20260723_000029_add_session_now_viewing::Migration),
            Box::new(m20260723_000030_add_session_additional_users::Migration),
            Box::new(m20260723_000031_add_session_playback_state::Migration),
            Box::new(m20260723_000032_create_password_resets::Migration),
            Box::new(m20260724_000033_create_display_preferences::Migration),
            Box::new(m20260724_000034_optimize_year_queries::Migration),
            Box::new(m20260724_000035_add_base_item_official_rating::Migration),
            Box::new(m20260724_000036_add_plugin_repositories::Migration),
            Box::new(m20260724_000037_create_named_configurations::Migration),
            Box::new(m20260724_000038_add_base_item_premiere_date::Migration),
            Box::new(m20260725_000039_create_trickplay_infos::Migration),
            Box::new(m20260725_000040_create_linked_children::Migration),
            Box::new(m20260725_000041_optimize_trickplay_manifests::Migration),
            Box::new(m20260725_000042_add_trickplay_configuration::Migration),
            Box::new(m20260725_000043_create_playlists::Migration),
            Box::new(m20260725_000044_add_remote_access_configuration::Migration),
        ]
    }
}
