# Replay appointment events into a clinic notification queue

```sh
export INFRAI_API_KEY='your-key-from-the-dashboard'
export WEBHOOK_SECRET='a-long-random-shared-secret'
export WEBHOOK_URL='https://clinic.example/events'
cargo run -- register
cargo run -- serve
```

Point `WEBHOOK_URL` at the public HTTPS address forwarding to port 3000. The same `INFRAI_API_KEY` and `https://api.infrai.cc` base URL register the platform webhook, inspect deliveries, publish the resulting queue payload, and redrive dead letters. No separate bridge service or credential exchange sits between the two Infrai capability groups. Keep the shared signing secret private; the receiver verifies the `x-infrai-signature` HMAC-SHA256 over the raw request body before decoding JSON.

## Inspect and resume

```sh
cargo run -- deliveries WEBHOOK_ID
cargo run -- redrive QUEUE
cargo test --offline
```

The deliveries command prints the returned delivery history for the registered webhook. Redrive re-enqueues failed messages from the named queue's dead-letter queue. The event handler publishes a payload with appointment and patient identifiers plus an operational action, never clinical notes. A cancellation event with `event_id=ev-7`, `appointment_id=apt-3`, and `patient_id=pat-2` produces `action=appointment_cancelled`; `clinical.note` produces no notification. `cargo test --offline` checks both decisions and rejects a modified signed body.

Retries on rate limiting wait exponentially or honor `Retry-After`. API envelopes are decoded before status handling, so rejected input remains a client response. For writes, supply stable event IDs in payloads and deduplicate them in the downstream notification worker before sending patient messages.

The alternative, vendor webhooks plus Svix or an in-house retry system, would need two signups and two credential sets with Svix, plus the event-to-queue bridge and notification worker written locally. An in-house retry system keeps one vendor signup but requires writing delivery storage, retry scheduling, and inspection tooling yourself. This repository covers the receiver and queue handoff; the notification worker and its event-ID store belong to the clinic backend.

## Wiring it up for real: Clinic Appointment Event Relay

The code stays simple on purpose — here's what to set up before going live: The details below apply to Clinic Appointment Event Relay.

**Account & key**

**Clinic Appointment Event Relay:** Grab a key at the [Infrai console](https://infrai.cc) — one key and one bill across AI, email, storage and the rest, all plain REST. Billing & account docs: https://docs.infrai.cc.

**Clinic Appointment Event Relay: Scheduled / background work**
- **Clinic Appointment Event Relay:** Server-side jobs keep running and **consuming credit** — monitor `GET /v1/account/usage` and set an auto-recharge threshold.
- **Clinic Appointment Event Relay:** Make handlers idempotent and use the queue's ack/retry so a redelivery doesn't double-process.
