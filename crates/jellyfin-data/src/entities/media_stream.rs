use sea_orm::entity::prelude::*;

// The entity intentionally mirrors Jellyfin's normalized persistence columns.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media_streams", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub stream_index: i32,
    pub stream_type: i16,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channel_layout: Option<String>,
    pub profile: Option<String>,
    pub aspect_ratio: Option<String>,
    pub path: Option<String>,
    pub is_interlaced: Option<bool>,
    pub bit_rate: Option<i32>,
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub is_original: bool,
    pub height: Option<i32>,
    pub width: Option<i32>,
    pub average_frame_rate: Option<f32>,
    pub real_frame_rate: Option<f32>,
    pub level: Option<f32>,
    pub pixel_format: Option<String>,
    pub bit_depth: Option<i32>,
    pub is_anamorphic: Option<bool>,
    pub ref_frames: Option<i32>,
    pub codec_tag: Option<String>,
    pub comment: Option<String>,
    pub nal_length_size: Option<String>,
    pub is_avc: Option<bool>,
    pub title: Option<String>,
    pub time_base: Option<String>,
    pub codec_time_base: Option<String>,
    pub color_primaries: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub dv_version_major: Option<i32>,
    pub dv_version_minor: Option<i32>,
    pub dv_profile: Option<i32>,
    pub dv_level: Option<i32>,
    pub rpu_present_flag: Option<i32>,
    pub el_present_flag: Option<i32>,
    pub bl_present_flag: Option<i32>,
    pub dv_bl_signal_compatibility_id: Option<i32>,
    pub is_hearing_impaired: Option<bool>,
    pub rotation: Option<i32>,
    pub hdr10_plus_present_flag: Option<bool>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::base_item::Entity",
        from = "Column::ItemId",
        to = "super::base_item::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    BaseItem,
}

impl Related<super::base_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BaseItem.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
