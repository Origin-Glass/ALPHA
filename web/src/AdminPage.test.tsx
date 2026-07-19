import { cleanup, render, screen, waitFor } from "@testing-library/react";
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
  providers: { unhealthy: 1, unverified: 0, disabled: 1, action_items: [{ resource_id: "00000000-0000-7000-8000-000000000001", state: "disabled", reason: "provider_disabled", age_seconds: 3900 }] },
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
  expect(screen.getByRole("link", { name: "게시 권리 확인" })).toHaveAttribute("href", "/governance");
  expect(screen.getByRole("link", { name: "프로젝트 확인" })).toHaveAttribute("href", "/projects/workspace");
  expect(screen.getByRole("link", { name: "작업공간 확인" })).toHaveAttribute("href", "/workspace");
  expect(screen.getByText("생성 실패")).toBeInTheDocument();
  expect(screen.getByLabelText("AI 제공자 조치 1: 제공자 비활성화, 1시간 5분 경과")).toBeInTheDocument();
  expect(screen.queryByText("00000000-0000-7000-8000-000000000001")).not.toBeInTheDocument();
  await waitFor(() => expect(fetchMock).toHaveBeenCalledWith("/api/v1/admin/operations", { credentials: "include" }));
});

test("운영 현황 로딩 실패와 비어 있는 대기열을 구분해 알린다", async () => {
  let fail = true;
  const fetchMock = vi.fn().mockImplementation((path: string) => {
    if (path === "/api/v1/auth/me") return response({ handle: "admin", roles: ["ADMIN"] });
    if (path === "/api/v1/admin/operations") return fail ? response({ error: { message: "운영 진단을 불러오지 못했습니다." } }, 500) : response({
      providers: { unhealthy: 0, unverified: 0, disabled: 0, action_items: [] },
      generation_jobs: { queued: 0, running: 0, expired_leases: 0, failed: 0, blocked: 0, action_items: [] },
      reviews: { ai_pending: 0, human_pending: 0, rights_pending: 0, pilot_pending: 0, removal_pending: 0, action_items: [] },
      rights: { publication_blockers: 0, action_items: [] },
      learning: { active_projects: 0, stalled_projects: 0, action_items: [] },
      workspaces: { queued: 0, running: 0, expired_leases: 0, failed: 0, action_items: [] },
    });
    if (path === "/api/v1/admin/community/reports") return response({ items: [] });
    if (path === "/api/v1/admin/judge/workers") return response({ items: [] });
    if (path === "/api/v1/admin/audit?limit=30") return response({ items: [] });
    throw new Error(`unexpected ${path}`);
  });
  vi.stubGlobal("fetch", fetchMock);

  render(<AdminPage />);

  expect(await screen.findByRole("alert")).toHaveTextContent("운영 진단을 불러오지 못했습니다.");
  fail = false;
  screen.getByRole("button", { name: "운영 현황 다시 불러오기" }).click();
  await waitFor(() => expect(screen.getAllByText(/조치할 운영 항목이 없습니다/)).toHaveLength(6));
});
