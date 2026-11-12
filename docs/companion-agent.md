# Windows companion agent integration

## Guided Windows setup

Issue a credential for the existing device from **Device info → Companion agent**.
On Windows, open `rustdesk-agent-setup.exe` and enter this Console's HTTPS origin
and device credential. Setup binds its generated UUID and completes local setup;
manual UUID copying is only needed for older agents. Install this Console version
before using the new setup executable. No database migration is required.

`POST /v1/agent/enroll` uses the device Bearer credential and a strict
`{"agent_uuid":"<UUID v4>"}` body. It binds only the token's own enabled device,
returns 204 for the first binding and identical retries, and rejects rebinding or
UUID collisions with 409. Revoked, unknown and administrator credentials cannot
enroll. Normal polling continues to reject unbound credentials. Binding and token
rotation/revocation use the same database write lock.

`GET /v1/agent/enroll` returns 202 until the authenticated bound device has a stored
report, then 204. This confirms historical report receipt, not freshness after an
upgrade or live RustDesk behavior. Neither enrollment route updates last contact.
Both use the existing body limit and `Cache-Control: no-store` response policy.

The [isolated installer validation](https://github.com/enkhjile/rustdesk-agent/blob/codex/guided-installer/docs/validation/guided-setup.md)
records setup and upgrade behavior against this Console implementation.

## Existing management integration

This fork implements the first server integration for
[rustdesk-agent](https://github.com/enkhjile/rustdesk-agent). It registers one
agent per existing Console device, projects the device's effective strategy into
the agent protocol, and persists its latest report. A normally installed managed
agent applies the two supported settings; an observe-only agent only reports.
No special build or RustDesk client modification is required.

Administrators can also queue explicit permanent-password changes through the
separate [password management API and device controls](agent-passwords.md).
Password delivery requires the server encryption key and a capable managed agent;
passwords are never included in settings policies or reports.

## Provision one device

Administrators can open **Devices → Device info → Companion agent** to enroll
an existing device, bind the UUID printed by local agent enrollment, and view
its latest report and counters. Rotate and revoke require confirmation. Newly
issued credentials are displayed once, outside browser storage and query caches,
and cleared when hidden, when the drawer closes, or when the device/session changes.
Copying is explicit; the clipboard is not cleared automatically.

The panel shows the last successful contact, not a live online indicator. Desired
values reflect the current effective strategy. Changes awaiting an agent poll or
report show as pending, with readable per-setting outcomes afterward. Links open
the existing strategy editor and assignments; device, user, then group precedence
selects the effective strategy. The panel displays reported service mode, or
unknown for older agents. CLI matches do not establish live verification.
Local administrator enrollment, state initialization,
and service installation are still required on the Windows device.

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
   [service setup and enrollment instructions](https://github.com/enkhjile/rustdesk-agent/blob/alpha/docs/enrollment.md).
   Run `rustdesk-agent.exe --enroll https://console.example` as administrator,
   supplying the credential through redirected stdin from the secret provider.
   Enrollment prints a fresh agent UUID. It does not contact Console.
3. Call `POST /api/admin/agent/{id}/bind` with
   `{"agent_uuid":"<UUID printed by enrollment>"}`. The agent UUID is separate
   from the RustDesk client UUID. An unbound credential cannot poll or report.
   Repeating the same binding is safe; changing it or sharing a UUID is refused.
4. Run the agent's administrator-only `--init-state` once, then start its normal
   service. It polls Console using the per-device credential as a Bearer token.
5. Read `GET /api/admin/agent/peer/{row_id}` for current effective settings,
   report, sequence and successful-contact time. `policy_pending` identifies
   edits not fetched by the agent; `reported_revision` is a decimal string for
   exact comparisons. Reading status does not allocate revisions or update
   contact time. An unregistered device returns null. No credentials are returned.

`POST /api/admin/agent/{id}/revoke` immediately blocks subsequent authenticated
agent requests. It retains the UUID, counters and report. Duplicate registrations
are refused, including revoked ones. Do not delete rows to reset counters.

## Rotate or recover a device credential

Stop the agent service, then call `POST /api/admin/agent/{id}/rotate` using an
administrator session. The response data contains `id` and a fresh one-time
`credential`. Rotation immediately invalidates the old token and clears
revocation, preserving the device/UUID binding, desired revision, sequence,
report and last-contact time. Only the new credential's hash is stored.

Supply the replacement through redirected stdin to the agent's elevated
`--replace-credential` command, then restart its normal service. This requires
an agent build supporting replacement and intact enrollment and initialized
state. Do not run `--enroll` or `--init-state` again. Local replacement preserves
identity and durable state; verify resumed reports in Console afterward.

If the response is lost or setup is interrupted, an administrator can rotate
again and install the newest token. All older tokens remain invalid. An unbound
registration can also rotate, but remains inactive until explicitly bound.
Disabled or missing Console devices still cannot authenticate. This is recovery
of credentials, not recovery of destroyed local identity/state or reassignment
to another machine. The device panel's **Rotate credential** action performs the
same operation; **Revoke access** blocks subsequent polling/reporting without
changing RustDesk settings.

Issuance, rotation, binding and revocation log administrator/agent IDs, never
credentials. Rotation is serialized with authenticated polling/report updates.

The paired agent's [isolated rotation test](https://github.com/enkhjile/rustdesk-agent/blob/feat/credential-rotation/docs/validation/credential-rotation.md)
passed old-token rejection during an interrupted handoff, administrator local
replacement, Console/service restart retention and recovery from revocation.
Reports retained the UUID/revision and advanced the sequence; settings remained
`unsupported` with null applied revision. The VM was restored and stopped.
The actual replacement CLI also rejected the dummy ordinary-user service-logon
token with exit 1 and `PermissionDenied`; enrollment/state bytes and permissions
remained unchanged. The temporary account/service were removed by VM restoration.

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
applied revisions; no automated live verifier exists yet. Observe-only agents
report `unsupported`. Optional `management_mode` accepts `managed` or
`observe_only`; absence remains valid for old agents. Explicit null and unknown
values are rejected, and mode is part of identical-retry identity.

When no policy is assigned, the status route also accepts a heartbeat with
`desired_revision: null`, `applied_revision: null` and `results: {}`. All other
required fields, authentication and sequence rules remain the same. The device
panel identifies this as a heartbeat with no assigned policy and shows reported
RustDesk metadata and last contact. It does not claim live health or verified
application. A heartbeat conflicts if a policy was assigned before delivery;
the agent must poll again. Removing a policy retains the revision watermark;
subsequent reassignment allocates a higher revision.

Deploy this server support before the paired agent update. Numeric-revision
reports from existing agents remain supported. Older agents can poll without a
policy but will not send heartbeats. No database migration is needed.

The paired [agent #23](https://github.com/enkhjile/rustdesk-agent/pull/23) records
the isolated heartbeat canary. No-policy contact, identical retry after agent
and Console restarts, and first assignment passed. Policy removal resumed
heartbeats while retaining revision 1; reassignment produced revision 2.
Revocation returned 401 without advancing the stored sequence or last contact.
The real-browser device-page check also passed. Automatic writes stayed disabled,
before/after detection output matched, and the VM was restored and stopped.

Accepted reports and identical retries return 204. Lower sequences, changed
content at the same sequence, and revisions that do not match the current
assignment (including null versus assigned) return 409. Wrong
UUIDs return 403; invalid/unbound/revoked credentials and disabled/deleted devices
return 401. State survives database reconnects. The reported RustDesk ID is
metadata only and cannot rebind a device. Admin counter fields are decimal
strings to preserve u64 precision in JavaScript; protocol counters remain JSON
integers.

## Validation and remaining gates

Normal management changes are paired with
[agent #25](https://github.com/enkhjile/rustdesk-agent/pull/25).
Mode compatibility, retry identity, immediate pending-policy display and policy
removal are covered by backend/panel tests. A real Chrome test against an isolated
loopback Console instance confirmed the policy editor link, editing approval mode,
and pending display without an agent poll. That browser used synthetic reports.
The separate-host private two-VM run then passed managed reporting, identical
retry after a dropped acknowledgement, observe-only upgrades, explicit managed
recovery, policy removal/reassignment and unattended reboot. The normal CI package
ran as LocalService against this Console build. Reports retained null applied
revision and unverified CLI outcomes. Both VMs were restored and stopped, with
zero network adapters. See the
[paired validation record](https://github.com/enkhjile/rustdesk-agent/blob/feat/console-managed-settings/docs/validation/console-managed-settings.md)
for that validation run. Subsequent password API and connection-test results are
documented in [password management](agent-passwords.md).

Route/store tests use dummy credentials and SQLite, including a file database
reopened between requests and concurrent requests through independent database
connections. They cover authorization, immutable binding, allowlisting,
malformed reports, retry identity, timestamp collisions, unassignment,
revocation, disabled devices, body limits and counter exhaustion.

Run `cargo test --locked -p rustdesk-console features::agent`. Build embedded
assets first with `pnpm install --frozen-lockfile && pnpm run build` in `web`.
The PR workflow runs the full Rust suite, Clippy, build, and existing web tests
on Linux; it does not publish or deploy.

The [real Windows service canary](companion-agent-validation.md) passed enrollment,
trusted loopback HTTPS, exact report retry after restart, higher revisions,
Console/agent restart persistence and revocation with SQLite. The VM was restored
and shut down. This is separate from the earlier agent contract fixture tests.

The [device-page canary](companion-agent-ui-validation.md) also passed UI-issued
enrollment and binding, real-service report display, credential rotation,
revocation and recovery. Revision 2 was retained while the observation sequence
advanced to 4; results stayed unsupported. The isolated VM was restored afterward.

MySQL/PostgreSQL transaction behavior, a network/reverse-proxy deployment,
unattended reboot with Console and operational
audit retention still need validation or implementation before rollout.
Normally installed managed agents apply assigned supported settings. Use
observe-only mode when changes are not wanted; removing a policy leaves existing
RustDesk settings unchanged. No production deployment was performed by these
validation runs.
