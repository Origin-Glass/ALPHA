import { useEffect, useRef, useState } from 'react';

type Problem = {
  slug: string;
  title: string;
  statement: string;
  time_limit_ms: number;
  memory_limit_mb: number;
  samples: Array<{ ordinal: number; input: string; expected_output: string }>;
};

type Submission = {
  id: string;
  status: string;
  score: number | null;
  compile_output: string | null;
  run_kind: 'formal' | 'sample' | 'custom';
  run_output: string | null;
};

const templates: Record<string, string> = {
  cpp20: '#include <iostream>\nusing namespace std;\n\nint main() {\n    ios::sync_with_stdio(false);\n    cin.tie(nullptr);\n\n    return 0;\n}\n',
  python3: 'def main():\n    pass\n\nif __name__ == "__main__":\n    main()\n',
  java21: 'import java.io.*;\nimport java.util.*;\n\npublic class Main {\n    public static void main(String[] args) throws Exception {\n    }\n}\n',
};

const problemTemplates: Record<string, Partial<Record<string, string>>> = {
  'docs-url-normalizer': {
    python3: 'import sys\nfrom urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit\n\n\ndef normalize(url: str) -> str:\n    # 공식 문서의 계약을 확인한 뒤 구현하세요.\n    pass\n\n\ndef main():\n    print(normalize(sys.stdin.readline().strip()))\n\n\nif __name__ == "__main__":\n    main()\n',
  },
};

const statusLabels: Record<string, string> = {
  QUEUED: '대기 중', COMPILING: '컴파일 중', RUNNING: '실행 중', ACCEPTED: '정답',
  WRONG_ANSWER: '오답', PARTIAL_ACCEPTED: '부분 정답', TIME_LIMIT_EXCEEDED: '시간 초과',
  MEMORY_LIMIT_EXCEEDED: '메모리 초과', OUTPUT_LIMIT_EXCEEDED: '출력 초과',
  RUNTIME_ERROR: '런타임 오류', COMPILE_ERROR: '컴파일 오류', SYSTEM_ERROR: '시스템 오류', CANCELLED: '취소됨',
};

const terminal = new Set(['ACCEPTED', 'WRONG_ANSWER', 'PARTIAL_ACCEPTED', 'TIME_LIMIT_EXCEEDED', 'MEMORY_LIMIT_EXCEEDED', 'OUTPUT_LIMIT_EXCEEDED', 'RUNTIME_ERROR', 'COMPILE_ERROR', 'SYSTEM_ERROR', 'CANCELLED']);
const csrfToken = () => document.cookie.split(';').map((cookie) => cookie.trim()).find((cookie) => cookie.startsWith('alpha_csrf='))?.slice('alpha_csrf='.length) ?? '';

function SolvePage({ slug }: { slug: string }) {
  const contestSlug = new URLSearchParams(window.location.search).get('contest');
  const [problem, setProblem] = useState<Problem | null>(null);
  const [language, setLanguage] = useState('python3');
  const [source, setSource] = useState(templates.python3);
  const [saveState, setSaveState] = useState('불러오는 중');
  const [submission, setSubmission] = useState<Submission | null>(null);
  const [customInput, setCustomInput] = useState('1 2\n');
  const [message, setMessage] = useState('');
  const loadedDraft = useRef(false);

  useEffect(() => {
    const load = async () => {
      const session = await fetch('/api/v1/auth/me', { credentials: 'include' });
      if (session.status === 401) {
        window.location.assign(`/login?redirect_after=${encodeURIComponent(`/solve/${slug}`)}`);
        return;
      }
      const problemResponse = await fetch(`/api/v1/problems/${encodeURIComponent(slug)}`);
      const problemBody = await problemResponse.json();
      if (!problemResponse.ok) throw new Error(problemBody.error?.message ?? '문제를 불러오지 못했습니다.');
      setProblem(problemBody.problem);
      const draftResponse = await fetch(`/api/v1/problems/${encodeURIComponent(slug)}/draft`, { credentials: 'include' });
      if (draftResponse.ok) {
        const draft = await draftResponse.json();
        setLanguage(draft.language);
        setSource(draft.source);
        setSaveState(`저장본 ${draft.revision_count}회`);
      } else {
        setSource(problemTemplates[slug]?.[language] ?? templates[language]);
        setSaveState('새 코드');
      }
      loadedDraft.current = true;
    };
    load().catch((error) => setMessage(error instanceof Error ? error.message : '풀이 환경을 열지 못했습니다.'));
  }, [slug]);

  useEffect(() => {
    if (!loadedDraft.current) return;
    setSaveState('저장 대기');
    const timeout = window.setTimeout(async () => {
      try {
        const response = await fetch(`/api/v1/problems/${encodeURIComponent(slug)}/draft`, {
          method: 'PUT', credentials: 'include',
          headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken() },
          body: JSON.stringify({ language, source }),
        });
        if (!response.ok) throw new Error();
        setSaveState('자동 저장됨');
      } catch {
        setSaveState('저장 실패');
      }
    }, 800);
    return () => window.clearTimeout(timeout);
  }, [language, slug, source]);

  useEffect(() => {
    if (!submission || terminal.has(submission.status)) return;
    const eventSource = new EventSource(`/api/v1/submissions/${submission.id}/events`);
    eventSource.addEventListener('status', (event) => {
      const update = JSON.parse((event as MessageEvent).data);
      setSubmission((current) => current ? { ...current, status: update.status } : current);
      if (terminal.has(update.status)) {
        eventSource.close();
        fetch(`/api/v1/submissions/${submission.id}`, { credentials: 'include' })
          .then((response) => response.json()).then(setSubmission).catch(() => undefined);
      }
    });
    return () => eventSource.close();
  }, [submission?.id]);

  const enqueue = async (mode: 'formal' | 'sample' | 'custom') => {
    setMessage('');
    try {
      const response = await fetch(mode === 'formal' ? '/api/v1/submissions' : '/api/v1/runs', {
        method: 'POST', credentials: 'include',
        headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken() },
        body: JSON.stringify({
          problem_slug: slug, language, source, idempotency_key: crypto.randomUUID(),
          ...(mode === 'formal' ? (contestSlug ? { contest_slug: contestSlug } : {}) : { mode, ...(mode === 'custom' ? { custom_input: customInput } : {}) }),
        }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '제출하지 못했습니다.');
      setSubmission(body);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : '제출하지 못했습니다.');
    }
  };

  return (
    <div className="solve-page">
      <header className="solve-header">
        <a className="brand" href="/">ALPHA<span>.</span></a>
        <a href={contestSlug ? `/contests/${contestSlug}` : `/problems/${slug}`}>{contestSlug ? '대회로 돌아가기' : '문제 상세'}</a>
        <span>{problem?.title ?? '풀이 환경'}</span>
        <span className="save-state">{saveState}</span>
      </header>
      {problem ? (
        <main className="solve-workspace">
          <section className="solve-problem" aria-labelledby="solve-title">
            <p className="eyebrow">알고리즘 풀이 공간</p>
            <h1 id="solve-title">{problem.title}</h1>
            <div className="solve-limits"><span>{problem.time_limit_ms}ms</span><span>{problem.memory_limit_mb}MB</span></div>
            <article>{problem.statement.split('\n').map((line, index) => <p key={`${index}-${line}`}>{line || '\u00a0'}</p>)}</article>
            {problem.samples.map((sample) => <div className="solve-sample" key={sample.ordinal}><div><strong>입력 {sample.ordinal}</strong><pre>{sample.input}</pre></div><div><strong>출력 {sample.ordinal}</strong><pre>{sample.expected_output}</pre></div></div>)}
          </section>
          <section className="editor-panel" aria-label="소스 코드 편집기">
            <div className="editor-toolbar">
              <label>언어<select value={language} onChange={(event) => { const next = event.target.value; setLanguage(next); setSource(templates[next]); }}><option value="cpp20">GNU C++20</option><option value="python3">Python 3</option><option value="java21">Java 21</option></select></label>
              <div className="editor-actions"><button type="button" onClick={() => enqueue('sample')}>샘플 실행</button><button className="primary-action" type="button" onClick={() => enqueue('formal')}>정식 제출</button></div>
            </div>
            <textarea aria-label="소스 코드" value={source} onChange={(event) => setSource(event.target.value)} spellCheck={false} />
            <div className="verdict-panel" aria-live="polite">
              <details className="custom-run"><summary>사용자 입력</summary><div><textarea aria-label="사용자 입력" value={customInput} onChange={(event) => setCustomInput(event.target.value)} /><button type="button" onClick={() => enqueue('custom')}>실행</button></div></details>
              {submission ? <><span className={`verdict ${submission.status.toLowerCase()}`}>{submission.status === 'ACCEPTED' && submission.run_kind === 'custom' ? '실행 완료' : submission.status === 'ACCEPTED' && submission.run_kind === 'sample' ? '샘플 통과' : statusLabels[submission.status] ?? submission.status}</span>{submission.score !== null && submission.run_kind === 'formal' && <strong>{submission.score}점</strong>}{submission.compile_output && <pre>{submission.compile_output}</pre>}{submission.run_output && <pre>{submission.run_output}</pre>}</> : <p>샘플·사용자 입력·정식 제출 상태가 여기에 표시됩니다.</p>}
              {message && <p className="auth-message" role="alert">{message}</p>}
            </div>
          </section>
        </main>
      ) : !message && <p className="loading-state solve-loading">풀이 환경을 준비하고 있습니다…</p>}
      {message && !problem && <p className="auth-message solve-loading" role="alert">{message}</p>}
    </div>
  );
}

export default SolvePage;
