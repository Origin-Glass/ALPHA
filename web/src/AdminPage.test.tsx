import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import AdminPage from "./AdminPage";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const response = (payload: unknown, status = 200) => Promise.resolve({
  ok: status >= 200 && status < 300,
  status,
  json: async () => payload,
});

const operations = {
  providers: { unhealthy: 1, unverified: 0, disabled: 1, action_items: [{ resource_id: "00000000-0000-7000-8000-000000000001", state: "new_raw_state", reason: "provider_disabled", age_seconds: 3900 }] },
  generation_jobs: { queued: 2, running: 1, expired_leases: 1, failed: 1, blocked: 1, action_items: [{ resource_id: "00000000-0000-7000-8000-000000000002", state: "failed", reason: "generation_failed", age_seconds: 120 }] },
  reviews: { ai_pending: 1, human_pending: 1, rights_pending: 1, pilot_pending: 0, removal_pending: 0, action_items: [{ resource_id: "00000000-0000-7000-8000-000000000003", state: "rights_review_pending", reason: "rights_review_pending", age_seconds: 61 }] },
  rights: { publication_blockers: 1, action_items: [{ resource_id: "00000000-0000-7000-8000-000000000004", state: "blocked", reason: "commercial_use_denied", age_seconds: 3600 }] },
  learning: { active_projects: 4, stalled_projects: 1, action_items: [{ resource_id: "00000000-0000-7000-8000-000000000005", state: "active", reason: "no_learning_event_7d", age_seconds: 7200 }] },
  workspaces: { queued: 1, running: 1, expired_leases: 0, failed: 1, action_items: [{ resource_id: "00000000-0000-7000-8000-000000000006", state: "failed", reason: "workspace_failed", age_seconds: 10 }] },
};

test("관리자가 비밀과 식별자 없이 운영 대기열을 판단하고 담당 화면으로 이동한다", async () => {
  const fetchMock = vi.fn().mockImplementation((path: string) => {
    if (path === "/api/v1/auth/me") return response({ handle: "admin", roles: ["ADMIN"] });
    if (path === "/api/v1/admin/operations") return response(operations);
    if (path === "/api/v1/admin/community/reports") return response({ items: [] });
    if (path === "/api/v1/admin/judge/workers") return response({ items: [] });
    if (path === "/api/v1/admin/audit?limit=30") return response({ items: [] });
    throw new Error(`unexpected ${path}`);
  });
  vi.stubGlobal("fetch", fetchMock);

  render(<AdminPage />);

  await screen.findByRole("heading", { name: "운영 현황" });
  expect(screen.getByRole("link", { name: "제공자 관리" })).toHaveAttribute("href", "/provider-controls");
  expect(screen.getByRole("link", { name: "생성 작업 확인" })).toHaveAttribute("href", "/content-studio");
  expect(screen.getByRole("link", { name: "검토 대기열 확인" })).toHaveAttribute("href", "/content-reviews");
  expect(screen.getByRole("link", { name: "게시 권리 확인" })).toHaveAttribute("href", "/content-reviews");
  expect(screen.getByRole("link", { name: "프로젝트 확인" })).toHaveAttribute("href", "/projects/ideas");
  expect(screen.getByRole("link", { name: "작업공간 확인" })).toHaveAttribute("href", "/workspace");
  expect(screen.getByText("생성 실패")).toBeInTheDocument();
  expect(screen.getByLabelText("AI 제공자 조치 1: 제공자 비활성화, 1시간 5분 경과")).toBeInTheDocument();
  expect(screen.getByText(/상태 확인 필요 · 1시간 5분 경과/)).toBeInTheDocument();
  expect(screen.queryByText(/new_raw_state/)).not.toBeInTheDocument();
  expect(document.getElementById("operations-learning")).toHaveTextContent("학습 프로젝트");
  expect([...document.querySelectorAll("[id]")].some((element) => /\s/.test(element.id))).toBe(false);
  expect(screen.queryByText("00000000-0000-7000-8000-000000000001")).not.toBeInTheDocument();
  await waitFor(() => expect(fetchMock).toHaveBeenCalledWith("/api/v1/admin/operations", { credentials: "include" }));
});

test("다른 관리자 API 실패와 운영 현황 실패를 격리하고 실패한 새로고침에서 이전 값을 지운다", async () => {
  let operationsFail = false;
  const fetchMock = vi.fn().mockImplementation((path: string) => {
    if (path === "/api/v1/auth/me") return response({ handle: "admin", roles: ["ADMIN"] });
    if (path === "/api/v1/admin/operations") return operationsFail
      ? response({ error: { message: "운영 진단을 불러오지 못했습니다." } }, 500)
      : response(operations);
    if (path === "/api/v1/admin/community/reports") return response({ error: { message: "신고 API 실패" } }, 500);
    if (path === "/api/v1/admin/judge/workers") return response({ items: [] });
    if (path === "/api/v1/admin/audit?limit=30") return response({ error: { message: "감사 API 실패" } }, 500);
    throw new Error(`unexpected ${path}`);
  });
  vi.stubGlobal("fetch", fetchMock);

  render(<AdminPage />);

  const panel = (await screen.findByRole("heading", { name: "운영 현황" })).closest("section")!;
  expect(await within(panel).findByText("생성 실패")).toBeInTheDocument();
  expect(within(panel).queryByRole("alert")).not.toBeInTheDocument();

  operationsFail = true;
  fireEvent.click(within(panel).getByRole("button", { name: "운영 현황 다시 불러오기" }));
  expect(within(panel).queryByText("생성 실패")).not.toBeInTheDocument();
  expect(await within(panel).findByRole("alert")).toHaveTextContent("운영 진단을 불러오지 못했습니다.");
});
