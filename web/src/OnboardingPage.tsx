import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type Question = {
  question_key: string;
  axis: string;
  prompt: string;
  options: string[];
};

type Result = {
  scores: Record<string, number>;
  recommended_track: { slug: string; title: string; description: string };
};

const axisLabels: Record<string, string> = {
  algorithmic_reasoning: '알고리즘 추론',
  code_literacy: '코드 읽기',
  docs_learning: '문서 기반 구현',
  independent_coding: '독립 코딩',
};

const csrfToken = () => document.cookie
  .split(';')
  .map((cookie) => cookie.trim())
  .find((cookie) => cookie.startsWith('alpha_csrf='))
  ?.slice('alpha_csrf='.length) ?? '';

function OnboardingPage() {
  const [questions, setQuestions] = useState<Question[]>([]);
  const [answers, setAnswers] = useState<Record<string, number>>({});
  const [step, setStep] = useState(0);
  const [result, setResult] = useState<Result | null>(null);
  const [message, setMessage] = useState('');
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    const load = async () => {
      try {
        const session = await fetch('/api/v1/auth/me', { credentials: 'include' });
        if (session.status === 401) {
          window.location.assign('/login?redirect_after=/onboarding');
          return;
        }
        if (!session.ok) throw new Error('계정 상태를 확인하지 못했습니다.');
        const user = await session.json();
        if (!user.terms_accepted) {
          window.location.assign('/terms');
          return;
        }
        const response = await fetch('/api/v1/onboarding');
        if (!response.ok) throw new Error('진단 문항을 불러오지 못했습니다.');
        setQuestions(await response.json());
      } catch (error) {
        setMessage(error instanceof Error ? error.message : '진단을 시작하지 못했습니다.');
      }
    };
    load();
  }, []);

  const finish = async () => {
    setSubmitting(true);
    setMessage('');
    try {
      const response = await fetch('/api/v1/onboarding/complete', {
        method: 'POST',
        credentials: 'include',
        headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken() },
        body: JSON.stringify({
          answers: questions.map((question) => ({
            question_key: question.question_key,
            selected_option: answers[question.question_key],
          })),
        }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '진단을 저장하지 못했습니다.');
      setResult(body);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : '진단을 저장하지 못했습니다.');
    } finally {
      setSubmitting(false);
    }
  };

  if (result) {
    return (
      <div className="learning-page">
        <PortalHeader />
        <main className="result-panel">
          <p className="eyebrow">내 시작 경로</p>
          <span className="result-kicker">가장 먼저 키울 역량</span>
          <h1>{result.recommended_track.title}</h1>
          <p className="result-description">{result.recommended_track.description}</p>
          <div className="score-grid" aria-label="역량 진단 점수">
            {Object.entries(result.scores).map(([axis, score]) => (
              <div key={axis}><span>{axisLabels[axis]}</span><strong>{score}</strong></div>
            ))}
          </div>
          <a className="primary-action result-action" href="/learn">내 학습 경로 열기</a>
        </main>
      </div>
    );
  }

  const question = questions[step];
  const selected = question ? answers[question.question_key] : undefined;
  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="diagnostic-panel" aria-labelledby="diagnostic-title">
        <div className="diagnostic-heading">
          <div>
            <p className="eyebrow">역량 진단</p>
            <h1 id="diagnostic-title">현재의 출발점을<br /><span>정확히 찾아요</span></h1>
          </div>
          {question && <span className="step-count">{step + 1} / {questions.length}</span>}
        </div>
        {question ? (
          <section className="question-card" aria-labelledby="question-prompt">
            <span className="axis-label">{axisLabels[question.axis]}</span>
            <h2 id="question-prompt">{question.prompt}</h2>
            <div className="answer-list" role="radiogroup" aria-labelledby="question-prompt">
              {question.options.map((option, index) => (
                <label className={selected === index ? 'answer-option selected' : 'answer-option'} key={option}>
                  <input
                    type="radio"
                    name={question.question_key}
                    checked={selected === index}
                    onChange={() => setAnswers({ ...answers, [question.question_key]: index })}
                  />
                  <span>{option}</span>
                </label>
              ))}
            </div>
            <div className="question-actions">
              <button type="button" className="back-action" disabled={step === 0} onClick={() => setStep(step - 1)}>이전</button>
              {step < questions.length - 1 ? (
                <button type="button" className="primary-action" disabled={selected === undefined} onClick={() => setStep(step + 1)}>다음 문항</button>
              ) : (
                <button type="button" className="primary-action" disabled={selected === undefined || submitting} onClick={finish}>
                  {submitting ? '판정 중…' : '진단 완료'}
                </button>
              )}
            </div>
          </section>
        ) : !message && <p className="loading-state">진단 문항을 준비하고 있습니다…</p>}
        {message && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default OnboardingPage;
