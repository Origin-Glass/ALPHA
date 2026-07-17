import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type Problem = {
  slug: string;
  title: string;
  statement: string;
  difficulty: number | null;
  difficulty_source: string;
  learning_axis: string;
  source_url: string | null;
  time_limit_ms: number;
  memory_limit_mb: number;
  tags: Array<{ tag: string; label: string }>;
  samples: Array<{ ordinal: number; input: string; expected_output: string }>;
  external_metadata: null | {
    state: 'fresh' | 'stale' | 'blocked';
    fetched_at: string;
    attribution: string;
  };
};

function ProblemDetailPage({ slug }: { slug: string }) {
  const [problem, setProblem] = useState<Problem | null>(null);
  const [message, setMessage] = useState('');

  useEffect(() => {
    fetch(`/api/v1/problems/${encodeURIComponent(slug)}`)
      .then(async (response) => {
        const body = await response.json();
        if (!response.ok) throw new Error(body.error?.message ?? '문제를 불러오지 못했습니다.');
        return body.problem;
      })
      .then(setProblem)
      .catch((error) => setMessage(error instanceof Error ? error.message : '문제를 불러오지 못했습니다.'));
  }, [slug]);

  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="problem-detail-panel">
        {problem ? (
          <>
            <a className="back-link" href="/problems">← 문제 목록</a>
            <div className="problem-title-row">
              <div><p className="eyebrow">ALGORITHM PROBLEM</p><h1>{problem.title}</h1></div>
              <div className="limit-card"><span>시간 {problem.time_limit_ms}ms</span><span>메모리 {problem.memory_limit_mb}MB</span></div>
            </div>
            {problem.external_metadata && (
              <aside className={`metadata-notice ${problem.external_metadata.state}`}>
                <strong>{problem.external_metadata.attribution}</strong>
                <span>{problem.external_metadata.state === 'fresh' ? '정상 동기화' : problem.external_metadata.state === 'stale' ? '마지막 정상 값을 표시 중' : '실시간 연동 중단 · 확인되지 않은 값을 만들지 않음'}</span>
                {problem.source_url && <a href={problem.source_url} rel="noreferrer">원문 출처 열기</a>}
              </aside>
            )}
            <article className="statement-card">
              <h2>문제</h2>
              {problem.statement.split('\n').map((line, index) => <p key={`${index}-${line}`}>{line || '\u00a0'}</p>)}
            </article>
            {problem.samples.map((sample) => (
              <section className="sample-grid" key={sample.ordinal}>
                <div><h2>예제 입력 {sample.ordinal}</h2><pre>{sample.input}</pre></div>
                <div><h2>예제 출력 {sample.ordinal}</h2><pre>{sample.expected_output}</pre></div>
              </section>
            ))}
          </>
        ) : !message && <p className="loading-state">문제를 불러오고 있습니다…</p>}
        {message && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default ProblemDetailPage;
