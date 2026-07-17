import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type TrackData = {
  track: { title: string; description: string };
  modules: Array<{ slug: string; title: string; goal: string; position: number }>;
  lessons: Array<{ module_slug: string; slug: string; title: string; goal: string; mastery_criteria: string; remediation: string }>;
  resources: Array<{ lesson_slug: string; title: string; url: string; publisher: string; reading_goal: string }>;
  activities: Array<{ lesson_slug: string; slug: string; kind: string; title: string }>;
  capstones: Array<{ lesson_slug: string; problem_slug: string; title: string; exercise_kind: string }>;
};

function DocsTrackPage() {
  const [data, setData] = useState<TrackData | null>(null);
  const [message, setMessage] = useState('');

  useEffect(() => {
    fetch('/api/v1/tracks/docs-builder').then(async (response) => {
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '문서 학습 트랙을 불러오지 못했습니다.');
      return body;
    }).then(setData).catch((error) => setMessage(error instanceof Error ? error.message : '문서 학습 트랙을 불러오지 못했습니다.'));
  }, []);

  return (
    <div className="learning-page">
      <PortalHeader />
      <main className="docs-track">
        {data ? <>
          <p className="eyebrow">DOCUMENTATION-DRIVEN TRACK</p>
          <h1>{data.track.title}</h1>
          <p className="path-description">{data.track.description}</p>
          <div className="track-roadmap">
            {data.modules.map((module) => <section className="track-module" key={module.slug}>
              <div className="track-module__number">{String(module.position).padStart(2, '0')}</div>
              <div className="track-module__content"><h2>{module.title}</h2><p>{module.goal}</p>
                {data.lessons.filter((lesson) => lesson.module_slug === module.slug).map((lesson) => <article className="track-lesson" key={lesson.slug}>
                  <h3>{lesson.title}</h3><p>{lesson.goal}</p>
                  {data.resources.filter((resource) => resource.lesson_slug === lesson.slug).map((resource) => <a className="doc-link" href={resource.url} target="_blank" rel="noreferrer" key={resource.url}><strong>{resource.title}</strong><span>{resource.reading_goal}</span><small>{resource.publisher} 공식 문서 · 새 창</small></a>)}
                  <div className="lesson-actions">
                    {data.activities.filter((activity) => activity.lesson_slug === lesson.slug).map((activity) => <a href={`/activities/${activity.slug}`} key={activity.slug}>체크포인트 · {activity.title}</a>)}
                    {data.capstones.filter((capstone) => capstone.lesson_slug === lesson.slug).map((capstone) => <a className="primary-action" href={`/solve/${capstone.problem_slug}`} key={capstone.problem_slug}>최종 실행 검증 · {capstone.title}</a>)}
                  </div>
                  <details><summary>통과·보충 기준</summary><p><strong>통과</strong> {lesson.mastery_criteria}</p><p><strong>보충</strong> {lesson.remediation}</p></details>
                </article>)}
              </div>
            </section>)}
          </div>
        </> : !message && <p className="loading-state">문서 학습 트랙을 불러오고 있습니다…</p>}
        {message && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default DocsTrackPage;
