import { FormEvent, useState } from 'react';
import PortalHeader from './PortalHeader';

type Plan = { target_outcome: string; recommendation_key: string; reason_codes: string[]; items: Array<{ kind: string; title: string; estimated_minutes: number }>; provider_used: boolean; rule_version: string };
const csrf = () => document.cookie.split(';').map((v) => v.trim()).find((v) => v.startsWith('alpha_csrf='))?.slice(11) ?? '';

function LearningPlanBuilderPage() {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [message, setMessage] = useState('');
  const [target, setTarget] = useState('작동하는 한국어 학습 기록 프로젝트 완성');
  const [minutes, setMinutes] = useState(180);

  const submit = async (event: FormEvent) => {
    event.preventDefault(); setMessage('');
    const response = await fetch('/api/v1/learning/plans', { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf() }, body: JSON.stringify({
      rule_version: 'project-learning-v1', idempotency_key: crypto.randomUUID(), target_outcome: target, weekly_minutes: minutes,
      preferred_language: 'typescript', path_mode: 'structured', interests: ['웹', '학습 기록'], goals: ['독립 구현'],
      diagnostic_scores: { algorithmic_reasoning: 50, code_literacy: 50, docs_learning: 50, independent_coding: 50 },
    }) });
    const body = await response.json();
    if (!response.ok) { setMessage(body.error?.message ?? '계획을 만들지 못했습니다.'); return; }
    setPlan(body);
  };

  const reject = async () => {
    if (!plan) return;
    const response = await fetch(`/api/v1/learning/recommendations/${plan.recommendation_key}/reject`, { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf() }, body: JSON.stringify({ reason: '다른 프로젝트 경로를 선택하고 싶음', idempotency_key: crypto.randomUUID() }) });
    setMessage(response.ok ? '추천을 거부했습니다. 다시 계획하면 다른 경로를 제안합니다.' : '추천 거부를 기록하지 못했습니다.');
  };

  return <div className="learning-page"><PortalHeader /><main className="path-panel project-learning-panel">
    <p className="eyebrow">설명 가능한 개인화</p><h1>내가 고르는 학습 계획</h1>
    <form className="project-form" onSubmit={submit}>
      <label>목표 결과<input value={target} maxLength={300} onChange={(e) => setTarget(e.target.value)} required /></label>
      <label>주간 학습 시간(분)<input type="number" min={30} max={2400} value={minutes} onChange={(e) => setMinutes(Number(e.target.value))} required /></label>
      <button className="primary-action" type="submit">계획 만들기</button>
    </form>
    {plan && <section className="project-result" aria-live="polite"><p className="status-dot">규칙 {plan.rule_version} · AI 사용 {plan.provider_used ? '예' : '아니요'}</p><h2>{plan.target_outcome}</h2>
      <p>추천 이유: {plan.reason_codes.join(' · ')}</p><ol>{plan.items.map((item) => <li key={item.kind}><strong>{item.title}</strong><span>{item.estimated_minutes}분</span></li>)}</ol>
      <button type="button" onClick={reject}>이 추천 거부</button></section>}
    {message && <p role="status" className="auth-message">{message}</p>}
  </main></div>;
}
export default LearningPlanBuilderPage;
