import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type LearningPath = {
  track: { slug: string; title: string; description: string };
  units: Array<{
    slug: string;
    title: string;
    summary: string;
    activity_kind: string;
    position: number;
    estimated_minutes: number;
    available: boolean;
    status: 'started' | 'completed' | null;
    mastery_score: number | null;
  }>;
};

const activityLabels: Record<string, string> = {
  algorithm: '알고리즘',
  code_reading: '코드 독해',
  debugging: '디버깅',
  docs_project: '문서 프로젝트',
  independent_build: '독립 구현',
};

function LearningPathPage() {
  const [path, setPath] = useState<LearningPath | null>(null);
  const [needsDiagnostic, setNeedsDiagnostic] = useState(false);
  const [message, setMessage] = useState('');

  useEffect(() => {
    fetch('/api/v1/learning/path', { credentials: 'include' })
      .then(async (response) => {
        if (response.status === 401) {
          window.location.assign('/login?redirect_after=/learn');
          return null;
        }
        if (response.status === 404) {
          setNeedsDiagnostic(true);
          return null;
        }
        const body = await response.json();
        if (!response.ok) throw new Error(body.error?.message ?? '학습 경로를 불러오지 못했습니다.');
        return body;
      })
      .then((body) => body && setPath(body))
      .catch((error) => setMessage(error instanceof Error ? error.message : '학습 경로를 불러오지 못했습니다.'));
  }, []);

  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="path-panel">
        {path ? (
          <>
            <p className="eyebrow">PERSONAL LEARNING PATH</p>
            <h1>{path.track.title}</h1>
            <p className="path-description">{path.track.description}</p>
            <ol className="unit-list">
              {path.units.map((unit) => (
                <li className={unit.available ? 'unit-card' : 'unit-card locked'} key={unit.slug}>
                  <div className="unit-position">{String(unit.position).padStart(2, '0')}</div>
                  <div>
                    <div className="unit-meta">
                      <span>{activityLabels[unit.activity_kind]}</span>
                      <span>{unit.estimated_minutes}분</span>
                    </div>
                    <h2>{unit.title}</h2>
                    <p>{unit.summary}</p>
                  </div>
                  <span className="unit-state">
                    {unit.status === 'completed' ? `완료 · ${unit.mastery_score ?? 0}점` : unit.available ? '학습 가능' : '이전 단원 완료 필요'}
                  </span>
                </li>
              ))}
            </ol>
          </>
        ) : needsDiagnostic ? (
          <section className="empty-path">
            <p className="eyebrow">PATH NOT SET</p>
            <h1>나에게 맞는<br /><span>출발점을 찾아요</span></h1>
            <p>네 가지 역량을 짧게 확인하면 첫 학습 경로를 추천합니다.</p>
            <a className="primary-action result-action" href="/onboarding">역량 진단 시작</a>
          </section>
        ) : !message && <p className="loading-state">학습 경로를 불러오고 있습니다…</p>}
        {message && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default LearningPathPage;
