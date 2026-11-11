use sea_orm::{
    sea_query::{Alias, Index},
    ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};

const COLUMNS: [&str; 3] = ["peer_row_id", "agent_uuid", "token_hash"];

pub(super) async fn migrate(db: &DatabaseConnection) -> anyhow::Result<()> {
    let backend = db.get_database_backend();
    // Refuse ambiguous identities before adding any indexes. Never choose a
    // surviving credential or delete enrollment/password history automatically.
    // All interpolated names below are fixed constants, not external input.
    for column in COLUMNS {
        let duplicate = db
            .query_one(Statement::from_string(
                backend,
                format!(
                    "SELECT 1 FROM companion_agents WHERE {column} IS NOT NULL \
             GROUP BY {column} HAVING COUNT(*) > 1 LIMIT 1"
                ),
            ))
            .await?;
        anyhow::ensure!(
            duplicate.is_none(),
            "duplicate companion agent {column}; stop Console instances, back up the database, \
             and reconcile duplicate enrollments before restarting; no agent rows were changed"
        );
    }
    for column in COLUMNS {
        let name = format!("companion_agents_{column}_unique");
        // MySQL has no CREATE INDEX IF NOT EXISTS. Check each named unique
        // index so a restart can resume after partially committed DDL.
        let sql = match backend {
            DatabaseBackend::MySql => format!(
                "SELECT 1 FROM information_schema.statistics WHERE table_schema = DATABASE() \
                 AND table_name = 'companion_agents' AND index_name = '{name}' AND non_unique = 0"
            ),
            DatabaseBackend::Postgres => format!(
                "SELECT 1 FROM pg_indexes WHERE schemaname = current_schema() \
                 AND tablename = 'companion_agents' AND indexname = '{name}' \
                 AND indexdef LIKE 'CREATE UNIQUE INDEX %'"
            ),
            DatabaseBackend::Sqlite => format!(
                "SELECT 1 FROM pragma_index_list('companion_agents') \
                 WHERE name = '{name}' AND \"unique\" = 1"
            ),
        };
        if db
            .query_one(Statement::from_string(backend, sql))
            .await?
            .is_none()
        {
            let index = Index::create()
                .name(&name)
                .table(Alias::new("companion_agents"))
                .col(Alias::new(column))
                .unique()
                .to_owned();
            db.execute(backend.build(&index)).await?;
        }
    }
    Ok(())
}
