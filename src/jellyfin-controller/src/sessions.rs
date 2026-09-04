use chrono::{DateTime, Datelike, FixedOffset, Local, Timelike, Weekday};
use jellyfin_data::{
    ActivityLogRepository, AuthenticationStoreError, DeviceQuery, DeviceRepository, NewActivityLog,
    NewDevice,
    entities::{activity_log::LogSeverity, device, user},
};
use jellyfin_model::{ClientCapabilitiesDto, DynamicDayOfWeek, UserPolicy};
use jellyfin_server_implementations::{
    AuthenticationError, DefaultAuthenticationProvider, SessionStore, SessionStoreFuture,
    ValidatedAuthenticationRequest,
};
use serde::Deserialize;
use thiserror::Error;

use crate::{UserError, UserService};

/// Result of a persisted local-user session authentication.
#[derive(Debug, Clone)]
pub struct PostgresAuthenticationResult {
    pub user: user::Model,
    pub device: device::Model,
}

/// Persistence failures raised while creating or refreshing a session.
#[derive(Debug, Error)]
pub enum PostgresSessionStoreError {
    #[error("session authentication requires a username and password")]
    MissingCredentials,
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("the user account is disabled")]
    Disabled,
    #[error("the user's authentication provider is not enabled")]
    UnsupportedAuthenticationProvider,
    #[error("the user is currently blocked by parental control")]
    ParentalSchedule,
    #[error("the user is not allowed remote access")]
    RemoteAccess,
    #[error("the user is not allowed access from this device")]
    DeviceAccessDenied,
    #[error("the user is at their maximum number of sessions")]
    MaxActiveSessions,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    Authentication(#[from] AuthenticationError),
    #[error(transparent)]
    AuthenticationStore(#[from] AuthenticationStoreError),
    #[error(transparent)]
    ActivityLog(#[from] jellyfin_data::ActivityLogError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

/// PostgreSQL-backed [`SessionStore`] used by the server runtime.
///
/// The store owns the complete local authentication lifecycle: user lookup,
/// password verification, lockout/activity accounting, device token creation,
/// and successful-session activity logging.
#[derive(Clone)]
pub struct PostgresSessionStore {
    users: UserService,
    devices: DeviceRepository,
    activity_logs: ActivityLogRepository,
    authentication: DefaultAuthenticationProvider,
}

impl PostgresSessionStore {
    #[must_use]
    pub fn new(
        users: UserService,
        devices: DeviceRepository,
        activity_logs: ActivityLogRepository,
        authentication: DefaultAuthenticationProvider,
    ) -> Self {
        Self {
            users,
            devices,
            activity_logs,
            authentication,
        }
    }
}

impl SessionStore for PostgresSessionStore {
    type AuthenticationResult = PostgresAuthenticationResult;
    type Error = PostgresSessionStoreError;

    fn authenticate_new_session(
        &self,
        request: ValidatedAuthenticationRequest,
        _enforce_password: bool,
    ) -> SessionStoreFuture<'_, Self::AuthenticationResult, Self::Error> {
        Box::pin(async move {
            let username = request
                .username()
                .ok_or(PostgresSessionStoreError::MissingCredentials)?;
            let password = request
                .password()
                .ok_or(PostgresSessionStoreError::MissingCredentials)?;
            let mut user = self
                .users
                .get_by_name(username)
                .await?
                .ok_or(PostgresSessionStoreError::InvalidCredentials)?;

            if user.is_disabled {
                return Err(PostgresSessionStoreError::Disabled);
            }
            if !user
                .authentication_provider_id
                .eq_ignore_ascii_case(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID)
            {
                return Err(PostgresSessionStoreError::UnsupportedAuthenticationProvider);
            }

            let user_id = user.id;
            let authentication = self.authentication;
            let username = username.to_owned();
            let password = password.to_owned();
            let (verified, logged_username) = tokio::task::spawn_blocking(move || {
                let verified = authentication
                    .authenticate(&username, &password, Some(&mut user))
                    .map(|_| user);
                (verified, username)
            })
            .await?;

            let user = match verified {
                Ok(user) => user,
                Err(AuthenticationError::InvalidCredentials) => {
                    let _ = self.users.record_failed_authentication(user_id).await;
                    self.log_activity(NewActivityLog {
                        log_severity: LogSeverity::Error,
                        ..NewActivityLog::new(
                            format!("Failed login attempt for {logged_username}"),
                            "AuthenticationFailed",
                            user_id,
                        )
                    });
                    return Err(PostgresSessionStoreError::InvalidCredentials);
                }
                Err(error) => return Err(error.into()),
            };

            let policy = UserPolicy::deserialize(&user.policy)?;
            if !policy.enable_remote_access && !request.is_local_network() {
                return Err(PostgresSessionStoreError::RemoteAccess);
            }
            if !is_parental_schedule_allowed(&policy, Local::now().fixed_offset()) {
                return Err(PostgresSessionStoreError::ParentalSchedule);
            }

            let user = self.users.record_successful_authentication(user).await?;
            if !self
                .can_access_device(&policy, user.is_administrator, request.device_id())
                .await?
            {
                return Err(PostgresSessionStoreError::DeviceAccessDenied);
            }
            let active_sessions = self
                .devices
                .query(&DeviceQuery {
                    user_id: Some(user.id),
                    is_active: Some(true),
                    ..DeviceQuery::default()
                })
                .await?
                .total_record_count;
            let at_max = u32::try_from(policy.max_active_sessions)
                .is_ok_and(|max| max > 0 && active_sessions >= u64::from(max));
            if at_max {
                return Err(PostgresSessionStoreError::MaxActiveSessions);
            }
            self.devices
                .delete_by_user_and_device(user.id, request.device_id())
                .await?;
            let device = self
                .devices
                .create_session(NewDevice::new(
                    user.id,
                    request.app(),
                    request.app_version(),
                    request.device_name(),
                    request.device_id(),
                ))
                .await?;

            self.log_activity(NewActivityLog::new(
                format!("Authentication succeeded for {}", user.username),
                "AuthenticationSucceeded",
                user.id,
            ));
            self.log_activity(NewActivityLog::new(
                format!("{} is online from {}", user.username, device.device_name),
                "SessionStarted",
                user.id,
            ));

            Ok(PostgresAuthenticationResult { user, device })
        })
    }

    fn issue_authorization_token(
        &self,
        request: jellyfin_server_implementations::AuthorizationTokenRequest,
    ) -> SessionStoreFuture<'_, String, Self::Error> {
        Box::pin(async move {
            self.devices
                .delete_by_user_and_device(request.user_id, &request.device_id)
                .await?;
            let device = self
                .devices
                .create_session(NewDevice::new(
                    request.user_id,
                    &request.app,
                    &request.app_version,
                    &request.device_name,
                    &request.device_id,
                ))
                .await?;
            Ok(device.access_token)
        })
    }
}

impl PostgresSessionStore {
    async fn can_access_device(
        &self,
        policy: &UserPolicy,
        is_administrator: bool,
        device_id: &str,
    ) -> Result<bool, AuthenticationStoreError> {
        if policy.enable_all_devices || is_administrator {
            return Ok(true);
        }
        if policy
            .enabled_devices
            .iter()
            .any(|enabled| enabled.eq_ignore_ascii_case(device_id))
        {
            return Ok(true);
        }

        let supports_persistent_identifier = self
            .devices
            .latest_by_device_id(device_id)
            .await?
            .is_none_or(|existing| {
                ClientCapabilitiesDto::from_stored_value(existing.capabilities)
                    .supports_persistent_identifier
            });
        Ok(!supports_persistent_identifier)
    }

    fn log_activity(&self, entry: NewActivityLog) {
        let repository = self.activity_logs.clone();
        tokio::spawn(async move {
            if let Err(error) = repository.create(entry).await {
                tracing::warn!(%error, "failed to write activity log entry");
            }
        });
    }
}

fn is_parental_schedule_allowed(policy: &UserPolicy, local_now: DateTime<FixedOffset>) -> bool {
    if policy.access_schedules.is_empty() {
        return true;
    }

    let hour = f64::from(local_now.hour())
        + f64::from(local_now.minute()) / 60.0
        + f64::from(local_now.second()) / 3_600.0
        + f64::from(local_now.nanosecond()) / 3_600_000_000_000.0;
    policy.access_schedules.iter().any(|schedule| {
        schedule_day_matches(schedule.day_of_week, local_now.weekday())
            && hour >= schedule.start_hour
            && hour <= schedule.end_hour
    })
}

const fn schedule_day_matches(day: DynamicDayOfWeek, weekday: Weekday) -> bool {
    match day {
        DynamicDayOfWeek::Everyday => true,
        DynamicDayOfWeek::Weekday => matches!(
            weekday,
            Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu | Weekday::Fri
        ),
        DynamicDayOfWeek::Weekend => matches!(weekday, Weekday::Sat | Weekday::Sun),
        DynamicDayOfWeek::Sunday => matches!(weekday, Weekday::Sun),
        DynamicDayOfWeek::Monday => matches!(weekday, Weekday::Mon),
        DynamicDayOfWeek::Tuesday => matches!(weekday, Weekday::Tue),
        DynamicDayOfWeek::Wednesday => matches!(weekday, Weekday::Wed),
        DynamicDayOfWeek::Thursday => matches!(weekday, Weekday::Thu),
        DynamicDayOfWeek::Friday => matches!(weekday, Weekday::Fri),
        DynamicDayOfWeek::Saturday => matches!(weekday, Weekday::Sat),
    }
}
