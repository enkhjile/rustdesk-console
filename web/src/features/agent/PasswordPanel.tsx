import { useEffect, useId, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { useTranslation } from "react-i18next";
import { apiGet, apiPost } from "../../lib/api";
import { getAuthSessionId } from "../../lib/auth";

interface Status { available: boolean; operation: { status: string; sequence: string } | null }
const labels: Record<string, string> = {
  queued: "passwordQueued", claimed: "passwordClaimed", expired: "passwordExpired",
  command_accepted: "passwordAccepted", indeterminate: "agentIndeterminate", unsupported: "agentUnsupported",
};
const inputClass = "w-full rounded-lg border border-kumo-line bg-kumo-base px-3 py-2 text-sm text-kumo-default focus-visible:outline focus-visible:outline-2 focus-visible:outline-kumo-brand";

export function PasswordPanel({ agentId, session, enabled }: { agentId: number; session: string; enabled: boolean }) {
  const { t } = useTranslation();
  const id = useId();
  const password = useRef<HTMLInputElement>(null);
  const confirmation = useRef<HTMLInputElement>(null);
  const mounted = useRef(false);
  const submitting = useRef(false);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [feedback, setFeedback] = useState("");
  const status = useQuery({
    queryKey: ["agent-password-operation", session, agentId],
    queryFn: () => apiGet<Status>(`/api/admin/agent/${agentId}/password`),
    enabled: open && enabled, refetchInterval: open && enabled ? 5000 : false,
  });
  const pending = ["queued", "claimed"].includes(status.data?.operation?.status || "");
  const disabled = !enabled || busy || status.isPending || status.isError || !status.data?.available || pending;
  function clear() { if (password.current) password.current.value = ""; if (confirmation.current) confirmation.current.value = ""; }
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; clear(); };
  }, []);
  useEffect(() => { if (!enabled) { clear(); setOpen(false); } }, [enabled]);
  useEffect(() => {
    const first = password.current; const second = confirmation.current;
    return () => { if (first) first.value = ""; if (second) second.value = ""; };
  }, [open]);
  async function submit() {
    if (disabled || submitting.current || getAuthSessionId() !== session) return;
    const value = password.current?.value || "";
    if (!/^[\x20-\x7e]{8,128}$/.test(value) || value !== confirmation.current?.value) {
      setError("passwordInvalid"); return;
    }
    submitting.current = true; setBusy(true); setError(""); setFeedback("passwordSending"); clear();
    try {
      // No mutation cache, retries, password retrieval or browser persistence.
      await apiPost(`/api/admin/agent/${agentId}/password`, { password: value });
      if (mounted.current && getAuthSessionId() === session) setFeedback("passwordSubmitted");
    } catch { if (mounted.current) { setFeedback(""); setError("passwordRequestFailed"); } }
    finally {
      if (mounted.current && getAuthSessionId() === session) {
        await status.refetch();
        if (mounted.current) setBusy(false);
      }
      submitting.current = false;
    }
  }
  return <div className="grid gap-3">
    <Button variant="secondary" disabled={!enabled || busy} onClick={() => { clear(); setFeedback(""); setOpen(!open); }}>{t("passwordAction")}</Button>
    {open && <form className="grid gap-3 rounded-lg border border-kumo-line p-3" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <p className="text-xs leading-5 text-kumo-subtle">{t("passwordHelp")}</p>
      {feedback && <p role="status" className="rounded-lg border border-kumo-line bg-kumo-base p-3 text-sm">{t(feedback)}</p>}
      {status.data && !status.data.available && <p role="status">{t("passwordUnavailable")}</p>}
      {status.data?.operation && <p role="status">{t(labels[status.data.operation.status] || "agentNoResult")}</p>}
      {(error || status.isError) && <p role="alert">{t(error || "agentLoadFailed")}</p>}
      <label htmlFor={`${id}-password`}>{t("passwordNew")}</label>
      <input id={`${id}-password`} ref={password} type="password" autoComplete="off" minLength={8} maxLength={128} required disabled={disabled} className={inputClass} />
      <label htmlFor={`${id}-confirmation`}>{t("passwordConfirm")}</label>
      <input id={`${id}-confirmation`} ref={confirmation} type="password" autoComplete="off" minLength={8} maxLength={128} required disabled={disabled} className={inputClass} />
      <Button type="submit" disabled={disabled}>{t("passwordSubmit")}</Button>
    </form>}
  </div>;
}
