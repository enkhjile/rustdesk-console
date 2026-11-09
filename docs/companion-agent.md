# Windows companion agent integration

This fork implements the first server integration for
[rustdesk-agent](https://github.com/enkhjile/rustdesk-agent). It registers one
agent per existing Console device, projects the device's effective strategy into
the agent protocol, and persists its latest report. It does not enable agent
writes. Run the agent's normal service configuration without the experimental
`lab-apply` feature/flag. No RustDesk client modification is required.

## Provision one device

Use a test instance with trusted HTTPS first. Console's usual TLS reverse proxy
must forward `/v1/agent/*` as well as `/api/*`, without redirects. These routes
must not be cached. Set proxy request deadlines and rate limits; the integration
limits request bodies to 64 KiB and returns `Cache-Control: no-store`.

The following administrator routes use Console's existing `api-token` session
header and recheck the current administrator role. Successful replies use the
usual `{"code":0,"data":...}` envelope. Agent routes use HTTP status codes and
raw protocol JSON, without that envelope.

1. Find the device's numeric `row_id` in Console. Call
   `POST /api/admin/agent/create` with `{"peer_row_id":123}`. The response data
   contains `id` and a one-time `credential`. Only its SHA-256 hash is stored.
   Keep the credential in an approved secret provider; never put it in shell
   literals, arguments, environment variables, logs or source control.
2. Follow the agent's
   [service setup and enrollment instructions](https://github.com/enkhjile/rustdesk-agent/blob/main/docs/enrollment.md).
   Run `rustdesk-agent.exe --enroll https://console.example` as administrator,
   supplying the credential through redirected stdin from the secret provider.
   Enrollment prints a fresh agent UUID. It does not contact Console.
3. Call `POST /api/admin/agent/{id}/bind` with
   `{"agent_uuid":"<UUID printed by enrollment>"}`. The agent UUID is separate
   from the RustDesk client UUID. An unbound credential cannot poll or report.
   Repeating the same binding is safe; changing it or sharing a UUID is refused.
4. Run the agent's administrator-only `--init-state` once, then start its normal
   service. It polls Console using the per-device credential as a Bearer token.
5. Read `GET /api/admin/agent/peer/{row_id}` for the last persisted desired
   snapshot, report, sequence and successful-contact time. An unregistered device
   returns null. This API never returns credentials or hashes. Desired state is
   refreshed on agent requests, so the displayed snapshot can lag strategy edits.

`POST /api/admin/agent/{id}/revoke` immediately blocks subsequent authenticated
agent requests. It retains the UUID, counters and report. Revocation is permanent
in this slice; duplicate registrations are refused, including revoked ones.
Credential rotation, reenrollment/recovery and enrollment UI are not implemented.
Do not delete database rows to reset identities or counters. Issuance, binding
and revocation log administrator/agent IDs, never credentials.

## Desired state and reports

`GET /v1/agent/desired` uses Console's existing effective-strategy precedence
(device, user, then device group). Only these options are forwarded:

| Console option | Agent field | Values |
| --- | --- | --- |
| `approve-mode` | `approve_mode` | `password`, `click`, `password-click` |
| `verification-method` | `verification_method` | `use-temporary-password`, `use-permanent-password`, `use-both-passwords` |

Other options, including secrets, are excluded. An invalid selected value returns
503 rather than a partial strategy. No effective managed options returns 204;
the agent retains its watermark and leaves settings untouched. Omitted options
are unmanaged, not reset. This does not change Console's existing native-client
heartbeat strategy handling, which is a separate path.

A 200 response contains `schema_version: 1`, a positive u64 `revision`, and
`settings`. Revisions are allocated transactionally per agent when its observed
effective payload changes, independently of strategy timestamps. Removing an
assignment retains the counter; reassigning after a 204 allocates a higher one.
Only the current snapshot is retained, not a strategy-history archive.

`POST /v1/agent/status` requires the bound UUID, current desired revision,
positive u64 `observation_sequence`, nullable RustDesk ID/version, null
`applied_revision`, and results for exactly the requested setting keys. Unknown,
duplicate or missing required fields are rejected. Allowed outcomes are
`unsupported`, `unverified_cli_match`, `read_failed`, `write_failed`, `mismatch`
and `indeterminate`. This integration deliberately rejects `verified` and non-null
applied revisions; no automated live verifier exists yet. The default agent
reports `unsupported`, which is receipt/observation evidence, not application.

Accepted reports and identical retries return 204. Lower sequences, changed
content at the same sequence, and stale/unassigned revisions return 409. Wrong
UUIDs return 403; invalid/unbound/revoked credentials and disabled/deleted devices
return 401. State survives database reconnects. The reported RustDesk ID is
metadata only and cannot rebind a device. Admin counter fields are decimal
strings to preserve u64 precision in JavaScript; protocol counters remain JSON
integers.

## Validation and remaining gates

Route/store tests use dummy credentials and SQLite, including a file database
reopened between requests and concurrent requests through independent database
connections. They cover authorization, immutable binding, allowlisting,
malformed reports, retry identity, timestamp collisions, unassignment,
revocation, disabled devices, body limits and counter exhaustion.

Run `cargo test --locked -p rustdesk-console features::agent`. Build embedded
assets first with `pnpm install --frozen-lockfile && pnpm run build` in `web`.
The PR workflow runs the full Rust suite, Clippy, build, and existing web tests
on Linux; it does not publish or deploy.

MySQL/PostgreSQL transaction behavior, a real Windows agent against this Console
over HTTPS, credential rotation/recovery and operational audit retention still
need validation or implementation before rollout. The existing agent fixture/VM
tests do not substitute for that end-to-end Console canary. Production automatic
application remains disabled.
