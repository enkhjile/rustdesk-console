# Real Windows agent canary

The actual Windows service completed enrollment, HTTPS polling and report
delivery against this Console implementation. This was not the agent's
in-process contract fixture. Automatic application stayed disabled.

## Tested configuration

- Console source: `4b831f8`, based on upstream main `52c64bf`, SQLite database.
- Agent source: `b4050ef`, with the same tree as merged `alpha` commit `dc1fea9`.
  Built with `cargo build --locked --target x86_64-pc-windows-msvc`, without
  `lab-apply`. Console used `cargo +1.99.1 build --locked -p rustdesk-console`.
- Disposable `RustDeskAgent-Validation` VM: Windows 11 Pro, `10.0.26200`, x64.
  Agent service account: `NT AUTHORITY\LocalService`, with its service SID.
  Administrator enrollment and initialization ran as the guest's `LabAdmin`.
- Official RustDesk 1.4.7. The executables used the lab's app-local Visual C++
  runtime DLLs. No custom RustDesk client was built or installed.
- No VM network adapters. Console listened on `127.0.0.1:21124`; a local TLS
  proxy exposed only the two agent routes at `https://localhost:8444` using the
  guest's trusted lab certificate. Normal agent certificate/hostname validation
  remained enabled. All accounts and credentials were disposable lab data.

Executable SHA-256:

| Executable | Hash |
| --- | --- |
| Agent | `F52D9EACC2D181ACC1EA2A1B0FC005B4F87BFC818FEE492E195F7254CBC8D4D9` |
| Console | `FDDAAF39832CB6F93E8E780B565B542018789B85892CD543876F4EBF778C57BB` |

## Behaviors and results

| Check | Result |
| --- | --- |
| Enrollment | Console's administrator APIs created a device and issued a unique credential. `--enroll https://localhost:8444` received it through redirected stdin; the returned UUID was bound through the admin API. Administrator `--init-state` succeeded. |
| No assignment | Real service GET returned 204 without accepting desired settings. |
| First assignment | Existing strategy APIs assigned `click` and `use-temporary-password`. Console allocated revision 1; the service reported sequence 1, both results `unsupported`, and null applied revision. |
| Lost response and retry | The proxy forwarded the first report to Console, observed its 204, then closed TLS without delivering that response. The agent persisted the pending report. After service restart, the exact same report body was accepted again with 204 and the same sequence. |
| Higher revision | Updating approval to `password-click` allocated revision 2. A pending revision-1 report received 409; the service fetched fresh state and Console accepted revision 2 at sequence 3. |
| Console restart | With the agent stopped, restarting the Console process retained its desired snapshot, report, revision and sequence exactly. |
| Agent restart | Restarting the service retained accepted revision 2 and delivered sequence 4, greater than the previous accepted sequence. |
| Revocation | The admin revoke API blocked both the service's POST and GET with 401. The service retained watermark 2 and its pending sequence-5 report; Console's last accepted report stayed at sequence 4. |
| Read-only behavior | Every accepted report remained `unsupported` with null applied revision. Before/after administrator CLI observations were identical; both setting getters reported `ambiguous_default`. The service detector also reported those ambiguous values, so this is not a claim of independent live verification. |
| Cleanup | The pre-test checkpoint was restored, including the original enrollment, durable state, service configuration and RustDesk settings. The VM was verified Off, with zero network adapters and zero canary checkpoints. |

The service command was:

```powershell
C:\Lab\ConsoleE2E\rustdesk-agent.exe --service "C:\Program Files\RustDesk\rustdesk.exe" C:\ProgramData\RustDeskAgent\console-e2e.json
```

Lifecycle checks used `Start-Service RustDeskAgent` and `Stop-Service
RustDeskAgent`; Console restarted against the same SQLite database. Provisioning
and strategy changes used the APIs described in the [integration guide](companion-agent.md),
not direct database modifications. No RustDesk setter, TOML edit or permanent
password operation was used. A freshly observed report can replace an acknowledged
pending report during a polling cycle; delivery is checked by report identity and
sequence, not by requiring the pending queue to remain empty.

## Remaining gates

This validates one Windows service against one Windows-hosted SQLite Console over
loopback HTTPS. It does not validate a real network/reverse-proxy deployment,
guest reboot with both components configured for unattended startup,
MySQL/PostgreSQL, credential rotation/recovery, enrollment UI, or production
automatic application. Existing fixture-based reboot tests do not close those
Console-specific gates. Production writes remain disabled.
