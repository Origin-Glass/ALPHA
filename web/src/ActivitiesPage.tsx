import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type Activity = {
  slug: string;
  kind: string;
  title: string;
  instructions: string;
  estimated_minutes: number;
  progress: null | { status: string; best_score: number; mastery_class: string | null };
};

const labels: Record<string, string> = {
  predict_output: '출력 예측', trace_state: '상태 추적', explain_behavior: '동작 설명',
  identify_invariant: '불변식', locate_bug: '버그 찾기', compare_implementations: '구현 비교',
  estimate_complexity: '복잡도', reconstruct_code: '코드 복원', assess_tests: '테스트 평가',
  code_review: '코드 리뷰', docs_checkpoint: '문서 체크포인트',
};

const masteryLabels: Record<string, string> = {
  independent: '독립 해결', assisted: '도움 활용', reviewed: '리뷰 활용', explained: '전체 설명 활용',
};

function ActivitiesPage() {
  const [items, setItems] = useState<Activity[]>([]);
  const [message, setMessage] = useState('');

  useEffect(() => {
    fetch('/api/v1/activities', { credentials: 'include' })
      .then(async (response) => {
        if (response.status === 401) {
          window.location.assign('/login?redirect_after=/activities');
          return null;
        }
        const body = await response.json();
        if (!response.ok) throw new Error(body.error?.message ?? '학습 활동을 불러오지 못했습니다.');
        return body;
      })
      .then((body) => body && setItems(body.items))
      .catch((error) => setMessage(error instanceof Error ? error.message : '학습 활동을 불러오지 못했습니다.'));
  }, []);

  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="activity-catalog">
        <p className="eyebrow">코드 문해력 학습실</p>
        <h1>코드를 쓰기 전에<br /><span>읽는 힘부터</span></h1>
        <p className="catalog-description">출력 예측, 상태 추적, 결함 분류, 테스트 평가를 각각 다른 형식으로 연습합니다.</p>
        <div className="activity-grid">
          {items.map((activity) => (
            <a className="activity-card" href={`/activities/${activity.slug}`} key={activity.slug}>
              <div className="activity-card__meta"><span>{labels[activity.kind] ?? activity.kind}</span><span>{activity.estimated_minutes}분</span></div>
              <h2>{activity.title}</h2>
              <p>{activity.instructions}</p>
              <strong>{activity.progress?.status === 'completed'
                ? `${masteryLabels[activity.progress.mastery_class ?? ''] ?? '완료'} · ${activity.progress.best_score}점`
                : activity.progress ? '이어 하기' : '시작하기'}</strong>
            </a>
          ))}
        </div>
        {!items.length && !message && <p className="loading-state">학습 활동을 불러오고 있습니다…</p>}
        {message && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default ActivitiesPage;
