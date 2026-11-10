import { useEffect, useId, useRef, useState, useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { useTranslation } from "react-i18next";
import { apiGet, apiPost } from "../../lib/api";
import { getAuthSessionId, subscribeAuthState } from "../../lib/auth";
import { formatUnixSeconds } from "../../lib/dateFormat";

type Settings = Partial<Record<"approve_mode" | "verification_method", string>>;
export interface AgentStatus {
  id: number;
  peer_row_id: number;
  agent_uuid: string | null;
  revoked: boolean;
  desired_revision: string;
  desired_settings: Settings | null;
  observation_sequence: string;
  last_seen: number;
  report: {
    desired_revision: number | null;
    rustdesk_id: string | null;
    rustdesk_version: string | null;
    applied_revision: null;
    results: Settings;
  } | null;
}
type Action = "create" | "bind" | "rotate" | "revoke";
const uuidV4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const inputClass = "w-full rounded-lg border border-kumo-line bg-kumo-base px-3 py-2 text-sm text-kumo-default focus-visible:outline focus-visible:outline-2 focus-visible:outline-kumo-brand";

export function AgentPanel({ peerRowId }: { peerRowId: number }) {
  const session = useSyncExternalStore(subscribeAuthState, getAuthSessionId);
  if (!session || !Number.isSafeInteger(peerRowId) || peerRowId <= 0) return null;
  return <Panel key={`${session}:${peerRowId}`} peerRowId={peerRowId} session={session} />;
}

function Panel({ peerRowId, session }: { peerRowId: number; session: string }) {
  const { t } = useTranslation();
  const id = useId();
  const credential = useRef<HTMLInputElement>(null);
  const mounted = useRef(false);
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [hasCredential, setHasCredential] = useState(false);
  const [copied, setCopied] = useState(false);
  const [uuid, setUuid] = useState("");
  const [error, setError] = useState("");
  const [confirm, setConfirm] = useState<"rotate" | "revoke" | null>(null);
  const status = useQuery({
    queryKey: ["companion-agent", session, peerRowId],
    queryFn: () => apiGet<AgentStatus | null>(`/api/admin/agent/peer/${peerRowId}`),
  });
  const agent = status.data;
  const disabled = busy || hasCredential || status.isFetching || status.isError;

  useEffect(() => {
    mounted.current = true;
    const field = credential.current;
    return () => { mounted.current = false; if (field) field.value = ""; };
  }, []);

  function hideCredential() {
    if (credential.current) credential.current.value = "";
    setHasCredential(false);
    setCopied(false);
  }

  async function perform(action: Action) {
    if (pending.current || disabled) return;
    if (action === "bind" && !uuidV4.test(uuid.trim())) {
      setError("agentUuidInvalid");
      return;
    }
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      // Keep one-time credentials out of React Query's mutation and query caches.
      const result = action === "create"
        ? await apiPost<{ credential: string }>("/api/admin/agent/create", { peer_row_id: peerRowId })
        : action === "bind"
          ? await apiPost(`/api/admin/agent/${agent!.id}/bind`, { agent_uuid: uuid.trim() })
          : await apiPost<{ credential?: string }>(`/api/admin/agent/${agent!.id}/${action}`);
      if (!mounted.current || getAuthSessionId() !== session) return;
      if ((action === "create" || action === "rotate") && credential.current) {
        const value = (result as { credential: string }).credential;
        if (typeof value !== "string" || !value) throw new Error("Missing credential");
        credential.current.value = value;
        setHasCredential(true);
      }
      setUuid("");
    } catch {
      if (mounted.current) setError("agentActionFailed");
    } finally {
      if (mounted.current && getAuthSessionId() === session) {
        setConfirm(null);
        // A failed response may still have committed. Refresh, never retry the POST.
        await status.refetch();
        if (mounted.current) setBusy(false);
      }
      pending.current = false;
    }
  }

  return (
    <section className="mt-5 grid gap-3 border-t border-kumo-line pt-5" aria-labelledby={`${id}-title`}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 id={`${id}-title`} className="text-sm font-semibold">{t("agentTitle")}</h3>
        <Button size="sm" variant="secondary" disabled={busy || status.isFetching} onClick={() => void status.refetch()}>{t("refresh")}</Button>
      </div>
      <p className="text-xs leading-5 text-kumo-subtle">{t("agentReadOnly")}</p>
      {status.isPending && <p role="status">{t("loading")}</p>}
      {(status.isError || error) && <p role="alert" className="text-sm text-kumo-danger">{t(status.isError ? "agentLoadFailed" : error)}</p>}
      <div hidden={!hasCredential} className="rounded-lg border border-kumo-line bg-kumo-base p-3">
        <label htmlFor={`${id}-credential`} className="text-sm font-medium">{t("agentCredential")}</label>
        <p className="my-2 text-xs leading-5 text-kumo-subtle">{t("agentCredentialHelp")}</p>
        <input ref={credential} id={`${id}-credential`} className={inputClass} readOnly autoComplete="off" spellCheck={false} />
        <div className="mt-2 flex flex-wrap gap-2">
          <Button variant="secondary" onClick={async () => {
            try { await navigator.clipboard.writeText(credential.current?.value || ""); if (mounted.current) setCopied(true); }
            catch { if (mounted.current) setError("agentCopyFailed"); }
          }}>{t(copied ? "copied" : "copy")}</Button>
          <Button variant="secondary" onClick={hideCredential}>{t("agentHide")}</Button>
        </div>
      </div>
      {status.isSuccess && (
        <>
          <p className="text-sm font-medium" role="status">{t(!agent ? "agentUnregistered" : agent.revoked ? "agentRevoked" : agent.agent_uuid ? "agentBound" : "agentAwaiting")}</p>
          {!agent?.agent_uuid && <p className="text-xs leading-5 text-kumo-subtle">{t("agentSetup")}</p>}
          {!agent ? (
            <>
              <Button className="justify-self-start" disabled={disabled} onClick={() => void perform("create")}>{t("agentEnroll")}</Button>
            </>
          ) : (
            <>
              <dl className="grid gap-3 text-sm sm:grid-cols-2">
                <div className="min-w-0 sm:col-span-2"><dt className="text-xs text-kumo-subtle">{t("agentUuid")}</dt><dd className="break-all font-mono">{agent.agent_uuid || "—"}</dd></div>
                <div><dt className="text-xs text-kumo-subtle">{t("agentRevision")}</dt><dd className="break-all font-mono">{agent.desired_revision}</dd></div>
                <div><dt className="text-xs text-kumo-subtle">{t("agentSequence")}</dt><dd className="break-all font-mono">{agent.observation_sequence}</dd></div>
                <div className="sm:col-span-2"><dt className="text-xs text-kumo-subtle">{t("agentContact")}</dt><dd>{agent.last_seen > 0 ? formatUnixSeconds(agent.last_seen) : t("agentNever")}</dd></div>
              </dl>
              <p className="text-xs text-kumo-subtle">{t("agentContactHelp")}</p>
              {!agent.agent_uuid && !agent.revoked && (
                <form className="grid gap-2" onSubmit={event => { event.preventDefault(); void perform("bind"); }}>
                  <label htmlFor={`${id}-uuid`} className="text-sm font-medium">{t("agentUuid")}</label>
                  <input id={`${id}-uuid`} className={inputClass} value={uuid} onChange={event => setUuid(event.target.value)} maxLength={36} autoComplete="off" aria-describedby={`${id}-uuid-help`} />
                  <p id={`${id}-uuid-help`} className="text-xs text-kumo-subtle">{t("agentUuidHelp")}</p>
                  <Button type="submit" className="justify-self-start" disabled={disabled}>{t("agentBind")}</Button>
                </form>
              )}
              <div className="rounded-lg border border-kumo-line bg-kumo-base p-3 text-sm">
                <h4 className="font-medium">{t("agentReport")}</h4>
                <p className="mt-1 text-kumo-subtle">{t(!agent.report ? "agentNoReport" : agent.report.desired_revision === null ? "agentHeartbeat" : "agentNoApplication")}</p>
                {agent.report && <p className="mt-2 break-words">RustDesk: {agent.report.rustdesk_id || "—"} · {agent.report.rustdesk_version || "—"}</p>}
                {(["approve_mode", "verification_method"] as const).map(key => (
                  <div key={key} className="mt-3 break-words text-xs">
                    <p className="font-medium">{t(key === "approve_mode" ? "agentApproval" : "agentVerification")}</p>
                    <p>{t("agentDesired")}: {agent.desired_settings?.[key] || t("agentUnmanaged")}</p>
                    <p>{t("agentResult")}: <span>{agent.report?.results[key] || "—"}</span></p>
                  </div>
                ))}
              </div>
              <div className="flex flex-wrap gap-2">
                <Button variant="secondary" disabled={disabled || !!confirm} onClick={() => setConfirm("rotate")}>{t("agentRotate")}</Button>
                {!agent.revoked && <Button variant="destructive" disabled={disabled || !!confirm} onClick={() => setConfirm("revoke")}>{t("agentRevoke")}</Button>}
              </div>
              {confirm && <div className="grid gap-3 rounded-lg border border-kumo-line p-3">
                <p className="text-sm" role="alert">{t(confirm === "rotate" ? "agentRotateWarning" : "agentRevokeWarning")}</p>
                <div className="flex flex-wrap gap-2">
                  <Button variant="destructive" disabled={disabled} onClick={() => void perform(confirm)}>{t(confirm === "rotate" ? "agentConfirmRotate" : "agentConfirmRevoke")}</Button>
                  <Button variant="secondary" disabled={busy} onClick={() => setConfirm(null)}>{t("cancel")}</Button>
                </div>
              </div>}
            </>
          )}
        </>
      )}
    </section>
  );
}
