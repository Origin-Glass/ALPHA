import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import ContentStudioPage from "./ContentStudioPage";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const jsonResponse = (payload: unknown, status = 200) => Promise.resolve({
  ok: status >= 200 && status < 300,
  status,
  json: async () => payload,
});

const providerPayload = { providers: [{
  id: "provider-1", name: "frontier", kind: "external", protocol: "openai_compatible",
  model: "test-model", cost_per_generation_microunits: 1000, enabled: true,
  credential_available: true,
}] };

test("자격 증명이 없는 생성 요청을 성공으로 꾸미지 않고 차단 상태로 표시한다", async () => {
  const fetchMock = vi.fn().mockImplementation((path: string, options?: RequestInit) => {
    if (path === "/api/v1/content/providers") {
      return Promise.resolve({ ok: true, json: async () => ({ providers: [{ id: "provider-1", name: "frontier", kind: "external", protocol: "openai_compatible", model: "test-model", cost_per_generation_microunits: 1000, enabled: true, credential_available: false }] }) });
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

test("폴링으로 대기 작업의 완료를 반영하고 불변 산출물을 표시하며 언마운트 때 폴링을 정리한다", async () => {
  vi.useFakeTimers();
  let jobLoads = 0;
  const fetchMock = vi.fn().mockImplementation((path: string) => {
    if (path === "/api/v1/content/providers") return jsonResponse(providerPayload);
    if (path === "/api/v1/content/jobs") {
      jobLoads += 1;
      const completed = jobLoads > 1;
      return jsonResponse({ jobs: [{ id: "job-1", provider: "frontier", content_type: "code_reading", status: completed ? "completed" : "queued", attempts: completed ? 1 : 0, has_artifact: completed }] });
    }
    if (path === "/api/v1/content/jobs/job-1") return jsonResponse({
      job: { id: "job-1", provider: "frontier", content_type: "code_reading", status: "completed", attempts: 1, request_spec: {} },
      attempts: [{ attempt_number: 1, request_hash: "request-hash" }],
      artifacts: [{ id: "artifact-1", kind: "generated_content", hash: "abc123", payload: { candidates: [{ title: "경계값 분석" }] } }],
      audit: [{ action: "content.job.completed" }],
    });
    throw new Error(`unexpected fetch: ${path}`);
  });
  vi.stubGlobal("fetch", fetchMock);

  const view = render(<ContentStudioPage />);
  await act(async () => { await Promise.resolve(); await Promise.resolve(); });
  expect(screen.getByText("생성 대기 중")).toBeInTheDocument();

  await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
  expect(screen.getByText("생성 완료")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "상세 보기" }));
  await act(async () => { await Promise.resolve(); await Promise.resolve(); });
  expect(screen.getByText("SHA-256 abc123")).toBeInTheDocument();
  expect(screen.getByText(/경계값 분석/)).toBeInTheDocument();

  const callsBeforeUnmount = fetchMock.mock.calls.length;
  view.unmount();
  await act(async () => { await vi.advanceTimersByTimeAsync(9000); });
  expect(fetchMock).toHaveBeenCalledTimes(callsBeforeUnmount);
});

test("취소·복제·보관은 CSRF와 멱등성 키를 보내고 성공 결과로 작업 목록과 상세를 갱신한다", async () => {
  document.cookie = "alpha_csrf=csrf-test; path=/";
  const postPaths: string[] = [];
  const fetchMock = vi.fn().mockImplementation((path: string, options?: RequestInit) => {
    if (path === "/api/v1/content/providers") return jsonResponse(providerPayload);
    if (path === "/api/v1/content/jobs") return jsonResponse({ jobs: [
      { id: "queued-1", provider: "frontier", content_type: "code_reading", status: "queued", attempts: 0 },
      { id: "completed-1", provider: "frontier", content_type: "debugging", status: "completed", attempts: 1, has_artifact: true },
    ] });
    if (options?.method === "POST") {
      postPaths.push(path);
      return jsonResponse({ id: path.endsWith("/clone") ? "clone-1" : path.split("/").at(-2), status: "queued" });
    }
    if (path === "/api/v1/content/jobs/queued-1") return jsonResponse({ job: { id: "queued-1", provider: "frontier", content_type: "code_reading", status: "cancelled", attempts: 0, request_spec: {} }, attempts: [], artifacts: [], audit: [{ action: "content.job.cancelled" }] });
    if (path === "/api/v1/content/jobs/clone-1") return jsonResponse({ job: { id: "clone-1", provider: "frontier", content_type: "debugging", status: "queued", attempts: 0, request_spec: {} }, attempts: [], artifacts: [], audit: [{ action: "content.job.cloned" }] });
    throw new Error(`unexpected fetch: ${path}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("crypto", { randomUUID: vi.fn()
    .mockReturnValueOnce("cancel-key")
    .mockReturnValueOnce("clone-key")
    .mockReturnValueOnce("archive-key") });

  render(<ContentStudioPage />);
  await screen.findByText("생성 대기 중");
  fireEvent.click(screen.getByRole("button", { name: "취소" }));
  await screen.findByText(/content.job.cancelled/);
  fireEvent.click(screen.getAllByRole("button", { name: "복제" })[1]);
  await screen.findByText(/content.job.cloned/);
  fireEvent.click(screen.getByRole("button", { name: "보관" }));
  await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("작업을 보관했습니다."));

  expect(postPaths).toEqual([
    "/api/v1/content/jobs/queued-1/cancel",
    "/api/v1/content/jobs/completed-1/clone",
    "/api/v1/content/jobs/completed-1/archive",
  ]);
  const actionCalls = fetchMock.mock.calls.filter(([, options]) => options?.method === "POST");
  expect(actionCalls.map(([, options]) => options?.headers)).toEqual([
    { "content-type": "application/json", "x-csrf-token": "csrf-test" },
    { "content-type": "application/json", "x-csrf-token": "csrf-test" },
    { "content-type": "application/json", "x-csrf-token": "csrf-test" },
  ]);
  expect(actionCalls.map(([, options]) => JSON.parse(String(options?.body)).idempotency_key)).toEqual(["cancel-key", "clone-key", "archive-key"]);
  expect(fetchMock.mock.calls.filter(([path]) => path === "/api/v1/content/jobs")).toHaveLength(4);
  expect(screen.getAllByRole("button", { name: "비교 선택" })).toHaveLength(1);
});

test("응답 유실 뒤 같은 동작을 다시 누르면 멱등키를 재사용하고 실행 중에는 잘못된 복제·비교를 숨긴다", async () => {
  let actionAttempts = 0;
  const keys: string[] = [];
  const fetchMock = vi.fn().mockImplementation((path: string, options?: RequestInit) => {
    if (path === "/api/v1/content/providers") return jsonResponse(providerPayload);
    if (path === "/api/v1/content/jobs") return jsonResponse({ jobs: [
      { id: "leased-1", provider: "frontier", content_type: "debugging", status: "leased", attempts: 1 },
      { id: "failed-1", provider: "frontier", content_type: "debugging", status: "failed", attempts: 1 },
      { id: "corrupt-1", provider: "frontier", content_type: "debugging", status: "completed", attempts: 1, has_artifact: false },
    ] });
    if (path === "/api/v1/content/jobs/failed-1/retry" && options?.method === "POST") {
      keys.push(JSON.parse(String(options.body)).idempotency_key);
      actionAttempts += 1;
      if (actionAttempts === 1) return Promise.reject(new Error("응답 유실"));
      return jsonResponse({ id: "retry-1", status: "queued" });
    }
    if (path === "/api/v1/content/jobs/retry-1") return jsonResponse({ job: { id: "retry-1", status: "queued", request_spec: {} }, attempts: [], artifacts: [], audit: [] });
    throw new Error(`unexpected fetch: ${path}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("crypto", { randomUUID: vi.fn().mockReturnValue("stable-retry-key") });
  render(<ContentStudioPage />);
  await screen.findByText("생성 중");
  expect(screen.queryByRole("button", { name: "비교 선택" })).not.toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "복제" })).toHaveLength(2);
  const leasedCard = screen.getByText("생성 중").closest("article");
  expect(leasedCard && within(leasedCard).queryByRole("button", { name: "복제" })).toBeNull();
  const retry = screen.getByRole("button", { name: "수동 재시도" });
  fireEvent.click(retry);
  await screen.findByText("응답 유실");
  fireEvent.click(retry);
  await waitFor(() => expect(keys).toEqual(["stable-retry-key", "stable-retry-key"]));
});

test("생성 응답 유실에는 같은 키를 재사용하고 성공한 다음 생성에는 새 키를 쓴다", async () => {
  let creates = 0;
  const keys: string[] = [];
  const fetchMock = vi.fn().mockImplementation((path: string, options?: RequestInit) => {
    if (path === "/api/v1/content/providers") return jsonResponse(providerPayload);
    if (path === "/api/v1/content/jobs" && options?.method !== "POST") return jsonResponse({ jobs: [] });
    if (path === "/api/v1/content/jobs" && options?.method === "POST") {
      keys.push(JSON.parse(String(options.body)).idempotency_key);
      creates += 1;
      if (creates === 1) return Promise.reject(new Error("생성 응답 유실"));
      return jsonResponse({ id: `job-${creates}`, status: "queued" }, 201);
    }
    throw new Error(`unexpected fetch: ${path}`);
  });
  vi.stubGlobal("fetch", fetchMock);
  vi.stubGlobal("crypto", { randomUUID: vi.fn().mockReturnValueOnce("create-stable").mockReturnValueOnce("create-next") });
  render(<ContentStudioPage />);
  await screen.findByRole("option", { name: /frontier/ });
  fireEvent.change(screen.getByRole("textbox", { name: "생성 주제" }), { target: { value: "멱등 생성 요청" } });
  const submit = screen.getByRole("button", { name: "생성 작업 요청" });
  fireEvent.click(submit);
  await screen.findByText("생성 응답 유실");
  fireEvent.click(submit);
  await screen.findByText("생성 대기 중");
  fireEvent.click(submit);
  await waitFor(() => expect(keys).toEqual(["create-stable", "create-stable", "create-next"]));
});
