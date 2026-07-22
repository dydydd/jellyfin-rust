use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice, entities::user};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Video Tests\", DeviceId=\"video-tests\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn alternate_source_route_enforces_official_contract() {
    let fixture = Fixture::new().await;
    let route = Fixture::route(fixture.group_a.primary);
    assert_eq!(
        fixture.send(Method::DELETE, &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &Fixture::route(Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &Fixture::route(fixture.non_video_id),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let incorrect_official_test_uri = format!("/Videos/{}", fixture.group_a.primary);
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &incorrect_official_test_uri,
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn primary_and_alternate_entries_persist_complete_group_detachment() {
    let fixture = Fixture::new().await;
    let group_b_before = fixture.load_group(&fixture.group_b).await;

    let alternate_route = Fixture::route(fixture.group_a.alternates[0]);
    assert_eq!(
        fixture
            .send(Method::DELETE, &alternate_route, Some(&fixture.admin_token),)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let group_a = fixture.load_group(&fixture.group_a).await;
    assert_eq!(group_a.len(), 3);
    assert!(group_a.iter().all(|item| item.primary_version_id.is_none()));
    assert_eq!(fixture.load_group(&fixture.group_b).await, group_b_before);

    let primary_route = Fixture::route(fixture.group_b.primary);
    assert_eq!(
        fixture
            .send(Method::DELETE, &primary_route, Some(&fixture.admin_token),)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        fixture
            .load_group(&fixture.group_b)
            .await
            .iter()
            .all(|item| item.primary_version_id.is_none())
    );
    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    repository: BaseItemRepository,
    app: axum::Router,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    group_a: VersionGroup,
    group_b: VersionGroup,
    non_video_id: Uuid,
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
        let admin = users
            .create_initial_administrator(&format!("video-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("video-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("video-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("video-user-{suffix}")).await;
        let repository = BaseItemRepository::new(database.clone());
        let group_a = create_group(
            &repository,
            &format!("{suffix}-a"),
            "MediaBrowser.Controller.Entities.Movies.Movie",
        )
        .await;
        let group_b = create_group(&repository, &format!("{suffix}-b"), "Movie").await;
        let non_video_id = Uuid::new_v4();
        create_item(
            &repository,
            non_video_id,
            &format!("{suffix}-folder"),
            "Folder",
            None,
        )
        .await;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Video Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            repository,
            app,
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
            group_a,
            group_b,
            non_video_id,
        }
    }

    fn route(item_id: Uuid) -> String {
        format!("/Videos/{item_id}/AlternateSources")
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn load_group(
        &self,
        group: &VersionGroup,
    ) -> Vec<jellyfin_data::entities::base_item::Model> {
        let mut items = Vec::new();
        for id in group.ids() {
            items.push(
                self.repository
                    .get(id)
                    .await
                    .expect("version lookup")
                    .expect("version must remain persisted"),
            );
        }
        items
    }

    async fn cleanup(self) {
        let ids = self
            .group_a
            .ids()
            .into_iter()
            .chain(self.group_b.ids())
            .chain([self.non_video_id])
            .collect::<Vec<_>>();
        self.repository
            .delete_many(&ids)
            .await
            .expect("video fixtures must clean up");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("video users must clean up");
    }
}

struct VersionGroup {
    primary: Uuid,
    alternates: [Uuid; 2],
}

impl VersionGroup {
    fn ids(&self) -> [Uuid; 3] {
        [self.primary, self.alternates[0], self.alternates[1]]
    }
}

async fn create_group(
    repository: &BaseItemRepository,
    label: &str,
    primary_type: &str,
) -> VersionGroup {
    let primary = Uuid::new_v4();
    let alternates = [Uuid::new_v4(), Uuid::new_v4()];
    create_item(repository, primary, label, primary_type, None).await;
    create_item(repository, alternates[0], label, "Video", Some(primary)).await;
    create_item(repository, alternates[1], label, "Movie", Some(primary)).await;
    VersionGroup {
        primary,
        alternates,
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    id: Uuid,
    label: &str,
    item_type: &str,
    primary_version_id: Option<Uuid>,
) {
    let mut item = NewBaseItem::new(id, item_type);
    item.name = Some(label.to_owned());
    item.path = Some(format!("/media/{label}/{id}.mkv"));
    item.media_type = Some("Video".to_owned());
    item.presentation_unique_key = Some(label.to_owned());
    item.primary_version_id = primary_version_id;
    repository.create(item).await.expect("video item creation");
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Video Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}
