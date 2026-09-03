use sea_orm::entity::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum ItemValueType {
    #[sea_orm(num_value = 0)]
    Artist,
    #[sea_orm(num_value = 1)]
    AlbumArtist,
    #[sea_orm(num_value = 2)]
    Genre,
    #[sea_orm(num_value = 3)]
    Studios,
    #[sea_orm(num_value = 4)]
    Tags,
    #[sea_orm(num_value = 6)]
    InheritedTags,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_values", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_value_id: Uuid,
    #[sea_orm(column_name = "type")]
    pub value_type: ItemValueType,
    pub value: String,
    pub clean_value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
