//! API en tiempo real y persistencia para Halloween.
//!
//! Rutas migradas:
//! - `GET /api/halloween/status`: estado actual de los 8 participantes.
//! - `POST /api/halloween/toggle`: alterna color/B&W de un participante (1..8).
//! - `POST /api/halloween/reset`: resetea todos los participantes a color.

use std::sync::{Arc, RwLock};
use axum::{
    extract::State,
    http::StatusCode,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use data::DocSink;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::routes::json_response;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HalloweenState {
    pub user1: bool,
    pub user2: bool,
    pub user3: bool,
    pub user4: bool,
    pub user5: bool,
    pub user6: bool,
    pub user7: bool,
    pub user8: bool,
    #[serde(rename = "lastVibrate")]
    pub last_vibrate: i64,
    #[serde(rename = "lastUpdatedUser")]
    pub last_updated_user: Option<u8>,
}

impl Default for HalloweenState {
    fn default() -> Self {
        Self {
            user1: false,
            user2: false,
            user3: false,
            user4: false,
            user5: false,
            user6: false,
            user7: false,
            user8: false,
            last_vibrate: Utc::now().timestamp_millis(),
            last_updated_user: None,
        }
    }
}

#[derive(Clone)]
pub struct HalloweenApiState {
    pub state: Arc<RwLock<HalloweenState>>,
    pub doc_sink: Option<Arc<dyn DocSink>>,
}

#[derive(Debug, Deserialize)]
pub struct TogglePayload {
    #[serde(rename = "userId")]
    pub user_id: Option<Value>,
}

pub fn router(doc_sink: Option<Arc<dyn DocSink>>) -> Router {
    let state = HalloweenApiState {
        state: Arc::new(RwLock::new(HalloweenState::default())),
        doc_sink,
    };

    Router::new()
        .route("/api/halloween/status", get(halloween_status))
        .route("/api/halloween/toggle", post(halloween_toggle))
        .route("/api/halloween/reset", post(halloween_reset))
        .with_state(state)
}

async fn halloween_status(State(st): State<HalloweenApiState>) -> Response {
    let state = st.state.read().unwrap().clone();
    json_response(StatusCode::OK, &json!({ "success": true, "state": state }))
}

async fn halloween_toggle(
    State(st): State<HalloweenApiState>,
    Json(payload): Json<TogglePayload>,
) -> Response {
    let user_num = match &payload.user_id {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0) as u8,
        Some(Value::String(s)) => s.trim().parse::<u8>().unwrap_or(0),
        _ => 0,
    };

    if user_num < 1 || user_num > 8 {
        return json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "success": false, "error": "userId debe ser entre 1 y 8" }),
        );
    }

    let updated_state = {
        let mut s = st.state.write().unwrap();
        match user_num {
            1 => s.user1 = !s.user1,
            2 => s.user2 = !s.user2,
            3 => s.user3 = !s.user3,
            4 => s.user4 = !s.user4,
            5 => s.user5 = !s.user5,
            6 => s.user6 = !s.user6,
            7 => s.user7 = !s.user7,
            8 => s.user8 = !s.user8,
            _ => {}
        }
        s.last_vibrate = Utc::now().timestamp_millis();
        s.last_updated_user = Some(user_num);
        s.clone()
    };

    if let Some(sink) = &st.doc_sink {
        let user_key = format!("user{user_num}");
        let is_bw = match user_num {
            1 => updated_state.user1,
            2 => updated_state.user2,
            3 => updated_state.user3,
            4 => updated_state.user4,
            5 => updated_state.user5,
            6 => updated_state.user6,
            7 => updated_state.user7,
            8 => updated_state.user8,
            _ => false,
        };

        let update_doc = json!({
            user_key: is_bw,
            "lastVibrate": updated_state.last_vibrate,
            "lastUpdatedUser": user_num,
            "updatedAt": data::timestamp_now(),
        });
        let _ = sink.merge_doc("halloween_state", "participants", &update_doc).await;

        let action_id = format!("action_{}", Utc::now().timestamp_millis());
        let action_doc = json!({
            "user": format!("user{user_num}"),
            "userNumber": user_num,
            "isBw": is_bw,
            "action": if is_bw { "eliminate_to_bw" } else { "restore_to_color" },
            "createdAt": Utc::now().timestamp_millis(),
        });
        let _ = sink.set_doc("halloween_actions", &action_id, &action_doc).await;
    }

    json_response(StatusCode::OK, &json!({ "success": true, "state": updated_state }))
}

async fn halloween_reset(State(st): State<HalloweenApiState>) -> Response {
    let updated_state = {
        let mut s = st.state.write().unwrap();
        s.user1 = false;
        s.user2 = false;
        s.user3 = false;
        s.user4 = false;
        s.user5 = false;
        s.user6 = false;
        s.user7 = false;
        s.user8 = false;
        s.last_vibrate = Utc::now().timestamp_millis();
        s.last_updated_user = None;
        s.clone()
    };

    if let Some(sink) = &st.doc_sink {
        let reset_doc = json!({
            "user1": false,
            "user2": false,
            "user3": false,
            "user4": false,
            "user5": false,
            "user6": false,
            "user7": false,
            "user8": false,
            "lastVibrate": updated_state.last_vibrate,
            "lastUpdatedUser": Value::Null,
            "updatedAt": data::timestamp_now(),
        });
        let _ = sink.merge_doc("halloween_state", "participants", &reset_doc).await;
    }

    json_response(StatusCode::OK, &json!({ "success": true, "state": updated_state }))
}
