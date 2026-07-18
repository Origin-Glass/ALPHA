import { FormEvent, useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type ResponseSchema =
  | { type: 'text'; label: string }
  | { type: 'tokens'; label: string }
  | { type: 'choice'; options: Array<{ value: string; label: string }> }
  | { type: 'fields'; fields: Array<{ key: string; label: string }> };

type Activity = {
  slug: string;
  kind: string;
  title: string;
  instructions: string;
  starter_code: string | null;
  response_schema: ResponseSchema;
  estimated_minutes: number;
  lesson_title: string | null;
  mastery_criteria: string | null;
  remediation: string | null;
  resources: Array<{ title: string; url: string; publisher: string; reading_goal: string }>;
  progress: null | {
    attempt_count: number;
    max_assistance_level: number;
    best_score: number;
    mastery_class: string | null;
  };
};

type Result = { score: number; passed: boolean; mastery_class: string | null; max_assistance_level: number };

const csrfToken = () => document.cookie.split(';').map((cookie) => cookie.trim())
  .find((cookie) => cookie.startsWith('alpha_csrf='))?.slice('alpha_csrf='.length) ?? '';

const masteryLabels: Record<string, string> = {
  independent: '독립 해결', assisted: '도움 활용', reviewed: '리뷰 활용', explained: '전체 설명 활용',
};

function ActivityDetailPage({ slug }: { slug: string }) {
  const [activity, setActivity] = useState<Activity | null>(null);
  const [answer, setAnswer] = useState('');
  const [fields, setFields] = useState<Record<string, string>>({});
  const [reflection, setReflection] = useState('');
  const [helpLevel, setHelpLevel] = useState(0);
  const [help, setHelp] = useState<Array<{ level: number; kind: string; content: string }>>([]);
  const [aiEnabled, setAiEnabled] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const load = async () => {
      const response = await fetch(`/api/v1/activities/${encodeURIComponent(slug)}`, { credentials: 'include' });
      if (response.status === 401) {
        window.location.assign(`/login?redirect_after=${encodeURIComponent(`/activities/${slug}`)}`);
        return;
      }
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '학습 활동을 불러오지 못했습니다.');
      setActivity(body);
      setHelpLevel(body.progress?.max_assistance_level ?? 0);
      await fetch(`/api/v1/activities/${encodeURIComponent(slug)}/start`, {
        method: 'POST', credentials: 'include', headers: { 'x-csrf-token': csrfToken() },
      });
      const ai = await fetch('/api/v1/assistance/status');
      if (ai.ok) setAiEnabled((await ai.json()).enabled);
    };
    load().catch((error) => setMessage(error instanceof Error ? error.message : '학습 활동을 불러오지 못했습니다.'));
  }, [slug]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!activity) return;
    setBusy(true);
    setMessage('');
    const schema = activity.response_schema;
    const responseValue = schema.type === 'choice' ? { choice: answer }
      : schema.type === 'fields' ? { fields }
        : schema.type === 'tokens' ? { tokens: answer.split(',').map((token) => token.trim()).filter(Boolean) }
          : { answer };
    try {
      const response = await fetch(`/api/v1/activities/${encodeURIComponent(slug)}/attempts`, {
        method: 'POST', credentials: 'include',
        headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken() },
        body: JSON.stringify({ response: responseValue, ...(reflection.trim() ? { reflection: reflection.trim() } : {}) }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '답안을 제출하지 못했습니다.');
      setResult(body);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : '답안을 제출하지 못했습니다.');
    } finally {
      setBusy(false);
    }
  };

  const requestHelp = async () => {
    const next = helpLevel + 1;
    setBusy(true);
    setMessage('');
    try {
      const response = await fetch(`/api/v1/activities/${encodeURIComponent(slug)}/assistance/${next}`, {
        method: 'POST', credentials: 'include', headers: { 'x-csrf-token': csrfToken() },
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '도움을 열지 못했습니다.');
      setHelp((current) => [...current, body]);
      setHelpLevel(next);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : '도움을 열지 못했습니다.');
    } finally {
      setBusy(false);
    }
  };

  const schema = activity?.response_schema;
  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="activity-workspace">
        {activity ? <>
          <section className="activity-prompt">
            <a className="back-link" href="/activities">← 활동 목록</a>
            <p className="eyebrow">{activity.lesson_title ?? '코드 문해력'}</p>
            <h1>{activity.title}</h1>
            <p className="activity-instructions">{activity.instructions}</p>
            {activity.starter_code && <pre>{activity.starter_code}</pre>}
            {activity.resources.length > 0 && <aside className="reading-resources" aria-label="공식 문서">
              <h2>공식 문서</h2>
              {activity.resources.map((resource) => <a href={resource.url} target="_blank" rel="noreferrer" key={resource.url}>
                <strong>{resource.title}</strong><span>{resource.reading_goal}</span><small>{resource.publisher} · 새 창</small>
              </a>)}
            </aside>}
            <div className="learning-criteria"><div><strong>통과 기준</strong><span>{activity.mastery_criteria}</span></div><div><strong>막혔을 때</strong><span>{activity.remediation}</span></div></div>
          </section>
          <section className="activity-answer" aria-labelledby="answer-title">
            <div className="activity-answer__heading"><div><p className="eyebrow">구조화 답안</p><h2 id="answer-title">내 답안</h2></div><span>{activity.estimated_minutes}분</span></div>
            <form onSubmit={submit}>
              {schema?.type === 'choice' && <fieldset className="activity-choices"><legend className="sr-only">답 선택</legend>{schema.options.map((option) => <label key={option.value}><input type="radio" name="answer" value={option.value} checked={answer === option.value} onChange={(event) => setAnswer(event.target.value)} /><span>{option.label}</span></label>)}</fieldset>}
              {(schema?.type === 'text' || schema?.type === 'tokens') && <label className="activity-input"><span>{schema.label}</span><input value={answer} onChange={(event) => setAnswer(event.target.value)} placeholder={schema.type === 'tokens' ? '예: -, >=, -=' : '답을 입력하세요'} /></label>}
              {schema?.type === 'fields' && <div className="activity-fields">{schema.fields.map((field) => <label key={field.key}><span>{field.label}</span><input value={fields[field.key] ?? ''} onChange={(event) => setFields((current) => ({ ...current, [field.key]: event.target.value }))} /></label>)}</div>}
              <label className="activity-reflection"><span>짧은 회고 <small>선택</small></span><textarea value={reflection} onChange={(event) => setReflection(event.target.value)} placeholder="어떤 근거로 답했나요? 숨은 사고 과정이 아닌 확인 가능한 근거만 기록하세요." /></label>
              <button className="primary-action activity-submit" type="submit" disabled={busy}>평가받기</button>
            </form>
            {result && <div className={`activity-result ${result.passed ? 'passed' : 'retry'}`} role="status"><strong>{result.score}점 · {result.passed ? '통과' : '다시 시도'}</strong><span>{result.passed ? masteryLabels[result.mastery_class ?? ''] : '틀린 필드를 다시 추적해 보세요.'}</span></div>}
            <section className="assistance-panel" aria-labelledby="assistance-title">
              <div><h2 id="assistance-title">단계형 도움</h2><span className={aiEnabled ? 'ai-state enabled' : 'ai-state'}>AI {aiEnabled ? '사용 가능' : '비활성'}</span></div>
              <p>1~6단계는 정해진 학습 발판입니다. 받은 최고 단계는 성취 분류에 반영됩니다.</p>
              {help.map((item) => <div className="help-item" key={item.level}><strong>{item.level}단계</strong><span>{item.content}</span></div>)}
              <button type="button" onClick={requestHelp} disabled={busy || helpLevel >= 9}>{helpLevel < 6 ? `${helpLevel + 1}단계 도움 열기` : helpLevel === 6 ? '7단계 AI 추론 비평 요청' : '다음 도움 열기'}</button>
            </section>
            {message && <p className="auth-message" role="alert">{message}</p>}
          </section>
        </> : !message && <p className="loading-state">학습 활동을 준비하고 있습니다…</p>}
        {message && !activity && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default ActivityDetailPage;
