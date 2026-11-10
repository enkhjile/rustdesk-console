# RustDesk password changes through the companion agent

On a bound device's companion-agent panel, **Change RustDesk password** queues a
replacement permanent password. Enter it twice, then submit once. Approval and
password-type policy are unchanged. The device must use a password-capable agent
in Managed mode with the tested official RustDesk 1.4.7 LocalService installation.
Older managed agents do not consume this action; their queued delivery expires.

## Server setup

Provision `RUSTDESK_AGENT_PASSWORD_KEY` in the Console server's environment as
64 hexadecimal characters representing 32 cryptographically random bytes. Use a
dedicated key, independent of JWT/session keys. For example, a deployment operator
can generate one with `openssl rand -hex 32`, then store it in their deployment's
secret manager. Do not put the value in source control, command arguments or logs.
Missing or malformed keys disable password delivery without disabling existing
agent settings management.

Use HTTPS for Console administrator access and the enrolled agent origin. Exclude
password endpoint bodies from reverse-proxy/application tracing. The pending
payload is encrypted with AES-256-GCM, a fresh random nonce, and authenticated
device/operation identity. The key must remain available until pending operations
have been resolved. Changing the key prevents decryption of outstanding payloads;
there is no automatic key-rotation or password-retrieval facility.

## Operation behavior

Only administrators can create or inspect per-device operations at
`/api/admin/agent/:id/password`. The response contains metadata, never a password.
The existing device credential authenticates the separate agent claim and result
routes. Normal settings policies and reports contain no password.

Only one unresolved operation is allowed per device. Each has an immutable UUID
and increasing sequence. Pending secret delivery expires after 15 minutes;
cleanup runs every 30 seconds while Console is running and resumes on startup.
Terminal acknowledgement and revocation also remove the ciphertext. Metadata
for a claimed operation remains until the agent reports an outcome, preventing a
new request from overtaking unfinished execution. An agent that missed the secret
receives a null-password tombstone after expiry and acknowledges it without
changing RustDesk.

The agent persists non-secret execution intent before calling the official CLI.
An interrupted attempt is reported as unknown and is not automatically replayed.
Lost result acknowledgements retry the same metadata. Once resolved, an operator
can submit a new explicit change. A request error in the browser may follow a
committed queue operation: inspect status before retrying.

`command_accepted` means the CLI acknowledged the command; it does not mean the
product independently verified a RustDesk connection. `indeterminate` requires
operator review, `expired` means no execution by the receiving agent, and
`unsupported` means the agent cannot execute in its current client/service context.

The official RustDesk command takes the password in an argument. Ordinary-user
and unrelated-LocalService access were denied in the tested Windows service
configuration, but administrators, SYSTEM and privileged auditing/EDR can inspect
arguments or process memory. Database backups can retain old ciphertext after its
live row is cleared. Protect the server key and apply appropriate backup and
privileged-telemetry retention controls.

The paired agent validation record covers a private two-VM HTTPS run: observe-only
refusal, successful managed commands without a settings policy, lost-result-ack
recovery with a newer queued operation, and history retained after a proven reboot.
Both guests were restored and disconnected. Browser testing confirmed queued
status, cleared inputs and disabled repeat submission; all 31 UI tests and 18
agent backend tests passed. Separate official-command tests confirmed replacement
and password persistence through reboot. Command acceptance remains distinct from
independent live verification. No custom client or production deployment occurred.
