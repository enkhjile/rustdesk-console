// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import i18n from "../../i18n";
import { apiGet, apiPost } from "../../lib/api";
import { getAuthSessionId, setToken } from "../../lib/auth";
import { PasswordPanel } from "./PasswordPanel";

vi.mock("../../lib/api", () => ({ apiGet: vi.fn(), apiPost: vi.fn() }));
let client: QueryClient;
beforeEach(async () => {
  vi.resetAllMocks(); localStorage.clear(); setToken("dummy-session");
  await i18n.changeLanguage("en");
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.mocked(apiGet).mockResolvedValue({ available: true, operation: null });
  vi.mocked(apiPost).mockResolvedValue({ status: "queued" });
});
afterEach(() => { cleanup(); client.clear(); });
async function mount(enabled = true) {
  const view = render(<QueryClientProvider client={client}><PasswordPanel agentId={8} session={getAuthSessionId()!} enabled={enabled} /></QueryClientProvider>);
  if (enabled) {
    fireEvent.click(screen.getByRole("button", { name: "Change RustDesk password" }));
    await waitFor(() => expect((screen.getByLabelText("New RustDesk password") as HTMLInputElement).disabled).toBe(false));
  }
  return view;
}
it("submits once, clears both fields, and keeps the password out of caches", async () => {
  await mount();
  const secret = "test-only-password";
  fireEvent.change(screen.getByLabelText("New RustDesk password"), { target: { value: secret } });
  fireEvent.change(screen.getByLabelText("Confirm RustDesk password"), { target: { value: secret } });
  fireEvent.click(screen.getByRole("button", { name: "Queue password change" }));
  await waitFor(() => expect(apiPost).toHaveBeenCalledExactlyOnceWith("/api/admin/agent/8/password", { password: secret }));
  expect((screen.getByLabelText("New RustDesk password") as HTMLInputElement).value).toBe("");
  expect((screen.getByLabelText("Confirm RustDesk password") as HTMLInputElement).value).toBe("");
  expect(JSON.stringify(client.getQueryCache().getAll().map(q => q.state.data))).not.toContain(secret);
  expect(client.getMutationCache().getAll()).toHaveLength(0);
  expect(JSON.stringify(localStorage)).not.toContain(secret);
});
it("rejects mismatches without a request and clears detached inputs on unmount", async () => {
  const view = await mount();
  const first = screen.getByLabelText("New RustDesk password") as HTMLInputElement;
  const second = screen.getByLabelText("Confirm RustDesk password") as HTMLInputElement;
  fireEvent.change(first, { target: { value: "test-only-password" } });
  fireEvent.change(second, { target: { value: "different-password" } });
  fireEvent.click(screen.getByRole("button", { name: "Queue password change" }));
  expect(apiPost).not.toHaveBeenCalled();
  expect(screen.getByRole("alert").textContent).toContain("matching passwords");
  view.unmount(); expect(first.value).toBe(""); expect(second.value).toBe("");
});
it("does not offer a change for observe-only or otherwise ineligible agents", async () => {
  await mount(false);
  expect((screen.getByRole("button", { name: "Change RustDesk password" }) as HTMLButtonElement).disabled).toBe(true);
  expect(apiGet).not.toHaveBeenCalled(); expect(apiPost).not.toHaveBeenCalled();
});
it("blocks another change while an operation is claimed", async () => {
  vi.mocked(apiGet).mockResolvedValue({ available: true, operation: { status: "claimed", sequence: "1" } });
  render(<QueryClientProvider client={client}><PasswordPanel agentId={8} session={getAuthSessionId()!} enabled /></QueryClientProvider>);
  fireEvent.click(screen.getByRole("button", { name: "Change RustDesk password" }));
  await screen.findByText("Waiting for the agent to resolve this password change");
  expect((screen.getByRole("button", { name: "Queue password change" }) as HTMLButtonElement).disabled).toBe(true);
});
