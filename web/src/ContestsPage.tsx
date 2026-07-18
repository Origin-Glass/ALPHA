import { useEffect, useState } from 'react';
import PortalHeader from './PortalHeader';

type Contest = { slug: string; title: string; description: string; scoring_mode: string; starts_at: string; ends_at: string; state: string; participant_count: number; problem_count: number };
const stateLabels: Record<string, string> = { scheduled: '예정', running: '진행 중', ended: '종료' };
const dateTime = (value: string) => new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit', timeZone: 'Asia/Seoul' }).format(new Date(value));

function ContestsPage() {
  const [items, setItems] = useState<Contest[]>([]);
  const [message, setMessage] = useState('');
  useEffect(() => { fetch('/api/v1/contests').then(async (response) => { const body = await response.json(); if (!response.ok) throw new Error(body.error?.message ?? '대회를 불러오지 못했습니다.'); return body; }).then((body) => setItems(body.items)).catch((error) => setMessage(error instanceof Error ? error.message : '대회를 불러오지 못했습니다.')); }, []);
  return <div className="learning-page"><PortalHeader /><main className="contest-catalog"><p className="eyebrow">ALPHA 대회</p><h1>시간 안에서<br /><span>실력을 증명하세요</span></h1><p className="catalog-description">대회 결과는 일반 숙련과 분리된 대회 검증 근거로 기록됩니다.</p><div className="contest-list">{items.map((contest) => <a href={`/contests/${contest.slug}`} className="contest-card" key={contest.slug}><div><span className={`contest-state ${contest.state}`}>{stateLabels[contest.state]}</span><span>{contest.scoring_mode === 'icpc' ? 'ICPC 페널티' : '점수 합산'}</span></div><h2>{contest.title}</h2><p>{contest.description}</p><footer><span>{dateTime(contest.starts_at)} — {dateTime(contest.ends_at)}</span><span>{contest.problem_count}문제 · {contest.participant_count}명</span></footer></a>)}</div>{!items.length && !message && <p className="loading-state">대회를 불러오고 있습니다…</p>}{message && <p className="auth-message" role="alert">{message}</p>}</main></div>;
}

export default ContestsPage;
