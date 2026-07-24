use jellyfin_data::{
    BaseItemError, BaseItemRepository, PersonError as PersonRepositoryError, PersonQuery,
    PersonRepository,
    entities::{base_item, person, user},
};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub model: person::Model,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonPage {
    pub people: Vec<Person>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Error)]
pub enum PersonError {
    #[error("person was not found")]
    NotFound,
    #[error("target user was not found")]
    UserNotFound,
    #[error("person query is forbidden")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    Repository(#[from] PersonRepositoryError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
}

#[derive(Clone)]
pub struct PersonService {
    users: UserService,
    items: BaseItemRepository,
    people: PersonRepository,
}

impl PersonService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            people: PersonRepository::new(database),
        }
    }

    /// Resolves a person by exact display name and then Unicode clean name.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
    ) -> Result<Person, PersonError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let person = match self.people.get_exact(name).await {
            Ok(Some(person)) => person,
            Ok(None) => match self.people.get_normalized(name).await {
                Ok(Some(person)) => person,
                Ok(None) | Err(PersonRepositoryError::InvalidName) => {
                    return Err(PersonError::NotFound);
                }
                Err(error) => return Err(error.into()),
            },
            Err(PersonRepositoryError::InvalidName) => return Err(PersonError::NotFound),
            Err(error) => return Err(error.into()),
        };
        Ok(Person { model: person })
    }

    /// Resolves the persisted `Person` item that owns image metadata.
    ///
    /// # Errors
    ///
    /// Returns a database error when the item lookup fails.
    pub async fn image_item(&self, name: &str) -> Result<Option<base_item::Model>, PersonError> {
        Ok(self.items.get_by_type_and_name("Person", name).await?)
    }

    /// Lists people credited to filtered library items.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn list(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: PersonQuery,
    ) -> Result<PersonPage, PersonError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let page = self.people.query(&query).await?;
        Ok(PersonPage {
            people: page
                .people
                .into_iter()
                .map(|person| Person { model: person })
                .collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), PersonError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(PersonError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(PersonError::Forbidden);
        }
        Ok(())
    }
}
