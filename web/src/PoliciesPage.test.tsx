import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import PoliciesPage from "./PoliciesPage";

afterEach(() => vi.unstubAllGlobals());

test("필수 정책은 사용자가 명시적으로 확인한 버전만 동의 요청한다", async () => {
  const fetchMock = vi.fn()
    .mockResolvedValueOnce({
      ok: true,
      json: async () => ({ items: [{ version: "2026-07-18", title: "이용약관 및 개인정보 처리방침", required: true, consented: false }] }),
    })
    .mockResolvedValueOnce({ ok: true, json: async () => ({ consented: true }) });
  vi.stubGlobal("fetch", fetchMock);

  render(<PoliciesPage />);

  const consent = await screen.findByRole("button", { name: "필수 정책에 동의" });
  expect(consent).toBeDisabled();
  fireEvent.click(screen.getByRole("checkbox", { name: "2026-07-18 정책을 확인했습니다" }));
  fireEvent.click(consent);

  await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
  expect(fetchMock.mock.calls[1][0]).toBe("/api/v1/policies/consents");
  expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toEqual({ version: "2026-07-18" });
});
