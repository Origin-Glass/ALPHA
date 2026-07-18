import { FormEvent, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type Provider = { id: string; name: string; kind: string; protocol: string; model: string; cost_per_generation_microunits: number; enabled: boolean; credential_available: boolean };
type Job = { id: string; provider: string; content_type: string; status: string; attempts: number; last_error_code?: string };
type JobDetail = { job: Job & { request_spec: object }; attempts: Array<Record<string, unknown>>; artifacts: Array<{ id: string; kind: string; payload: unknown; hash: string }>; audit: Array<Record<string, unknown>> };

const csrf = () => document.cookie.split(";").map((item) => item.trim())
  .find((item) => item.startsWith("alpha_csrf="))?.slice("alpha_csrf=".length) ?? "";
const statusLabel: Record<string, string> = {
  queued: "생성 대기 중", leased: "생성 중", completed: "생성 완료", failed: "생성 실패",
  blocked_disabled: "AI 생성 기능이 꺼져 차단됨",
  blocked_missing_credential: "자격 증명 환경 변수가 없어 차단됨",
};

function ContentStudioPage() {
  const [providers, setProviders] = useState<Provider[]>([]);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [providerId, setProviderId] = useState("");
  const [contentType, setContentType] = useState("code_reading");
  const [topic, setTopic] = useState("");
  const [count, setCount] = useState(1);
  const [message, setMessage] = useState("");
  const [detail, setDetail] = useState<JobDetail | null>(null);
  const [compareIds, setCompareIds] = useState<string[]>([]);
  const [comparison, setComparison] = useState<unknown>(null);

  const load = async () => {
    try {
      const [providersResponse, jobsResponse] = await Promise.all([
        fetch("/api/v1/content/providers", { credentials: "include" }),
        fetch("/api/v1/content/jobs", { credentials: "include" }),
      ]);
      const providerPayload = await providersResponse.json();
      const jobPayload = await jobsResponse.json();
      if (!providersResponse.ok || !jobsResponse.ok) throw new Error("생성 현황을 불러오지 못했습니다.");
      setProviders(providerPayload.providers ?? []);
      setJobs(jobPayload.jobs ?? []);
      setProviderId((current) => current || providerPayload.providers?.[0]?.id || "");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "생성 현황을 불러오지 못했습니다.");
    }
  };
  useEffect(() => { void load(); const timer = window.setInterval(() => { void load(); }, 3000); return () => window.clearInterval(timer); }, []);

  const loadDetail = async (id: string) => { try { const response = await fetch(`/api/v1/content/jobs/${id}`, { credentials: "include" }); const payload = await response.json(); if (!response.ok) throw new Error(payload.error?.message ?? "작업 상세를 불러오지 못했습니다."); setDetail(payload); } catch (error) { setMessage(error instanceof Error ? error.message : "작업 상세를 불러오지 못했습니다."); } };
  const action = async (id: string, name: "cancel" | "retry" | "clone" | "archive") => { try { const response = await fetch(`/api/v1/content/jobs/${id}/${name}`, { method: "POST", credentials: "include", headers: { "content-type": "application/json", "x-csrf-token": csrf() }, body: JSON.stringify({ idempotency_key: crypto.randomUUID() }) }); const payload = await response.json(); if (!response.ok) throw new Error(payload.error?.message ?? "작업 상태를 바꾸지 못했습니다."); setMessage(name === "cancel" ? "작업을 취소했습니다." : name === "retry" ? "수동 재시도 작업을 만들었습니다." : name === "clone" ? "복제 작업을 만들었습니다." : "작업을 보관했습니다."); await load(); if (name !== "archive") await loadDetail(payload.id ?? id); } catch (error) { setMessage(error instanceof Error ? error.message : "작업 상태를 바꾸지 못했습니다."); } };
  const toggleCompare = (id: string) => setCompareIds((current) => current.includes(id) ? current.filter((item) => item !== id) : current.length < 2 ? [...current, id] : [current[1], id]);
  const compare = async () => { if (compareIds.length !== 2) return; try { const response = await fetch(`/api/v1/content/jobs/compare?left=${compareIds[0]}&right=${compareIds[1]}`, { credentials: "include" }); const payload = await response.json(); if (!response.ok) throw new Error(payload.error?.message ?? "작업을 비교하지 못했습니다."); setComparison(payload); } catch (error) { setMessage(error instanceof Error ? error.message : "작업을 비교하지 못했습니다."); } };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setMessage("");
    try {
      const response = await fetch("/api/v1/content/jobs", {
        method: "POST", credentials: "include",
        headers: { "content-type": "application/json", "x-csrf-token": csrf() },
        body: JSON.stringify({ provider_id: providerId, content_type: contentType, topic, target_language: "ko", generation_count: count }),
      });
      const payload = await response.json();
      if (!response.ok) throw new Error(payload.error?.message ?? "생성 작업을 요청하지 못했습니다.");
      setMessage(statusLabel[payload.status] ?? "생성 작업을 접수했습니다.");
      await load();
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "생성 작업을 요청하지 못했습니다.");
    }
  };

  return <div className="learning-page"><PortalHeader /><main className="factory-page">
    <p className="eyebrow">AI 콘텐츠 공장</p><h1>제공자는 바꿔도,<br /><span>생성 기록은 남겨요</span></h1>
    <div className="factory-grid">
      <section><h2>새 생성 작업</h2><form onSubmit={submit}>
        <label><span>AI 제공자</span><select value={providerId} onChange={(event) => setProviderId(event.target.value)} required>
          <option value="" disabled>제공자를 선택하세요</option>
          {providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.name} · {provider.protocol === "anthropic" ? "Anthropic" : "OpenAI 호환"} · {provider.model}{!provider.credential_available ? " · 자격 증명 없음" : ""}</option>)}
        </select></label>
        <label><span>콘텐츠 유형</span><select value={contentType} onChange={(event) => setContentType(event.target.value)}>
          <option value="algorithm_problem">알고리즘 문제</option><option value="code_reading">코드 읽기</option><option value="debugging">디버깅</option><option value="documentation_lesson">문서 학습</option><option value="implementation_task">구현 과제</option>
        </select></label>
        <label><span>생성 주제</span><textarea minLength={2} maxLength={200} value={topic} onChange={(event) => setTopic(event.target.value)} required /></label>
        <div className="factory-fields"><label><span>생성 수</span><input type="number" min="1" max="20" value={count} onChange={(event) => setCount(Number(event.target.value))} /></label><p>재시도 3회를 포함한 최대 예약액: {((providers.find((provider) => provider.id === providerId)?.cost_per_generation_microunits ?? 0) * count * 3).toLocaleString()} 마이크로 단위</p></div>
        <button type="submit" disabled={!providerId || topic.trim().length < 2}>생성 작업 요청</button>
      </form></section>
      <section><h2>최근 작업</h2><div className="factory-jobs">
        {jobs.length === 0 && <p className="dashboard-empty">아직 생성 작업이 없습니다.</p>}
        {jobs.map((job) => <article key={job.id}><div><strong>{job.provider}</strong><span>{statusLabel[job.status] ?? job.status}</span></div><small>{job.content_type} · 시도 {job.attempts}회{job.last_error_code ? ` · ${job.last_error_code}` : ""}</small><div><button type="button" onClick={() => loadDetail(job.id)}>상세 보기</button><button type="button" onClick={() => toggleCompare(job.id)}>{compareIds.includes(job.id) ? "비교 해제" : "비교 선택"}</button>{["queued", "leased", "blocked_disabled", "blocked_missing_credential"].includes(job.status) && <button type="button" onClick={() => action(job.id, "cancel")}>취소</button>}{job.status === "failed" && <button type="button" onClick={() => action(job.id, "retry")}>수동 재시도</button>}<button type="button" onClick={() => action(job.id, "clone")}>복제</button>{["completed", "failed", "cancelled"].includes(job.status) && <button type="button" onClick={() => action(job.id, "archive")}>보관</button>}</div></article>)}
      </div></section>
    </div>
    {compareIds.length === 2 && <button type="button" className="primary-action" onClick={compare}>선택한 작업 비교</button>}
    {detail && <section className="factory-detail"><h2>작업 상세와 산출물</h2><p>{statusLabel[detail.job.status] ?? detail.job.status}</p><h3>시도 기록</h3><pre>{JSON.stringify(detail.attempts, null, 2)}</pre><h3>불변 산출물</h3>{detail.artifacts.map((artifact) => <article key={artifact.id}><strong>{artifact.kind}</strong><small>SHA-256 {artifact.hash}</small><pre>{JSON.stringify(artifact.payload, null, 2)}</pre></article>)}<h3>감사 기록</h3><pre>{JSON.stringify(detail.audit, null, 2)}</pre></section>}
    {comparison !== null && <section className="factory-detail"><h2>작업 비교</h2><pre>{JSON.stringify(comparison, null, 2)}</pre></section>}
    {message && <p className="auth-message factory-message" role="alert">{message}</p>}
  </main></div>;
}

export default ContentStudioPage;
