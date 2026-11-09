// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "../i18n";
import { PeerInfoDrawer } from "./PeerInfoDrawer";

vi.mock("../lib/api", () => ({ apiGet: vi.fn().mockResolvedValue(null) }));
vi.mock("../features/agent/AgentPanel", () => ({
  AgentPanel: ({ peerRowId }: { peerRowId: number }) => <div>Agent for row {peerRowId}</div>,
}));
afterEach(cleanup);

it.each(["my", "admin"] as const)("mounts agent controls only in an open admin device drawer (%s)", scope => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const row = { row_id: 42, id: "rustdesk-id", uuid: "rustdesk-client-uuid" };
  const view = render(<QueryClientProvider client={client}><PeerInfoDrawer row={row} scope={scope} /></QueryClientProvider>);
  expect(screen.queryByText("Agent for row 42")).toBeNull();
  fireEvent.click(screen.getByRole("button"));
  expect(view.container.querySelector('[role="dialog"]')).toBeNull();
  expect(screen.getByRole("dialog")).toBeDefined();
  expect(!!screen.queryByText("Agent for row 42")).toBe(scope === "admin");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByText("Agent for row 42")).toBeNull();
  view.unmount();
  client.clear();
});
