use jellyfin_migration::{ALL_STARTUP_ROUTINES, MigrationStage};

/// Every upstream routine id, in the order upstream applies them.
const OFFICIAL_ROUTINE_IDS: &[&str] = &[
    "2025-04-20T00:00:00",
    "2025-04-20T01:00:00",
    "2025-04-20T02:00:00",
    "2025-04-20T03:00:00",
    "2025-04-20T04:00:00",
    "2025-04-20T05:00:00",
    "2025-04-20T06:00:00",
    "2025-04-20T07:00:00",
    "2025-04-20T08:00:00",
    "2025-04-20T09:00:00",
    "2025-04-20T10:00:00",
    "2025-04-20T11:00:00",
    "2025-04-20T12:00:00",
    "2025-04-20T13:00:00",
    "2025-04-20T14:00:00",
    "2025-04-20T15:00:00",
    "2025-04-20T16:00:00",
    "2025-04-20T17:00:00",
    "2025-04-20T18:00:00",
    "2025-04-20T19:30:00",
    "2025-04-20T20:00:00",
    "2025-04-20T21:00:00",
    "2025-04-20T23:00:00",
    "2025-04-20T23:00:00",
    "2025-04-21T00:00:00",
    "2025-06-18T01:00:00",
    "2025-06-20T18:00:00",
    "2025-07-30T21:50:00",
    "2025-10-09T20:00:00",
    "2026-01-13T12:00:00",
    "2026-01-13T23:00:00",
    "2026-01-15T12:00:00",
    "2026-02-06T20:00:00",
    "2026-03-02T09:00:00",
    "2026-05-08T12:00:00",
    "2026-05-08T13:00:00",
    "2026-05-22T09:23:04",
    "2026-05-25T01:00:00",
    "2026-05-31T16:00:00",
    "2026-06-10T12:00:00",
    "2026-07-22T12:00:00",
    "2026-07-29T12:00:00",
    "2026-08-21T12:00:00",
    "2026-08-25T20:00:00",
];

#[test]
fn startup_routines_match_the_official_set() {
    let ids = ALL_STARTUP_ROUTINES
        .iter()
        .map(|routine| routine.id)
        .collect::<Vec<_>>();

    assert_eq!(ids, OFFICIAL_ROUTINE_IDS);
}

#[test]
fn startup_routines_carry_the_official_names_and_guids() {
    let expected = [
        ("CreateNetworkConfiguration", Some("9B354818-94D5-4B68-AC49-E35CB85F9D84")),
        ("MigrateNetworkConfiguration", Some("4FB5C950-1991-11EE-9B4B-0800200C9A66")),
        ("MigrateMusicBrainzTimeout", Some("A6DCACF4-C057-4Ef9-80D3-61CEF9DDB4F0")),
        ("MigrateEncodingOptions", Some("A8E61960-7726-4450-8F3D-82C12DAABBCB")),
        ("RenameEnableGroupingIntoCollections", Some("E73B777D-CD5C-4E71-957A-B86B3660B7CF")),
        ("DisableTranscodingThrottling", Some("4124C2CD-E939-4FFB-9BE9-9B311C413638")),
        ("CreateUserLoggingConfigFile", Some("EF103419-8451-40D8-9F34-D1A8E93A1679")),
        ("MigrateActivityLogDb", Some("3793eb59-bc8c-456c-8b9f-bd5a62a42978")),
        ("RemoveDuplicateExtras", Some("ACBE17B7-8435-4A83-8B64-6FCF162CB9BD")),
        ("AddDefaultPluginRepository", Some("EB58EBEE-9514-4B9B-8225-12E1A40020DF")),
        ("MigrateUserDb", Some("5C4B82A2-F053-4009-BD05-B6FCAD82F14C")),
        ("ReaddDefaultPluginRepository", Some("5F86E7F6-D966-4C77-849D-7A7B40B68C4E")),
        ("MigrateDisplayPreferencesDb", Some("06387815-C3CC-421F-A888-FB5F9992BEA8")),
        ("RemoveDownloadImagesInAdvance", Some("A81F75E0-8F43-416F-A5E8-516CCAB4D8CC")),
        ("MigrateAuthenticationDb", Some("5BD72F41-E6F3-4F60-90AA-09869ABE0E22")),
        ("FixPlaylistOwner", Some("615DFA9E-2497-4DBB-A472-61938B752C5B")),
        ("AddDefaultCastReceivers", Some("34A1A1C4-5572-418E-A2F8-32CDFE2668E8")),
        ("UpdateDefaultPluginRepository", Some("852816E0-2712-49A9-9240-C6FC5FCAD1A8")),
        ("FixAudioData", Some("CF6FABC2-9FBE-4933-84A5-FFE52EF22A58")),
    ];

    for (name, guid) in expected {
        let routine = ALL_STARTUP_ROUTINES
            .iter()
            .find(|routine| routine.name == name)
            .unwrap_or_else(|| panic!("missing routine {name}"));
        assert_eq!(routine.guid, guid, "unexpected guid for {name}");
    }

    // Everything past the EF migration cutover has no legacy key.
    for routine in ALL_STARTUP_ROUTINES {
        let has_guid_by_era = routine.id < "2025-04-20T19:30:00";
        assert_eq!(
            routine.guid.is_some(),
            has_guid_by_era,
            "unexpected legacy key presence for {}",
            routine.name
        );
    }
}

#[test]
fn only_the_official_setup_routines_run_on_a_fresh_install() {
    let setup_routines = ALL_STARTUP_ROUTINES
        .iter()
        .filter(|routine| routine.run_on_setup)
        .map(|routine| routine.name)
        .collect::<Vec<_>>();

    assert_eq!(
        setup_routines,
        [
            "AddDefaultPluginRepository",
            "ReaddDefaultPluginRepository",
            "AddDefaultCastReceivers",
            "UpdateDefaultPluginRepository",
            "MoveTrickplayFiles",
        ]
    );
}

#[test]
fn cleanup_orphaned_extras_runs_during_app_initialisation() {
    let routine = ALL_STARTUP_ROUTINES
        .iter()
        .find(|routine| routine.name == "CleanupOrphanedExtras")
        .expect("CleanupOrphanedExtras is part of the official set");

    assert_eq!(routine.stage, MigrationStage::AppInitialisation);
}

#[test]
fn pre_startup_routines_run_before_core_initialisation() {
    for routine in ALL_STARTUP_ROUTINES
        .iter()
        .filter(|routine| routine.stage == MigrationStage::PreInitialisation)
    {
        assert!(
            matches!(
                routine.name,
                "CreateNetworkConfiguration"
                    | "MigrateNetworkConfiguration"
                    | "MigrateMusicBrainzTimeout"
                    | "MigrateEncodingOptions"
                    | "RenameEnableGroupingIntoCollections"
            ),
            "unexpected pre-initialisation routine {}",
            routine.name
        );
    }
}
