use sea_orm::entity::prelude::*;

/// One current operation per agent. Plaintext is never stored here.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "agent_password_operations")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub agent_id: i32,
    pub operation_id: String,
    pub sequence: String,
    pub agent_uuid: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub ciphertext: Option<String>,
    pub expires_at: i64,
    pub status: String,
    pub actor_id: i32,
}
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
