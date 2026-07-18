import { FormEvent, useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type Dashboard = {
  handle: string;
  display_name: string;
  progression: { xp: number; level: number; current_streak: number; best_streak: number; streak_protections: number; mastered_count: number; independent_mastered_count: number };
  independent_mastery_ratio: number;
  mastery: Array<{ axis: string; rating: number; tier: string; evidence_count: number }>;
  quests: Array<{ slug: string; title: string; cadence: string; target: number; reward_xp: number; progress: number; completed: boolean }>;
  achievements: Array<{ slug: string; title: string; description: string; icon_token: string }>;
  recent_xp: Array<{ xp: number; reason: string; source_kind: string; occurred_at: string }>;
  profile: { bio: string; visibility: string; show_in_rankings: boolean; allow_friend_requests: boolean; selected_title_slug: string; selected_cosmetic_slug: string };
  titles: Array<{ slug: string; label: string }>;
  cosmetics: Array<{ slug: string; label: string }>;
};

const axisLabels: Record<string, string> = {
  algorithm: '알고리즘', code_reading: '코드 독해', debugging: '디버깅', documentation: '문서 활용',
  framework: '프레임워크', contest: '대회', instructor_course: '과정 이수',
};

const csrfToken = () => document.cookie.split(';').map((cookie) => cookie.trim())
  .find((cookie) => cookie.startsWith('alpha_csrf='))?.slice('alpha_csrf='.length) ?? '';

function ProgressionPage() {
  const [data, setData] = useState<Dashboard | null>(null);
  const [message, setMessage] = useState('');
  const [saved, setSaved] = useState('');

  const load = () => fetch('/api/v1/progression/dashboard', { credentials: 'include' })
    .then(async (response) => {
      if (response.status === 401) {
        window.location.assign('/login?redirect_after=/dashboard');
        return null;
      }
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message ?? '성장 기록을 불러오지 못했습니다.');
      return body;
    }).then((body) => body && setData(body));

  useEffect(() => { load().catch((error) => setMessage(error instanceof Error ? error.message : '성장 기록을 불러오지 못했습니다.')); }, []);

  const saveProfile = async (event: FormEvent) => {
    event.preventDefault();
    if (!data) return;
    setSaved('저장 중…');
    const response = await fetch('/api/v1/profile', {
      method: 'PUT', credentials: 'include',
      headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken() },
      body: JSON.stringify({ display_name: data.display_name, ...data.profile }),
    });
    if (response.ok) setSaved('저장됨');
    else {
      const body = await response.json();
      setSaved(body.error?.message ?? '저장하지 못했습니다.');
    }
  };

  const updateProfile = (patch: Partial<Dashboard['profile']>) => data && setData({ ...data, profile: { ...data.profile, ...patch } });
  const xpInLevel = data ? data.progression.xp % 500 : 0;

  return (
    <div className="learning-page progression-page">
      <PortalHeader />
      <main className="progression-dashboard">
        {data ? <>
          <section className="progression-hero">
            <div><p className="eyebrow">나의 ALPHA 성장</p><h1>{data.display_name}의<br /><span>성장 기록</span></h1><a href={`/u/${data.handle}`}>공개 프로필 보기</a></div>
            <div className="level-orb" aria-label={`레벨 ${data.progression.level}, 다음 레벨까지 ${500 - xpInLevel} XP`}><span>레벨</span><strong>{data.progression.level}</strong><progress max="500" value={xpInLevel} /><small>{data.progression.xp.toLocaleString()} XP</small></div>
          </section>
          <section className="progression-stats" aria-label="핵심 성장 지표">
            <article><span>현재 스트릭</span><strong>{data.progression.current_streak}일</strong><small>최고 {data.progression.best_streak}일 · 보호권 {data.progression.streak_protections}개</small></article>
            <article><span>독립 숙련 비율</span><strong>{Math.round(data.independent_mastery_ratio * 100)}%</strong><small>{data.progression.independent_mastered_count}/{data.progression.mastered_count}개 항목</small></article>
            <article><span>이번 시즌</span><strong>{data.progression.xp} XP</strong><a href="/rankings?season=launch-2026">시즌 랭킹 보기</a></article>
          </section>
          <section className="dashboard-section"><div className="dashboard-heading"><h2>다중 역량</h2><a href="/rankings">역량 랭킹</a></div><div className="mastery-grid">{data.mastery.map((mastery) => <article key={mastery.axis}><span>{axisLabels[mastery.axis] ?? mastery.axis}</span><strong>{mastery.rating}</strong><div><i style={{ width: `${mastery.rating / 10}%` }} /></div><small>{mastery.tier} · 근거 {mastery.evidence_count}개</small></article>)}</div></section>
          <section className="dashboard-columns">
            <div className="dashboard-section"><h2>퀘스트</h2><div className="quest-list">{data.quests.map((quest) => <article className={quest.completed ? 'completed' : ''} key={quest.slug}><div><span>{quest.cadence === 'daily' ? '오늘' : '이번 주'}</span><strong>{quest.title}</strong></div><progress max={quest.target} value={quest.progress} /><small>{quest.progress}/{quest.target} · +{quest.reward_xp} XP</small></article>)}</div></div>
            <div className="dashboard-section"><h2>업적</h2>{data.achievements.length ? <div className="achievement-list">{data.achievements.map((achievement) => <article key={achievement.slug}><span aria-hidden="true">◆</span><div><strong>{achievement.title}</strong><small>{achievement.description}</small></div></article>)}</div> : <p className="dashboard-empty">독립 활동을 통과하면 첫 업적이 열립니다.</p>}</div>
          </section>
          <section className="dashboard-columns">
            <div className="dashboard-section"><h2>최근 XP</h2>{data.recent_xp.length ? <ul className="xp-ledger">{data.recent_xp.map((event, index) => <li key={`${event.occurred_at}-${index}`}><span>{event.reason}</span><strong>+{event.xp} XP</strong></li>)}</ul> : <p className="dashboard-empty">아직 보상 가능한 학습 기록이 없습니다.</p>}</div>
            <form className="dashboard-section profile-settings" onSubmit={saveProfile}><h2>프로필·공개 설정</h2><label><span>표시 이름</span><input value={data.display_name} onChange={(event) => setData({ ...data, display_name: event.target.value })} /></label><label><span>소개</span><textarea value={data.profile.bio} onChange={(event) => updateProfile({ bio: event.target.value })} /></label><div className="profile-setting-row"><label><span>프로필 공개</span><select value={data.profile.visibility} onChange={(event) => updateProfile({ visibility: event.target.value })}><option value="public">공개</option><option value="private">비공개</option></select></label><label><span>칭호</span><select value={data.profile.selected_title_slug} onChange={(event) => updateProfile({ selected_title_slug: event.target.value })}>{data.titles.map((title) => <option value={title.slug} key={title.slug}>{title.label}</option>)}</select></label></div><label className="profile-check"><input type="checkbox" checked={data.profile.show_in_rankings} onChange={(event) => updateProfile({ show_in_rankings: event.target.checked })} /><span>공개 랭킹에 표시</span></label><label className="profile-check"><input type="checkbox" checked={data.profile.allow_friend_requests} onChange={(event) => updateProfile({ allow_friend_requests: event.target.checked })} /><span>친구 요청 허용</span></label><button type="submit">설정 저장</button>{saved && <small role="status">{saved}</small>}</form>
          </section>
        </> : !message && <p className="loading-state">성장 기록을 불러오고 있습니다…</p>}
        {message && <p className="auth-message" role="alert">{message}</p>}
      </main>
    </div>
  );
}

export default ProgressionPage;
