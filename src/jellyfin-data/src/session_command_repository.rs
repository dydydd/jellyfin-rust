use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, Set};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::session_command;

#[derive(Debug, Error)]
pub enum SessionCommandStoreError {
    #[error("session command {0} cannot be empty")]
    EmptyField(&'static str),
    #[error("session command {field} exceeds its {max} character limit")]
    FieldTooLong { field: &'static str, max: usize },
    #[error("session command payload must be a JSON object")]
    InvalidPayload,
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Debug, Clone)]
pub struct NewSessionCommand {
    pub target_session_id: String,
    pub controlling_session_id: Option<String>,
    pub message_type: String,
    pub payload: Value,
}

#[derive(Clone)]
pub struct SessionCommandRepository {
    database: crate::SharedDatabase,
}

impl SessionCommandRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Appends a session command to the `PostgreSQL` outbox.
    ///
    /// # Errors
    ///
    /// Returns validation errors for invalid command metadata or a database
    /// error when insertion fails.
    pub async fn enqueue(
        &self,
        command: NewSessionCommand,
    ) -> Result<session_command::Model, SessionCommandStoreError> {
        validate_command(&command)?;
        Ok(session_command::ActiveModel {
            id: Set(Uuid::new_v4()),
            target_session_id: Set(command.target_session_id),
            controlling_session_id: Set(command.controlling_session_id),
            message_type: Set(command.message_type),
            payload: Set(command.payload),
            date_created: Set(Utc::now()),
        }
        .insert(self.database.as_ref())
        .await?)
    }

    /// Lists queued commands for a target session in creation order.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn list_for_session(
        &self,
        target_session_id: &str,
    ) -> Result<Vec<session_command::Model>, SessionCommandStoreError> {
        Ok(session_command::Entity::find()
            .filter(session_command::Column::TargetSessionId.eq(target_session_id))
            .order_by_asc(session_command::Column::DateCreated)
            .order_by_asc(session_command::Column::Id)
            .all(self.database.as_ref())
            .await?)
    }

    /// Removes delivered commands from the outbox.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete(&self, ids: &[Uuid]) -> Result<u64, SessionCommandStoreError> {
        if ids.is_empty() {
            return Ok(0);
        }
        Ok(session_command::Entity::delete_many()
            .filter(session_command::Column::Id.is_in(ids.iter().copied()))
            .exec(self.database.as_ref())
            .await?
            .rows_affected)
    }
}

fn validate_command(command: &NewSessionCommand) -> Result<(), SessionCommandStoreError> {
    validate_required("target session id", &command.target_session_id, 64)?;
    if let Some(controlling_session_id) = command.controlling_session_id.as_deref() {
        validate_required("controlling session id", controlling_session_id, 64)?;
    }
    validate_required("message type", &command.message_type, 64)?;
    if !command.payload.is_object() {
        return Err(SessionCommandStoreError::InvalidPayload);
    }
    Ok(())
}

fn validate_required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), SessionCommandStoreError> {
    if value.is_empty() {
        return Err(SessionCommandStoreError::EmptyField(field));
    }
    if value.chars().count() > max {
        return Err(SessionCommandStoreError::FieldTooLong { field, max });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_session_command_metadata() {
        assert!(matches!(
            validate_required("target session id", "", 64),
            Err(SessionCommandStoreError::EmptyField("target session id"))
        ));
        assert!(matches!(
            validate_required("target session id", &"x".repeat(65), 64),
            Err(SessionCommandStoreError::FieldTooLong {
                field: "target session id",
                max: 64
            })
        ));
        assert!(matches!(
            validate_command(&NewSessionCommand {
                target_session_id: "target".to_owned(),
                controlling_session_id: None,
                message_type: "GeneralCommand".to_owned(),
                payload: Value::Null,
            }),
            Err(SessionCommandStoreError::InvalidPayload)
        ));
    }
}
