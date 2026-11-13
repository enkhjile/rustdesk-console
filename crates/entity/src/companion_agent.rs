use sea_orm::entity::prelude::*;

/// Companion agent identity and durable protocol state; separate from client UUIDs.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "companion_agents")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub peer_row_id: i32,
    #[sea_orm(unique)]
    pub agent_uuid: Option<String>,
    #[sea_orm(unique)]
    pub token_hash: String,
    pub revoked: bool,
    // Decimal strings preserve the full protocol u64 range on every supported DB.
    pub revision: String,
    #[sea_orm(column_type = "Text")]
    pub desired_json: String,
    pub sequence: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub report_json: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub network_json: Option<String>,
    pub last_seen: i64,
}
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
