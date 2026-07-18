import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import ContentStudioPage from "./ContentStudioPage";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

test("자격 증명이 없는 생성 요청을 성공으로 꾸미지 않고 차단 상태로 표시한다", async () => {
  const fetchMock = vi.fn().mockImplementation((path: string, options?: RequestInit) => {
    if (path === "/api/v1/content/providers") {
      return Promise.resolve({ ok: true, json: async () => ({ providers: [{ id: "provider-1", name: "frontier", kind: "external", protocol: "openai_compatible", model: "test-model", enabled: true, credential_available: false }] }) });
    }
    if (path === "/api/v1/content/jobs" && !options?.method) {
      return Promise.resolve({ ok: true, json: async () => ({ jobs: [] }) });
    }
    return Promise.resolve({ ok: true, status: 201, json: async () => ({ id: "job-1", status: "blocked_missing_credential" }) });
  });
  vi.stubGlobal("fetch", fetchMock);

  render(<ContentStudioPage />);
  await screen.findByRole("option", { name: /frontier/ });
  fireEvent.change(screen.getByRole("textbox", { name: "생성 주제" }), { target: { value: "이진 탐색 경계 오류를 설명하는 코드 읽기 문제" } });
  fireEvent.click(screen.getByRole("button", { name: "생성 작업 요청" }));

  await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("자격 증명 환경 변수가 없어 차단됨"));
  expect(screen.queryByText("생성이 완료됐습니다")).not.toBeInTheDocument();
});
