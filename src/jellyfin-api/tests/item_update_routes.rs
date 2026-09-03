use std::collections::BTreeMap;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{ItemUpdateInput, ItemUpdateService, UserService};
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice,
    entities::{api_key, base_item, item_value, user},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn item_update_route_matches_official_contract_and_postgres_semantics() {
    let fixture = Fixture::new().await;
    fixture.assert_access_and_errors().await;
    fixture.assert_official_collection_rows().await;
    fixture.assert_three_state_normalization_and_api_key().await;
    fixture.assert_transaction_rollback().await;
    fixture.assert_concurrent_partial_updates().await;
    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    item_id: Uuid,
    administrator_id: Uuid,
    user_id: Uuid,
    administrator_token: String,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("item-update-admin-{suffix}"))
            .await
            .expect("administrator creation must succeed");
        let user = users
            .create(&format!("item-update-user-{suffix}"))
            .await
            .expect("ordinary user creation must succeed");
        let devices = DeviceRepository::new(database.clone());
        let administrator_token =
            create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = create_session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("item-update-key-{suffix}"))
            .await
            .expect("API key creation must succeed");
        let item = BaseItemRepository::new(database.clone())
            .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
            .await
            .expect("item creation must succeed");
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Item Update Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            item_id: item.id,
            administrator_id: administrator.id,
            user_id: user.id,
            administrator_token,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
        }
    }

    async fn assert_access_and_errors(&self) {
        let body = json!({ "Tags": ["new-tag"] });
        assert_eq!(
            self.post(self.item_id, None, body.clone()).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            self.post(self.item_id, Some(&self.user_token), body.clone())
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            self.post(Uuid::new_v4(), Some(&self.administrator_token), body)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            self.post_raw(
                format!("/Items/{}", self.item_id),
                Some(&self.administrator_token),
                "{",
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            self.post(
                self.item_id,
                Some(&self.administrator_token),
                json!({ "Tags": "not-an-array" }),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    async fn assert_official_collection_rows(&self) {
        self.seed(ItemUpdateInput {
            tags: Some(vec!["old-tag".to_owned()]),
            genres: Some(vec!["Action".to_owned()]),
            provider_ids: Some(BTreeMap::from([(
                "Imdb".to_owned(),
                Some("tt1234567".to_owned()),
            )])),
        })
        .await;

        let response = self
            .post(
                self.item_id,
                Some(&self.administrator_token),
                json!({
                    "Id": self.item_id,
                    "Name": "ignored official DTO field",
                    "Tags": ["new-tag-1", "new-tag-2"]
                }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let updated = self.persisted_item().await;
        assert_eq!(
            metadata_strings(&updated, "Tags"),
            ["new-tag-1", "new-tag-2"]
        );
        assert_eq!(metadata_strings(&updated, "Genres"), ["Action"]);
        assert_eq!(
            metadata_value(&updated, "ProviderIds"),
            Some(&json!({ "Imdb": "tt1234567" }))
        );
        assert_eq!(
            self.value_names(item_value::ItemValueType::Tags).await,
            ["new-tag-1", "new-tag-2"]
        );

        let row_version = updated.row_version;
        let response = self
            .post(
                self.item_id,
                Some(&self.administrator_token),
                json!({ "Tags": [] }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cleared = self.persisted_item().await;
        assert!(cleared.row_version > row_version);
        assert!(metadata_strings(&cleared, "Tags").is_empty());
        assert_eq!(metadata_strings(&cleared, "Genres"), ["Action"]);
        assert_eq!(
            metadata_value(&cleared, "ProviderIds"),
            Some(&json!({ "Imdb": "tt1234567" }))
        );
        assert!(
            self.value_names(item_value::ItemValueType::Tags)
                .await
                .is_empty()
        );
    }

    async fn assert_three_state_normalization_and_api_key(&self) {
        let response = self
            .post_uri(
                &format!("/Items/{}?api_key={}", self.item_id, self.api_key_token),
                None,
                json!({
                    "Tags": ["  Alpha  ", "alpha", "Beta"],
                    "Genres": ["Action", "ACTION", " Épopée ", " éPOPÉE "],
                    "ProviderIds": {
                        "Imdb": "tt7654321",
                        "Null": null,
                        "Empty": "",
                        "Whitespace": "  "
                    }
                }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let normalized = self.persisted_item().await;
        assert_eq!(metadata_strings(&normalized, "Tags"), ["Alpha", "Beta"]);
        assert_eq!(
            metadata_strings(&normalized, "Genres"),
            ["Action", " Épopée "]
        );
        assert_eq!(
            metadata_value(&normalized, "ProviderIds"),
            Some(&json!({ "Imdb": "tt7654321", "Whitespace": "  " }))
        );
        assert_eq!(
            self.value_names(item_value::ItemValueType::Genre).await,
            ["Action", "Épopée"]
        );

        let row_version = normalized.row_version;
        let response = self
            .post(
                self.item_id,
                Some(&self.administrator_token),
                json!({ "Tags": null, "Genres": null, "ProviderIds": null }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let preserved = self.persisted_item().await;
        assert!(preserved.row_version > row_version);
        assert_eq!(metadata_strings(&preserved, "Tags"), ["Alpha", "Beta"]);
        assert_eq!(
            metadata_strings(&preserved, "Genres"),
            ["Action", " Épopée "]
        );
        assert_eq!(
            metadata_value(&preserved, "ProviderIds"),
            Some(&json!({ "Imdb": "tt7654321", "Whitespace": "  " }))
        );

        let response = self
            .post(
                self.item_id,
                Some(&self.administrator_token),
                json!({ "Genres": [], "ProviderIds": {} }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cleared = self.persisted_item().await;
        assert_eq!(metadata_strings(&cleared, "Tags"), ["Alpha", "Beta"]);
        assert!(metadata_strings(&cleared, "Genres").is_empty());
        assert_eq!(metadata_value(&cleared, "ProviderIds"), Some(&json!({})));
        assert!(
            self.value_names(item_value::ItemValueType::Genre)
                .await
                .is_empty()
        );
    }

    async fn assert_transaction_rollback(&self) {
        self.seed(ItemUpdateInput {
            tags: Some(vec!["stable-tag".to_owned()]),
            genres: Some(vec!["Action".to_owned()]),
            provider_ids: None,
        })
        .await;
        let before = self.persisted_item().await;
        let response = self
            .post(
                self.item_id,
                Some(&self.administrator_token),
                json!({ "Tags": ["must-roll-back"], "Genres": ["---"] }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let after = self.persisted_item().await;
        assert_eq!(after.row_version, before.row_version);
        assert_eq!(metadata_strings(&after, "Tags"), ["stable-tag"]);
        assert_eq!(metadata_strings(&after, "Genres"), ["Action"]);
        assert_eq!(
            self.value_names(item_value::ItemValueType::Tags).await,
            ["stable-tag"]
        );
    }

    async fn assert_concurrent_partial_updates(&self) {
        let api_key_uri = format!("/Items/{}?ApiKey={}", self.item_id, self.api_key_token);
        let first = self.post(
            self.item_id,
            Some(&self.administrator_token),
            json!({ "Tags": ["parallel-tag"] }),
        );
        let second = self.post_uri(
            &api_key_uri,
            None,
            json!({
                "Genres": ["Drama"],
                "ProviderIds": { "Tmdb": "12345" }
            }),
        );
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.status(), StatusCode::NO_CONTENT);
        assert_eq!(second.status(), StatusCode::NO_CONTENT);

        let persisted = self.persisted_item().await;
        assert_eq!(metadata_strings(&persisted, "Tags"), ["parallel-tag"]);
        assert_eq!(metadata_strings(&persisted, "Genres"), ["Drama"]);
        assert_eq!(
            metadata_value(&persisted, "ProviderIds"),
            Some(&json!({ "Tmdb": "12345" }))
        );
    }

    async fn seed(&self, input: ItemUpdateInput) {
        ItemUpdateService::new(self.database.clone())
            .update(self.item_id, input)
            .await
            .expect("metadata setup must succeed");
    }

    async fn persisted_item(&self) -> base_item::Model {
        BaseItemRepository::new(self.database.clone())
            .get(self.item_id)
            .await
            .expect("item lookup must succeed")
            .expect("item must exist")
    }

    async fn value_names(&self, value_type: item_value::ItemValueType) -> Vec<String> {
        ItemValueRepository::new(self.database.clone())
            .values_for_item(self.item_id, value_type)
            .await
            .expect("normalized value lookup must succeed")
            .into_iter()
            .map(|value| value.value)
            .collect()
    }

    async fn post(
        &self,
        item_id: Uuid,
        token: Option<&str>,
        body: Value,
    ) -> axum::response::Response {
        self.post_uri(&format!("/Items/{item_id}"), token, body)
            .await
    }

    async fn post_uri(
        &self,
        uri: &str,
        token: Option<&str>,
        body: Value,
    ) -> axum::response::Response {
        self.post_raw(uri, token, &body.to_string()).await
    }

    async fn post_raw(
        &self,
        uri: impl ToString,
        token: Option<&str>,
        body: &str,
    ) -> axum::response::Response {
        let mut request =
            Request::post(uri.to_string()).header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header("x-emby-token", token);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        base_item::Entity::delete_by_id(self.item_id)
            .exec(&self.database)
            .await
            .expect("item cleanup must succeed");
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("API key cleanup must succeed");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.administrator_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup must succeed");
    }
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Item Update Tests",
            "1.0",
            "Test Device",
            device_id,
        ))
        .await
        .expect("device session creation must succeed")
        .access_token
}

fn metadata_value<'a>(item: &'a base_item::Model, key: &str) -> Option<&'a Value> {
    item.data.as_ref()?.as_object()?.get(key)
}

fn metadata_strings(item: &base_item::Model, key: &str) -> Vec<String> {
    metadata_value(item, key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}
