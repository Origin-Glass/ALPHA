import { useEffect, useRef, useState } from 'react';
import PortalHeader from './PortalHeader';

type Portfolio = { id: string; title: string; problem: string; target_user: string; visibility: string; assistance_disclosure: string; evidence_labels: string[]; status: string };
const csrf = () => document.cookie.split(';').map((value) => value.trim()).find((value) => value.startsWith('alpha_csrf='))?.slice(11) ?? '';
const evidenceLabels: Record<string, string> = { BUILT: '직접 구현함', TESTED: '테스트함', DEBUGGED: '디버깅함', EXPLAINED: '설명함', INDEPENDENTLY_MODIFIED: '독립적으로 수정함', TRANSFER_VERIFIED: '전이 검증됨', MAINTAINED: '유지보수함' };

function PortfolioPage() {
  const [items, setItems] = useState<Portfolio[]>([]);
  const [form, setForm] = useState({ title: '', problem: '', target_user: '', workspace_id: '', source_run_id: '', rights_receipt_id: '', assistance_disclosure: '', visibility: 'private' });
  const [message, setMessage] = useState('');
  const keys = useRef<Record<string, string>>({});
  const keyFor = (action: string, value: unknown) => { const signature = `${action}:${JSON.stringify(value)}`; return keys.current[signature] ??= crypto.randomUUID(); };
  const headers = { 'content-type': 'application/json', 'x-csrf-token': csrf() };

  useEffect(() => { fetch('/api/v1/portfolio', { credentials: 'include' }).then((response) => response.json()).then((body) => setItems((current) => [...current, ...(body.items ?? []).filter((item: Portfolio) => !current.some((candidate) => candidate.id === item.id))])).catch(() => setMessage('포트폴리오를 불러오지 못했습니다.')); }, []);
  const update = (field: keyof typeof form, value: string) => setForm((current) => ({ ...current, [field]: value }));
  const create = async () => {
    const payload = { ...form, evidence_labels: ['BUILT', 'TESTED', 'EXPLAINED'] };
    const response = await fetch('/api/v1/portfolio', { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: keyFor('create', payload) }) });
    const body = await response.json();
    if (response.ok) setItems((current) => [body, ...current]); else setMessage(body.error?.message ?? '포트폴리오를 만들지 못했습니다.');
  };
  const publish = async (item: Portfolio) => {
    const response = await fetch(`/api/v1/portfolio/${item.id}/publish`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ idempotency_key: keyFor('publish', item.id) }) });
    const body = await response.json();
    if (response.ok) setItems((current) => current.map((candidate) => candidate.id === item.id ? body : candidate)); else setMessage(body.error?.message ?? '게시하지 못했습니다.');
  };

  return <div className="learning-page"><PortalHeader /><main className="path-panel portfolio-panel">
    <p className="eyebrow">증거 기반 포트폴리오</p><h1>과정과 도움을 정확히 공개하세요</h1>
    <section className="project-form" aria-label="포트폴리오 만들기">
      <label>제목<input value={form.title} onChange={(event) => update('title', event.target.value)} /></label>
      <label>해결한 문제<textarea value={form.problem} onChange={(event) => update('problem', event.target.value)} /></label>
      <label>대상 사용자<input value={form.target_user} onChange={(event) => update('target_user', event.target.value)} /></label>
      <label>연결할 작업공간 ID<input value={form.workspace_id} onChange={(event) => update('workspace_id', event.target.value)} /></label>
      <label>성공한 실행 ID<input value={form.source_run_id} onChange={(event) => update('source_run_id', event.target.value)} /></label>
      <label>권리 승인 영수증 ID<input value={form.rights_receipt_id} onChange={(event) => update('rights_receipt_id', event.target.value)} /></label>
      <label>공개 범위<select value={form.visibility} onChange={(event) => update('visibility', event.target.value)}><option value="private">비공개</option><option value="public">공개</option></select></label>
      <label>도움 사용 공개<textarea value={form.assistance_disclosure} onChange={(event) => update('assistance_disclosure', event.target.value)} /></label>
      <button type="button" disabled={!form.title.trim() || !form.problem.trim() || !form.target_user.trim() || !form.workspace_id || !form.source_run_id || !form.rights_receipt_id || !form.assistance_disclosure.trim()} onClick={create}>포트폴리오 만들기</button>
    </section>
    <div className="portfolio-grid">{items.map((item) => <article className="evidence-card" key={item.id}><h2>{item.title}</h2><p>{item.problem}</p><p>대상: {item.target_user}</p><p>공개 범위: {item.visibility === 'public' ? '공개' : '비공개'}</p><p>도움 공개: {item.assistance_disclosure}</p><ul className="evidence-list">{item.evidence_labels.map((label) => <li key={label}>{evidenceLabels[label] ?? label}</li>)}</ul>{item.visibility === 'public' && item.status === 'draft' && <button type="button" onClick={() => publish(item)}>권리 확인 후 게시</button>}</article>)}</div>
    {message && <p role="alert" className="auth-message">{message}</p>}
  </main></div>;
}

export default PortfolioPage;
