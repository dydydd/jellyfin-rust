use jellyfin_model::{
    HasProviderIds, MetadataProvider, ProviderIdError, ProviderIdMap, ProviderIdsExtensions,
};

const EXAMPLE_IMDB_ID: &str = "tt0113375";

#[derive(Default)]
struct TestEntity {
    provider_ids: Option<ProviderIdMap>,
}

impl TestEntity {
    fn empty() -> Self {
        Self {
            provider_ids: Some(ProviderIdMap::new()),
        }
    }
}

impl HasProviderIds for TestEntity {
    fn provider_ids(&self) -> Option<&ProviderIdMap> {
        self.provider_ids.as_ref()
    }

    fn provider_ids_mut(&mut self) -> &mut Option<ProviderIdMap> {
        &mut self.provider_ids
    }
}

#[test]
fn has_provider_id_official_cases() {
    assert_eq!(
        jellyfin_model::entities::has_provider_id::<TestEntity>(None, MetadataProvider::Imdb),
        Err(ProviderIdError::NullInstance)
    );
    assert!(!TestEntity::default().has_provider_id(MetadataProvider::Imdb));
    assert_eq!(
        TestEntity::empty().has_provider_id_named(None),
        Err(ProviderIdError::NullName)
    );
    assert!(!TestEntity::empty().has_provider_id(MetadataProvider::Imdb));

    let mut found = TestEntity::empty();
    found
        .provider_ids
        .as_mut()
        .unwrap()
        .insert("Imdb".to_owned(), EXAMPLE_IMDB_ID.to_owned());
    assert!(found.has_provider_id(MetadataProvider::Imdb));

    let mut empty_value = TestEntity::empty();
    empty_value
        .provider_ids
        .as_mut()
        .unwrap()
        .insert("Imdb".to_owned(), String::new());
    assert!(!empty_value.has_provider_id(MetadataProvider::Imdb));
}

#[test]
fn get_provider_id_official_cases() {
    assert_eq!(
        jellyfin_model::entities::get_provider_id::<TestEntity>(None, MetadataProvider::Imdb),
        Err(ProviderIdError::NullInstance)
    );
    assert_eq!(
        TestEntity::empty().get_provider_id_named(None),
        Err(ProviderIdError::NullName)
    );
    assert_eq!(
        TestEntity::empty().get_provider_id(MetadataProvider::Imdb),
        None
    );
    assert_eq!(
        TestEntity::default().get_provider_id(MetadataProvider::Imdb),
        None
    );

    let mut found = TestEntity::empty();
    found
        .provider_ids
        .as_mut()
        .unwrap()
        .insert("Imdb".to_owned(), EXAMPLE_IMDB_ID.to_owned());
    assert_eq!(
        found.get_provider_id(MetadataProvider::Imdb),
        Some(EXAMPLE_IMDB_ID)
    );
}

#[test]
fn try_get_provider_id_official_cases() {
    assert_eq!(
        TestEntity::empty().try_get_provider_id(MetadataProvider::Imdb),
        None
    );
    assert_eq!(
        TestEntity::default().try_get_provider_id(MetadataProvider::Imdb),
        None
    );

    let mut found = TestEntity::empty();
    found
        .provider_ids
        .as_mut()
        .unwrap()
        .insert("Imdb".to_owned(), EXAMPLE_IMDB_ID.to_owned());
    assert_eq!(
        found.try_get_provider_id(MetadataProvider::Imdb),
        Some(EXAMPLE_IMDB_ID)
    );

    found
        .provider_ids
        .as_mut()
        .unwrap()
        .insert("Imdb".to_owned(), String::new());
    assert_eq!(found.try_get_provider_id(MetadataProvider::Imdb), None);
}

#[test]
fn set_provider_id_official_cases() {
    assert_eq!(
        jellyfin_model::entities::set_provider_id::<TestEntity>(
            None,
            MetadataProvider::Imdb,
            EXAMPLE_IMDB_ID,
        ),
        Err(ProviderIdError::NullInstance)
    );

    let mut provider = TestEntity::empty();
    assert_eq!(
        provider.set_provider_id_named(Some("Imdb"), None),
        Err(ProviderIdError::NullValue)
    );
    assert!(provider.provider_ids.as_ref().unwrap().is_empty());

    provider
        .provider_ids
        .as_mut()
        .unwrap()
        .insert("Imdb".to_owned(), EXAMPLE_IMDB_ID.to_owned());
    assert_eq!(
        provider.set_provider_id(MetadataProvider::Imdb, ""),
        Err(ProviderIdError::EmptyValue)
    );
    assert_eq!(provider.provider_ids.as_ref().unwrap().len(), 1);

    let mut success = TestEntity::empty();
    success
        .set_provider_id(MetadataProvider::Imdb, EXAMPLE_IMDB_ID)
        .unwrap();
    assert_eq!(success.provider_ids.as_ref().unwrap().len(), 1);

    let mut null_provider = TestEntity::default();
    null_provider
        .set_provider_id(MetadataProvider::Imdb, EXAMPLE_IMDB_ID)
        .unwrap();
    assert_eq!(null_provider.provider_ids.as_ref().unwrap().len(), 1);

    let mut null_and_empty = TestEntity::default();
    assert_eq!(
        null_and_empty.set_provider_id(MetadataProvider::Imdb, ""),
        Err(ProviderIdError::EmptyValue)
    );
    assert!(null_and_empty.provider_ids.is_none());
}

#[test]
fn remove_provider_id_official_case() {
    let mut provider = TestEntity::empty();
    provider
        .provider_ids
        .as_mut()
        .unwrap()
        .insert("Imdb".to_owned(), EXAMPLE_IMDB_ID.to_owned());
    provider.remove_provider_id(MetadataProvider::Imdb);
    assert!(provider.provider_ids.as_ref().unwrap().is_empty());
}

#[test]
fn provider_names_are_canonicalized_without_path_parsing() {
    let mut provider = TestEntity::default();
    assert!(provider.try_set_provider_id_named(Some("iMdB"), Some(EXAMPLE_IMDB_ID)));
    assert_eq!(
        provider.provider_ids.as_ref().unwrap().get("Imdb"),
        Some(&EXAMPLE_IMDB_ID.to_owned())
    );
    assert!(!provider.try_set_provider_id_named(Some("bad=name"), Some("1")));
}
