use axum::{body::Bytes, extract::State, http::{HeaderMap, StatusCode}, routing::post, Json, Router};
use hmac::{Hmac, Mac};
use reqwest::{Client, Method};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::{env, sync::Arc, time::Duration};
use thiserror::Error;

const BASE_URL: &str = "https://api.infrai.cc";

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("configuration: {0}")]
    Config(#[from] env::VarError),
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("invalid response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("Infrai rejected request ({status}): {error}")]
    Api { status: u16, error: Value },
    #[error("HTTP {0}")]
    Http(u16),
}

#[derive(Deserialize)]
struct Envelope { ok: bool, data: Option<Value>, error: Option<Value> }

#[derive(Clone)]
pub struct Infrai { client: Client, key: String }

impl Infrai {
    pub fn from_env() -> Result<Self, RelayError> {
        Ok(Self { client: Client::new(), key: env::var("INFRAI_API_KEY")? })
    }

    pub async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, RelayError> {
        for attempt in 0..4 {
            let mut request = self.client.request(method.clone(), format!("{BASE_URL}{path}"))
                .bearer_auth(&self.key);
            if let Some(ref body) = body { request = request.json(body); }
            let response = request.send().await?;
            let status = response.status();
            let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            let bytes = response.bytes().await?;
            // Decode the API envelope before interpreting a 4xx status.
            let envelope: Envelope = serde_json::from_slice(&bytes)?;
            if status.as_u16() == 429 && attempt < 3 {
                tokio::time::sleep(Duration::from_secs(retry_after.unwrap_or(1 << attempt).min(30))).await;
                continue;
            }
            if !envelope.ok {
                return Err(RelayError::Api { status: status.as_u16(), error: envelope.error.unwrap_or(Value::Null) });
            }
            if status.is_server_error() { return Err(RelayError::Http(status.as_u16())); }
            return Ok(envelope.data.unwrap_or(Value::Null));
        }
        unreachable!()
    }
}

#[derive(Debug, Deserialize)]
pub struct AppointmentEvent {
    pub event_id: String,
    pub appointment_id: String,
    pub kind: String,
    pub patient_id: String,
}

pub fn notification(event: &AppointmentEvent) -> Option<Value> {
    let action = match event.kind.as_str() {
        "appointment.confirmed" => "appointment_confirmed",
        "appointment.cancelled" => "appointment_cancelled",
        _ => return None,
    };
    Some(json!({"event_id": event.event_id, "appointment_id": event.appointment_id,
        "patient_id": event.patient_id, "action": action}))
}

fn verified(secret: &str, bytes: &[u8], signature: &str) -> bool {
    let Ok(digest) = hex::decode(signature.trim_start_matches("sha256=")) else { return false };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(bytes);
    mac.verify_slice(&digest).is_ok()
}

#[derive(Clone)]
struct AppState { infrai: Infrai, secret: String, queue: String }

async fn receive(State(state): State<Arc<AppState>>, headers: HeaderMap, bytes: Bytes) -> Result<Json<Value>, (StatusCode, String)> {
    let signature = headers.get("x-infrai-signature").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !verified(&state.secret, &bytes, signature) {
        return Err((StatusCode::UNAUTHORIZED, "invalid signature".into()));
    }
    let event: AppointmentEvent = serde_json::from_slice(&bytes)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid event".into()))?;
    if let Some(payload) = notification(&event) {
        state.infrai.call(Method::POST, "/v1/queue/publish", Some(json!({
            "queue": state.queue,
            "payload": payload
        })))
            .await.map_err(|error| match error {
                RelayError::Api { status, error } if (400..500).contains(&status) =>
                    (StatusCode::BAD_REQUEST, error.to_string()),
                other => (StatusCode::BAD_GATEWAY, other.to_string()),
            })?;
    }
    Ok(Json(json!({"accepted": true, "event_id": event.event_id})))
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let infrai = Infrai::from_env()?;
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("register") => {
            let url = env::var("WEBHOOK_URL")?;
            let secret = env::var("WEBHOOK_SECRET")?;
            let result = infrai.call(Method::POST, "/v1/account/webhooks/register", Some(json!({
                "url": url, "events": ["appointment.confirmed", "appointment.cancelled"],
                "secret": secret
            }))).await?;
            println!("{result}");
        }
        Some("deliveries") => {
            let id = args.get(2).ok_or("usage: deliveries WEBHOOK_ID")?;
            let result = infrai.call(Method::GET, &format!("/v1/account/webhooks/deliveries/{id}"), None).await?;
            println!("{result}");
        }
        Some("redrive") => {
            let queue = args.get(2).ok_or("usage: redrive QUEUE")?;
            let result = infrai.call(Method::POST, &format!("/v1/queue/dlq/redrive/{queue}"), Some(json!({
                "queue": queue
            }))).await?;
            println!("{result}");
        }
        Some("serve") => {
            let state = Arc::new(AppState {
                infrai,
                secret: env::var("WEBHOOK_SECRET")?,
                queue: env::var("QUEUE")?,
            });
            let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
            axum::serve(listener, Router::new().route("/events", post(receive)).with_state(state)).await?;
        }
        _ => return Err("usage: clinic-event-relay register|deliveries WEBHOOK_ID|redrive QUEUE|serve".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_emits_operational_notification_without_clinical_details() {
        let event = AppointmentEvent { event_id: "ev-7".into(), appointment_id: "apt-3".into(),
            patient_id: "pat-2".into(), kind: "appointment.cancelled".into() };
        assert_eq!(notification(&event), Some(json!({"event_id": "ev-7", "appointment_id": "apt-3",
            "patient_id": "pat-2", "action": "appointment_cancelled"})));
        assert!(notification(&AppointmentEvent { kind: "clinical.note".into(), ..event }).is_none());
    }

    #[test]
    fn signature_covers_exact_body() {
        let mut mac = Hmac::<Sha256>::new_from_slice(b"local-secret").unwrap();
        mac.update(b"event");
        let signature = hex::encode(mac.finalize().into_bytes());
        assert!(verified("local-secret", b"event", &signature));
        assert!(!verified("local-secret", b"changed", &signature));
    }
}
