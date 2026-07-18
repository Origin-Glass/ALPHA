import { FormEvent, useEffect, useState } from "react";
import PortalHeader from "./PortalHeader";

type Policy = { version: string; title: string; body: string; required: boolean; consented: boolean };
const csrf = () => document.cookie.split(";").map((item) => item.trim())
  .find((item) => item.startsWith("alpha_csrf="))?.slice("alpha_csrf=".length) ?? "";

function PoliciesPage() {
  const [items, setItems] = useState<Policy[]>([]);
  const [choices, setChoices] = useState({ terms: false, privacy: false });
  const [message, setMessage] = useState("");

  useEffect(() => {
    fetch("/api/v1/policies", { credentials: "include" })
      .then(async (response) => {
        if (response.status === 401) { window.location.assign("/login?redirect_after=/policies"); return null; }
        if (!response.ok) throw new Error();
        return response.json();
      })
      .then((body) => body && setItems(body.items))
      .catch(() => setMessage("정책 정보를 불러오지 못했습니다."));
  }, []);

  const pending = items.find((item) => item.required && !item.consented);
  const consent = async (event: FormEvent) => {
    event.preventDefault();
    if (!pending || !choices.terms || !choices.privacy) return;
    const response = await fetch("/api/v1/policies/consents", {
      method: "POST", credentials: "include",
      headers: { "content-type": "application/json", "x-csrf-token": csrf() },
      body: JSON.stringify({ version: pending.version, choices }),
    });
    if (!response.ok) { setMessage("정책 동의를 저장하지 못했습니다."); return; }
    setItems((current) => current.map((item) => item.version === pending.version ? { ...item, consented: true } : item));
    setMessage("정책 동의를 저장했습니다.");
  };

  const requestData = async (kind: "export" | "delete") => {
    const response = await fetch("/api/v1/data-requests", {
      method: "POST", credentials: "include",
      headers: { "content-type": "application/json", "x-csrf-token": csrf() },
      body: JSON.stringify({ kind }),
    });
    setMessage(response.ok ? (kind === "export" ? "데이터 내보내기를 요청했습니다." : "데이터 삭제를 요청했습니다.") : "데이터 요청을 저장하지 못했습니다.");
  };

  return <div className="learning-page"><PortalHeader /><main className="policy-page">
    <p className="eyebrow">정책과 개인정보</p><h1>동의한 버전을<br /><span>분명히 기록해요</span></h1>
    <section className="policy-list" aria-label="정책 버전">
      {items.map((item) => <article key={item.version}><div><strong>{item.title}</strong><span>버전 {item.version}{item.required ? " · 필수" : ""}</span><p className="policy-body">{item.body}</p></div><b>{item.consented ? "동의 완료" : "동의 필요"}</b></article>)}
    </section>
    {pending && <form className="policy-consent" onSubmit={consent}>
      <label><input type="checkbox" checked={choices.terms} onChange={(event) => setChoices((current) => ({ ...current, terms: event.target.checked }))} /><span>이용약관에 동의합니다</span></label>
      <label><input type="checkbox" checked={choices.privacy} onChange={(event) => setChoices((current) => ({ ...current, privacy: event.target.checked }))} /><span>개인정보 처리방침에 동의합니다</span></label>
      <button type="submit" disabled={!choices.terms || !choices.privacy}>필수 정책에 동의</button>
    </form>}
    {!pending && items.length > 0 && <section className="data-controls"><h2>내 데이터 요청</h2><p>요청 상태와 결과는 본인 계정에서만 확인할 수 있습니다.</p><div><button type="button" onClick={() => requestData("export")}>내 데이터 내보내기</button><button type="button" className="danger" onClick={() => requestData("delete")}>내 데이터 삭제 요청</button></div></section>}
    {message && <p className="auth-message" role="alert">{message}</p>}
  </main></div>;
}

export default PoliciesPage;
