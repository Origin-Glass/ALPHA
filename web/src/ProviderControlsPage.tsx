import { FormEvent, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type Provider = { id: string; name: string; kind: string; protocol: string; model: string; credential_available: boolean; enabled: boolean };
type Budget = { limit_microunits: number; reserved_microunits: number; spent_microunits: number };
const csrf = () => document.cookie.split(";").map((item) => item.trim()).find((item) => item.startsWith("alpha_csrf="))?.slice(11) ?? "";

function ProviderControlsPage() {
  const [providers, setProviders] = useState<Provider[]>([]);
  const [budget, setBudget] = useState<Budget | null>(null);
  const [name, setName] = useState(""); const [kind, setKind] = useState("external");
  const [protocol, setProtocol] = useState("openai_compatible");
  const [baseUrl, setBaseUrl] = useState("https://api.openai.com"); const [model, setModel] = useState("");
  const [credential, setCredential] = useState("OPENAI_API_KEY"); const [enabled, setEnabled] = useState(false);
  const [limit, setLimit] = useState(0); const [message, setMessage] = useState("");
  const load = async () => {
    const [providerResponse, budgetResponse] = await Promise.all([fetch("/api/v1/content/providers", { credentials: "include" }), fetch("/api/v1/content/budget", { credentials: "include" })]);
    const providerPayload = await providerResponse.json(); const budgetPayload = await budgetResponse.json();
    if (!providerResponse.ok || !budgetResponse.ok) throw new Error("제공자 설정을 불러오지 못했습니다.");
    setProviders(providerPayload.providers ?? []); setBudget(budgetPayload); setLimit(budgetPayload.limit_microunits);
  };
  useEffect(() => { load().catch((error) => setMessage(error.message)); }, []);
  const send = async (path: string, body: object) => {
    const response = await fetch(path, { method: "POST", credentials: "include", headers: { "content-type": "application/json", "x-csrf-token": csrf() }, body: JSON.stringify(body) });
    const payload = await response.json(); if (!response.ok) throw new Error(payload.error?.message ?? "설정을 저장하지 못했습니다.");
  };
  const saveProvider = async (event: FormEvent) => { event.preventDefault(); try { await send("/api/v1/content/providers", { name, kind, protocol, base_url: baseUrl, model, credential_env_var: kind === "local" ? null : credential, enabled }); setMessage("제공자 설정을 저장했습니다."); await load(); } catch (error) { setMessage(error instanceof Error ? error.message : "설정을 저장하지 못했습니다."); } };
  const saveBudget = async (event: FormEvent) => { event.preventDefault(); try { await send("/api/v1/content/budget", { limit_microunits: limit }); setMessage("전역 생성 예산을 저장했습니다."); await load(); } catch (error) { setMessage(error instanceof Error ? error.message : "예산을 저장하지 못했습니다."); } };

  return <div className="learning-page"><PortalHeader /><main className="factory-page">
    <p className="eyebrow">제공자 운영 제어</p><h1>키는 환경에만,<br /><span>정책과 비용은 기록으로</span></h1>
    <div className="factory-grid">
      <section><h2>제공자 추가</h2><form onSubmit={saveProvider}>
        <label><span>설정 이름</span><input pattern="[a-z0-9][a-z0-9_-]{2,39}" value={name} onChange={(event) => setName(event.target.value.toLowerCase())} required /></label>
        <label><span>제공자 종류</span><select value={kind} onChange={(event) => { const value = event.target.value; setKind(value); if (value === "local") { setProtocol("openai_compatible"); setBaseUrl("http://127.0.0.1:11434"); } else { setBaseUrl("https://api.openai.com"); } }}><option value="external">외부 HTTPS</option><option value="local">로컬 HTTP 루프백 11434</option></select></label>
        <label><span>호출 프로토콜</span><select value={protocol} disabled={kind === "local"} onChange={(event) => { const value = event.target.value; setProtocol(value); if (value === "anthropic") { setCredential("ANTHROPIC_API_KEY"); setBaseUrl("https://api.anthropic.com"); } else { setCredential("OPENAI_API_KEY"); setBaseUrl("https://api.openai.com"); } }}><option value="openai_compatible">OpenAI 호환</option><option value="anthropic">Anthropic Messages</option></select></label>
        <label><span>기본 URL</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} required /></label>
        <label><span>모델</span><input value={model} onChange={(event) => setModel(event.target.value)} required /></label>
        {kind === "external" && <label><span>자격 증명 환경 변수</span><select value={credential} onChange={(event) => setCredential(event.target.value)}>{protocol === "anthropic" ? <option>ANTHROPIC_API_KEY</option> : <><option>OPENAI_API_KEY</option><option>OPENROUTER_API_KEY</option><option>CONTENT_AI_CUSTOM_API_KEY</option></>}</select></label>}
        <label className="factory-check"><input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} /><span>이 제공자 활성화</span></label><button type="submit">제공자 저장</button>
      </form></section>
      <section><h2>예산과 상태</h2><form onSubmit={saveBudget}><label><span>전역 한도(마이크로 단위)</span><input type="number" min="0" max="1000000000000" value={limit} onChange={(event) => setLimit(Number(event.target.value))} /></label><button type="submit">예산 저장</button></form>
        {budget && <p>예약 {budget.reserved_microunits.toLocaleString()} · 사용 {budget.spent_microunits.toLocaleString()} · 한도 {budget.limit_microunits.toLocaleString()}</p>}
        <div className="factory-jobs">{providers.map((provider) => <article key={provider.id}><div><strong>{provider.name}</strong><span>{provider.enabled ? "활성" : "비활성"}</span></div><small>{provider.kind} · {provider.protocol === "anthropic" ? "Anthropic" : "OpenAI 호환"} · {provider.model} · {provider.credential_available ? "실행 준비됨" : "자격 증명 없음"}</small></article>)}</div>
      </section>
    </div>{message && <p className="auth-message factory-message" role="alert">{message}</p>}
  </main></div>;
}

export default ProviderControlsPage;
