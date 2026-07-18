import { useEffect, useRef, useState } from 'react';
import PortalHeader from './PortalHeader';

type Challenge = { id: string; prompt: Record<string, string>; state: string };
type Result = { state: string; evidence_labels: string[]; assistance_disclosure: string };
const csrf = () => document.cookie.split(';').map((value) => value.trim()).find((value) => value.startsWith('alpha_csrf='))?.slice(11) ?? '';
const evidenceLabels: Record<string, string> = { BUILT: '직접 구현함', TESTED: '테스트함', DEBUGGED: '디버깅함', EXPLAINED: '설명함', INDEPENDENTLY_MODIFIED: '독립적으로 수정함', TRANSFER_VERIFIED: '전이 검증됨', MAINTAINED: '유지보수함' };
const stateLabels: Record<string, string> = { EXECUTABLE: '실행 가능', REVIEWED: '검토됨', UNDERSTOOD: '이해함', INDEPENDENTLY_MODIFIABLE: '독립 수정 가능', TRANSFER_VERIFIED: '전이 검증됨', MASTERED: '숙달 검증됨' };

function UnderstandingPage() {
  const workspaceId = new URLSearchParams(window.location.search).get('workspace') ?? '';
  const [challenge, setChallenge] = useState<Challenge | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  const [answers, setAnswers] = useState({ explanation: '', prediction: '', modification: '', transfer_answer: '', modification_run_id: '', transfer_run_id: '' });
  const [predictionsCommitted, setPredictionsCommitted] = useState(false);
  const [transferWorkspace, setTransferWorkspace] = useState('');
  const [workspaces, setWorkspaces] = useState<Array<{ id: string; title: string; runtime_status?: string }>>([]);
  const [runStates, setRunStates] = useState<Record<string, string>>({});
  const [message, setMessage] = useState('');
  const createKey = useRef(crypto.randomUUID());
  const submitKeys = useRef<Record<string, string>>({});
  const headers = { 'content-type': 'application/json', 'x-csrf-token': csrf() };

  useEffect(() => {
    fetch('/api/v1/understanding/challenges', { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ workspace_id: workspaceId, kind: 'explanation_modification_transfer', idempotency_key: createKey.current }) })
      .then(async (response) => { const body = await response.json(); if (!response.ok) throw new Error(body.error?.message); return body; })
      .then(setChallenge).catch((error) => setMessage(error.message ?? '이해 과제를 만들지 못했습니다.'));
    fetch('/api/v1/workspaces', { credentials: 'include' }).then((response) => response.json()).then((body) => setWorkspaces((body.workspaces ?? []).filter((workspace: { id: string; runtime_status?: string }) => workspace.id !== workspaceId && workspace.runtime_status !== 'implemented_unverified' && workspace.runtime_status !== 'blocked'))).catch(() => undefined);
  }, [workspaceId]);
  useEffect(() => {
    const ids = [answers.modification_run_id, answers.transfer_run_id].filter(Boolean);
    if (!ids.length) return;
    const poll = () => ids.forEach((id) => fetch(`/api/v1/workspace-runs/${id}`, { credentials: 'include' }).then((response) => response.json()).then((body) => setRunStates((current) => ({ ...current, [id]: body.status }))).catch(() => undefined));
    poll(); const timer = window.setInterval(poll, 2000); return () => window.clearInterval(timer);
  }, [answers.modification_run_id, answers.transfer_run_id]);
  const update = (field: keyof typeof answers, value: string) => setAnswers((current) => ({ ...current, [field]: value }));
  const submit = async () => {
    if (!challenge) return;
    const payload = { explanation: answers.explanation, modification: answers.modification, modification_run_id: answers.modification_run_id, transfer_run_id: answers.transfer_run_id };
    const signature = JSON.stringify(payload);
    submitKeys.current[signature] ??= crypto.randomUUID();
    const response = await fetch(`/api/v1/understanding/challenges/${challenge.id}/submit`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: submitKeys.current[signature] }) });
    const body = await response.json();
    if (response.ok) setResult(body); else setMessage(body.error?.message ?? '이해 증거를 제출하지 못했습니다.');
  };
  const commitPredictions = async () => { if (!challenge) return; const payload = { modification_prediction: answers.prediction, transfer_prediction: answers.transfer_answer }; const response = await fetch(`/api/v1/understanding/challenges/${challenge.id}/predictions`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: submitKeys.current[`predictions:${JSON.stringify(payload)}`] ??= crypto.randomUUID() }) }); const body = await response.json(); if (response.ok) { setPredictionsCommitted(true); setMessage('예측을 확정했습니다. 이제 코드를 변경하고 검증 실행을 만드세요.'); } else setMessage(body.error?.message ?? '예측을 확정하지 못했습니다.'); };
  const createValidationRun = async (kind: 'modification' | 'transfer') => { if (!challenge) return; const target = kind === 'modification' ? workspaceId : transferWorkspace; if (!target) return setMessage('전이 작업공간을 선택해 주세요.'); const detail = await fetch(`/api/v1/workspaces/${target}`, { credentials: 'include' }); const workspace = await detail.json(); if (!detail.ok) return setMessage(workspace.error?.message ?? '작업공간을 불러오지 못했습니다.'); const payload = { expected_version: workspace.version, validation_kind: kind, challenge_id: challenge.id }; const response = await fetch(`/api/v1/workspaces/${target}/runs`, { method: 'POST', credentials: 'include', headers, body: JSON.stringify({ ...payload, idempotency_key: submitKeys.current[`run:${kind}:${JSON.stringify(payload)}`] ??= crypto.randomUUID() }) }); const body = await response.json(); if (response.ok) { update(kind === 'modification' ? 'modification_run_id' : 'transfer_run_id', body.id); setRunStates((current) => ({ ...current, [body.id]: body.status })); setMessage(`${kind === 'modification' ? '변형' : '전이'} 실행을 만들었습니다. 실행 완료 후 제출하세요.`); } else setMessage(body.error?.message ?? '검증 실행을 만들지 못했습니다.'); };

  return <div className="learning-page"><PortalHeader /><main className="path-panel understanding-panel">
    <p className="eyebrow">검증된 이해</p><h1>설명하고, 바꾸고, 전이하세요</h1>
    {challenge && <section className="project-form">{Object.values(challenge.prompt).map((prompt) => <p key={prompt}>{prompt}</p>)}<p>현재 상태: {stateLabels[challenge.state] ?? challenge.state}</p>
      <label>코드 목적과 흐름 설명<textarea value={answers.explanation} onChange={(event) => update('explanation', event.target.value)} /></label>
      <label>변경 결과 예측<textarea disabled={predictionsCommitted} value={answers.prediction} onChange={(event) => update('prediction', event.target.value)} /></label>
      <label>독립 변형 내용<textarea value={answers.modification} onChange={(event) => update('modification', event.target.value)} /></label>
      <label>다른 맥락의 전이 답변<textarea disabled={predictionsCommitted} value={answers.transfer_answer} onChange={(event) => update('transfer_answer', event.target.value)} /></label>
      {!predictionsCommitted && <button type="button" disabled={!answers.prediction.trim() || !answers.transfer_answer.trim()} onClick={commitPredictions}>실행 전 예측 확정</button>}
      {predictionsCommitted && <><p><a href={`/workspace?id=${workspaceId}`}>원 작업공간에서 코드를 의미 있게 변경하고 저장</a></p><button type="button" onClick={() => createValidationRun('modification')}>변형 실행 만들기</button><label>전이 작업공간<select value={transferWorkspace} onChange={(event) => setTransferWorkspace(event.target.value)}><option value="">선택</option>{workspaces.map((workspace) => <option key={workspace.id} value={workspace.id}>{workspace.title}</option>)}</select></label><button type="button" disabled={!transferWorkspace} onClick={() => createValidationRun('transfer')}>전이 실행 만들기</button></>}
      <label>독립 변형 실행 ID<input readOnly value={answers.modification_run_id} /></label>{answers.modification_run_id && <p>변형 실행: {runStates[answers.modification_run_id] ?? '확인 중'}</p>}
      <label>전이 실행 ID<input readOnly value={answers.transfer_run_id} /></label>{answers.transfer_run_id && <p>전이 실행: {runStates[answers.transfer_run_id] ?? '확인 중'}</p>}
      <button type="button" disabled={!predictionsCommitted || !answers.explanation.trim() || !answers.modification.trim() || runStates[answers.modification_run_id] !== 'succeeded' || runStates[answers.transfer_run_id] !== 'succeeded'} onClick={submit}>이해 증거 제출</button></section>}
    {result && <section className="evidence-card" aria-live="polite"><h2>{stateLabels[result.state] ?? result.state}</h2><p>도움 공개: {result.assistance_disclosure}</p><ul className="evidence-list">{result.evidence_labels.map((label) => <li key={label}>{evidenceLabels[label] ?? label}</li>)}</ul></section>}
    {message && <p role="alert" className="auth-message">{message}</p>}
  </main></div>;
}

export default UnderstandingPage;
