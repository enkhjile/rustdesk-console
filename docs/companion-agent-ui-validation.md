# Device-page validation with the Windows agent

The merged device-page controls completed enrollment, binding, credential
rotation, revocation and recovery against the actual Windows service. No product
failure was found in this flow. Automatic settings application stayed disabled.

## Configuration

- Console: `e236d61` (PR #3), SQLite, with its production web build.
- Agent: source tree identical to merged `039ec48` (PR #21), built with pinned
  Rust 1.99.1 and no `lab-apply` feature or service flag.
- Disposable `RustDeskAgent-Validation`: Windows 11 Pro 10.0.26200, x64,
  official RustDesk 1.4.7. Setup and replacement ran as `LabAdmin`; the agent
  service ran as `NT AUTHORITY\LocalService` with its service SID.
- The VM had **zero network adapters**. Console and SQLite ran inside it on
  `127.0.0.1:21124`. The agent used `https://localhost:8444`, with the guest's
  trusted lab certificate and normal hostname/certificate validation.
- A dedicated host browser loaded the matching built UI from host loopback.
  A temporary loopback bridge forwarded its API requests through PowerShell
  Direct to guest Console. The debug server's web assets were served separately;
  this was not a release-package installation or a network/proxy deployment test.
- Only dummy accounts, device data and credentials were used. The runner kept
  UI-issued credentials in memory and supplied them to the actual agent CLI
  through redirected stdin. Credentials were not read from browser storage,
  passed in command arguments, or included in the recorded evidence.

## Behaviors and results

| Behavior | Result |
| --- | --- |
| Enroll from **Devices → Info → Companion agent** | UI issued one device credential. Administrator `--enroll` and `--init-state` succeeded in the guest. |
| Bind the UUID returned by local enrollment | UI binding succeeded; the panel showed Bound with the actual enrolled UUID. |
| Start the real Windows service | Authenticated HTTPS GET returned 200 and status POST returned 204. UI showed revision 1, observation sequence 1, RustDesk ID/version and successful-contact time. |
| Change desired state and restart the service | A lab strategy changed approval from `click` to `password-click`, retaining temporary-password verification. UI showed revision 2 and sequence 2 after the service reported. Strategy assignment used the existing API; the strategy editor was not under test. |
| Rotate through the UI before local replacement | After confirmation, the old credential received 401 from actual service polling. Console retained revision 2 and sequence 2. |
| Install the UI-issued replacement | With the service stopped, administrator `--replace-credential` preserved UUID, HTTPS origin and state-file bytes. Restarted service reporting advanced to sequence 3 at revision 2, visible after UI refresh. |
| Revoke through the UI | After confirmation, the panel showed Revoked. Restarted service polling received 401; Console retained the last accepted report at sequence 3. |
| Recover through UI rotation and local replacement | Rotation cleared revocation. Local replacement again preserved identity and state-file bytes. Reporting resumed at revision 2, sequence 4; the panel showed Bound. No reenrollment or state initialization was used for recovery. |
| Read-only results | Every accepted report contained `unsupported` for both settings and null applied revision. The UI displayed “No verified application.” Before/after detector observations matched, including `ambiguous_default` for both settings; this is not independent live-state verification. |
| Restoration | Pre-test checkpoint restored and removed. VM verified Off, with zero network adapters and zero test checkpoints. Original enrollment, durable state, service configuration and RustDesk settings were restored with the checkpoint. Dedicated host browser and bridge servers were stopped. |

All five management requests (create, bind, rotate, revoke, rotate) originated
from the UI and succeeded. The local helper performed only administrator CLI
setup/replacement, service control, lab strategy assignment and evidence capture.
The test did not invoke agent setters or edit RustDesk TOML.

## Commands and checks

```text
Console: pnpm test; pnpm run build
Console: cargo +1.99.1 build --locked -p rustdesk-console
Agent: cargo fmt --check
Agent: cargo build --locked --target x86_64-pc-windows-msvc
rustdesk-agent.exe --enroll https://localhost:8444
rustdesk-agent.exe --init-state
rustdesk-agent.exe --replace-credential
rustdesk-agent.exe --service "C:\Program Files\RustDesk\rustdesk.exe" C:\ProgramData\RustDeskAgent\ui-canary.json
```

The 21 Console web tests passed. Agent formatting and the default Windows x64
build passed. On the non-elevated host, 26 agent unit tests passed and three
administrator-only protected-storage tests refused execution; that local full
suite was not green. The real guest administrator enrollment/replacement and
LocalService polling results above are separate runtime evidence.

Tested executable SHA-256:

| Executable | SHA-256 |
| --- | --- |
| Agent | `1AB4D9F9CDE5A308D6F4510E303D8BEFC26AC150F6E4D964E1690FF43E70A325` |
| Console | `8E4587CAC11D6FBDE749002D3050ABCAC34B9747A0B9B3A2CF18882885D75495` |

Remaining gates include release-package/installer behavior, a real TLS reverse
proxy and network deployment, MySQL/PostgreSQL behavior, operational audit
retention and an independently verified automatic-application path. This test
does not authorize production rollout or enable writes.
