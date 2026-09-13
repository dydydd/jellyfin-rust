use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository,
    NewBaseItem, NewDevice, NewPerson, PersonRepository, entities::item_value,
};
use jellyfin_model::UserPolicy;
use md5::{Digest, Md5};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Person Credits Tests\", DeviceId=\"emby-person-credits\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_person_credits_";

#[tokio::test]
async fn person_credits_are_canonical_policy_filtered_and_sdk_decodable() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert!(database_name.strip_prefix(DATABASE_PREFIX).is_some_and(
        |suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    ));
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;
    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup");
    administrator.close().await.expect("administrator close");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary database connection");
    jellyfin_data::migrate(&database)
        .await
        .expect("temporary database migrations");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_policy_and_grouping().await;
    fixture.assert_api_key_global_view().await;
    fixture.assert_unknown_and_noncanonical_people().await;
    fixture.assert_contract_and_protocol_isolation().await;

    database.close().await.expect("database close");
}

struct Fixture {
    emby: axum::Router,
    jellyfin: axum::Router,
    canonical_person_id: Uuid,
    noncanonical_person_id: Uuid,
    non_person_id: Uuid,
    user_token: String,
    admin_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("person-credits-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("person-credits-user-{suffix}"))
            .await
            .expect("user creation");

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let visible_folder = create_item(
            &items,
            "CollectionFolder",
            "Visible",
            Some(root.id),
            true,
            None,
        )
        .await;
        let hidden_folder = create_item(
            &items,
            "CollectionFolder",
            "Hidden",
            Some(root.id),
            true,
            None,
        )
        .await;

        let canonical_person_id = official_person_id("Ada Example");
        let mut canonical = NewBaseItem::new(canonical_person_id, "Person");
        canonical.name = Some("Ada Example".to_owned());
        canonical.sort_name = canonical.name.clone();
        canonical.path = Some("metadata/People/A/Ada Example".to_owned());
        canonical.is_virtual_item = false;
        items
            .create(canonical)
            .await
            .expect("canonical Person item creation");
        let noncanonical_person =
            create_item(&items, "Person", "Ada Example", None, false, None).await;

        let actor_alpha = create_item(
            &items,
            "Movie",
            "Actor Alpha",
            Some(visible_folder.id),
            false,
            Some(json!({
                "OriginalTitle": "Actor Alpha Original",
                "ProviderIds": {"Imdb": "tt-alpha"},
                "IndexNumberEnd": 4
            })),
        )
        .await;
        let writer = create_item(
            &items,
            "Movie",
            "Writer Middle",
            Some(visible_folder.id),
            false,
            Some(json!({"ProviderIds": {"Tmdb": "22"}})),
        )
        .await;
        let actor_zulu = create_item(
            &items,
            "MediaBrowser.Controller.Entities.Movies.Movie",
            "Actor Zulu",
            Some(visible_folder.id),
            false,
            None,
        )
        .await;
        let hidden_director = create_item(
            &items,
            "Movie",
            "Hidden Director",
            Some(hidden_folder.id),
            false,
            None,
        )
        .await;
        let blocked_producer = create_item(
            &items,
            "Movie",
            "Blocked Producer",
            Some(visible_folder.id),
            false,
            None,
        )
        .await;
        let unsupported_narrator = create_item(
            &items,
            "AudioBook",
            "Unsupported Narrator",
            Some(visible_folder.id),
            false,
            None,
        )
        .await;

        ItemValueRepository::new(database.clone())
            .link(
                blocked_producer.id,
                item_value::ItemValueType::Tags,
                "Blocked Credit",
            )
            .await
            .expect("blocked tag relation");

        let people = PersonRepository::new(database.clone());
        for (item_id, person_type, role, list_order) in [
            (actor_alpha.id, "actor", "Lead", 0),
            (writer.id, "Writer", "Screenplay", 0),
            (actor_zulu.id, "Actor", "Supporting", 0),
            (hidden_director.id, "Director", "", 0),
            (blocked_producer.id, "Producer", "", 0),
            (unsupported_narrator.id, "Narrator", "Narration", 0),
        ] {
            people
                .link(
                    item_id,
                    NewPerson::new("Ada Example"),
                    person_type,
                    Some(role),
                    None,
                    list_order,
                )
                .await
                .expect("person credit relation");
        }

        users
            .update_policy(
                user.id,
                &UserPolicy {
                    authentication_provider_id: Some(
                        UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
                    ),
                    password_reset_provider_id: Some(
                        UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
                    ),
                    enable_all_folders: false,
                    enabled_folders: vec![visible_folder.id],
                    blocked_tags: vec!["Blocked Credit".to_owned()],
                    ..UserPolicy::default()
                },
            )
            .await
            .expect("user policy");

        let devices = DeviceRepository::new(database.clone());
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let admin_token = session(
            &devices,
            administrator.id,
            &format!("administrator-{suffix}"),
        )
        .await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("person-credits-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Person Credits Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            canonical_person_id,
            noncanonical_person_id: noncanonical_person.id,
            non_person_id: actor_alpha.id,
            user_token,
            admin_token,
            api_key_token,
        }
    }

    async fn assert_policy_and_grouping(&self) {
        let path = format!(
            "/emby/pErSoNs/{}/cReDiTs",
            self.canonical_person_id.simple()
        );
        let response = request(&self.emby, &path, Some(&self.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let groups = response_json(response).await;
        let swift_groups: Vec<SwiftCreditsList> =
            serde_json::from_value(groups.clone()).expect("generated Swift CreditsList wire shape");
        assert_eq!(swift_groups.len(), 2);
        assert_eq!(swift_groups[0].person_type, Some(SwiftPersonType::Actor));
        let swift_actor = swift_groups[0].items.as_ref().expect("Swift Items array");
        assert_eq!(swift_actor[0].person_type, Some(SwiftPersonType::Actor));
        assert_eq!(swift_actor[0].item_type.as_deref(), Some("Movie"));
        assert_eq!(
            swift_actor[0]
                .provider_ids
                .as_ref()
                .and_then(|ids| ids.get("Imdb"))
                .map(String::as_str),
            Some("tt-alpha")
        );
        let groups = groups.as_array().expect("CreditsList array");
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0]["PersonType"], "Actor");
        assert_eq!(groups[1]["PersonType"], "Writer");

        let actor_items = groups[0]["Items"].as_array().expect("Actor items");
        assert_eq!(actor_items.len(), 2);
        assert_eq!(actor_items[0]["Name"], "Actor Alpha");
        assert_eq!(actor_items[1]["Name"], "Actor Zulu");
        assert_eq!(actor_items[0]["OriginalTitle"], "Actor Alpha Original");
        assert_eq!(actor_items[0]["ProviderIds"]["Imdb"], "tt-alpha");
        assert_eq!(actor_items[0]["IndexNumberEnd"], 4);
        assert_eq!(actor_items[0]["PersonType"], "Actor");
        assert_eq!(actor_items[0]["Role"], "Lead");
        assert_eq!(actor_items[0]["Type"], "Movie");
        assert_eq!(groups[1]["Items"][0]["Name"], "Writer Middle");
        assert!(
            groups.iter().all(|group| !matches!(
                group["PersonType"].as_str(),
                Some("Director" | "Producer" | "Narrator")
            )),
            "policy-hidden and unsupported groups must be omitted: {groups:?}"
        );
    }

    async fn assert_api_key_global_view(&self) {
        let groups = response_json(
            request(
                &self.emby,
                &format!("/emby/Persons/{}/Credits", self.canonical_person_id),
                Some(&self.api_key_token),
            )
            .await,
        )
        .await;
        let types = groups
            .as_array()
            .expect("CreditsList array")
            .iter()
            .map(|group| group["PersonType"].as_str().expect("PersonType"))
            .collect::<Vec<_>>();
        assert_eq!(types, ["Actor", "Director", "Writer", "Producer"]);
        assert!(!types.contains(&"Narrator"));

        let admin = response_json(
            request(
                &self.emby,
                &format!("/emby/Persons/{}/Credits", self.canonical_person_id),
                Some(&self.admin_token),
            )
            .await,
        )
        .await;
        assert_eq!(admin, groups, "administrator and global fixture views");
    }

    async fn assert_unknown_and_noncanonical_people(&self) {
        for person_id in [
            Uuid::new_v4(),
            self.noncanonical_person_id,
            self.non_person_id,
        ] {
            assert_eq!(
                request(
                    &self.emby,
                    &format!("/emby/Persons/{person_id}/Credits"),
                    Some(&self.admin_token),
                )
                .await
                .status(),
                StatusCode::NOT_FOUND,
                "invalid canonical Person {person_id}"
            );
        }
        assert_eq!(
            request(
                &self.emby,
                "/emby/Persons/not-a-uuid/Credits",
                Some(&self.admin_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    async fn assert_contract_and_protocol_isolation(&self) {
        assert_eq!(
            request(
                &self.emby,
                &format!("/emby/Persons/{}/Credits", self.canonical_person_id),
                None,
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );

        // Swagger, Java, and Swift define no query paging on this operation;
        // unknown query pairs are ignored by ASP.NET rather than paging it.
        let with_unknown_query = response_json(
            request(
                &self.emby,
                &format!(
                    "/emby/Persons/{}/Credits?StartIndex=1&Limit=0&Unknown=value",
                    self.canonical_person_id
                ),
                Some(&self.user_token),
            )
            .await,
        )
        .await;
        assert_eq!(with_unknown_query.as_array().map(Vec::len), Some(2));

        for prefix in ["", "/api"] {
            assert_eq!(
                request(
                    &self.jellyfin,
                    &format!("{prefix}/Persons/{}/Credits", self.canonical_person_id),
                    Some(&self.admin_token),
                )
                .await
                .status(),
                StatusCode::NOT_FOUND,
                "Emby-only route leaked through {prefix:?}"
            );
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
enum SwiftPersonType {
    Actor,
    Director,
    Writer,
    Producer,
    GuestStar,
    Composer,
    Conductor,
    Lyricist,
}

#[derive(Debug, Deserialize)]
struct SwiftCreditsList {
    #[serde(rename = "PersonType")]
    person_type: Option<SwiftPersonType>,
    #[serde(rename = "Items")]
    items: Option<Vec<SwiftRemoteSearchResult>>,
}

#[derive(Debug, Deserialize)]
struct SwiftRemoteSearchResult {
    #[serde(rename = "ProviderIds")]
    provider_ids: Option<std::collections::HashMap<String, String>>,
    #[serde(rename = "PersonType")]
    person_type: Option<SwiftPersonType>,
    #[serde(rename = "Type")]
    item_type: Option<String>,
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
    data: Option<Value>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    item.data = data;
    item.production_year = (!is_folder).then_some(2024);
    repository.create(item).await.expect("base item creation")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Person Credits Tests",
            "1.0",
            "Test",
            format!("emby-person-credits-{suffix}"),
        ))
        .await
        .expect("device session")
        .access_token
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request
            .header(header::AUTHORIZATION, AUTHORIZATION)
            .header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

async fn response_json(response: Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response bytes"),
    )
    .expect("JSON response")
}

fn official_person_id(name: &str) -> Uuid {
    let prefix = name
        .chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_string())
        .unwrap_or_default();
    let path = if prefix.is_empty() {
        format!("metadata/People/{name}")
    } else {
        format!("metadata/People/{prefix}/{name}")
    };
    let key = format!(
        "MediaBrowser.Controller.Entities.Person{}",
        path.to_lowercase()
    );
    let bytes = key
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let digest = Md5::digest(bytes);
    Uuid::from_bytes([
        digest[3], digest[2], digest[1], digest[0], digest[5], digest[4], digest[7], digest[6],
        digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14],
        digest[15],
    ])
}
