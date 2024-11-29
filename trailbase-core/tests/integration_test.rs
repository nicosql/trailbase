use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum_test::multipart::MultipartForm;
use axum_test::TestServer;
use cookie::Cookie;
use std::rc::Rc;
use tokio_rusqlite::params;

use trailbase_core::api::{create_user_handler, login_with_password, CreateUserRequest};
use trailbase_core::config::proto::PermissionFlag;
use trailbase_core::constants::{COOKIE_AUTH_TOKEN, RECORD_API_PATH};
use trailbase_core::records::*;
use trailbase_core::util::id_to_b64;
use trailbase_core::AppState;
use trailbase_core::{DataDir, Server, ServerOptions};

#[test]
fn test_record_apis() {
  let runtime = Rc::new(
    tokio::runtime::Builder::new_multi_thread()
      .enable_all()
      .build()
      .unwrap(),
  );

  let data_dir = temp_dir::TempDir::new().unwrap();

  let _ = runtime.block_on(async move {
  let app = Server::init(ServerOptions {
    data_dir: DataDir(data_dir.path().to_path_buf()),
    address: "".to_string(),
    admin_address: None,
    public_dir: None,
    dev: false,
    disable_auth_ui: false,
    cors_allowed_origins: vec![],
    js_runtime_threads: None,
  })
  .await
  .unwrap();

  let state = app.state();
  let conn = state.conn();

  create_chat_message_app_tables(conn).await.unwrap();
  state.refresh_table_cache().await.unwrap();

  let room = add_room(conn, "room0").await.unwrap();
  let password = "Secret!1!!";

  // Register message table as record API with moderator read access.
  add_record_api(
      &state,
      "messages_api",
      "message",
    Acls {
      authenticated: vec![PermissionFlag::Create, PermissionFlag::Read],
      ..Default::default()
    },
      AccessRules {
        create: Some(
          "(SELECT 1 FROM room_members AS m WHERE _USER_.id = _REQ_._owner AND m.user = _USER_.id AND m.room = _REQ_.room)".to_string(),
        ),
        ..Default::default()
      },
    )
    .await.unwrap();

  let user_x_email = "user_x@test.com";
  let user_x = create_user_for_test(&state, user_x_email, password)
    .await.unwrap()
    .into_bytes();

  let user_x_token = login_with_password(&state, user_x_email, password).await.unwrap();

  add_user_to_room(conn, user_x, room).await.unwrap();

  let server = TestServer::new(app.router().clone()).unwrap();

  {
    // User X can post to a JSON message.
    let test_response = server
      .post(&format!("/{RECORD_API_PATH}/messages_api"))
      .add_cookie(Cookie::new(
        COOKIE_AUTH_TOKEN,
        user_x_token.auth_token.clone(),
      ))
      .json(&serde_json::json!({
        "_owner": id_to_b64(&user_x),
        "room": id_to_b64(&room),
        "data": "user_x message to room",
      }))
      .await;

    assert_eq!(
      test_response.status_code(),
      StatusCode::OK,
      "{:?}",
      test_response
    );
  }

  {
    // User X can post a form message.
    let test_response = server
      .post(&format!("/{RECORD_API_PATH}/messages_api"))
      .add_cookie(Cookie::new(
        COOKIE_AUTH_TOKEN,
        user_x_token.auth_token.clone(),
      ))
      .form(&serde_json::json!({
        "_owner": id_to_b64(&user_x),
        "room": id_to_b64(&room),
        "data": "user_x message to room",
      }))
      .await;

    assert_eq!(test_response.status_code(), StatusCode::OK);
  }

  {
    // User X can post a multipart message.
    let form = MultipartForm::new()
      .add_text("_owner", id_to_b64(&user_x))
      .add_text("room", id_to_b64(&room))
      .add_text("data", "user_x message to room");

    let test_response = server
      .post(&format!("/{RECORD_API_PATH}/messages_api"))
      .add_cookie(Cookie::new(
        COOKIE_AUTH_TOKEN,
        user_x_token.auth_token.clone(),
      ))
      .multipart(form)
      .await;

    assert_eq!(test_response.status_code(), StatusCode::OK);
  }

  {
    // Add a second record API for the same table
    add_record_api(
      &state,
      "messages_api_yolo",
      "message",
      Acls {
        world: vec![PermissionFlag::Create, PermissionFlag::Read],
        ..Default::default()
      },
      AccessRules::default(),
    )
    .await.unwrap();

    // Anonymous can post to a JSON message (i.e. no credentials/tokens are attached).
    let test_response = server
      .post(&format!("/{RECORD_API_PATH}/messages_api_yolo"))
      .json(&serde_json::json!({
        // NOTE: Id must be not null and a random id would violate foreign key constraint as
        // defined by the `message` table.
        // "_owner": id_to_b64(&Uuid::now_v7().into_bytes()),
        "_owner": id_to_b64(&user_x),
        "room": id_to_b64(&room),
        "data": "anonymous' message to room",
      }))
      .await;

    assert_eq!(
      test_response.status_code(),
      StatusCode::OK,
      "{test_response:?}"
    );
  }

  });
}

pub async fn create_chat_message_app_tables(
  conn: &tokio_rusqlite::Connection,
) -> Result<(), anyhow::Error> {
  // Create a messages, chat room and members tables.
  conn
    .execute_batch(
      r#"
          CREATE TABLE room (
            id           BLOB PRIMARY KEY NOT NULL CHECK(is_uuid_v7(id)) DEFAULT(uuid_v7()),
            name         TEXT
          ) STRICT;

          CREATE TABLE message (
            id           BLOB PRIMARY KEY NOT NULL CHECK(is_uuid_v7(id)) DEFAULT (uuid_v7()),
            _owner       BLOB NOT NULL,
            room         BLOB NOT NULL,
            data         TEXT NOT NULL DEFAULT 'empty',

            -- on user delete, toombstone it.
            FOREIGN KEY(_owner) REFERENCES _user(id) ON DELETE SET NULL,
            -- On chatroom delete, delete message
            FOREIGN KEY(room) REFERENCES room(id) ON DELETE CASCADE
          ) STRICT;

          CREATE TABLE room_members (
            user         BLOB NOT NULL,
            room         BLOB NOT NULL,

            FOREIGN KEY(room) REFERENCES room(id) ON DELETE CASCADE,
            FOREIGN KEY(user) REFERENCES _user(id) ON DELETE CASCADE
          ) STRICT;
        "#,
    )
    .await?;

  return Ok(());
}

pub async fn add_room(
  conn: &tokio_rusqlite::Connection,
  name: &str,
) -> Result<[u8; 16], anyhow::Error> {
  let room: [u8; 16] = conn
    .query_row(
      "INSERT INTO room (name) VALUES ($1) RETURNING id",
      params!(name.to_string()),
    )
    .await?
    .unwrap()
    .get(0)?;

  return Ok(room);
}

pub async fn add_user_to_room(
  conn: &tokio_rusqlite::Connection,
  user: [u8; 16],
  room: [u8; 16],
) -> Result<(), anyhow::Error> {
  conn
    .execute(
      "INSERT INTO room_members (user, room) VALUES ($1, $2)",
      params!(user, room),
    )
    .await?;
  return Ok(());
}

pub(crate) async fn create_user_for_test(
  state: &AppState,
  email: &str,
  password: &str,
) -> Result<uuid::Uuid, anyhow::Error> {
  return Ok(
    create_user_handler(
      State(state.clone()),
      Json(CreateUserRequest {
        email: email.to_string(),
        password: password.to_string(),
        verified: true,
        admin: false,
      }),
    )
    .await?
    .id,
  );
}
