import { FormEvent, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type Viewer = { roles: string[] };
const csrf = () => document.cookie.split(";").map((item) => item.trim())
  .find((item) => item.startsWith("alpha_csrf="))?.slice("alpha_csrf=".length) ?? "";

function GovernancePage() {
  const [viewer, setViewer] = useState<Viewer | null>(null);
  const [slug, setSlug] = useState("");
  const [basis, setBasis] = useState("original");
  const [evidence, setEvidence] = useState("");
  const [commercial, setCommercial] = useState(false);
  const [redistribution, setRedistribution] = useState(false);
  const [note, setNote] = useState("");
  const [message, setMessage] = useState("");

  useEffect(() => { fetch("/api/v1/auth/me", { credentials: "include" }).then((response) => response.ok ? response.json() : null).then(setViewer).catch(() => setMessage("권한을 확인하지 못했습니다.")); }, []);
  const send = async (path: string, body: object) => {
    const response = await fetch(path, { method: "POST", credentials: "include", headers: { "content-type": "application/json", "x-csrf-token": csrf() }, body: JSON.stringify(body) });
    const payload = await response.json();
    if (!response.ok) throw new Error(payload.error?.message ?? "요청을 처리하지 못했습니다.");
  };
  const act = async (path: string, body: object, success: string) => { try { await send(path, body); setMessage(success); } catch (error) { setMessage(error instanceof Error ? error.message : "요청을 처리하지 못했습니다."); } };
  const base = `/api/v1/governance/problems/${encodeURIComponent(slug)}`;
  const creator = viewer?.roles.some((role) => ["CONTENT_CREATOR", "PROBLEM_SETTER", "ADMIN"].includes(role));
  const publisher = viewer?.roles.some((role) => ["CONTENT_CREATOR", "ADMIN"].includes(role));
  const contentReviewer = viewer?.roles.some((role) => ["CONTENT_REVIEWER", "ADMIN"].includes(role));
  const rightsReviewer = viewer?.roles.some((role) => ["RIGHTS_REVIEWER", "ADMIN"].includes(role));
  const saveRights = (event: FormEvent) => { event.preventDefault(); act(`${base}/rights`, { basis, evidence, commercial_use_allowed: commercial, redistribution_allowed: redistribution }, "권리 근거를 저장하고 기존 승인을 초기화했습니다."); };

  return <div className="learning-page"><PortalHeader /><main className="governance-page">
    <p className="eyebrow">콘텐츠 게시 거버넌스</p><h1>작성과 검토를<br /><span>한 사람에게 맡기지 않아요</span></h1>
    <label className="governance-slug"><span>문제 식별자</span><input value={slug} onChange={(event) => setSlug(event.target.value.toLowerCase())} pattern="[a-z0-9-]+" required /></label>
    <div className="governance-grid">
      {creator && <section><h2>권리 근거 기록</h2><form onSubmit={saveRights}><label><span>권리 유형</span><select value={basis} onChange={(event) => setBasis(event.target.value)}><option value="original">직접 제작</option><option value="licensed">라이선스 허락</option><option value="public_domain">퍼블릭 도메인</option></select></label><label><span>출처·라이선스 근거</span><textarea minLength={3} maxLength={2000} value={evidence} onChange={(event) => setEvidence(event.target.value)} required /></label><label className="governance-check"><input type="checkbox" checked={commercial} onChange={(event) => setCommercial(event.target.checked)} /><span>상업 이용 허용</span></label><label className="governance-check"><input type="checkbox" checked={redistribution} onChange={(event) => setRedistribution(event.target.checked)} /><span>재배포 허용</span></label><button type="submit" disabled={!slug}>권리 근거 저장</button></form>{publisher && <button type="button" disabled={!slug} onClick={() => act(`${base}/publish`, {}, "게시 승인이 완료됐습니다.")}>분리 승인 확인 후 게시</button>}</section>}
      {(contentReviewer || rightsReviewer) && <section><h2>독립 검토</h2><label><span>검토 근거</span><textarea minLength={3} maxLength={1000} value={note} onChange={(event) => setNote(event.target.value)} required /></label><div>{contentReviewer && <button type="button" disabled={!slug || note.length < 3} onClick={() => act(`${base}/reviews/content`, { note }, "사람 검토를 승인했습니다.")}>사람 검토 승인</button>}{rightsReviewer && <button type="button" disabled={!slug || note.length < 3} onClick={() => act(`${base}/reviews/rights`, { note }, "권리 검토를 승인했습니다.")}>권리 검토 승인</button>}</div><p>작성자는 자기 콘텐츠를 승인할 수 없고, 두 검토자는 서로 달라야 합니다.</p></section>}
    </div>
    {viewer && !creator && !contentReviewer && !rightsReviewer && <p className="dashboard-empty">콘텐츠 거버넌스 역할이 없습니다.</p>}
    {message && <p className="auth-message" role="alert">{message}</p>}
  </main></div>;
}

export default GovernancePage;
