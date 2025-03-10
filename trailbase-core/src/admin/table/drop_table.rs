use axum::{
  extract::State,
  http::StatusCode,
  response::{IntoResponse, Response},
  Json,
};
use log::*;
use serde::Deserialize;
use ts_rs::TS;

use crate::admin::AdminError as Error;
use crate::app_state::AppState;
use crate::transaction::TransactionRecorder;

#[derive(Clone, Debug, Deserialize, TS)]
#[ts(export)]
pub struct DropTableRequest {
  pub name: String,
}

pub async fn drop_table_handler(
  State(state): State<AppState>,
  Json(request): Json<DropTableRequest>,
) -> Result<Response, Error> {
  let conn = state.conn();
  let table_name = &request.name;

  let entity_type: &str;
  if state.table_metadata().get(table_name).is_some() {
    entity_type = "TABLE";
  } else if state.table_metadata().get_view(table_name).is_some() {
    entity_type = "VIEW";
  } else {
    return Err(Error::Precondition(format!(
      "Table or view '{table_name}' not found"
    )));
  }

  let writer = {
    let table_name = table_name.clone();
    let migration_path = state.data_dir().migrations_path();
    conn
      .call(move |conn| {
        let mut tx = TransactionRecorder::new(
          conn,
          migration_path,
          format!("drop_{}_{table_name}", entity_type.to_lowercase()),
        )?;

        let query = format!("DROP {entity_type} IF EXISTS {table_name}");
        info!("dropping table: {query}");
        tx.execute(&query)?;

        return tx
          .rollback_and_create_migration()
          .map_err(|err| trailbase_sqlite::Error::Other(err.into()));
      })
      .await?
  };

  // Update configuration: remove all APIs reference the no longer existing table.
  let mut config = state.get_config();
  let old_config_hash = config.hash();

  config.record_apis.retain(move |c| {
    if let Some(ref table) = c.table_name {
      return table != table_name;
    }
    return true;
  });
  state
    .validate_and_update_config(config, Some(old_config_hash))
    .await?;

  // Write migration file and apply it right away.
  if let Some(writer) = writer {
    let _report = writer.write(conn).await?;
  }

  state.table_metadata().invalidate_all().await?;

  return Ok((StatusCode::OK, "").into_response());
}
