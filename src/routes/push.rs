use axum::{extract::State, response::Json, Json as JsonBody};
use serde::{Deserialize, Serialize};

use crate::{
    db,
    notify::{self, VapidConfig},
    scheduler, AppState,
};

#[derive(Deserialize)]
pub struct SubscribeRequest {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Deserialize)]
pub struct UnsubscribeRequest {
    pub endpoint: String,
}

#[derive(Serialize)]
pub struct ApiResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn subscribe(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<SubscribeRequest>,
) -> Json<ApiResponse> {
    if let Err(e) = state
        .db
        .insert_subscription(&body.endpoint, &body.p256dh, &body.auth)
        .await
    {
        return Json(ApiResponse {
            ok: false,
            error: Some(format!("{e}")),
        });
    }

    tracing::info!("Subscription added: {}", body.endpoint);
    let sub = db::Subscription {
        endpoint: body.endpoint,
        p256dh: body.p256dh,
        auth: body.auth,
    };
    let vapid = VapidConfig {
        subject: state.config.vapid_subject.clone(),
        public_key_b64: state.config.vapid_public_key.clone(),
        private_key_b64: state.config.vapid_private_key.clone(),
    };
    let _ = notify::send_one_sub(&sub, "Notifications enabled!", &vapid).await;

    Json(ApiResponse {
        ok: true,
        error: None,
    })
}

pub async fn test_summary(State(state): State<AppState>) -> Json<ApiResponse> {
    let message = match scheduler::build_daily_summary(&state.db, &state.config).await {
        Ok(m) => m,
        Err(e) => {
            return Json(ApiResponse {
                ok: false,
                error: Some(format!("{e}")),
            });
        }
    };

    let subscriptions = match state.db.list_subscriptions().await {
        Ok(s) => s,
        Err(e) => {
            return Json(ApiResponse {
                ok: false,
                error: Some(format!("{e}")),
            });
        }
    };

    let vapid = VapidConfig {
        subject: state.config.vapid_subject.clone(),
        public_key_b64: state.config.vapid_public_key.clone(),
        private_key_b64: state.config.vapid_private_key.clone(),
    };

    let results = notify::send_all(&subscriptions, &message, &vapid).await;
    let success_count = results.iter().filter(|r| r.is_ok()).count();
    tracing::info!(
        "Test summary sent to {}/{} subscribers",
        success_count,
        subscriptions.len()
    );

    Json(ApiResponse {
        ok: true,
        error: None,
    })
}

pub async fn unsubscribe(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<UnsubscribeRequest>,
) -> Json<ApiResponse> {
    match state.db.delete_subscription(&body.endpoint).await {
        Ok(_) => {
            tracing::info!("Subscription removed: {}", body.endpoint);
            Json(ApiResponse {
                ok: true,
                error: None,
            })
        }
        Err(e) => Json(ApiResponse {
            ok: false,
            error: Some(format!("{e}")),
        }),
    }
}
