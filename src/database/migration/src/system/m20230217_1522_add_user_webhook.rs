use database_model::system::prelude::*;
use sea_orm_migration::{
    prelude::*,
    sea_orm::{DbBackend, EntityTrait, Schema},
};
pub struct Migration;

fn get_seaorm_create_stmt<E: EntityTrait>(e: E) -> TableCreateStatement {
    let schema = Schema::new(DbBackend::Postgres);
    schema.create_table_from_entity(e).if_not_exists().to_owned()
}

fn get_seaorm_drop_stmt<E: EntityTrait>(e: E) -> TableDropStatement {
    Table::drop().table(e).if_exists().to_owned()
}

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20230217_1522_add_user_webhook"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let stmts = vec![get_seaorm_create_stmt(UserWebhookEntity)];
        for stmt in stmts {
            manager.create_table(stmt.to_owned()).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let stmts = vec![get_seaorm_drop_stmt(UserWebhookEntity)];

        for stmt in stmts {
            manager.drop_table(stmt.to_owned()).await?;
        }

        Ok(())
    }
}
