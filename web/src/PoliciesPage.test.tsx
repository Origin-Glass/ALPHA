import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import PoliciesPage from "./PoliciesPage";

afterEach(() => vi.unstubAllGlobals());

test("필수 정책은 사용자가 명시적으로 확인한 버전만 동의 요청한다", async () => {
  const fetchMock = vi.fn()
    .mockResolvedValueOnce({
      status: 200,
      ok: true,
      json: async () => ({ items: [{
        version: "2026-07-18",
        title: "이용약관 및 개인정보 처리방침",
        body: "이용약관\n학습 서비스를 제공합니다.\n\n개인정보 처리방침\n학습 기록을 보호합니다.",
        required: true,
        consented: false,
        consent: null,
      }] }),
    })
    .mockResolvedValueOnce({ status: 200, ok: true, json: async () => ({ consented: true }) });
  vi.stubGlobal("fetch", fetchMock);

  render(<PoliciesPage />);

  const consent = await screen.findByRole("button", { name: "필수 정책에 동의" });
  expect(screen.getByText(/학습 서비스를 제공합니다\./)).toBeInTheDocument();
  expect(consent).toBeDisabled();
  fireEvent.click(screen.getByRole("checkbox", { name: "이용약관에 동의합니다" }));
  expect(consent).toBeDisabled();
  fireEvent.click(screen.getByRole("checkbox", { name: "개인정보 처리방침에 동의합니다" }));
  fireEvent.click(consent);

  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
  expect(fetchMock.mock.calls[1][0]).toBe("/api/v1/policies/consents");
  expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toEqual({
    version: "2026-07-18",
    choices: { terms: true, privacy: true },
  });
});
