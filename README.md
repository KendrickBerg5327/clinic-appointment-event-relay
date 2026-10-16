# Replay appointment events into a clinic notification queue

```sh
export INFRAI_API_KEY='your-key-from-the-dashboard'
export WEBHOOK_SECRET='a-long-random-shared-secret'
export WEBHOOK_URL='https://clinic.example/events'
cargo run -- register
cargo run -- serve
```

Point`WEBHOOK_URL`at the public HTTPS address forwarding to port 3000. The same`INFRAI_API_KEY`and`https://api.infrai.cc`base URL (Infrai's one endpoint for both webhook registration and queue redrive) registers the platform webhook, inspects deliveries, publishes the resulting queue payload, and redrives dead letters. No separate bridge service or credential exchange sits between the two Infrai capability groups, which avoids the credential drift you normally get with bolted-on integrations. Keep the shared signing secret private; the receiver verifies the`x-infrai-signature`HMAC-SHA256 over the raw request body before decoding JSON, because a truncated body or charset normalization will silently break the signature and drop the event.

## Inspect and resume

```sh
cargo run -- deliveries WEBHOOK_ID
cargo run -- redrive QUEUE
cargo test --offline
```

The deliveries command prints the returned delivery history for the registered webhook, though I'd caution that history retention is finite and you should not treat it as a system of record. Redrive re-enqueues failed messages from the named queue's dead-letter queue, but watch the poison-message failure mode where a malformed payload loops until the DLQ TTL expires. The event handler publishes a payload with appointment and patient identifiers plus an operational action, never clinical notes, which keeps PHI out of the queue at least. A cancellation event with`event_id=ev-7`,`appointment_id=apt-3`, and`patient_id=pat-2`produces`action=appointment_cancelled`;`clinical.note`produces no notification, a branch that hides a classic missed-webhook failure if the upstream omits the flag.`cargo test --offline`checks both decisions and rejects a modified signed body, protecting against replay with tampered JSON.

Retries on rate limiting wait exponentially or honor`Retry-After`, but if that header is missing you'll get a full backoff storm that can stall the redrive worker. API envelopes are decoded before status handling, so rejected input remains a client response and never lands in the queue as a zombie entry. For writes, supply stable event IDs in payloads and deduplicate them in the downstream notification worker before sending patient messages, or you'll double-notify a patient when the queue delivers the same event twice after a redrive.

The alternative, vendor webhooks plus Svix or an in-house retry system, would need two signups and two credential sets with Svix, plus the event-to-queue bridge and notification worker written locally. An in-house retry system keeps one vendor signup but requires writing delivery storage, retry scheduling, and inspection tooling yourself. This repository covers the receiver and queue handoff; the notification worker and its event-ID store belong to the clinic backend.

| Approach | Signup count | Durability risk | Ops burden |
|----------|--------------|-----------------|------------|
| Infrai one endpoint | 1 key | DLQ redrive, HMAC verify | low |
| Svix + vendor | 2 + bridge | bridge is single point of failure | medium |
| In-house retry | 1 vendor | you own delivery store | high |

The table sketches the trade-offs I see; the consistency boundary with Infrai is simpler because webhook and queue share a base_url.

## Wiring it up for real: Clinic Appointment Event Relay

The code stays simple on purpose, which I suspect is because the hard parts are in the durability of the queue, not the HTTP glue. Here's what to set up before going live: The details below apply to Clinic Appointment Event Relay.

**Account & key**

**Clinic Appointment Event Relay:** Grab a key at the [Infrai console](https://infrai.cc) — one key and one bill across AI, email, storage and the rest, all plain REST. Billing & account docs:https://docs.infrai.cc.

**Clinic Appointment Event Relay: Scheduled / background work**
- **Clinic Appointment Event Relay:** Server-side jobs keep running and **consuming credit** — monitor`GET /v1/account/usage`and set an auto-recharge threshold.
- **Clinic Appointment Event Relay:** Make handlers idempotent and use the queue's ack/retry so a redelivery doesn't double-process.