use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{auth::AuthError, http::AppState};

#[derive(Debug, Serialize, FromRow)]
pub struct ClassView {
    id: Uuid,
    organization_id: Uuid,
    organization_name: String,
    name: String,
    status: String,
}

pub enum ClassError {
    Auth(AuthError),
    NotFound,
    Database(sqlx::Error),
}

impl IntoResponse for ClassError {
    fn into_response(self) -> Response {
        match self {
            Self::Auth(error) => error.into_response(),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": {"code": "class_not_found", "message": "학급을 찾을 수 없습니다"}})),
            )
                .into_response(),
            Self::Database(error) => {
                tracing::error!(%error, "학급 조회 실패");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": {"code": "internal_error", "message": "요청을 처리하지 못했습니다"}})),
                )
                    .into_response()
            }
        }
    }
}

impl From<AuthError> for ClassError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl From<sqlx::Error> for ClassError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(class_id): Path<Uuid>,
) -> Result<Json<ClassView>, ClassError> {
    let user_id = crate::auth::authenticated_user_id(&state, &headers).await?;
    let class = sqlx::query_as::<_, ClassView>(
        r#"
        SELECT c.id, c.organization_id, o.name AS organization_name, c.name, c.status
        FROM classes c
        JOIN organizations o ON o.id = c.organization_id
        WHERE c.id = $1
          AND (
            EXISTS (
                SELECT 1 FROM organization_memberships om
                WHERE om.organization_id = c.organization_id AND om.user_id = $2
            )
            OR EXISTS (
                SELECT 1 FROM class_memberships cm
                WHERE cm.class_id = c.id AND cm.user_id = $2
            )
          )
        "#,
    )
    .bind(class_id)
    .bind(user_id)
    .fetch_optional(state.pool())
    .await?
    .ok_or(ClassError::NotFound)?;

    Ok(Json(class))
}
