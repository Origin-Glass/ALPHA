import { useEffect, useRef, useState } from 'react';
import PortalHeader from './PortalHeader';

type Challenge = { id: string; prompt: string; state: string };
type Result = { state: string; evidence_labels: string[]; assistance_disclosure: string };
const csrf = () => document.cookie.split(';').map((value) => value.trim()).find((value) => value.startsWith('alpha_csrf='))?.slice(11) ?? '';
const evidenceLabels: Record<string, string> = { BUILT: '직접 구현함', TESTED: '테스트함', DEBUGGED: '디버깅함', EXPLAINED: '설명함', INDEPENDENTLY_MODIFIED: '독립적으로 수정함', TRANSFER_VERIFIED: '전이 검증됨', MAINTAINED: '유지보수함' };
const stateLabels: Record<string, string> = { EXECUTABLE: '실행 가능', REVIEWED: '검토됨', UNDERSTOOD: '이해함', INDEPENDENTLY_MODIFIABLE: '독립 수정 가능', TRANSFER_VERIFIED: '전이 검증됨', MASTERED: '숙달 검증됨' };

function UnderstandingPage() {
  const workspaceId = new URLSearchParams(window.location.search).get('workspace') ?? '';
  const [challenge, setChallenge] = useState<Challenge | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  const [answers, setAnswers] = useState({ explanation: '', prediction: '', modification: '', transfer_answer: '' });
  const [message, setMessage] = useState('');
  const createKey = useRef(crypto.randomUUID());
  const submitKeys = useRef<Record<string, string>>({});
  const headers = { 'content-type': 'application/json', 'x-csrf-token': csrf() };

  useEffect(() => {
    fetch('/api/understanding/challenges', { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ workspace_id: workspaceId, kind: 'transfer', idempotency_key: createKey.current }) })
      .then(async (response) => { const body = await response.json(); if (!response.ok) throw new Error(body.error?.message); return body; })
      .then(setChallenge).catch((error) => setMessage(error.message ?? '이해 과제를 만들지 못했습니다.'));
  }, [workspaceId]);
  const update = (field: keyof typeof answers, value: string) => setAnswers((current) => ({ ...current, [field]: value }));
  const submit = async () => {
    if (!challenge) return;
    const signature = JSON.stringify(answers);
    submitKeys.current[signature] ??= crypto.randomUUID();
    const response = await fetch(`/api/understanding/challenges/${challenge.id}/submit`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...answers, idempotency_key: submitKeys.current[signature] }) });
    const body = await response.json();
    if (response.ok) setResult(body); else setMessage(body.error?.message ?? '이해 증거를 제출하지 못했습니다.');
  };

  return <div className="learning-page"><PortalHeader /><main className="path-panel understanding-panel">
    <p className="eyebrow">검증된 이해</p><h1>설명하고, 바꾸고, 전이하세요</h1>
    {challenge && <section className="project-form"><p>{challenge.prompt}</p><p>현재 상태: {stateLabels[challenge.state] ?? challenge.state}</p>
      <label>코드 목적과 흐름 설명<textarea value={answers.explanation} onChange={(event) => update('explanation', event.target.value)} /></label>
      <label>변경 결과 예측<textarea value={answers.prediction} onChange={(event) => update('prediction', event.target.value)} /></label>
      <label>독립 변형 내용<textarea value={answers.modification} onChange={(event) => update('modification', event.target.value)} /></label>
      <label>다른 맥락의 전이 답변<textarea value={answers.transfer_answer} onChange={(event) => update('transfer_answer', event.target.value)} /></label>
      <button type="button" disabled={Object.values(answers).some((answer) => !answer.trim())} onClick={submit}>이해 증거 제출</button></section>}
    {result && <section className="evidence-card" aria-live="polite"><h2>{stateLabels[result.state] ?? result.state}</h2><p>도움 공개: {result.assistance_disclosure}</p><ul className="evidence-list">{result.evidence_labels.map((label) => <li key={label}>{evidenceLabels[label] ?? label}</li>)}</ul></section>}
    {message && <p role="alert" className="auth-message">{message}</p>}
  </main></div>;
}

export default UnderstandingPage;
