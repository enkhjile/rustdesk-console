// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import i18n from "../../i18n";
import { apiGet, apiPost } from "../../lib/api";
import { clearToken, setToken } from "../../lib/auth";
import { AgentPanel, type AgentStatus } from "./AgentPanel";

vi.mock("../../lib/api", () => ({ apiGet: vi.fn(), apiPost: vi.fn() }));
const uuid = "a49f96c4-968b-4e7a-a3be-a5452385584d";
const secret = "rda_dummy_device_token_never_cache";
const enrolled: AgentStatus = {
  id: 8, peer_row_id: 42, agent_uuid: null, revoked: false,
  desired_revision: "0", desired_settings: null, observation_sequence: "0",
  report: null, last_seen: 0,
};
let client: QueryClient;
beforeEach(async () => {
  vi.resetAllMocks();
  localStorage.clear();
  setToken("dummy-admin-session");
  await i18n.changeLanguage("en");
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.mocked(apiGet).mockResolvedValue(null);
});
afterEach(() => { cleanup(); client.clear(); });
function mount(peerRowId = 42) {
  return render(<QueryClientProvider client={client}><AgentPanel peerRowId={peerRowId} /></QueryClientProvider>);
}
async function issue() {
  vi.mocked(apiPost).mockResolvedValue({ id: 8, credential: secret });
  mount();
  fireEvent.click(await screen.findByRole("button", { name: "Enroll agent" }));
  await screen.findByDisplayValue(secret);
}

it("shows a no-policy heartbeat distinctly from no report and application results", async () => {
  vi.mocked(apiGet).mockResolvedValue({ ...enrolled, agent_uuid: uuid, last_seen: 1700000000,
    observation_sequence: "1", report: { desired_revision: null, rustdesk_id: "123", rustdesk_version: "1.4.7", applied_revision: null, results: {} },
  });
  mount();
  expect(await screen.findByText("Heartbeat received — no policy assigned")).toBeDefined();
  expect(screen.queryByText("No report received")).toBeNull();
  expect(screen.queryByText("Online")).toBeNull();
  expect(screen.queryByText("No verified application")).toBeNull();
});

it("enrolls by Console row ID and retains the returned credential only until dismissed", async () => {
  await issue();
  expect(apiPost).toHaveBeenCalledWith("/api/admin/agent/create", { peer_row_id: 42 });
  expect(JSON.stringify(client.getQueryCache().getAll().map(q => q.state.data))).not.toContain(secret);
  expect(client.getMutationCache().getAll()).toHaveLength(0);
  expect(JSON.stringify(localStorage)).not.toContain(secret);
  expect(JSON.stringify(sessionStorage)).not.toContain(secret);
  fireEvent.click(screen.getByRole("button", { name: "Hide credential" }));
  expect(screen.queryByDisplayValue(secret)).toBeNull();
});

it("validates and binds the enrollment UUID, never the RustDesk device UUID", async () => {
  vi.mocked(apiGet).mockResolvedValue(enrolled);
  mount();
  const input = await screen.findByLabelText("Agent UUID");
  fireEvent.change(input, { target: { value: "wrong-uuid" } });
  fireEvent.submit(input.closest("form")!);
  expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Enter the UUID v4 printed by agent enrollment.");
  expect(apiPost).not.toHaveBeenCalled();
  fireEvent.change(input, { target: { value: uuid } });
  fireEvent.submit(input.closest("form")!);
  await waitFor(() => expect(apiPost).toHaveBeenCalledWith("/api/admin/agent/8/bind", { agent_uuid: uuid }));
});

it("requires confirmation for rotation and displays the replacement once", async () => {
  vi.mocked(apiGet).mockResolvedValue({ ...enrolled, agent_uuid: uuid });
  vi.mocked(apiPost).mockResolvedValue({ id: 8, credential: secret });
  mount();
  fireEvent.click(await screen.findByRole("button", { name: "Rotate credential" }));
  expect(apiPost).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(apiPost).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Rotate credential" }));
  fireEvent.click(screen.getByRole("button", { name: "Confirm rotation" }));
  await screen.findByDisplayValue(secret);
  expect(apiPost).toHaveBeenCalledWith("/api/admin/agent/8/rotate");
});

it("revokes only after confirmation and shows revoked enrollment", async () => {
  vi.mocked(apiGet).mockResolvedValue({ ...enrolled, agent_uuid: uuid });
  mount();
  fireEvent.click(await screen.findByRole("button", { name: "Revoke access" }));
  expect(apiPost).not.toHaveBeenCalled();
  vi.mocked(apiGet).mockResolvedValue({ ...enrolled, agent_uuid: uuid, revoked: true });
  fireEvent.click(screen.getByRole("button", { name: "Confirm revocation" }));
  await screen.findByText("Revoked");
  expect(apiPost).toHaveBeenCalledWith("/api/admin/agent/8/revoke");
  expect(screen.queryByRole("button", { name: "Revoke access" })).toBeNull();
  expect(screen.getByRole("button", { name: "Rotate credential" })).toBeDefined();
});

it("shows status and exact counters without claiming settings were applied or the agent is online", async () => {
  vi.mocked(apiGet).mockResolvedValue({ ...enrolled, agent_uuid: uuid, last_seen: 1700000000,
    desired_revision: "18446744073709551615", observation_sequence: "9007199254740993",
    desired_settings: { approve_mode: "click" },
    report: { rustdesk_id: "123", rustdesk_version: "1.4.7", results: { approve_mode: "unsupported" }, applied_revision: null },
  });
  mount();
  await screen.findByText("18446744073709551615");
  expect(screen.getByText("9007199254740993")).toBeDefined();
  expect(screen.getByText("unsupported")).toBeDefined();
  expect(screen.getByText("No verified application")).toBeDefined();
  expect(screen.queryByText("Online")).toBeNull();
});

it("blocks actions on load failure and offers retry instead of assuming unenrolled", async () => {
  vi.mocked(apiGet).mockRejectedValue(new Error("server failed"));
  mount();
  await screen.findByRole("alert");
  expect(screen.queryByRole("button", { name: "Enroll agent" })).toBeNull();
  vi.mocked(apiGet).mockResolvedValue(null);
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByRole("button", { name: "Enroll agent" });
});

it("does not echo mutation errors or retry a possibly completed credential issuance", async () => {
  vi.mocked(apiPost).mockRejectedValue(new Error(secret));
  mount();
  fireEvent.click(await screen.findByRole("button", { name: "Enroll agent" }));
  await screen.findByRole("alert");
  expect(document.body.textContent).not.toContain(secret);
  expect(apiPost).toHaveBeenCalledTimes(1);
});

it("clears a revealed credential on device changes and logout", async () => {
  vi.mocked(apiPost).mockResolvedValue({ id: 8, credential: secret });
  const view = mount();
  fireEvent.click(await screen.findByRole("button", { name: "Enroll agent" }));
  await screen.findByDisplayValue(secret);
  view.rerender(<QueryClientProvider client={client}><AgentPanel peerRowId={43} /></QueryClientProvider>);
  expect(screen.queryByDisplayValue(secret)).toBeNull();
  await screen.findByRole("button", { name: "Enroll agent" });
  fireEvent.click(screen.getByRole("button", { name: "Enroll agent" }));
  await screen.findByDisplayValue(secret);
  act(() => clearToken());
  expect(screen.queryByDisplayValue(secret)).toBeNull();
});

it("blocks repeated issuance while pending or while the credential is visible", async () => {
  let finish!: (value: unknown) => void;
  vi.mocked(apiPost).mockReturnValue(new Promise(resolve => { finish = resolve; }));
  mount();
  const enroll = await screen.findByRole("button", { name: "Enroll agent" });
  fireEvent.click(enroll);
  fireEvent.click(enroll);
  expect(apiPost).toHaveBeenCalledTimes(1);
  await act(async () => finish({ id: 8, credential: secret }));
  await screen.findByDisplayValue(secret);
  fireEvent.click(enroll);
  expect(apiPost).toHaveBeenCalledTimes(1);
});

it("does not act on stale status after refresh fails", async () => {
  vi.mocked(apiGet).mockResolvedValue(enrolled);
  mount();
  await screen.findByRole("button", { name: "Rotate credential" });
  vi.mocked(apiGet).mockRejectedValue(new Error("unavailable"));
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  await screen.findByRole("alert");
  expect(screen.queryByRole("button", { name: "Rotate credential" })).toBeNull();
  expect(apiPost).not.toHaveBeenCalled();
});

it("does not load an invalid device or an unauthenticated session", async () => {
  mount(0);
  await act(async () => {});
  expect(apiGet).not.toHaveBeenCalled();
  cleanup();
  clearToken();
  mount();
  await act(async () => {});
  expect(apiGet).not.toHaveBeenCalled();
});

it("discards a late credential response after the panel closes", async () => {
  let finish!: (value: unknown) => void;
  vi.mocked(apiPost).mockReturnValue(new Promise(resolve => { finish = resolve; }));
  const view = mount();
  fireEvent.click(await screen.findByRole("button", { name: "Enroll agent" }));
  view.unmount();
  await act(async () => finish({ id: 8, credential: secret }));
  mount();
  await screen.findByRole("button", { name: "Enroll agent" });
  expect(screen.queryByDisplayValue(secret)).toBeNull();
});
