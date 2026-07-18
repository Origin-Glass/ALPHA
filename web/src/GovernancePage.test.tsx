import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import GovernancePage from "./GovernancePage";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const viewerResponse = (roles: string[]) => ({
  status: 200,
  ok: true,
  json: async () => ({
    id: "00000000-0000-0000-0000-000000000001",
    handle: "governance-user",
    display_name: "거버넌스 사용자",
    terms_accepted: true,
    roles,
  }),
});

test("선택한 라이선스 권리 유형과 근거를 그대로 기록한다", async () => {
  const fetchMock = vi.fn()
    .mockResolvedValueOnce(viewerResponse(["CONTENT_CREATOR"]))
    .mockResolvedValueOnce({ status: 200, ok: true, json: async () => ({ status: "rights_recorded" }) });
  vi.stubGlobal("fetch", fetchMock);

  render(<GovernancePage />);

  await screen.findByRole("heading", { name: "권리 근거 기록" });
  fireEvent.change(screen.getByRole("textbox", { name: "문제 식별자" }), { target: { value: "licensed-problem" } });
  expect(screen.getAllByRole("option").map((option) => option.getAttribute("value"))).toEqual([
    "original", "licensed", "public_domain",
  ]);
  fireEvent.change(screen.getByRole("combobox", { name: "권리 유형" }), { target: { value: "licensed" } });
  fireEvent.change(screen.getByRole("textbox", { name: "출처·라이선스 근거" }), {
    target: { value: "저작권자 계약서 ALPHA-LIC-42, 상업 재배포 허용" },
  });
  fireEvent.click(screen.getByRole("checkbox", { name: "상업 이용 허용" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "재배포 허용" }));
  fireEvent.click(screen.getByRole("button", { name: "권리 근거 저장" }));

  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
  expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toEqual({
    basis: "licensed",
    evidence: "저작권자 계약서 ALPHA-LIC-42, 상업 재배포 허용",
    commercial_use_allowed: true,
    redistribution_allowed: true,
  });
});

test("PROBLEM_SETTER에게 게시 권한 버튼을 노출하지 않는다", async () => {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(viewerResponse(["PROBLEM_SETTER"])));

  render(<GovernancePage />);

  expect(await screen.findByRole("button", { name: "권리 근거 저장" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "분리 승인 확인 후 게시" })).not.toBeInTheDocument();
});
