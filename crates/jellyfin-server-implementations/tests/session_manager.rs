use std::sync::Mutex;

use jellyfin_server_implementations::{
    AuthenticationField, AuthenticationRequest, AuthorizationTokenRequest, SessionManager,
    SessionManagerError, SessionStore, SessionStoreFuture, SessionValidationError,
    ValidatedAuthenticationRequest,
};
use thiserror::Error;
use uuid::Uuid;

#[tokio::test]
async fn authorization_token_device_id_matches_the_official_two_state_contract() {
    for (device_id, expected) in [
        (
            Some(String::new()),
            SessionValidationError::EmptyField(AuthenticationField::DeviceId),
        ),
        (
            None,
            SessionValidationError::MissingField(AuthenticationField::DeviceId),
        ),
    ] {
        let manager = SessionManager::new(RecordingStore::default());
        assert_eq!(
            manager
                .get_authorization_token(
                    Uuid::new_v4(),
                    device_id,
                    "app_name".to_owned(),
                    "0.0.0".to_owned(),
                    "device_name".to_owned(),
                )
                .await,
            Err(SessionManagerError::Validation(expected))
        );
        assert!(manager.store().token_requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn new_session_fields_match_the_official_eight_state_contract() {
    for (request, expected) in invalid_authentication_requests() {
        let manager = SessionManager::new(RecordingStore::default());
        assert_eq!(
            manager
                .authenticate_new_session_internal(&request, false)
                .await,
            Err(SessionManagerError::Validation(expected))
        );
        assert!(
            manager
                .store()
                .authentication_requests
                .lock()
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn validation_order_is_app_then_device_id_then_device_name_then_app_version() {
    let cases = [
        (AuthenticationRequest::default(), AuthenticationField::App),
        (
            AuthenticationRequest {
                app: Some("app".to_owned()),
                ..Default::default()
            },
            AuthenticationField::DeviceId,
        ),
        (
            AuthenticationRequest {
                app: Some("app".to_owned()),
                device_id: Some("device".to_owned()),
                ..Default::default()
            },
            AuthenticationField::DeviceName,
        ),
        (
            AuthenticationRequest {
                app: Some("app".to_owned()),
                device_id: Some("device".to_owned()),
                device_name: Some("name".to_owned()),
                ..Default::default()
            },
            AuthenticationField::AppVersion,
        ),
    ];

    for (request, expected_field) in cases {
        let manager = SessionManager::new(RecordingStore::default());
        assert_eq!(
            manager
                .authenticate_new_session_internal(&request, false)
                .await,
            Err(SessionManagerError::Validation(
                SessionValidationError::MissingField(expected_field)
            ))
        );
    }
}

#[tokio::test]
async fn valid_requests_are_forwarded_to_the_store_without_hardcoded_results() {
    let manager = SessionManager::new(RecordingStore::default());
    let request = AuthenticationRequest::new("app", "device-id", "device-name", "1.2.3");
    let result = manager
        .authenticate_new_session_internal(&request, true)
        .await
        .unwrap();
    assert_eq!(result, "stored:device-id:true");

    let user_id = Uuid::new_v4();
    let token = manager
        .get_authorization_token(
            user_id,
            Some("token-device".to_owned()),
            "token-app".to_owned(),
            "2.0".to_owned(),
            "token-name".to_owned(),
        )
        .await
        .unwrap();
    assert_eq!(token, "token:token-device");

    let authentication_requests = manager.store().authentication_requests.lock().unwrap();
    assert_eq!(authentication_requests.len(), 1);
    assert_eq!(authentication_requests[0].0.app(), "app");
    assert_eq!(authentication_requests[0].0.device_id(), "device-id");
    assert_eq!(authentication_requests[0].0.device_name(), "device-name");
    assert_eq!(authentication_requests[0].0.app_version(), "1.2.3");
    assert!(authentication_requests[0].1);
    drop(authentication_requests);

    let token_requests = manager.store().token_requests.lock().unwrap();
    assert_eq!(token_requests.len(), 1);
    assert_eq!(token_requests[0].user_id, user_id);
    assert_eq!(token_requests[0].device_id, "token-device");
    assert_eq!(token_requests[0].app, "token-app");
    assert_eq!(token_requests[0].app_version, "2.0");
    assert_eq!(token_requests[0].device_name, "token-name");
}

#[tokio::test]
async fn store_errors_are_propagated_without_becoming_fake_successes() {
    let manager = SessionManager::new(RecordingStore {
        error: Some(StoreError::Rejected),
        ..Default::default()
    });
    assert_eq!(
        manager
            .authenticate_new_session_internal(
                &AuthenticationRequest::new("app", "device", "name", "version"),
                false,
            )
            .await,
        Err(SessionManagerError::Store(StoreError::Rejected))
    );
    assert_eq!(
        manager
            .get_authorization_token(
                Uuid::new_v4(),
                Some("device".to_owned()),
                "app".to_owned(),
                "version".to_owned(),
                "name".to_owned(),
            )
            .await,
        Err(SessionManagerError::Store(StoreError::Rejected))
    );
}

fn invalid_authentication_requests() -> Vec<(AuthenticationRequest, SessionValidationError)> {
    let valid =
        || AuthenticationRequest::new("app_name", "device_id", "device_name", "app_version");
    vec![
        (
            AuthenticationRequest {
                app: Some(String::new()),
                ..valid()
            },
            SessionValidationError::EmptyField(AuthenticationField::App),
        ),
        (
            AuthenticationRequest {
                app: None,
                ..valid()
            },
            SessionValidationError::MissingField(AuthenticationField::App),
        ),
        (
            AuthenticationRequest {
                device_id: Some(String::new()),
                ..valid()
            },
            SessionValidationError::EmptyField(AuthenticationField::DeviceId),
        ),
        (
            AuthenticationRequest {
                device_id: None,
                ..valid()
            },
            SessionValidationError::MissingField(AuthenticationField::DeviceId),
        ),
        (
            AuthenticationRequest {
                device_name: Some(String::new()),
                ..valid()
            },
            SessionValidationError::EmptyField(AuthenticationField::DeviceName),
        ),
        (
            AuthenticationRequest {
                device_name: None,
                ..valid()
            },
            SessionValidationError::MissingField(AuthenticationField::DeviceName),
        ),
        (
            AuthenticationRequest {
                app_version: Some(String::new()),
                ..valid()
            },
            SessionValidationError::EmptyField(AuthenticationField::AppVersion),
        ),
        (
            AuthenticationRequest {
                app_version: None,
                ..valid()
            },
            SessionValidationError::MissingField(AuthenticationField::AppVersion),
        ),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("session store rejected the request")]
enum StoreError {
    Rejected,
}

#[derive(Debug, Default)]
struct RecordingStore {
    authentication_requests: Mutex<Vec<(ValidatedAuthenticationRequest, bool)>>,
    token_requests: Mutex<Vec<AuthorizationTokenRequest>>,
    error: Option<StoreError>,
}

impl SessionStore for RecordingStore {
    type AuthenticationResult = String;
    type Error = StoreError;

    fn authenticate_new_session(
        &self,
        request: ValidatedAuthenticationRequest,
        enforce_password: bool,
    ) -> SessionStoreFuture<'_, Self::AuthenticationResult, Self::Error> {
        Box::pin(async move {
            self.authentication_requests
                .lock()
                .unwrap()
                .push((request.clone(), enforce_password));
            if let Some(error) = self.error {
                Err(error)
            } else {
                Ok(format!("stored:{}:{enforce_password}", request.device_id()))
            }
        })
    }

    fn issue_authorization_token(
        &self,
        request: AuthorizationTokenRequest,
    ) -> SessionStoreFuture<'_, String, Self::Error> {
        Box::pin(async move {
            let token = format!("token:{}", request.device_id);
            self.token_requests.lock().unwrap().push(request);
            self.error.map_or(Ok(token), Err)
        })
    }
}
